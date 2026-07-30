//! SQLite storage: open the single `orcastudio.db` and run migrations.
//!
//! Per ADR-004 the whole app is backed by one SQLite file in the user data
//! directory. Phase 0 only needs a `settings` key/value table.

use std::path::Path;

use rusqlite::Connection;

use crate::error::AppError;

/// Current schema version. Bump this and add a migration arm when the schema
/// changes.
///
/// - v1: `settings` key/value table (Phase 0).
/// - v2: `jobs` table — job model + state machine (Phase 1).
/// - v3: `molecules` table — persistent molecule library (Phase 2.3).
/// - v4: `jobs.scene_json` — SceneFragment snapshot, written once at create time
///   (Phase 2.5, ADR-008 #5). A nullable annotation over `input_content`, which
///   stays authoritative for geometry.
/// - v5: `results` table — parsed `.property.txt` (Phase 3, ADR-012). Narrow typed
///   columns for the card + one JSON column with per-atom arrays stored WITH their
///   element order. One row per job (`job_id` PK), upserted idempotently.
/// - v6: `results.imaginary_count` — negative-frequency count from `.hess` (unit
///   3.6). A narrow column the job list sorts by and the minimum/TS warning stands
///   on; NULL when a job has no `.hess`.
/// - v7: `results.homo_lumo_gap_eh` — HOMO/LUMO gap from `orca_2json` (unit 3.7);
///   the card shows it and the list will sort by it. NULL without an ORCA `.gbw`.
const SCHEMA_VERSION: i64 = 8;

/// Open (creating if needed) `orcastudio.db` under `data_dir` and migrate it to
/// the current schema.
pub fn init_db(data_dir: &Path) -> Result<Connection, AppError> {
    std::fs::create_dir_all(data_dir)?;
    let db_path = data_dir.join("orcastudio.db");
    let conn = Connection::open(db_path)?;
    migrate(&conn)?;
    Ok(conn)
}

