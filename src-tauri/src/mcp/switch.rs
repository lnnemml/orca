//! The MCP switch's row (ADR-027 M2 amendment, item 2): one `settings` row, `mcp_enabled`, holding
//! `"true"` or `"false"`. A missing row means off. The row is in the **shared** DB, so it is one
//! switch for every instance using that DB.
//!
//! The lifecycle's `set_enabled` is the row's **only writer**: the generic `set_setting` command
//! refuses the key ([`is_reserved_setting`]). Every tick re-reads it with [`read_enabled_conn`].

use rusqlite::{params, Connection, OptionalExtension};

use crate::error::AppError;

/// The `settings` key of the switch.
pub const MCP_ENABLED_KEY: &str = "mcp_enabled";

/// Whether the generic `set_setting` must refuse `key`: the switch's row has one writer.
pub fn is_reserved_setting(key: &str) -> bool {
    key == MCP_ENABLED_KEY
}

/// The switch as the DB holds it: `"true"` is on; a missing row is off. Any other value is refused
/// rather than read as either state (fails closed: the caller changes nothing on an error).
pub fn read_enabled_conn(conn: &Connection) -> Result<bool, AppError> {
    let value: Option<String> = conn
        .query_row("SELECT value FROM settings WHERE key = ?1", [MCP_ENABLED_KEY], |r| r.get(0))
        .optional()?;
    match value.as_deref() {
        None | Some("false") => Ok(false),
        Some("true") => Ok(true),
        Some(other) => Err(AppError::Invalid(format!("{MCP_ENABLED_KEY} holds `{other}`, not \"true\" or \"false\""))),
    }
}

/// Write the switch. Called only by the lifecycle's `set_enabled`, under the lifecycle mutex.
pub fn write_enabled_conn(conn: &Connection, on: bool) -> Result<(), AppError> {
    conn.execute(
        "INSERT INTO settings (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![MCP_ENABLED_KEY, if on { "true" } else { "false" }],
    )?;
    Ok(())
}
