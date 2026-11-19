//! Step A and step C of a SUBMITTED `create_job`, the crash reconciliation, and the `submit_job`
//! guard (ADR-027 decision 5, unit M1b). These are the DB-level cores; M3 runs the whole
//! `create_job` (memory verdict, step A, step B, step C) as one tracked task (M0 amendment item 6),
//! and M2's lock holder calls the reconciliation.
//!
//! **Who writes `origin = 'mcp'`.** Only [`charge_and_insert_conn`] and [`insert_proposed_conn`].
//! Both insert through the app's own `create_job_conn`, so an MCP job mints its index map like any
//! job, then stamp the MCP columns in the same transaction.
//!
//! **The step-B window.** From step A's commit to step C's, the row holds `mcp_state =
//! 'submitting'` and a `grant_id`; [`submit_refusal_for_mcp_state`] is the `submit_job` command's
//! guard against Anton submitting it by hand meanwhile (wired in Part B).
//!
//! **Step C acts on the row, not on step B's outcome type.** It re-reads the row: still a `draft`
//! charged to the same grant means step B never handed the job over, so the charge is refunded;
//! anything else keeps the charge (decision 5's table).

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use super::classify::{grant_problems, ProposedReason, Target};
use super::grant::{begin_immediate, check_utc_text, grant_snapshot_conn, in_flight_conn, now_utc_conn, CurrentGrant};
use super::signature::MethodSignature;
use crate::commands::jobs::{create_job_conn, get_job_conn};
use crate::error::AppError;
use crate::models::job::{Job, JobOrigin, JobStatus};

/// The only value `jobs.mcp_state` holds (the v24 CHECK).
pub const SUBMITTING: &str = "submitting";

/// One reason on an MCP job, as stored in `jobs.mcp_reasons` (a JSON array) and shown to Anton.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpReason {
    /// The ADR row (`"P1"` … `"P5"`), or `"submit"` for a step-C refund, or `"interrupted"` for a
    /// reconciliation refund.
    pub row: String,
    pub message: String,
}

impl McpReason {
    pub fn proposed(r: &ProposedReason) -> McpReason {
        McpReason { row: r.row().into(), message: r.message() }
    }

    /// Step C's refund: step B never handed the job over.
    pub fn submit_refused(outcome: &str) -> McpReason {
        McpReason { row: "submit".into(), message: format!("submit refused: {outcome}") }
    }

    /// The reconciliation's refund: the app stopped between step A and step C.
    pub fn interrupted() -> McpReason {
        McpReason { row: "interrupted".into(), message: "interrupted before submit".into() }
    }
}

/// A job's reasons with `extra` appended. A stored value that is not a reasons array is an error,
/// never overwritten (rule #9).
fn reasons_with(job_id: &str, stored: Option<&str>, extra: &[McpReason]) -> Result<String, AppError> {
    let mut all: Vec<McpReason> = match stored {
        None => Vec::new(),
        Some(json) => serde_json::from_str(json)
            .map_err(|e| AppError::Internal(format!("job {job_id}: mcp_reasons is not readable: {e}")))?,
    };
    all.extend_from_slice(extra);
    serde_json::to_string(&all).map_err(|e| AppError::Internal(format!("job {job_id}: mcp_reasons: {e}")))
}

/// What step A is asked to create. The caller (M3) has classified it SUBMITTED against a snapshot
/// read outside this transaction, assembled `input_content`, and computed its `signature`.
#[derive(Debug, Clone, Copy)]
pub struct McpJobDraft<'a> {
    pub title: &'a str,
    pub input_content: &'a str,
    pub target: &'a Target,
    pub signature: &'a MethodSignature,
}

/// What step A did.
#[derive(Debug, Clone)]
pub enum StepA {
    /// Charged to `grant_id`: a `draft` with `mcp_state = 'submitting'`; step B submits it.
    Charged { job: Job, grant_id: String },
    /// The grant no longer covers the job (it was revoked, expired, used up, or the in-flight cap
    /// was reached since the classifier's snapshot): a PROPOSED draft, uncharged.
    Proposed { job: Job, reasons: Vec<ProposedReason> },
}

/// Step A (ADR-027 decision 5): in one `BEGIN IMMEDIATE` transaction, read the time, re-read the
/// grant and the in-flight count, re-run the P5 rows on them, and insert the job — charged (`origin = 'mcp'`,
/// `grant_id`, `mcp_state = 'submitting'`, `charged_jobs + 1`) if the grant still covers it, else
/// PROPOSED with the reasons. Two step As on one grant are ordered by the write lock, so with
/// `max_in_flight = 1` the second sees the first's job and is PROPOSED.
///
/// **The time is read inside the transaction**, after the write lock is granted (`datetime('now')`,
/// [`now_utc_conn`]), never taken from the caller: a step A that waited on the lock judges expiry
/// at the moment it charges, so a grant that expired during the wait is PROPOSED, not charged.
pub fn charge_and_insert_conn(conn: &Connection, draft: McpJobDraft<'_>) -> Result<StepA, AppError> {
    charge_and_insert_with(conn, draft, || Ok(()), now_utc_conn)
}

