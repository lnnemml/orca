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
/// - v8: backfill `jobs.energy` from `results` (unit 3.9); a data migration, not a
///   schema change.
/// - v9: `manual_sections` + `manual_fts` (external-content FTS5) + `manual_provenance`
///   — the ORCA manual index (Phase 4.3, ADR-013). First tables the manual owns.
/// - v10: `jobs.index_map_json` — the serialized `IndexMap<OrcaIndex>` minted at
///   `create_job` (Phase 4.2, ADR-016 unit 1e). Nullable and additive; in unit 1d
///   EVERY row is NULL and the parser derives an identity map from the input
///   coordinate block instead (`results::job_index_map`). The column exists now so
///   the 1e minting has a home and this migration is not on 1e's critical path.
/// - v11: `jobs.scene_log_json` — the serialized operation log (ADR-017 unit 2b),
///   co-written with `scene_json` at `create_job`. Nullable and additive; "New
///   iteration" restores it, cross-checked against the snapshot.
/// - v12: `molecules.is_reagent` — a role flag (Phase 4.2 tail-2) so the molecules
///   table doubles as the user reagent catalog: a user-saved reagent is a molecules
///   row with `is_reagent = 1` plus its (mandatory) `charge`. Existing rows default
///   to 0 (not reagents), so the molecule library is unchanged. Guarded ALTER.
/// - v13: `reactions` + `pathways` tables and a nullable `jobs.pathway_id` (Phase 4.5
///   Stage C1, ADR-007 as amended). Jobs are the work; reactions/pathways are grouping
///   metadata — deleting a reaction/pathway NEVER deletes a job (the Rust commands null
///   `pathway_id` instead). Normalized: a job carries `pathway_id` ONLY (reaction derived
///   via `pathways`), no `reaction_id` on jobs. Nullable FK = standalone jobs unchanged.
///   Referential integrity is enforced BOTH by SQLite (FK enforcement is ON in this
///   build — `SQLITE_DEFAULT_FOREIGN_KEYS=1`, measured; see the v14 note) AND by the
///   commands (existence checks for clean `NotFound` errors, child-first delete order).
///   [Corrected 2026-08-09: this note previously claimed enforcement was off — wrong,
///   it contradicts the measured v14 reality.]
/// - v14: `reaction_reference_jobs` — the summed reactant reference for absolute barriers
///   (Phase 4.5 Stage C2b-2a, ADR-018). A lean join table: `(reaction_id, job_id)` PK, both
///   `REFERENCES` clauses (actively enforced — the bundled SQLite is built with
///   `SQLITE_DEFAULT_FOREIGN_KEYS=1`, measured; the commands additionally check existence for
///   clean `NotFound` errors and order deletes child-first). A reaction has 0+ reference
///   jobs whose parsed final energies SUM to E(ref); the sum is computed on demand from the
///   authoritative `results` tier, NEVER cached on `reactions` (no two-sources-of-truth
///   drift). Same jobs-survive rule as v13: deleting a reaction removes its reference rows,
///   the referenced jobs stay standalone. Additive + idempotent (CREATE IF NOT EXISTS).
/// - v15: two nullable `jobs` FKs — `source_ensemble_job_id` (TEXT) + `source_conformer_index`
///   (INTEGER) — tagging a DFT re-opt child back to the GOAT ensemble job + conformer it came
///   from (Phase 4.5 Stage D unit D2a). NO new table: the conformer set of a fan-out is DERIVED
///   by `GROUP BY source_ensemble_job_id` in D2b (Fork 1), never stored. Both nullable + additive
///   → every pre-existing job is a standalone job with both NULL. Jobs-survive: deleting the source
///   GOAT job nulls the link (in the commands), never cascades to the children. Guarded ALTER
///   (column_exists, like v10/v11). The REFERENCES clause documents intent, as elsewhere.
/// - v16: `groups` table (adjacency-list tree) + a nullable `jobs.group_id` (Phase 4.7.2,
///   ADR-019). Job groups are a **tree of metadata in SQLite, never a filesystem hierarchy** —
///   a job keeps its isolated `job_dir` (rule #3) wherever it sits; moving a job is `UPDATE
///   jobs.group_id`, zero fs ops. `groups(id, name, parent_id NULL REFERENCES groups, created_at)`;
///   `parent_id` is **NOT** `ON DELETE CASCADE` (a cascade would destroy a subtree — the opposite
///   of the promotion ADR-019 Decision 3 mandates; delete-with-promotion is done in the command).
///   `jobs.group_id … ON DELETE SET NULL` (one group per job — a tree, not tags). Both nullable +
///   additive → every pre-existing job is ungrouped (`group_id` NULL). Guarded ALTER (column_exists,
///   like v10/v11/v15). The `group_id` axis is ORTHOGONAL to pathway_id / source_ensemble_job_id /
///   reference rows (ADR-019 Decision 5).
const SCHEMA_VERSION: i64 = 17;

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
        INSERT OR IGNORE INTO settings (key, value) VALUES ('cpu_nprocs', '8');
        -- Explain-with-Claude model (Phase 4, ADR-015): a price/sufficiency choice, so a
        -- settings row (not a const). Sonnet 4.6 is sufficient for explain. Review at its
        -- deprecation / when the live /v1/models list drops it (ADR-015 amendment).
        INSERT OR IGNORE INTO settings (key, value) VALUES ('anthropic_model', 'claude-sonnet-4-6');",
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

    // --- v8 -> v9: the ORCA manual index (Phase 4.3, ADR-013). manual_sections +
    // an external-content FTS5 mirror (no 4 MB body duplication — the retrieval gate
    // chose the raw body column, and A-variant hit@5 88% justifies external-content)
    // + a provenance row. First tables the manual owns. ---
    if version < 9 {
        create_manual_tables(conn)?;
        version = 9;
    }

    // --- v9 -> v10: jobs.index_map_json (Phase 4.2, ADR-016). Nullable, additive.
    // The IndexMap<OrcaIndex> minted at create_job (unit 1e) is serialized here; in
    // unit 1d every row stays NULL and the parser derives an identity map from the
    // input coordinate block. Guarded ALTER, like v6/v7. ---
    if version < 10 {
        if column_exists(conn, "jobs", "id")? && !column_exists(conn, "jobs", "index_map_json")? {
            conn.execute_batch("ALTER TABLE jobs ADD COLUMN index_map_json TEXT;")?;
        }
        version = 10;
    }

    // --- v10 -> v11: jobs.scene_log_json (Phase 4.2, ADR-017 unit 2b). Nullable,
    // additive. The serialized operation log, co-written with scene_json at
    // create_job; "New iteration" restores it, cross-checked against the snapshot.
    // Guarded ALTER, like v6/v7/v10. ---
    if version < 11 {
        if column_exists(conn, "jobs", "id")? && !column_exists(conn, "jobs", "scene_log_json")? {
            conn.execute_batch("ALTER TABLE jobs ADD COLUMN scene_log_json TEXT;")?;
        }
        version = 11;
    }

    // --- v11 -> v12: molecules.is_reagent (Phase 4.2 tail-2). A role flag so the
    // molecules table doubles as the user reagent catalog: a user-saved reagent is
    // a molecules row with is_reagent=1 + its (mandatory) charge (the charge column
    // already exists). Existing rows default to 0 (not reagents), so the molecule
    // library and its screen are unchanged. Guarded ALTER, like v6/v7/v10/v11. ---
    if version < 12 {
        // Guard on the table existing (like v10/v11 guard on `jobs`): the migration
        // fixtures stub a DB at a version WITHOUT `molecules`, and migrate() runs every
        // remaining arm up to SCHEMA_VERSION. `column_exists` is Ok(false) for an absent
        // table (PRAGMA table_info returns empty), so this skips cleanly there.
        if column_exists(conn, "molecules", "id")?
            && !column_exists(conn, "molecules", "is_reagent")?
        {
            conn.execute_batch(
                "ALTER TABLE molecules ADD COLUMN is_reagent INTEGER NOT NULL DEFAULT 0;",
            )?;
        }
        version = 12;
    }

    // --- v12 -> v13: reaction/pathway data model (Phase 4.5 Stage C1, ADR-007
    // amended). Two grouping tables + a nullable jobs.pathway_id. Jobs are the work;
    // reactions/pathways are grouping metadata — deleting either NEVER deletes a job
    // (enforced in the Rust commands, which null pathway_id instead). Normalized: a
    // job carries pathway_id ONLY; its reaction is derived via `pathways` (no
    // reaction_id on jobs — deliberate deviation from ADR-007's both-FKs sketch, one
    // source of truth). Additive + idempotent: the CREATEs are IF NOT EXISTS, the
    // ALTER is column_exists-guarded (like v10/v11, on `jobs` existing). The
    // REFERENCES clauses document intent; app-level integrity lives in the commands. ---
    if version < 13 {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS reactions (
                id          TEXT PRIMARY KEY,
                name        TEXT NOT NULL,
                description TEXT,
                created_at  TEXT NOT NULL DEFAULT (datetime('now'))
            );
            CREATE TABLE IF NOT EXISTS pathways (
                id          TEXT PRIMARY KEY,
                reaction_id TEXT NOT NULL REFERENCES reactions(id),
                label       TEXT NOT NULL,
                created_at  TEXT NOT NULL DEFAULT (datetime('now'))
            );",
        )?;
        if column_exists(conn, "jobs", "id")? && !column_exists(conn, "jobs", "pathway_id")? {
            // ADD COLUMN with a REFERENCES clause is permitted because the default is
            // NULL (SQLite's only FK-on-ALTER restriction). Nullable + additive: every
            // existing job stays a standalone job with pathway_id = NULL.
            conn.execute_batch(
                "ALTER TABLE jobs ADD COLUMN pathway_id TEXT REFERENCES pathways(id);",
            )?;
        }
        version = 13;
    }

    // --- v13 -> v14: reaction_reference_jobs (Phase 4.5 Stage C2b-2a, ADR-018). The
    // summed reactant reference for ABSOLUTE barriers: a reaction has 0+ references to
    // optimized-reactant jobs whose parsed final energies SUM to E(ref). A lean join
    // table mirroring the v13 reactions/pathways normalization — a reference row is
    // grouping metadata, NEVER a job (the same jobs-survive rule: delete_reaction removes
    // these rows in the commands, the referenced jobs stay standalone). The (reaction_id,
    // job_id) PK makes add_reference_job idempotent. Integrity (both ids must exist) is
    // enforced in the commands; the REFERENCES clauses document intent, as elsewhere.
    // Additive + idempotent: CREATE IF NOT EXISTS, no data touched. ---
    if version < 14 {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS reaction_reference_jobs (
                reaction_id TEXT NOT NULL REFERENCES reactions(id),
                job_id      TEXT NOT NULL REFERENCES jobs(id),
                created_at  TEXT NOT NULL DEFAULT (datetime('now')),
                PRIMARY KEY (reaction_id, job_id)
            );",
        )?;
        version = 14;
    }

    // --- v14 -> v15: DFT re-opt fan-out linkage (Phase 4.5 Stage D unit D2a). Two nullable
    // FKs on `jobs` tagging a re-opt child back to its source: `source_ensemble_job_id` (the
    // GOAT ensemble job) + `source_conformer_index` (which conformer of that ensemble). NO new
    // table — the conformer set of a fan-out is DERIVED later by GROUP BY source_ensemble_job_id
    // (Fork 1), not stored. Nullable + additive: every existing job stays standalone with both
    // NULL. Jobs-survive (like v13/v14): deleting the source GOAT job nulls these in the commands,
    // never deletes the children. Guarded ALTER (column_exists on `jobs`, like v10/v11); the
    // REFERENCES clause documents intent (app-level integrity in the commands). ---
    if version < 15 {
        if column_exists(conn, "jobs", "id")?
            && !column_exists(conn, "jobs", "source_ensemble_job_id")?
        {
            // Nullable-default ADD COLUMN with a REFERENCES clause is permitted (SQLite's only
            // FK-on-ALTER restriction is a non-NULL default). Two separate ALTERs (SQLite adds
            // one column per statement).
            conn.execute_batch(
                "ALTER TABLE jobs ADD COLUMN source_ensemble_job_id TEXT REFERENCES jobs(id);
                 ALTER TABLE jobs ADD COLUMN source_conformer_index INTEGER;",
            )?;
        }
        version = 15;
    }

    // --- v15 -> v16: job groups (Phase 4.7.2, ADR-019). An adjacency-list tree of grouping
    // metadata (`groups`) plus a nullable `jobs.group_id`. Groups are NOT directories: a job
    // keeps its isolated `job_dir` (rule #3) wherever it sits in the tree; moving a job is a
    // one-row `UPDATE jobs.group_id` with ZERO filesystem ops (ADR-019 Decision 0). `parent_id`
    // is deliberately NOT `ON DELETE CASCADE` — deleting a group PROMOTES its children to the
    // deleted group's parent (Decision 3), done explicitly in `delete_group_conn`; a cascade would
    // destroy the subtree. `jobs.group_id … ON DELETE SET NULL` is the fallback (an orphaned job
    // drops to root) but the command re-parents jobs to the PARENT explicitly, so SET NULL only
    // fires if a group row is ever removed outside the command. Additive + idempotent. ---
    if version < 16 {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS groups (
                id         TEXT PRIMARY KEY,
                name       TEXT NOT NULL,
                parent_id  TEXT REFERENCES groups(id),
                created_at TEXT NOT NULL DEFAULT (datetime('now'))
            );",
        )?;
        if column_exists(conn, "jobs", "id")? && !column_exists(conn, "jobs", "group_id")? {
            // Nullable-default ADD COLUMN with a REFERENCES clause is permitted (SQLite's only
            // FK-on-ALTER restriction is a non-NULL default). `ON DELETE SET NULL` is legal here.
            conn.execute_batch(
                "ALTER TABLE jobs ADD COLUMN group_id TEXT REFERENCES groups(id) ON DELETE SET NULL;",
            )?;
        }
        version = 16;
    }

    // --- v16 -> v17: NEB two-file jobs (Phase 4.5 Stage E3a-1). A NEB-TS job needs a
    // SECOND file in its isolated dir — the product end image `product.xyz` the `%neb`
    // block references by relative path. `aux_files_json` is a nullable JSON object
    // {filename: content}, written at create time and MATERIALIZED into the job dir at
    // run (`prepare_job_dir`), so the "one dir, created at run" invariant (rule #3) is
    // unchanged — no pre-created draft dirs. Generic (any job type could carry aux
    // files); NULL for every existing/normal job. Additive + idempotent. ---
    if version < 17 {
        if column_exists(conn, "jobs", "id")? && !column_exists(conn, "jobs", "aux_files_json")? {
            conn.execute_batch("ALTER TABLE jobs ADD COLUMN aux_files_json TEXT;")?;
        }
        version = 17;
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

/// Create the manual-index tables (schema v9, ADR-013). Factored out so tests and
/// the ingest can build them on an in-memory DB.
///
/// `manual_sections` is the row-per-section store. The `anchor` is **nullable** and
/// paired with `anchor_source`: it is filled ONLY where the section's label was
/// cross-checked against `objects.inv` (1068 of 1586 on ORCA 6.1). For the rest the
/// correct HTML anchor is UNDETERMINED (rule #11) — `objects.inv` carries only
/// explicit labels, Sphinx auto-generates the others from the heading slug with
/// traversal-state suffixes we cannot recompute, and ~140 unlabelled sections
/// collide on that slug within a file. A guessed anchor points at a fragment that
/// does not exist and reads as "the manual moved", so we store NULL and let the
/// link land on the page. The synthetic `id` PK is deliberate: neither `anchor` nor
/// `(file, title_slug)` is unique (the collision above).
///
/// `manual_fts` is an **external-content** FTS5 over `manual_sections` — it stores
/// the search index but reads column values from the base table by `rowid=id`, so
/// the body text is not duplicated. Rebuilt wholesale on each ingest
/// (`INSERT INTO manual_fts(manual_fts) VALUES('rebuild')`). `bm25` is ASC
/// (less = more relevant — `fts5_is_available_with_ranking_and_snippet`).
pub(crate) fn create_manual_tables(conn: &Connection) -> Result<(), AppError> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS manual_sections (
            id            INTEGER PRIMARY KEY,
            orca_version  TEXT NOT NULL,
            file          TEXT NOT NULL,
            level         INTEGER NOT NULL,
            title         TEXT NOT NULL,
            breadcrumb    TEXT NOT NULL,        -- JSON array of ancestor titles
            labels        TEXT NOT NULL,        -- JSON array of MyST labels
            anchor        TEXT,                 -- NULL when UNDETERMINED (rule #11)
            anchor_source TEXT NOT NULL,        -- 'objects_inv' | 'undetermined'
            body_md       TEXT NOT NULL,
            line_start    INTEGER NOT NULL,
            line_end      INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_manual_sections_version_file
            ON manual_sections(orca_version, file);

        CREATE VIRTUAL TABLE IF NOT EXISTS manual_fts USING fts5(
            title, breadcrumb, body_md,
            content='manual_sections', content_rowid='id',
            tokenize='porter unicode61'
        );

        CREATE TABLE IF NOT EXISTS manual_provenance (
            orca_version        TEXT PRIMARY KEY,
            base_url            TEXT,
            corpus_collected_at TEXT,
            corpus_hash         TEXT NOT NULL,
            sectioner_version   INTEGER NOT NULL,
            section_count       INTEGER NOT NULL,
            anchors_verified    INTEGER NOT NULL,
            indexed_at          TEXT NOT NULL DEFAULT (datetime('now'))
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

    #[test]
    fn migrate_v8_to_v9_adds_manual_tables_and_preserves_data() {
        // A v8 DB: settings + jobs with one job. The v9 step adds the manual tables.
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);
             INSERT INTO settings (key, value) VALUES ('schema_version', '8');
             CREATE TABLE jobs (id TEXT PRIMARY KEY, energy REAL);
             INSERT INTO jobs (id, energy) VALUES ('j1', -76.4);",
        )
        .unwrap();

        migrate(&conn).expect("v8 -> v9");
        assert_eq!(current_version(&conn).unwrap(), SCHEMA_VERSION);

        // The pre-existing job is untouched.
        let e: Option<f64> = conn
            .query_row("SELECT energy FROM jobs WHERE id = 'j1'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(e, Some(-76.4));

        // The manual tables now exist and are empty; the external-content FTS works.
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM manual_sections", [], |r| r.get(0))
            .expect("manual_sections exists");
        assert_eq!(n, 0);
        conn.execute_batch(
            "INSERT INTO manual_sections
                (id, orca_version, file, level, title, breadcrumb, labels, anchor,
                 anchor_source, body_md, line_start, line_end)
             VALUES (1, '6.1', 'contents/x', 2, 'RIJCOSX', '[]', '[]', 'sec-x',
                 'objects_inv', 'the RIJCOSX approximation', 0, 3);
             INSERT INTO manual_fts(manual_fts) VALUES('rebuild');",
        )
        .expect("insert + rebuild external-content FTS");
        let hits: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM manual_fts WHERE manual_fts MATCH 'rijcosx'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(hits, 1, "FTS indexes the base table's body_md");
        // A NULL anchor is allowed (UNDETERMINED, rule #11).
        conn.execute_batch(
            "INSERT INTO manual_sections
                (id, orca_version, file, level, title, breadcrumb, labels, anchor,
                 anchor_source, body_md, line_start, line_end)
             VALUES (2, '6.1', 'contents/y', 2, 'Keywords', '[]', '[]', NULL,
                 'undetermined', 'body', 0, 1);",
        )
        .expect("NULL anchor is permitted");
    }

    #[test]
    fn migrate_v9_to_v10_adds_index_map_json_and_preserves_jobs() {
        // A v9 DB with one job carrying input_content. The v10 step adds the nullable
        // index_map_json column; the pre-existing job is untouched and its new column
        // is NULL (unit 1d: every row NULL, the parser derives an identity map).
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);
             INSERT INTO settings (key, value) VALUES ('schema_version', '9');
             CREATE TABLE jobs (id TEXT PRIMARY KEY, input_content TEXT NOT NULL);
             INSERT INTO jobs (id, input_content) VALUES ('j1', '* xyz 0 1');",
        )
        .unwrap();
        assert!(!column_exists(&conn, "jobs", "index_map_json").unwrap());

        migrate(&conn).expect("v9 -> v10");
        assert_eq!(current_version(&conn).unwrap(), SCHEMA_VERSION);
        assert!(column_exists(&conn, "jobs", "index_map_json").unwrap());

        // Pre-existing job preserved; the new column is NULL for it.
        let (input, map): (String, Option<String>) = conn
            .query_row(
                "SELECT input_content, index_map_json FROM jobs WHERE id = 'j1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(input, "* xyz 0 1");
        assert_eq!(map, None, "unit 1d: every row is NULL (minting is unit 1e)");
    }

    #[test]
    fn migrate_v10_to_v11_adds_scene_log_json_and_preserves_jobs() {
        // A v10 DB with one job. The v11 step adds the nullable scene_log_json column
        // (unit 2b); the pre-existing job is untouched and its new column is NULL (a
        // legacy job seeds a fresh log on New iteration).
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);
             INSERT INTO settings (key, value) VALUES ('schema_version', '10');
             CREATE TABLE jobs (id TEXT PRIMARY KEY, input_content TEXT NOT NULL);
             INSERT INTO jobs (id, input_content) VALUES ('j1', '* xyz 0 1');",
        )
        .unwrap();
        assert!(!column_exists(&conn, "jobs", "scene_log_json").unwrap());

        migrate(&conn).expect("v10 -> v11");
        assert_eq!(current_version(&conn).unwrap(), SCHEMA_VERSION);
        assert!(column_exists(&conn, "jobs", "scene_log_json").unwrap());

        let (input, log): (String, Option<String>) = conn
            .query_row(
                "SELECT input_content, scene_log_json FROM jobs WHERE id = 'j1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(input, "* xyz 0 1");
        assert_eq!(log, None, "legacy job: NULL log (seeds fresh on New iteration)");
    }

    #[test]
    fn migrate_v12_to_v13_adds_reaction_tables_and_preserves_data() {
        // A v12 DB: settings + jobs (one populated job) + molecules (one row). The
        // v13 step adds `reactions`/`pathways` and a nullable `jobs.pathway_id`; the
        // pre-existing data is untouched and the new column is NULL for the old job
        // (invariant 3: pathway_id = NULL is the normal state for every existing job).
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);
             INSERT INTO settings (key, value) VALUES ('schema_version', '12');
             CREATE TABLE jobs (
                id TEXT PRIMARY KEY, title TEXT NOT NULL, input_content TEXT NOT NULL,
                status TEXT NOT NULL DEFAULT 'draft', energy REAL,
                created_at TEXT NOT NULL DEFAULT (datetime('now'))
             );
             INSERT INTO jobs (id, title, input_content, status, energy)
                VALUES ('j1', 'ethane scan', '! r2SCAN-3c Opt', 'completed', -79.8);
             CREATE TABLE molecules (
                id TEXT PRIMARY KEY, name TEXT NOT NULL, xyz TEXT NOT NULL,
                is_reagent INTEGER NOT NULL DEFAULT 0
             );
             INSERT INTO molecules (id, name, xyz) VALUES ('m1', 'ethane', '* xyz 0 1');",
        )
        .unwrap();
        assert!(!column_exists(&conn, "jobs", "pathway_id").unwrap());

        migrate(&conn).expect("v12 -> v13");
        assert_eq!(current_version(&conn).unwrap(), SCHEMA_VERSION);

        // The two grouping tables now exist and are empty.
        let reactions: i64 = conn
            .query_row("SELECT COUNT(*) FROM reactions", [], |r| r.get(0))
            .expect("reactions table should exist");
        assert_eq!(reactions, 0);
        let pathways: i64 = conn
            .query_row("SELECT COUNT(*) FROM pathways", [], |r| r.get(0))
            .expect("pathways table should exist");
        assert_eq!(pathways, 0);

        // jobs.pathway_id exists and is NULL for the pre-existing job; the rest of the
        // job is intact.
        assert!(column_exists(&conn, "jobs", "pathway_id").unwrap());
        let (title, energy, pathway_id): (String, Option<f64>, Option<String>) = conn
            .query_row(
                "SELECT title, energy, pathway_id FROM jobs WHERE id = 'j1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .expect("job preserved with a pathway_id column");
        assert_eq!(title, "ethane scan");
        assert_eq!(energy, Some(-79.8));
        assert_eq!(pathway_id, None, "every existing job is standalone (pathway_id NULL)");

        // The molecule row is intact.
        let mol_name: String = conn
            .query_row("SELECT name FROM molecules WHERE id = 'm1'", [], |r| r.get(0))
            .expect("molecule preserved");
        assert_eq!(mol_name, "ethane");
    }

    #[test]
    fn migrate_v13_to_v14_adds_reference_jobs_and_preserves_data() {
        // A v13 DB: settings + jobs (one job) + reactions + pathways (one each) with a
        // job attached to the pathway. The v14 step adds `reaction_reference_jobs`; all
        // pre-existing data (jobs, reactions, pathways, the pathway_id grouping) is
        // untouched (C-migrate-preserves).
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);
             INSERT INTO settings (key, value) VALUES ('schema_version', '13');
             CREATE TABLE jobs (
                id TEXT PRIMARY KEY, title TEXT NOT NULL, input_content TEXT NOT NULL,
                status TEXT NOT NULL DEFAULT 'draft', energy REAL,
                created_at TEXT NOT NULL DEFAULT (datetime('now')), pathway_id TEXT
             );
             CREATE TABLE reactions (
                id TEXT PRIMARY KEY, name TEXT NOT NULL, description TEXT,
                created_at TEXT NOT NULL DEFAULT (datetime('now'))
             );
             CREATE TABLE pathways (
                id TEXT PRIMARY KEY, reaction_id TEXT NOT NULL, label TEXT NOT NULL,
                created_at TEXT NOT NULL DEFAULT (datetime('now'))
             );
             INSERT INTO reactions (id, name) VALUES ('r1', 'NaBH4 reduction');
             INSERT INTO pathways (id, reaction_id, label) VALUES ('p1', 'r1', 'si-face');
             INSERT INTO jobs (id, title, input_content, status, energy, pathway_id)
                VALUES ('j1', 'si scan', '! r2SCAN-3c Opt', 'completed', -79.8, 'p1');",
        )
        .unwrap();
        assert!(!column_exists(&conn, "reaction_reference_jobs", "job_id").unwrap());

        migrate(&conn).expect("v13 -> v14");
        assert_eq!(current_version(&conn).unwrap(), SCHEMA_VERSION);

        // The new join table now exists and is empty.
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM reaction_reference_jobs", [], |r| r.get(0))
            .expect("reaction_reference_jobs table should exist");
        assert_eq!(n, 0);

        // All pre-existing data is intact — reaction, pathway, and the attached job with
        // its grouping FK preserved.
        let rname: String = conn
            .query_row("SELECT name FROM reactions WHERE id = 'r1'", [], |r| r.get(0))
            .expect("reaction preserved");
        assert_eq!(rname, "NaBH4 reduction");
        let (title, energy, pathway_id): (String, Option<f64>, Option<String>) = conn
            .query_row(
                "SELECT title, energy, pathway_id FROM jobs WHERE id = 'j1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .expect("job preserved");
        assert_eq!(title, "si scan");
        assert_eq!(energy, Some(-79.8));
        assert_eq!(pathway_id.as_deref(), Some("p1"), "the grouping FK is untouched");

        // Idempotent: a second migrate() is a no-op.
        migrate(&conn).expect("v14 -> v14 no-op");
        assert_eq!(current_version(&conn).unwrap(), SCHEMA_VERSION);
    }

    #[test]
    fn migrate_v14_to_v15_adds_source_fks_and_preserves_jobs() {
        // A v14 DB: settings + jobs (one job). The v15 step adds the two nullable re-opt
        // linkage columns; the pre-existing job is untouched and both new columns are NULL
        // on it (every old job is standalone — not a re-opt child).
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);
             INSERT INTO settings (key, value) VALUES ('schema_version', '14');
             CREATE TABLE jobs (
                id TEXT PRIMARY KEY, title TEXT NOT NULL, input_content TEXT NOT NULL,
                status TEXT NOT NULL DEFAULT 'draft', energy REAL,
                created_at TEXT NOT NULL DEFAULT (datetime('now')), pathway_id TEXT
             );
             INSERT INTO jobs (id, title, input_content, status, energy)
                VALUES ('g1', 'Conformer search — butane', '! XTB GOAT', 'completed', -13.66);",
        )
        .unwrap();
        assert!(!column_exists(&conn, "jobs", "source_ensemble_job_id").unwrap());
        assert!(!column_exists(&conn, "jobs", "source_conformer_index").unwrap());

        migrate(&conn).expect("v14 -> v15");
        assert_eq!(current_version(&conn).unwrap(), SCHEMA_VERSION);

        // The two columns now exist and are NULL on the pre-existing job.
        let (title, energy, src_job, src_idx): (String, Option<f64>, Option<String>, Option<i64>) =
            conn.query_row(
                "SELECT title, energy, source_ensemble_job_id, source_conformer_index
                 FROM jobs WHERE id = 'g1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .expect("job preserved with the two new columns");
        assert_eq!(title, "Conformer search — butane");
        assert_eq!(energy, Some(-13.66));
        assert_eq!(src_job, None, "an existing job is not a re-opt child (link NULL)");
        assert_eq!(src_idx, None, "an existing job has no source conformer index");

        // A child can carry both FKs (the shape D2a writes).
        conn.execute_batch(
            "INSERT INTO jobs (id, title, input_content, status, source_ensemble_job_id, source_conformer_index)
             VALUES ('c1', 're-opt #0 — Conformer search — butane', '! r2SCAN-3c Opt Freq', 'queued', 'g1', 0);",
        )
        .unwrap();
        let (src_job, src_idx): (Option<String>, Option<i64>) = conn
            .query_row(
                "SELECT source_ensemble_job_id, source_conformer_index FROM jobs WHERE id = 'c1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(src_job.as_deref(), Some("g1"));
        assert_eq!(src_idx, Some(0));

        // Idempotent: a second migrate() is a no-op.
        migrate(&conn).expect("v15 -> v15 no-op");
        assert_eq!(current_version(&conn).unwrap(), SCHEMA_VERSION);
    }

    #[test]
    fn migrate_v15_to_v16_adds_groups_table_and_group_id_and_preserves_jobs() {
        // A v15 DB: settings + jobs (one job). The v16 step adds the `groups` table and a
        // nullable `jobs.group_id`; the pre-existing job is untouched and `group_id` is NULL
        // on it (every old job is ungrouped).
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);
             INSERT INTO settings (key, value) VALUES ('schema_version', '15');
             CREATE TABLE jobs (
                id TEXT PRIMARY KEY, title TEXT NOT NULL, input_content TEXT NOT NULL,
                status TEXT NOT NULL DEFAULT 'draft', energy REAL,
                created_at TEXT NOT NULL DEFAULT (datetime('now')), pathway_id TEXT
             );
             INSERT INTO jobs (id, title, input_content, status, energy)
                VALUES ('j1', 'water opt', '! r2SCAN-3c Opt', 'completed', -76.42);",
        )
        .unwrap();
        assert!(!column_exists(&conn, "jobs", "group_id").unwrap());

        migrate(&conn).expect("v15 -> v16");
        assert_eq!(current_version(&conn).unwrap(), SCHEMA_VERSION);

        // The `groups` table exists and `jobs.group_id` is NULL on the pre-existing job.
        assert!(column_exists(&conn, "groups", "id").unwrap());
        assert!(column_exists(&conn, "jobs", "group_id").unwrap());
        let (title, energy, group_id): (String, Option<f64>, Option<String>) = conn
            .query_row(
                "SELECT title, energy, group_id FROM jobs WHERE id = 'j1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .expect("job preserved with a group_id column");
        assert_eq!(title, "water opt");
        assert_eq!(energy, Some(-76.42));
        assert_eq!(group_id, None, "every existing job is ungrouped (group_id NULL)");

        // A group with a self-referential parent chain is representable (adjacency list).
        conn.execute_batch(
            "INSERT INTO groups (id, name, parent_id) VALUES ('r', 'root study', NULL);
             INSERT INTO groups (id, name, parent_id) VALUES ('s', 'sub', 'r');
             UPDATE jobs SET group_id = 's' WHERE id = 'j1';",
        )
        .unwrap();
        let (parent, gid): (Option<String>, Option<String>) = conn
            .query_row(
                "SELECT g.parent_id, j.group_id FROM groups g, jobs j WHERE g.id = 's' AND j.id = 'j1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(parent.as_deref(), Some("r"));
        assert_eq!(gid.as_deref(), Some("s"));

        // Idempotent: a second migrate() is a no-op.
        migrate(&conn).expect("v16 -> v16 no-op");
        assert_eq!(current_version(&conn).unwrap(), SCHEMA_VERSION);
    }

    /// FTS5 build-gate. NOT a migration and NOT a table the app ships — a tripwire
    /// for a future `rusqlite` / `libsqlite3-sys` bump. Measured: `libsqlite3-sys`
    /// 0.30.1 under `build_bundled` compiles the SQLite 3.46.0 amalgamation with
    /// `-DSQLITE_ENABLE_FTS5` UNCONDITIONALLY, so FTS5 + `porter`/`unicode61`
    /// tokenizers + `snippet()` + `bm25()` all work today. Phase 4's manual index
    /// (ADR-013) stands on exactly this. If a future upgrade ever stops carrying
    /// FTS5 in the build, this fails HERE, up front, instead of deep in Phase 4.
    #[test]
    fn fts5_is_available_with_ranking_and_snippet() {
        let conn = Connection::open_in_memory().unwrap();

        // A virtual table with two columns and the porter+unicode61 tokenizer.
        // The CREATE itself is the first half of the gate: it only compiles if the
        // linked SQLite carries the FTS5 module.
        conn.execute_batch(
            "CREATE VIRTUAL TABLE docs USING fts5(
                 title, body, tokenize = 'porter unicode61'
             );
             INSERT INTO docs (title, body) VALUES
                ('RIJCOSX', 'The RIJ-COSX approximation accelerates hybrid DFT calculations.'),
                ('CPCM',    'The conductor-like polarizable continuum model describes implicit solvation.'),
                ('Geometry','Geometry optimization searches for a stationary point on the surface.');",
        )
        .expect("CREATE VIRTUAL TABLE ... USING fts5 must compile — the build carries FTS5");

        // MATCH + snippet(): snippet wraps the hit in the given delimiters. Column
        // index 1 = body (0-based). 'solvation' lives in the CPCM row's body.
        let snippet: String = conn
            .query_row(
                "SELECT snippet(docs, 1, '[', ']', '…', 8)
                 FROM docs WHERE docs MATCH 'solvation'",
                [],
                |r| r.get(0),
            )
            .expect("MATCH should find the CPCM row");
        assert!(
            snippet.contains("[solvation]"),
            "snippet() should mark the matched term: got {snippet:?}"
        );

        // porter stemming is live: a query for 'accelerate' matches stored
        // 'accelerates' (both stem to the same root).
        let stemmed: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM docs WHERE docs MATCH 'accelerate'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(stemmed, 1, "porter tokenizer should stem 'accelerates' → 'accelerate'");

        // Ranking. Add a short, high-term-frequency row so it must outrank the
        // single-mention 'Geometry' row on the query 'geometry'.
        conn.execute(
            "INSERT INTO docs (title, body) VALUES
                ('Opt', 'geometry geometry geometry geometry geometry')",
            [],
        )
        .unwrap();

        let mut stmt = conn
            .prepare(
                "SELECT title, bm25(docs) FROM docs
                 WHERE docs MATCH 'geometry' ORDER BY bm25(docs)",
            )
            .unwrap();
        let ranked: Vec<(String, f64)> = stmt
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, f64>(1)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect();

        assert_eq!(ranked.len(), 2, "two rows mention 'geometry'");

        // The counter-intuitive core, and why this is a test not a comment:
        // SQLite's bm25() returns NEGATIVE scores where MORE negative = MORE
        // relevant. So `ORDER BY bm25(docs)` WITHOUT `DESC` (ascending) puts the
        // best match first. The high-frequency 'Opt' row must lead.
        assert_eq!(ranked[0].0, "Opt", "more mentions ⇒ more relevant ⇒ first under ascending bm25");
        assert!(ranked[0].1 < 0.0, "bm25 scores are negative: {}", ranked[0].1);
        assert!(
            ranked[0].1 < ranked[1].1,
            "ascending order = most relevant first: {} should be < {}",
            ranked[0].1,
            ranked[1].1,
        );
    }
}
