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
const SCHEMA_VERSION: i64 = 3;

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

    // Persist the resulting version so subsequent runs skip completed steps.
    conn.execute(
        "UPDATE settings SET value = ?1 WHERE key = 'schema_version'",
        rusqlite::params![version.to_string()],
    )?;
    debug_assert_eq!(version, SCHEMA_VERSION);
    Ok(())
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

        // Version advanced to 3.
        assert_eq!(current_version(&conn).unwrap(), 3);

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
}