/// Apply migrations forward from whatever version the database is currently at
/// to [`SCHEMA_VERSION`]. Each step is additive and gated on the stored version,
/// so re-running on an up-to-date database is a no-op and an older database is
/// upgraded in place without touching existing data.
fn migrate(conn: &Connection) -> Result<(), AppError> {
    // --- v1: base settings table + seeds. Always ensured (idempotent). ---
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS settings (
            key   TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );
        INSERT OR IGNORE INTO settings (key, value) VALUES ('orca_path', '/opt/orca/orca');
        -- Standalone xtb for pre-optimization (Phase 2.5.5), NOT xtb-via-ORCA.
        -- Seeded to the common PATH location; the user sets the real path in Settings.
        INSERT OR IGNORE INTO settings (key, value) VALUES ('xtb_path', 'xtb');
        INSERT OR IGNORE INTO settings (key, value) VALUES ('schema_version', '1');
        -- CPU pinning (Phase 2, domain rule #8). Defaults to the Interactive
        -- preset; cpu_mask/cpu_nprocs are only consulted when cpu_preset=custom.
        INSERT OR IGNORE INTO settings (key, value) VALUES ('cpu_preset', 'interactive');
        INSERT OR IGNORE INTO settings (key, value) VALUES ('cpu_mask', '8-15');
        INSERT OR IGNORE INTO settings (key, value) VALUES ('cpu_nprocs', '8');",
    )?;

    let mut version = current_version(conn)?;

    // --- v1 -> v2: jobs table (job model + state machine, Phase 1). ---
    if version < 2 {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS jobs (
                id            TEXT PRIMARY KEY,
                title         TEXT NOT NULL,
                input_content TEXT NOT NULL,
                status        TEXT NOT NULL DEFAULT 'draft',
                job_dir       TEXT,
                energy        REAL,
                wall_time     REAL,
                error_message TEXT,
                created_at    TEXT NOT NULL DEFAULT (datetime('now')),
                started_at    TEXT,
                completed_at  TEXT
            );",
        )?;
        version = 2;
    }

    // --- v2 -> v3: molecules table (persistent molecule library, Phase 2.3). ---
    // Molecules are a standalone library for now; they are NOT linked to `jobs`
    // (no `molecule_id` FK) — that association arrives with Phase 4.5 (reaction
    // modeling).
    if version < 3 {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS molecules (
                id           TEXT PRIMARY KEY,
                name         TEXT NOT NULL,
                formula      TEXT NOT NULL DEFAULT '',
                xyz          TEXT NOT NULL,
                charge       INTEGER NOT NULL DEFAULT 0,
                multiplicity INTEGER NOT NULL DEFAULT 1,
                tags         TEXT NOT NULL DEFAULT '',
                created_at   TEXT NOT NULL DEFAULT (datetime('now'))
            );",
        )?;
        version = 3;
    }

    // --- v3 -> v4: jobs.scene_json (SceneFragment snapshot, Phase 2.5). ---
    // A nullable annotation over input_content, written once at create time; the
    // input text stays authoritative for geometry (ADR-008 #5). Purely additive,
    // so jobs created before v4 simply carry NULL.
    if version < 4 {
        conn.execute_batch("ALTER TABLE jobs ADD COLUMN scene_json TEXT;")?;
        version = 4;
    }

    // --- v4 -> v5: results table (parsed .property.txt, Phase 3, ADR-012). ---
    // Per-atom arrays live in `data_json` WITH their element order, never a
    // position-keyed atom table (ADR-010). Purely additive; jobs parsed later.
    if version < 5 {
        create_results_table(conn)?;
        version = 5;
    }

    // --- v5 -> v6: results.imaginary_count (unit 3.6). ---
    // `create_results_table` already includes the column (current schema), so a
    // fresh v4→v5→v6 upgrade adds it in the v5 step; this guarded ALTER covers a
    // database that stopped at v5 before this column existed. Additive, nullable.
    if version < 6 {
        if !column_exists(conn, "results", "imaginary_count")? {
            conn.execute_batch("ALTER TABLE results ADD COLUMN imaginary_count INTEGER;")?;
        }
        version = 6;
    }

    // --- v6 -> v7: results.homo_lumo_gap_eh (unit 3.7). Guarded ALTER, as v6. ---
    if version < 7 {
        if !column_exists(conn, "results", "homo_lumo_gap_eh")? {
            conn.execute_batch("ALTER TABLE results ADD COLUMN homo_lumo_gap_eh REAL;")?;
        }
        version = 7;
    }

    // --- v7 -> v8: backfill jobs.energy from the AUTHORITATIVE results tier
    // (unit 3.9 defect 2). Jobs parsed before the header energy was sourced from
    // `results` have a NULL `jobs.energy` (the output.out tail regex missed the
    // final energy on a large molecule — measured 164 KB past the 64 KB window on
    // a 33-atom Freq) while `results.final_energy_eh` holds the real value. A
    // one-time data backfill (not a schema change): fill only the NULL ones from
    // their parsed result, so old jobs don't stay blank forever. Idempotent. ---
    if version < 8 {
        // Guarded: a real DB always has jobs.energy (created at v2), but some
        // migration-test fixtures stub `jobs` as `(id)` only — skip cleanly there.
        if column_exists(conn, "jobs", "energy")? {
            conn.execute_batch(
                "UPDATE jobs SET energy = (
                     SELECT r.final_energy_eh FROM results r WHERE r.job_id = jobs.id
                 )
                 WHERE energy IS NULL
                   AND EXISTS (
                     SELECT 1 FROM results r2
                     WHERE r2.job_id = jobs.id AND r2.final_energy_eh IS NOT NULL
                   );",
            )?;
        }
        version = 8;
    }

    // Persist the resulting version so subsequent runs skip completed steps.
    conn.execute(
        "UPDATE settings SET value = ?1 WHERE key = 'schema_version'",
        rusqlite::params![version.to_string()],
    )?;
    debug_assert_eq!(version, SCHEMA_VERSION);
    Ok(())
}

/// Create the `results` table (schema v5). Factored out so tests can build just
/// this table on an in-memory DB. Narrow typed columns carry what the card shows
/// and the job list will sort by; `data_json` carries the full parsed structure
/// including per-atom arrays with their element order. Units are in the column
/// names (rule #11) — notably `t_times_s_eh` is **T·S in Eh**, not entropy S.
pub(crate) fn create_results_table(conn: &Connection) -> Result<(), AppError> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS results (
            job_id              TEXT PRIMARY KEY REFERENCES jobs(id) ON DELETE CASCADE,
            final_energy_eh     REAL,
            dipole_magnitude_au REAL,
            zpe_eh              REAL,
            inner_energy_u_eh   REAL,
            enthalpy_h_eh       REAL,
            t_times_s_eh        REAL,
            free_energy_g_eh    REAL,
            imaginary_count     INTEGER,
            homo_lumo_gap_eh    REAL,
            data_json           TEXT NOT NULL,
            parser_version      INTEGER NOT NULL,
            parsed_at           TEXT NOT NULL DEFAULT (datetime('now'))
        );",
    )?;
    Ok(())
}

