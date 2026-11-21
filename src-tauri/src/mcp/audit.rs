//! The audit log, `mcp_audit` (ADR-027 decision 9; M3 amendment, item 3; schema v24, no change).
//!
//! **What gets a row: dispositions, not tool errors.**
//! - each classifier disposition of `create_job` — SUBMITTED, PROPOSED or REFUSED, a REFUSED
//!   internal error (decision 4's failed re-parse) included — written **inside step A's
//!   transaction**, so a charge without its audit row cannot commit ([`super::steps::decide_conn`]);
//! - step C's outcome, written inside step C's transaction ([`super::steps::finish_submit_audited_conn`]);
//! - the reconciliation's outcome for each row it touches, in its own transaction
//!   ([`super::steps::reconcile_submitting_conn`]).
//!
//! A call that ends before classification is not a disposition and has no row: a busy refusal,
//! "MCP is shutting down", an argument error, `SQLITE_BUSY` at step A, or a failure of the memory
//! preflight.
//!
//! **The columns.** `tool` (`create_job`), `args` (the call's bounded argument text, a JSON object;
//! the v24 CHECK caps it at [`MCP_AUDIT_ARGS_MAX_CHARS`] characters), `disposition` (`refused`,
//! `proposed`, `submitted`, or NULL for an outcome that is not a disposition), `reasons` (a JSON array
//! of [`McpReason`], at most [`AUDIT_REASONS_MAX`] of them), `job_id`, `grant_id`.
//!
//! **How an outcome row is encoded** (`disposition`, then `reasons`):
//!
//! | outcome | `disposition` | `reasons` |
//! |---|---|---|
//! | step C refunded (the row was still a charged `draft`) | `proposed` | `[{"row":"submit","message":"submit refused: <step B's message>"}]`, the reason appended to the job |
//! | step C kept the charge (the row left `draft`) | NULL | `[{"row":"state","message":"<job status>"},{"row":"step_b","message":"<step B's message>"}]` |
//! | step C found the row gone | NULL | `[{"row":"state","message":"gone"},{"row":"step_b","message":"<step B's message>"}]` |
//! | the reconciliation refunded a `submitting` draft | `proposed` | `[{"row":"interrupted","message":"interrupted before submit"}]` |
//! | the reconciliation cleared a non-`draft` row's flag | NULL | `[{"row":"state","message":"<job status>"},{"row":"interrupted","message":"…the charge is kept"}]` |
//!
//! The job status is `JobStatus::as_str` (`queued`, `running`, `failed`, …). A remote submit's
//! finer state — waiting for the server's slot ("pending"), "not on the server", `Unknown`,
//! `FailedAfterClaim`, the `KillUserProcesses` refusal — is in step B's message, which is the backend's
//! own outcome text.

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

use super::steps::McpReason;
use super::tools::clip;
use crate::db::MCP_AUDIT_ARGS_MAX_CHARS;
use crate::error::AppError;

/// The `tool` of every `create_job` row.
pub const CREATE_JOB: &str = "create_job";

/// At most this many reasons go into a row (and into `create_job`'s result); the rest are counted
/// in one last `{"row":"more"}` reason. A job's own `mcp_reasons` keep every reason.
pub const AUDIT_REASONS_MAX: usize = 20;

/// Each reason's message in a row (and in the result) is cut to this many bytes.
pub const REASON_MESSAGE_MAX_BYTES: usize = 400;

/// The `disposition` column.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditDisposition {
    Refused,
    Proposed,
    Submitted,
}

impl AuditDisposition {
    pub fn as_str(self) -> &'static str {
        match self {
            AuditDisposition::Refused => "refused",
            AuditDisposition::Proposed => "proposed",
            AuditDisposition::Submitted => "submitted",
        }
    }
}

