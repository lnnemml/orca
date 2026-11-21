//! `cancel_job`'s core (ADR-027 decisions 5 and 7; M3 amendment, items 1 and 3; unit M3b; the audit
//! encoding decided by Anton, 2026-11-21): Claude may cancel **only its own jobs** (`origin = 'mcp'`),
//! never a `Draft`, and never without an audit row.
//!
//! **The order** ([`run_cancel_job`]):
//! 1. **The argument bound** ([`JOB_ID_MAX_BYTES`], not empty): refused with no row.
//! 2. **The pre-check and its row** ([`precheck_conn`]): **one** `BEGIN IMMEDIATE` transaction reads
//!    the job's `origin`, `status` and `grant_id` and appends exactly one audit row:
//!    - `origin != 'mcp'` → a `refused` row, and the call ends (the cancel core is not called);
//!    - a `Draft` (PROPOSED, or a charged draft in step B with `mcp_state = 'submitting'`) → a
//!      `refused` row ("Anton deletes it"), and the call ends (`local_backend` refuses a draft
//!      anyway; decision 5);
//!    - otherwise a NULL-disposition **pre-row**, "requested (job was <status>)", and it commits.
//!
//!    An unknown job id is an argument error, not a disposition: no row (M3 amendment, item 3).
//!
//!    **Why reading the origin before the core is race-free:** `origin` is immutable once the job's
//!    creating transaction has committed. `create_job_conn` inserts `'user'`, and the only writers of
//!    `'mcp'` (`steps::stamp_proposed`, `steps::charge_in_tx`) run inside the same transaction that
//!    inserted the row; nothing writes `origin` afterwards. So the origin the pre-check reads is the
//!    origin the core acts on. (The status can move on between the two; the core re-reads it, as it
//!    does for Anton's Cancel.)
//! 3. **The cancel**: `commands::jobs::cancel_job_core`, the same core Anton's Cancel runs, with **no
//!    lock held** and the backend's cancel injected as `dispatch` (the app passes
//!    `Backend::for_job(..).cancel`).
//! 4. **The outcome row**, in its own transaction: NULL disposition, the `CancelResult` (or the
//!    core's error, bounded) in `reasons`. A crash between 3 and 4 leaves the pre-row only, which is
//!    accepted (Anton, 2026-11-21): the pre-row already records that a cancel was asked for. A failed
//!    outcome-row write is reported in the result (`audit_error`), never hidden.
//!
//! **No grant is needed** (M3 amendment, item 1): a cancel stops spending. Every row carries the
//! job's `grant_id` (informational; NULL for a PROPOSED job Anton ran).

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

use super::audit::{append_audit_conn, AuditDisposition, AuditEntry, REASON_MESSAGE_MAX_BYTES};
use super::grant::begin_immediate;
use super::steps::McpReason;
use super::tools::{clip, echo, within_cap, ToolError, RESULT_CAP_BYTES};
use crate::commands::jobs::{cancel_job_core, CancelResult};
use crate::error::AppError;
use crate::models::job::{Job, JobOrigin, JobStatus};
use crate::poller::Poller;

/// The `tool` of every `cancel_job` row.
pub const CANCEL_JOB: &str = "cancel_job";

/// The longest job id `cancel_job` takes, in bytes (the app mints v4 UUIDs, 36 bytes; the same bound
/// as `create_job`'s ids).
pub const JOB_ID_MAX_BYTES: usize = 64;

/// The refusal for a job that is not Claude's.
pub const NOT_CLAUDES: &str = "not Claude's job";
/// The refusal for a `Draft`.
pub const DRAFT_REFUSED: &str = "a PROPOSED draft cannot be cancelled; Anton deletes it";

/// The tool's arguments.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct CancelJobArgs {
    pub job_id: String,
}