/// [`charge_and_insert_conn`] with two seams. `before_begin` runs just before `BEGIN IMMEDIATE`:
/// a test runs a rival step A there, or advances a fake clock (the lock wait). `clock` reads the
/// time inside the transaction: production passes [`now_utc_conn`], a test a fixed or fake clock.
/// Because both the time and the grant are read **after** `BEGIN`, the rival's charge and the
/// advanced clock are seen; read either before the seam and it goes unseen — the controls that pin
/// both reads inside.
pub(crate) fn charge_and_insert_with(
    conn: &Connection,
    draft: McpJobDraft<'_>,
    before_begin: impl FnOnce() -> Result<(), AppError>,
    clock: impl FnOnce(&Connection) -> Result<String, AppError>,
) -> Result<StepA, AppError> {
    before_begin()?;
    let tx = begin_immediate(conn)?;
    let now = clock(&tx)?;
    check_utc_text(&now).map_err(AppError::Internal)?;
    let current = grant_snapshot_conn(&tx, &now)?;
    let problems = grant_problems(current.as_ref().map(|c| &c.snapshot), draft.target, draft.signature);
    let job = create_job_conn(&tx, draft.title, draft.input_content, None, None)?;

    let outcome = match (problems.is_empty(), current) {
        (true, Some(CurrentGrant { id: grant_id, snapshot })) => {
            tx.execute(
                "UPDATE jobs SET origin = ?1, grant_id = ?2, mcp_state = ?3 WHERE id = ?4",
                params![JobOrigin::Mcp.as_str(), grant_id, SUBMITTING, job.id],
            )?;
            let charged = tx.execute(
                "UPDATE mcp_grants SET charged_jobs = charged_jobs + 1 WHERE id = ?1 AND revoked_at IS NULL",
                [&grant_id],
            )?;
            // Post-condition (rule #9), in our terms: exactly one grant charged, and the counters
            // the P5 rows promised still hold after this job. A breach rolls the whole step back.
            let in_flight = in_flight_conn(&tx)?;
            if charged != 1 || in_flight > snapshot.max_in_flight {
                return Err(AppError::Internal(format!(
                    "step A post-condition: {charged} grant rows charged, {in_flight} in flight (cap {})",
                    snapshot.max_in_flight
                )));
            }
            StepA::Charged { job: get_job_conn(&tx, &job.id)?, grant_id }
        }
        (true, None) => {
            // P5 always lists NoGrant without a grant; reaching here is a broken invariant.
            return Err(AppError::Internal("step A: no grant, yet no P5 reason".into()));
        }
        (false, _) => {
            let reasons: Vec<McpReason> = problems.iter().map(McpReason::proposed).collect();
            stamp_proposed(&tx, &job.id, &reasons)?;
            StepA::Proposed { job: get_job_conn(&tx, &job.id)?, reasons: problems }
        }
    };
    tx.commit()?;
    Ok(outcome)
}

fn stamp_proposed(conn: &Connection, job_id: &str, reasons: &[McpReason]) -> Result<(), AppError> {
    let json = reasons_with(job_id, None, reasons)?;
    conn.execute(
        "UPDATE jobs SET origin = ?1, mcp_reasons = ?2 WHERE id = ?3",
        params![JobOrigin::Mcp.as_str(), json, job_id],
    )?;
    Ok(())
}

/// Insert a PROPOSED MCP job: a `draft` with `origin = 'mcp'` and its reasons, uncharged. A
/// PROPOSED job without a reason is refused (honest-or-absent).
pub fn insert_proposed_conn(
    conn: &Connection,
    title: &str,
    input_content: &str,
    reasons: &[ProposedReason],
) -> Result<Job, AppError> {
    if reasons.is_empty() {
        return Err(AppError::Internal("a proposed MCP job needs at least one reason".into()));
    }
    let tx = begin_immediate(conn)?;
    let job = create_job_conn(&tx, title, input_content, None, None)?;
    let reasons: Vec<McpReason> = reasons.iter().map(McpReason::proposed).collect();
    stamp_proposed(&tx, &job.id, &reasons)?;
    let job = get_job_conn(&tx, &job.id)?;
    tx.commit()?;
    Ok(job)
}

/// What step C did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StepC {
    /// The row was still a `draft` charged to the grant: step B never handed it over. The charge
    /// is refunded and the job is a PROPOSED draft with the reason "submit refused: …".
    Refunded,
    /// The row had left `draft` (or was not charged to this grant): the charge stays.
    Kept { status: JobStatus },
    /// The row is gone; the counter keeps the charge.
    Gone,
}

/// Refund job `job_id`'s charge to `grant_id` inside the caller's transaction: `grant_id` and
/// `mcp_state` cleared, `reason` appended, the grant's `charged_jobs` decremented (its CHECK keeps
/// it ≥ 0). The decrement is asserted to touch exactly one grant row.
fn refund(conn: &Connection, job_id: &str, stored_reasons: Option<&str>, grant_id: &str, reason: McpReason) -> Result<(), AppError> {
    let json = reasons_with(job_id, stored_reasons, &[reason])?;
    conn.execute(
        "UPDATE jobs SET grant_id = NULL, mcp_state = NULL, mcp_reasons = ?1 WHERE id = ?2",
        params![json, job_id],
    )?;
    let n = conn.execute("UPDATE mcp_grants SET charged_jobs = charged_jobs - 1 WHERE id = ?1", [grant_id])?;
    if n != 1 {
        return Err(AppError::Internal(format!("refund of job {job_id}: {n} rows of grant {grant_id} decremented")));
    }
    Ok(())
}

