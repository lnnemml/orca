//! The audit log as Settings → MCP reads it (ADR-027 M4 amendment, item 4; unit M4a Part A): pages
//! of `mcp_audit`, newest first, and one whole row on demand. The commands are `commands::mcp`
//! (`mcp_list_audit`, `mcp_audit_row`).
//!
//! - **Keyset paging on `id`.** `mcp_audit.id` is an `INTEGER PRIMARY KEY` and no code deletes an
//!   audit row, so ids only grow: a page is `id < before_id` (or the newest), `ORDER BY id DESC`,
//!   and the next page starts below the last row shown. No row is skipped or repeated at a page
//!   boundary, whatever is appended meanwhile. (A future purge would need `AUTOINCREMENT`.)
//! - **The limit** ([`page_limit`]): 50 by default, 0 → 1, above [`AUDIT_PAGE_MAX`] → that.
//! - **`args`** on a page is cut to [`AUDIT_ARGS_PAGE_CHARS`] characters and marked (`args_clipped`,
//!   with the full length in `args_chars`); [`audit_row_conn`] returns it whole (the v24 CHECK
//!   bounds it at 8,192 characters).
//! - **`reasons`** is parsed into `[{row, message}]`. A value that does not parse is shown as
//!   unreadable with its raw text cut to [`AUDIT_REASONS_RAW_CHARS`] characters, and **never fails
//!   the page** — unlike `mcp::audit::audit_rows_conn` (rule #9's error), which stays test-only.

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

use super::steps::McpReason;
use crate::error::AppError;

/// A page's rows when the caller names no limit.
pub const AUDIT_PAGE_DEFAULT: u32 = 50;
/// The most rows a page holds.
pub const AUDIT_PAGE_MAX: u32 = 200;
/// A page's `args` is cut to this many characters.
pub const AUDIT_ARGS_PAGE_CHARS: usize = 1024;
/// An unreadable `reasons` is shown cut to this many characters.
pub const AUDIT_REASONS_RAW_CHARS: usize = 1024;

/// The rows a page holds for the caller's `limit`: `None` → 50, 0 → 1, above 200 → 200.
pub fn page_limit(limit: Option<u32>) -> u32 {
    limit.unwrap_or(AUDIT_PAGE_DEFAULT).clamp(1, AUDIT_PAGE_MAX)
}

/// A row's `reasons`, as read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AuditReasons {
    /// Parsed (a NULL column is an empty list).
    Read { list: Vec<McpReason> },
    /// Not a reasons array: the raw text, cut to [`AUDIT_REASONS_RAW_CHARS`] characters.
    Unreadable { raw: String, raw_clipped: bool },
}

/// One audit row as the view shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AuditRowView {
    pub id: i64,
    /// UTC on the laptop's clock, `YYYY-MM-DD HH:MM:SS` (`datetime('now')`).
    pub at: String,
    pub tool: String,
    /// The call's argument text: cut on a page, whole from [`audit_row_conn`].
    pub args: String,
    /// `args` was cut here; [`audit_row_conn`] has it whole.
    pub args_clipped: bool,
    /// The stored `args`' full length, in characters.
    pub args_chars: u32,
    pub disposition: Option<String>,
    pub reasons: AuditReasons,
    pub job_id: Option<String>,
    pub grant_id: Option<String>,
}

/// One page, newest first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AuditPage {
    pub rows: Vec<AuditRowView>,
    /// The `before_id` of the next (older) page; `None` when no older row exists.
    pub next_before_id: Option<i64>,
}

/// The first `max` characters of `text`, and whether anything was cut.
fn cut_chars(text: &str, max: usize) -> (String, bool) {
    match text.char_indices().nth(max) {
        Some((at, _)) => (text[..at].to_string(), true),
        None => (text.to_string(), false),
    }
}

/// `reasons` parsed, or unreadable with its raw text cut; never an error.
pub fn read_reasons(raw: Option<&str>) -> AuditReasons {
    let Some(raw) = raw else { return AuditReasons::Read { list: Vec::new() } };
    match serde_json::from_str::<Vec<McpReason>>(raw) {
        Ok(list) => AuditReasons::Read { list },
        Err(_) => {
            let (raw, raw_clipped) = cut_chars(raw, AUDIT_REASONS_RAW_CHARS);
            AuditReasons::Unreadable { raw, raw_clipped }
        }
    }
}

const COLUMNS: &str = "id, at, tool, args, disposition, reasons, job_id, grant_id";

/// A stored row, with `args` cut to `args_max` characters (`None`: whole).
fn row_view(r: &rusqlite::Row<'_>, args_max: Option<usize>) -> rusqlite::Result<AuditRowView> {
    let args: String = r.get(3)?;
    let args_chars = args.chars().count() as u32;
    let (args, args_clipped) = match args_max {
        Some(max) => cut_chars(&args, max),
        None => (args, false),
    };
    Ok(AuditRowView {
        id: r.get(0)?,
        at: r.get(1)?,
        tool: r.get(2)?,
        args,
        args_clipped,
        args_chars,
        disposition: r.get(4)?,
        reasons: read_reasons(r.get::<_, Option<String>>(5)?.as_deref()),
        job_id: r.get(6)?,
        grant_id: r.get(7)?,
    })
}

/// One page: the rows below `before_id` (or the newest), newest first, at most [`page_limit`].
pub fn list_audit_conn(conn: &Connection, before_id: Option<i64>, limit: Option<u32>) -> Result<AuditPage, AppError> {
    let limit = page_limit(limit);
    // One row past the page says whether an older page exists.
    let mut stmt = conn.prepare(&format!(
        "SELECT {COLUMNS} FROM mcp_audit WHERE (?1 IS NULL OR id < ?1) ORDER BY id DESC LIMIT ?2"
    ))?;
    let mut rows = stmt
        .query_map(params![before_id, limit + 1], |r| row_view(r, Some(AUDIT_ARGS_PAGE_CHARS)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let more = rows.len() > limit as usize;
    rows.truncate(limit as usize);
    let next_before_id = if more { rows.last().map(|r| r.id) } else { None };
    Ok(AuditPage { rows, next_before_id })
}

/// Row `id` whole; `NotFound` for an id with no row.
pub fn audit_row_conn(conn: &Connection, id: i64) -> Result<AuditRowView, AppError> {
    conn.query_row(&format!("SELECT {COLUMNS} FROM mcp_audit WHERE id = ?1"), params![id], |r| row_view(r, None))
        .optional()?
        .ok_or_else(|| AppError::NotFound(format!("audit row {id}")))
}
