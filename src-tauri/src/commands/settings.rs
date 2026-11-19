//! Settings commands: read/write the `settings` key/value table.

use std::collections::HashMap;
use std::sync::Mutex;

use rusqlite::{params, Connection};
use tauri::State;

use crate::error::AppError;
use crate::mcp::switch::is_reserved_setting;

/// Managed state wrapping the single SQLite connection. `rusqlite::Connection`
/// is `Send` but not `Sync`, so a `Mutex` is required to share it across
/// Tauri's command threads.
pub struct DbState(pub Mutex<Connection>);

impl DbState {
    pub(crate) fn lock(&self) -> Result<std::sync::MutexGuard<'_, Connection>, AppError> {
        self.0
            .lock()
            .map_err(|_| AppError::Internal("database mutex poisoned".into()))
    }
}

/// Return all settings as a `key -> value` map.
#[tauri::command]
pub fn get_settings(db: State<'_, DbState>) -> Result<HashMap<String, String>, AppError> {
    let conn = db.lock()?;
    let mut stmt = conn.prepare("SELECT key, value FROM settings")?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;

    let mut map = HashMap::new();
    for row in rows {
        let (key, value) = row?;
        map.insert(key, value);
    }
    Ok(map)
}

/// Upsert a single setting.
#[tauri::command]
pub fn set_setting(db: State<'_, DbState>, key: String, value: String) -> Result<(), AppError> {
    let conn = db.lock()?;
    set_setting_conn(&conn, &key, &value)
}

/// The body of [`set_setting`]. It refuses the MCP switch's key (ADR-027 M2 amendment, item 2): the
/// MCP lifecycle's `set_enabled` is that row's only writer, so a write here could neither take the
/// lifecycle mutex nor respect the `exiting` rule. Nothing is written on a refusal.
pub(crate) fn set_setting_conn(conn: &Connection, key: &str, value: &str) -> Result<(), AppError> {
    if is_reserved_setting(key) {
        return Err(AppError::Invalid(format!(
            "the setting `{key}` is written only by the MCP switch (Settings → MCP), not by set_setting"
        )));
    }
    conn.execute(
        "INSERT INTO settings (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::switch::{read_enabled_conn, write_enabled_conn, MCP_ENABLED_KEY};
    use crate::mcp::test_db::TestDb;

    /// Control (M2 amendment item 1): `mcp_enabled` writable through the generic `set_setting`.
    #[test]
    fn set_setting_refuses_the_mcp_switch_and_writes_nothing() {
        let db = TestDb::new("setting-guard");
        write_enabled_conn(&db.conn, false).unwrap();

        let refused = set_setting_conn(&db.conn, MCP_ENABLED_KEY, "true");
        assert!(matches!(refused, Err(AppError::Invalid(_))), "{refused:?}");
        assert!(!read_enabled_conn(&db.conn).unwrap(), "the refused write must leave the row off");
    }

    #[test]
    fn set_setting_still_writes_every_other_key() {
        let db = TestDb::new("setting-other");
        set_setting_conn(&db.conn, "orca_path", "/opt/orca/orca").unwrap();
        let v: String =
            db.conn.query_row("SELECT value FROM settings WHERE key = 'orca_path'", [], |r| r.get(0)).unwrap();
        assert_eq!(v, "/opt/orca/orca");
    }
}