/// The MCP columns step C and the reconciliation read.
struct McpRow {
    status: JobStatus,
    grant_id: Option<String>,
    mcp_reasons: Option<String>,
}

fn mcp_row(conn: &Connection, job_id: &str) -> Result<Option<McpRow>, AppError> {
    let row = conn
        .query_row(
            "SELECT status, grant_id, mcp_reasons FROM jobs WHERE id = ?1",
            [job_id],
            |r| Ok((r.get::<_, String>(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    row.map(|(status, grant_id, mcp_reasons)| Ok(McpRow { status: JobStatus::from_db(&status)?, grant_id, mcp_reasons }))
    .transpose()
}

/// Step C (ADR-027 decision 5): in one `BEGIN IMMEDIATE` transaction, **re-read the row** and always
/// clear `mcp_state`. Still a `draft` charged to `grant_id` → refund (the job becomes PROPOSED with
/// "submit refused: <outcome_message>"). Left `draft`, or not charged to this grant → no refund.
/// Gone → nothing.
pub fn finish_submit_conn(conn: &Connection, job_id: &str, grant_id: &str, outcome_message: &str) -> Result<StepC, AppError> {
    let tx = begin_immediate(conn)?;
    let Some(row) = mcp_row(&tx, job_id)? else {
        tx.commit()?;
        return Ok(StepC::Gone);
    };
    let outcome = if row.status == JobStatus::Draft && row.grant_id.as_deref() == Some(grant_id) {
        refund(&tx, job_id, row.mcp_reasons.as_deref(), grant_id, McpReason::submit_refused(outcome_message))?;
        StepC::Refunded
    } else {
        tx.execute("UPDATE jobs SET mcp_state = NULL WHERE id = ?1", [job_id])?;
        StepC::Kept { status: row.status }
    };
    tx.commit()?;
    Ok(outcome)
}

/// What the reconciliation did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Reconciled {
    /// `submitting` drafts turned PROPOSED and refunded ("interrupted before submit").
    pub refunded: usize,
    /// Non-draft `submitting` rows whose flag was only cleared (they stay charged).
    pub cleared: usize,
}

/// The crash reconciliation (ADR-027 decision 5; M0 amendment item 6), in one `BEGIN IMMEDIATE`
/// transaction: every `draft` with `mcp_state = 'submitting'` becomes PROPOSED — refunded, with the
/// reason "interrupted before submit" — and every other row with the flag only has it cleared.
///
/// **Not called from anywhere yet.** Only the instance holding the `mcp.lock` `flock` may run it,
/// right after taking the lock and before serving (M2): an instance without the lock could refund a
/// job the serving instance is still submitting.
pub fn reconcile_submitting_conn(conn: &Connection) -> Result<Reconciled, AppError> {
    let tx = begin_immediate(conn)?;
    let ids: Vec<String> = {
        let mut stmt = tx.prepare("SELECT id FROM jobs WHERE mcp_state IS NOT NULL ORDER BY rowid")?;
        let ids = stmt.query_map([], |r| r.get(0))?.collect::<rusqlite::Result<Vec<String>>>()?;
        ids
    };
    // Every id was selected by `mcp_state IS NOT NULL` inside this same `BEGIN IMMEDIATE`
    // transaction, which holds the write lock, so each row still exists and still has the flag when
    // it is re-read below: the invariant holds by construction, and nothing here asserts it.
    let mut done = Reconciled::default();
    for id in ids {
        let Some(row) = mcp_row(&tx, &id)? else { continue };
        match (row.status, row.grant_id.as_deref()) {
            (JobStatus::Draft, Some(g)) => {
                refund(&tx, &id, row.mcp_reasons.as_deref(), g, McpReason::interrupted())?;
                done.refunded += 1;
            }
            _ => {
                tx.execute("UPDATE jobs SET mcp_state = NULL WHERE id = ?1", [&id])?;
                done.cleared += 1;
            }
        }
    }
    tx.commit()?;
    Ok(done)
}

/// The `submit_job` command's guard (ADR-027 decision 5, "the window"): a job whose `mcp_state` is
/// set is between step A and step C, and the MCP server is submitting it; Anton cannot submit it by
/// hand meanwhile. Any non-NULL value refuses (the CHECK admits only `submitting`; fail closed).
/// Wired into the command in Part B; the backends do not change.
pub fn submit_refusal_for_mcp_state(job: &Job) -> Option<AppError> {
    job.mcp_state.as_ref().map(|state| {
        AppError::Conflict(format!(
            "Claude is submitting job {} (mcp_state = {state}); it can be run by hand once that ends",
            job.id
        ))
    })
}
