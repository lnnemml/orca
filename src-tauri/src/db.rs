//! SQLite storage: open the single `orcastudio.db` and run migrations.
//!
//! Per ADR-004 the whole app is backed by one SQLite file in the user data
//! directory. Phase 0 only needs a `settings` key/value table.

use std::path::Path;

use rusqlite::Connection;

use crate::error::AppError;

/// Current schema version. Bump this and add a migration arm when the schema
/// changes.
const SCHEMA_VERSION: i64 = 1;

/// Open (creating if needed) `orcastudio.db` under `data_dir` and migrate it to
/// the current schema.
pub fn init_db(data_dir: &Path) -> Result<Connection, AppError> {
    std::fs::create_dir_all(data_dir)?;
    let db_path = data_dir.join("orcastudio.db");
    let conn = Connection::open(db_path)?;
    migrate(&conn)?;
    Ok(conn)
}

/// Apply migrations idempotently. Uses `IF NOT EXISTS` / `INSERT OR IGNORE` so
/// re-running on an existing database is safe.
fn migrate(conn: &Connection) -> Result<(), AppError> {
    conn.execute_batch(&format!(
        "CREATE TABLE IF NOT EXISTS settings (
            key   TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );
        INSERT OR IGNORE INTO settings (key, value) VALUES ('orca_path', '/opt/orca/orca');
        INSERT OR IGNORE INTO settings (key, value) VALUES ('schema_version', '{SCHEMA_VERSION}');"
    ))?;
    Ok(())
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
}