/// What `cancel_job` answers. Every text is bounded, so the result is bounded by construction and
/// checked against [`RESULT_CAP_BYTES`] as its post-condition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum CancelJobResult {
    /// The pre-check refused (not Claude's job, or a `Draft`); the `refused` row is written and the
    /// cancel core was not called.
    Refused { job_id: String, reasons: Vec<McpReason> },
    /// The cancel core ran. `result` is its `CancelResult` tag (`dispatched`, `delivered`,
    /// `pending`, `already_on_server`, `moved_on`); `detail` is the whole `CancelResult` as compact
    /// JSON, cut to [`REASON_MESSAGE_MAX_BYTES`].
    Requested { job_id: String, status_before: String, result: String, detail: String, audit_error: Option<String> },
    /// The cancel core returned an error (its message, cut); the outcome row records it.
    Failed { job_id: String, status_before: String, error: String, audit_error: Option<String> },
}

/// What the pre-check decided (its row already committed).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Precheck {
    /// Refused: the `refused` row is committed; `reasons` is what it says.
    Refused(Vec<McpReason>),
    /// The pre-row is committed; the cancel core runs next.
    Proceed { status: JobStatus, grant_id: Option<String> },
}

/// The call's argument text for the audit rows: a JSON object, the id cut to its bound.
pub fn audit_args(args: &CancelJobArgs) -> String {
    serde_json::json!({ "job_id": clip(&args.job_id, JOB_ID_MAX_BYTES).0 }).to_string()
}

/// The id over its bound, or empty: refused before anything is read or written (no row).
fn check_args(args: &CancelJobArgs) -> Result<(), ToolError> {
    if args.job_id.is_empty() {
        return Err(ToolError::refused("`job_id` is empty"));
    }
    if args.job_id.len() > JOB_ID_MAX_BYTES {
        return Err(ToolError::refused(format!("`job_id` is {} bytes; at most {JOB_ID_MAX_BYTES}", args.job_id.len())));
    }
    Ok(())
}

/// The pre-check (step 2 of the module docs): one `BEGIN IMMEDIATE` transaction that reads the job
/// and appends exactly one row — `refused` (not `origin = 'mcp'`, or a `Draft`) or the NULL pre-row.
/// An unknown id is [`AppError::NotFound`] and the transaction rolls back: no row.
pub fn precheck_conn(conn: &Connection, job_id: &str, audit_args: &str) -> Result<Precheck, AppError> {
    let tx = begin_immediate(conn)?;
    let row: Option<(String, String, Option<String>)> = tx
        .query_row("SELECT origin, status, grant_id FROM jobs WHERE id = ?1", params![job_id], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })
        .optional()?;
    let Some((origin, status, grant_id)) = row else {
        return Err(AppError::NotFound(format!("job {job_id}")));
    };
    // Unknown values are errors (rule #9), never read as "user" or as some status.
    let origin = JobOrigin::from_db(&origin)?;
    let status = JobStatus::from_db(&status)?;

    let refusal = if origin != JobOrigin::Mcp {
        Some(McpReason { row: "origin".into(), message: format!("{NOT_CLAUDES} (origin = {})", origin.as_str()) })
    } else if status == JobStatus::Draft {
        Some(McpReason { row: "draft".into(), message: DRAFT_REFUSED.into() })
    } else {
        None
    };
    let (disposition, reasons) = match &refusal {
        Some(reason) => (Some(AuditDisposition::Refused), vec![reason.clone()]),
        None => (None, vec![requested_reason(status)]),
    };
    append_audit_conn(
        &tx,
        &AuditEntry {
            tool: CANCEL_JOB,
            args: audit_args,
            disposition,
            reasons: &reasons,
            job_id: Some(job_id),
            grant_id: grant_id.as_deref(),
        },
    )?;
    tx.commit()?;
    Ok(match refusal {
        Some(_) => Precheck::Refused(reasons),
        None => Precheck::Proceed { status, grant_id },
    })
}

/// The pre-row's one reason.
pub fn requested_reason(status: JobStatus) -> McpReason {
    McpReason { row: "cancel".into(), message: format!("requested (job was {})", status.as_str()) }
}