/// Whether `table` has a column named `col` (via `PRAGMA table_info`). Used to make
/// an additive `ALTER` idempotent across migration paths.
fn column_exists(conn: &Connection, table: &str, col: &str) -> Result<bool, AppError> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let found = stmt
        .query_map([], |r| r.get::<_, String>(1))?
        .filter_map(|c| c.ok())
        .any(|name| name == col);
    Ok(found)
}

/// Read the stored `schema_version` from `settings`, defaulting to 1 for a
/// database seeded before versioning was tracked as an integer.
fn current_version(conn: &Connection) -> Result<i64, AppError> {
    let raw: String = conn.query_row(
        "SELECT value FROM settings WHERE key = 'schema_version'",
        [],
        |r| r.get(0),
    )?;
    Ok(raw.parse().unwrap_or(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init_db_seeds_defaults() {
        let dir = std::env::temp_dir().join(format!("orcastudio-test-{}", std::process::id()));
        let conn = init_db(&dir).expect("init_db should succeed");

        let orca_path: String = conn
            .query_row(
                "SELECT value FROM settings WHERE key = 'orca_path'",
                [],
                |r| r.get(0),
            )
            .expect("orca_path should be seeded");
        assert_eq!(orca_path, "/opt/orca/orca");

        let version: String = conn
            .query_row(
                "SELECT value FROM settings WHERE key = 'schema_version'",
                [],
                |r| r.get(0),
            )
            .expect("schema_version should be seeded");
        assert_eq!(version, SCHEMA_VERSION.to_string());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn migrate_v1_to_v2_preserves_settings() {
        // Simulate a Phase 0 (v1) database: settings only, a user-customised
        // orca_path, schema_version pinned at 1, and no jobs table.
        let conn = Connection::open_in_memory().expect("open in-memory db");
        conn.execute_batch(
            "CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);
             INSERT INTO settings (key, value) VALUES ('orca_path', '/custom/orca');
             INSERT INTO settings (key, value) VALUES ('schema_version', '1');",
        )
        .expect("seed v1 schema");

        migrate(&conn).expect("v1 -> v2 migration should succeed");

        // Existing settings must be untouched.
        let orca_path: String = conn
            .query_row(
                "SELECT value FROM settings WHERE key = 'orca_path'",
                [],
                |r| r.get(0),
            )
            .expect("orca_path preserved");
        assert_eq!(orca_path, "/custom/orca");

        // Migration is forward-only to the current schema version.
        assert_eq!(current_version(&conn).unwrap(), SCHEMA_VERSION);

        // The jobs table now exists and is empty.
        let jobs: i64 = conn
            .query_row("SELECT COUNT(*) FROM jobs", [], |r| r.get(0))
            .expect("jobs table should exist");
        assert_eq!(jobs, 0);
    }

    #[test]
    fn migrate_v2_to_v3_preserves_jobs() {
        // Simulate a Phase 1 (v2) database: settings + jobs table with one job,
        // schema_version pinned at 2, and no molecules table.
        let conn = Connection::open_in_memory().expect("open in-memory db");
        conn.execute_batch(
            "CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);
             INSERT INTO settings (key, value) VALUES ('orca_path', '/opt/orca/orca');
             INSERT INTO settings (key, value) VALUES ('schema_version', '2');
             CREATE TABLE jobs (
                id            TEXT PRIMARY KEY,
                title         TEXT NOT NULL,
                input_content TEXT NOT NULL,
                status        TEXT NOT NULL DEFAULT 'draft',
                job_dir       TEXT,
                energy        REAL,
                wall_time     REAL,
                error_message TEXT,
                created_at    TEXT NOT NULL DEFAULT (datetime('now')),
                started_at    TEXT,
                completed_at  TEXT
             );
             INSERT INTO jobs (id, title, input_content) VALUES ('j1', 'water opt', '! r2SCAN-3c Opt');",
        )
        .expect("seed v2 schema");

        migrate(&conn).expect("v2 -> v3 migration should succeed");

        // migrate() always runs fully forward to the latest schema; the point of
        // this test is that the v2→v3 step (the molecules table) ran and left the
        // existing job untouched on the way.
        assert_eq!(current_version(&conn).unwrap(), SCHEMA_VERSION);

        // The existing job survived untouched.
        let title: String = conn
            .query_row("SELECT title FROM jobs WHERE id = 'j1'", [], |r| r.get(0))
            .expect("job preserved");
        assert_eq!(title, "water opt");

        // The molecules table now exists and is empty.
        let molecules: i64 = conn
            .query_row("SELECT COUNT(*) FROM molecules", [], |r| r.get(0))
            .expect("molecules table should exist");
        assert_eq!(molecules, 0);
    }

    #[test]
    fn migrate_v3_to_v4_preserves_jobs() {
        // Simulate a Phase 2.3 (v3) database: settings + jobs (without the
        // scene_json column) + molecules, schema_version pinned at 3, one job.
        let conn = Connection::open_in_memory().expect("open in-memory db");
        conn.execute_batch(
            "CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);
             INSERT INTO settings (key, value) VALUES ('orca_path', '/opt/orca/orca');
             INSERT INTO settings (key, value) VALUES ('schema_version', '3');
             CREATE TABLE jobs (
                id            TEXT PRIMARY KEY,
                title         TEXT NOT NULL,
                input_content TEXT NOT NULL,
                status        TEXT NOT NULL DEFAULT 'draft',
                job_dir       TEXT,
                energy        REAL,
                wall_time     REAL,
                error_message TEXT,
                created_at    TEXT NOT NULL DEFAULT (datetime('now')),
                started_at    TEXT,
                completed_at  TEXT
             );
             CREATE TABLE molecules (id TEXT PRIMARY KEY, name TEXT NOT NULL, xyz TEXT NOT NULL);
             INSERT INTO jobs (id, title, input_content) VALUES ('j1', 'water opt', '! r2SCAN-3c Opt');",
        )
        .expect("seed v3 schema");

        migrate(&conn).expect("v3 -> v4 migration should succeed");

        // Migrated fully forward; the v3→v4 step added the scene_json column.
        assert_eq!(current_version(&conn).unwrap(), SCHEMA_VERSION);

        // The pre-existing job survived untouched, and its new scene_json is NULL
        // (a job created before v4 carries no snapshot — not an anomaly).
        let (title, scene_json): (String, Option<String>) = conn
            .query_row(
                "SELECT title, scene_json FROM jobs WHERE id = 'j1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .expect("job preserved with a scene_json column");
        assert_eq!(title, "water opt");
        assert_eq!(scene_json, None);
    }

    #[test]
    fn migrate_v4_to_v5_adds_results_and_preserves_jobs() {
        // A Phase 2.5 (v4) database: jobs (with scene_json) + a completed job, no
        // results table.
        let conn = Connection::open_in_memory().expect("open in-memory db");
        conn.execute_batch(
            "CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);
             INSERT INTO settings (key, value) VALUES ('schema_version', '4');
             CREATE TABLE jobs (
                id TEXT PRIMARY KEY, title TEXT NOT NULL, input_content TEXT NOT NULL,
                status TEXT NOT NULL DEFAULT 'draft', job_dir TEXT, energy REAL,
                wall_time REAL, error_message TEXT,
                created_at TEXT NOT NULL DEFAULT (datetime('now')),
                started_at TEXT, completed_at TEXT, scene_json TEXT
             );
             INSERT INTO jobs (id, title, input_content, status)
                VALUES ('j1', 'ethane', '! r2SCAN-3c Opt Freq', 'completed');",
        )
        .expect("seed v4 schema");

        migrate(&conn).expect("v4 -> v5 migration should succeed");
        assert_eq!(current_version(&conn).unwrap(), SCHEMA_VERSION);

        // Existing completed job untouched.
        let status: String = conn
            .query_row("SELECT status FROM jobs WHERE id = 'j1'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(status, "completed");

        // The results table now exists and is empty.
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM results", [], |r| r.get(0))
            .expect("results table should exist");
        assert_eq!(n, 0);
    }

    #[test]
    fn migrate_v5_to_v6_adds_imaginary_count_via_guarded_alter() {
        // A DB stopped at v5: a results table WITHOUT imaginary_count.
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);
             INSERT INTO settings (key, value) VALUES ('schema_version', '5');
             CREATE TABLE jobs (id TEXT PRIMARY KEY);
             CREATE TABLE results (
                job_id TEXT PRIMARY KEY, final_energy_eh REAL, dipole_magnitude_au REAL,
                zpe_eh REAL, inner_energy_u_eh REAL, enthalpy_h_eh REAL, t_times_s_eh REAL,
                free_energy_g_eh REAL, data_json TEXT NOT NULL, parser_version INTEGER NOT NULL,
                parsed_at TEXT NOT NULL DEFAULT (datetime('now'))
             );",
        )
        .unwrap();
        assert!(!column_exists(&conn, "results", "imaginary_count").unwrap());

        migrate(&conn).expect("v5 -> v6 should add the column");
        assert_eq!(current_version(&conn).unwrap(), SCHEMA_VERSION);
        assert!(column_exists(&conn, "results", "imaginary_count").unwrap());
        // migrate() runs fully forward, so v7's column is present too.
        assert!(column_exists(&conn, "results", "homo_lumo_gap_eh").unwrap());
    }

    #[test]
    fn migrate_v6_to_v7_adds_homo_lumo_gap() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);
             INSERT INTO settings (key, value) VALUES ('schema_version', '6');
             CREATE TABLE jobs (id TEXT PRIMARY KEY);
             CREATE TABLE results (
                job_id TEXT PRIMARY KEY, final_energy_eh REAL, dipole_magnitude_au REAL,
                zpe_eh REAL, inner_energy_u_eh REAL, enthalpy_h_eh REAL, t_times_s_eh REAL,
                free_energy_g_eh REAL, imaginary_count INTEGER, data_json TEXT NOT NULL,
                parser_version INTEGER NOT NULL, parsed_at TEXT NOT NULL DEFAULT (datetime('now'))
             );",
        )
        .unwrap();
        assert!(!column_exists(&conn, "results", "homo_lumo_gap_eh").unwrap());
        migrate(&conn).expect("v6 -> v7");
        assert_eq!(current_version(&conn).unwrap(), SCHEMA_VERSION);
        assert!(column_exists(&conn, "results", "homo_lumo_gap_eh").unwrap());
    }

    #[test]
    fn migrate_v7_to_v8_backfills_energy_from_results() {
        // A pre-v8 DB: two completed jobs both parsed (results present), but only
        // ONE has jobs.energy filled — the other is NULL (the output.out tail
        // missed the far-back final energy; unit 3.9 defect 2). A third job has a
        // NULL energy and NO results row (never parsed) → must stay NULL.
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);
             INSERT INTO settings (key, value) VALUES ('schema_version', '7');
             CREATE TABLE jobs (id TEXT PRIMARY KEY, energy REAL, wall_time REAL);
             INSERT INTO jobs (id, energy) VALUES ('big', NULL);
             INSERT INTO jobs (id, energy) VALUES ('small', -76.4);
             INSERT INTO jobs (id, energy) VALUES ('unparsed', NULL);
             CREATE TABLE results (
                job_id TEXT PRIMARY KEY, final_energy_eh REAL, dipole_magnitude_au REAL,
                zpe_eh REAL, inner_energy_u_eh REAL, enthalpy_h_eh REAL, t_times_s_eh REAL,
                free_energy_g_eh REAL, imaginary_count INTEGER, homo_lumo_gap_eh REAL,
                data_json TEXT NOT NULL, parser_version INTEGER NOT NULL,
                parsed_at TEXT NOT NULL DEFAULT (datetime('now'))
             );
             INSERT INTO results (job_id, final_energy_eh, data_json, parser_version)
                VALUES ('big', -843.690396, '{}', 3);
             INSERT INTO results (job_id, final_energy_eh, data_json, parser_version)
                VALUES ('small', -76.418939, '{}', 3);",
        )
        .unwrap();

        migrate(&conn).expect("v7 -> v8");
        assert_eq!(current_version(&conn).unwrap(), SCHEMA_VERSION);

        let energy = |id: &str| -> Option<f64> {
            conn.query_row("SELECT energy FROM jobs WHERE id = ?1", [id], |r| {
                r.get::<_, Option<f64>>(0)
            })
            .unwrap()
        };
        // The NULL-energy parsed job is backfilled from results.
        assert_eq!(energy("big"), Some(-843.690396));
        // A job that already had an energy is untouched (not clobbered).
        assert_eq!(energy("small"), Some(-76.4));
        // A job with no results row stays NULL.
        assert_eq!(energy("unparsed"), None);
    }
}
