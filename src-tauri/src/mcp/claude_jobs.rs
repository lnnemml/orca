//! "Cancel Claude's jobs" (ADR-027 decision 5; M4 amendment, item 3; unit M4a Part A): Anton's one
//! click that cancels every job Claude made that is still running or waiting, and the count its
//! confirm shows. The `AppHandle`-free core; the commands are `commands::mcp`
//! (`mcp_claude_jobs_summary`, `mcp_cancel_claude_jobs`).
//!
//! - **The selection** ([`select_claude_jobs_conn`]) is **one** SQL statement, so one lock and one
//!   read snapshot: every `origin = 'mcp'` job in `queued`, `running` or `cancelling`, and the count
//!   of charged drafts in step B (`status = 'draft'` with `mcp_state = 'submitting'`). It never
//!   selects a draft or a `user` job for cancelling.
//! - **The order.** The selected `queued` jobs come first, then `running`, then `cancelling`: a
//!   running job cancelled first frees a local slot, and the queue would start a queued `mcp` job
//!   only for this loop to kill it.
//! - **The summary** ([`claude_jobs_summary_conn`]) is the selection counted: the same statement, so
//!   the confirm's number and the cancel's selection cannot use different predicates.
//! - **Each job** goes through `cancel_one`, which the command makes the very per-job body Anton's
//!   Cancel runs (`commands::jobs::cancel_one_job`: `with_poller` + `cancel_job_core` +
//!   `dispatch_cancel`), on the blocking pool. One result per job, a failure included: the loop
//!   never stops at the first error, and every error is reported with its job.
//! - **No audit row.** It is Anton's action; the audit records Claude's dispositions.

use rusqlite::Connection;
use serde::Serialize;

use crate::commands::jobs::CancelResult;
use crate::commands::settings::DbState;
use crate::error::AppError;
use crate::models::job::JobStatus;

/// The one statement both the summary and the cancel read: Claude's live jobs, queued first, and
/// Claude's drafts in step B. `submitting` marks a step-B draft, which is counted and never cancelled;
/// it is `IS`, not `=`, so a NULL `mcp_state` gives 0, never NULL.
pub const CLAUDE_JOBS_SQL: &str = "SELECT id, status, \
         (status = 'draft' AND mcp_state IS 'submitting') AS submitting \
     FROM jobs \
     WHERE origin = 'mcp' \
       AND (status IN ('queued', 'running', 'cancelling') OR (status = 'draft' AND mcp_state = 'submitting')) \
     ORDER BY CASE status WHEN 'queued' THEN 0 WHEN 'running' THEN 1 WHEN 'cancelling' THEN 2 ELSE 3 END, rowid";

/// One job the cancel will act on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectedJob {
    pub id: String,
    pub status: JobStatus,
}

/// What [`select_claude_jobs_conn`] read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaudeJobsSelection {
    /// In cancel order: `queued`, then `running`, then `cancelling`.
    pub to_cancel: Vec<SelectedJob>,
    /// Claude's charged drafts in step B: not cancelled (a `Draft` cannot be).
    pub submitting: u32,
}

/// The selection, in one statement (one lock, one snapshot).
pub fn select_claude_jobs_conn(conn: &Connection) -> Result<ClaudeJobsSelection, AppError> {
    let mut stmt = conn.prepare(CLAUDE_JOBS_SQL)?;
    let rows = stmt
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, bool>(2)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut selection = ClaudeJobsSelection { to_cancel: Vec::new(), submitting: 0 };
    for (id, status, submitting) in rows {
        if submitting {
            selection.submitting += 1;
            continue;
        }
        let status = JobStatus::from_db(&status)?;
        // Post-condition (rule #9): the statement selected nothing else.
        if !matches!(status, JobStatus::Queued | JobStatus::Running | JobStatus::Cancelling) {
            return Err(AppError::Internal(format!("Claude's jobs: job {id} selected in status {}", status.as_str())));
        }
        selection.to_cancel.push(SelectedJob { id, status });
    }
    Ok(selection)
}

/// The confirm's numbers (`mcp_claude_jobs_summary`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ClaudeJobsSummary {
    /// Jobs the cancel would act on now.
    pub to_cancel: u32,
    /// Claude's drafts in step B, which it would skip.
    pub submitting: u32,
}

/// The selection counted: the same statement as the cancel.
pub fn claude_jobs_summary_conn(conn: &Connection) -> Result<ClaudeJobsSummary, AppError> {
    let s = select_claude_jobs_conn(conn)?;
    Ok(ClaudeJobsSummary { to_cancel: s.to_cancel.len() as u32, submitting: s.submitting })
}

/// One job's outcome: `{"job_id": …, "result": <CancelResult>}` or `{"job_id": …, "error": "…"}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ClaudeJobCancel {
    pub job_id: String,
    #[serde(flatten)]
    pub outcome: ClaudeJobOutcome,
}

/// See [`ClaudeJobCancel`]. Defined here rather than serde's `Result` (`Ok`/`Err`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ClaudeJobOutcome {
    Result(CancelResult),
    Error(String),
}

/// What "Cancel Claude's jobs" did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ClaudeCancelReport {
    /// One entry per selected job, in the order they were cancelled.
    pub jobs: Vec<ClaudeJobCancel>,
    /// Claude's drafts in step B that were skipped.
    pub skipped_submitting: u32,
}

/// Select under one lock, release it, then cancel each job through `cancel_one` in order. Runs on
/// the caller's thread: the command calls it on the blocking pool.
pub fn cancel_claude_jobs(
    db: &DbState,
    mut cancel_one: impl FnMut(&str) -> Result<CancelResult, AppError>,
) -> Result<ClaudeCancelReport, AppError> {
    let selection = {
        let conn = db.lock()?;
        select_claude_jobs_conn(&conn)?
    };
    let jobs = selection
        .to_cancel
        .into_iter()
        .map(|job| {
            let outcome = match cancel_one(&job.id) {
                Ok(result) => ClaudeJobOutcome::Result(result),
                Err(e) => ClaudeJobOutcome::Error(e.to_string()),
            };
            ClaudeJobCancel { job_id: job.id, outcome }
        })
        .collect();
    Ok(ClaudeCancelReport { jobs, skipped_submitting: selection.submitting })
}