/// The outcome row's reasons: the `CancelResult` (its tag in `"cancel"`, the whole value as compact
/// JSON in `"detail"`), or the core's error in `"cancel_error"`. Each message is cut by
/// `append_audit_conn` to [`REASON_MESSAGE_MAX_BYTES`].
pub fn outcome_reasons(outcome: &Result<CancelResult, AppError>) -> Vec<McpReason> {
    match outcome {
        Ok(result) => vec![
            McpReason { row: "cancel".into(), message: result_tag(result).into() },
            McpReason { row: "detail".into(), message: detail_of(result) },
        ],
        Err(e) => vec![McpReason { row: "cancel_error".into(), message: e.to_string() }],
    }
}

/// The `CancelResult` variant, as its serialised tag.
pub fn result_tag(result: &CancelResult) -> &'static str {
    match result {
        CancelResult::Delivered { .. } => "delivered",
        CancelResult::Pending { .. } => "pending",
        CancelResult::AlreadyOnServer => "already_on_server",
        CancelResult::MovedOn { .. } => "moved_on",
        CancelResult::Dispatched => "dispatched",
    }
}

fn detail_of(result: &CancelResult) -> String {
    let json = serde_json::to_string(result).unwrap_or_else(|e| format!("(not serialisable: {e})"));
    clip(&json, REASON_MESSAGE_MAX_BYTES).0
}

/// The outcome row (step 4), in its own transaction.
pub fn outcome_row_conn(
    conn: &Connection,
    job_id: &str,
    grant_id: Option<&str>,
    audit_args: &str,
    outcome: &Result<CancelResult, AppError>,
) -> Result<(), AppError> {
    let reasons = outcome_reasons(outcome);
    let tx = begin_immediate(conn)?;
    append_audit_conn(
        &tx,
        &AuditEntry { tool: CANCEL_JOB, args: audit_args, disposition: None, reasons: &reasons, job_id: Some(job_id), grant_id },
    )?;
    tx.commit()?;
    Ok(())
}

/// `cancel_job` (the module docs' order). Blocking: the cancel core may make ssh calls, so the server
/// runs this on the blocking pool. `dispatch` is the backend's cancel for the core's guarded path.
pub fn run_cancel_job(
    poller: &Poller<'_>,
    args: &CancelJobArgs,
    dispatch: impl FnOnce(Job) -> Result<(), AppError>,
) -> Result<CancelJobResult, ToolError> {
    check_args(args)?;
    let audit_args = audit_args(args);
    let job_id = args.job_id.as_str();

    // The pre-check and its row, under the lock for one transaction only.
    let precheck = {
        let conn = poller.db.lock().map_err(ToolError::App)?;
        precheck_conn(&conn, job_id, &audit_args).map_err(ToolError::from_app)?
    };
    let (status, grant_id) = match precheck {
        Precheck::Refused(reasons) => return fit(CancelJobResult::Refused { job_id: echo(job_id), reasons }),
        Precheck::Proceed { status, grant_id } => (status, grant_id),
    };

    // The cancel, with no lock held (the core takes it itself, for one read or write at a time).
    let outcome = cancel_job_core(poller, job_id, dispatch);

    let audit_error = poller
        .db
        .lock()
        .and_then(|conn| outcome_row_conn(&conn, job_id, grant_id.as_deref(), &audit_args, &outcome))
        .err()
        .map(|e| clip(&e.to_string(), REASON_MESSAGE_MAX_BYTES).0);

    let status_before = status.as_str().to_string();
    fit(match outcome {
        Ok(result) => CancelJobResult::Requested {
            job_id: echo(job_id),
            status_before,
            result: result_tag(&result).into(),
            detail: detail_of(&result),
            audit_error,
        },
        Err(e) => CancelJobResult::Failed {
            job_id: echo(job_id),
            status_before,
            error: clip(&e.to_string(), REASON_MESSAGE_MAX_BYTES).0,
            audit_error,
        },
    })
}

/// The size post-condition on the result.
fn fit(result: CancelJobResult) -> Result<CancelJobResult, ToolError> {
    within_cap(result, RESULT_CAP_BYTES)
}