/// One row to append.
#[derive(Debug, Clone, Copy)]
pub struct AuditEntry<'a> {
    pub tool: &'a str,
    /// The call's bounded argument text (a JSON object).
    pub args: &'a str,
    /// `None` for an outcome that is not a disposition (step C kept the charge, a reconciliation clear).
    pub disposition: Option<AuditDisposition>,
    pub reasons: &'a [McpReason],
    pub job_id: Option<&'a str>,
    pub grant_id: Option<&'a str>,
}

/// `reasons`, bounded: at most [`AUDIT_REASONS_MAX`], each message cut to
/// [`REASON_MESSAGE_MAX_BYTES`], and the rest counted in a last `"more"` reason (honest-or-absent:
/// a dropped reason is counted, never silently lost).
pub fn bounded_reasons(reasons: &[McpReason]) -> Vec<McpReason> {
    let mut out: Vec<McpReason> = reasons
        .iter()
        .take(AUDIT_REASONS_MAX)
        .map(|r| McpReason { row: clip(&r.row, 40).0, message: clip(&r.message, REASON_MESSAGE_MAX_BYTES).0 })
        .collect();
    if reasons.len() > AUDIT_REASONS_MAX {
        out.push(McpReason::more(reasons.len() - AUDIT_REASONS_MAX));
    }
    out
}

/// `args` as the column takes it: at most [`MCP_AUDIT_ARGS_MAX_CHARS`] characters. The caller builds
/// a bounded text already ([`super::create_job::audit_args`]); a text over the cap here is cut and
/// marked, never refused by the CHECK (which would roll back the step it rides in).
fn args_column(args: &str) -> String {
    if args.chars().count() <= MCP_AUDIT_ARGS_MAX_CHARS {
        return args.to_string();
    }
    const MARK: &str = "…[cut]";
    let keep = MCP_AUDIT_ARGS_MAX_CHARS - MARK.chars().count();
    args.chars().take(keep).collect::<String>() + MARK
}

/// Append one row inside the caller's transaction.
pub fn append_audit_conn(conn: &Connection, entry: &AuditEntry<'_>) -> Result<(), AppError> {
    let reasons = serde_json::to_string(&bounded_reasons(entry.reasons))
        .map_err(|e| AppError::Internal(format!("audit reasons: {e}")))?;
    conn.execute(
        "INSERT INTO mcp_audit (tool, args, disposition, reasons, job_id, grant_id) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            entry.tool,
            args_column(entry.args),
            entry.disposition.map(AuditDisposition::as_str),
            reasons,
            entry.job_id,
            entry.grant_id
        ],
    )?;
    Ok(())
}

/// One stored row, as read back (the tests; M4's Settings view).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AuditRow {
    pub id: i64,
    pub at: String,
    pub tool: String,
    pub args: String,
    pub disposition: Option<String>,
    pub reasons: Vec<McpReason>,
    pub job_id: Option<String>,
    pub grant_id: Option<String>,
}

/// Every row, oldest first. A `reasons` value that is not a reasons array is an error (rule #9).
pub fn audit_rows_conn(conn: &Connection) -> Result<Vec<AuditRow>, AppError> {
    let mut stmt = conn.prepare(
        "SELECT id, at, tool, args, disposition, reasons, job_id, grant_id FROM mcp_audit ORDER BY id",
    )?;
    let rows = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, Option<String>>(4)?,
                r.get::<_, Option<String>>(5)?,
                r.get::<_, Option<String>>(6)?,
                r.get::<_, Option<String>>(7)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    rows.into_iter()
        .map(|(id, at, tool, args, disposition, reasons, job_id, grant_id)| {
            let reasons = match reasons {
                None => Vec::new(),
                Some(json) => serde_json::from_str(&json)
                    .map_err(|e| AppError::Internal(format!("audit row {id}: reasons are not readable: {e}")))?,
            };
            Ok(AuditRow { id, at, tool, args, disposition, reasons, job_id, grant_id })
        })
        .collect()
}
