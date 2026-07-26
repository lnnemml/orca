//! Settings commands: read/write the `settings` key/value table.

use std::collections::HashMap;
use std::sync::Mutex;

use rusqlite::{params, Connection};
use tauri::State;

use crate::error::AppError;

/// Managed state wrapping the single SQLite connection. `rusqlite::Connection`
/// is `Send` but not `Sync`, so a `Mutex` is required to share it across
/// Tauri's command threads.
pub struct DbState(pub Mutex<Connection>);

impl DbState {
    fn lock(&self) -> Result<std::sync::MutexGuard<'_, Connection>, AppError> {
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
    conn.execute(
        "INSERT INTO settings (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )?;
    Ok(())
}
