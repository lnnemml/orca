//! Step A and step C of `create_job`, the crash reconciliation, and the `submit_job` guard (ADR-027
//! decision 5, units M1b and M3a). These are the DB-level cores; `super::create_job` runs the whole
//! `create_job` (memory verdict, step A, step B, step C) as one tracked task (M0 amendment item 6),
//! and M2's lock holder calls the reconciliation.
//!
//! **Step A** is [`decide_conn`]: one `BEGIN IMMEDIATE` transaction that classifies the job against
//! the grant, its counters and R8's open-proposal count read inside it, and writes the disposition
//! with its audit row. **Step C** is [`finish_submit_audited_conn`]: the row re-read, the refund or
//! not, and the outcome row, in one transaction. M1b's [`charge_and_insert_conn`] (P5 only) and
//! [`finish_submit_conn`] (no audit row) share those bodies ([`charge_in_tx`], [`finish_in_tx`]).
//!
//! **Who writes `origin = 'mcp'`.** Only [`charge_and_insert_conn`] and [`insert_proposed_conn`].
//! Both insert through the app's own `create_job_conn`, so an MCP job mints its index map like any
//! job, then stamp the MCP columns in the same transaction.
//!
//! **The step-B window.** From step A's commit to step C's, the row holds `mcp_state =
//! 'submitting'` and a `grant_id`; [`refuse_while_mcp_submitting`] is the guard of every command
//! that submits a job by hand (`submit_job`, `retry_remote_submit`), so Anton cannot submit it
//! meanwhile. The backends carry no such guard: step B calls them on this very job.
//!
//! **Step C acts on the row, not on step B's outcome type.** It re-reads the row: still a `draft`
//! charged to the same grant means step B never handed the job over, so the charge is refunded;
//! anything else keeps the charge (decision 5's table).

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use super::assemble::{Assembled, AssemblyError};
use super::audit::{append_audit_conn, AuditDisposition, AuditEntry, CREATE_JOB};
use super::classify::{
    classify_mcp_job, grant_problems, ClassifyInput, Disposition, GrantSnapshot, MemoryVerdict, ProposedReason,
    RefusedReason, ResolvedSource, Target,
};
use super::grant::{begin_immediate, check_utc_text, grant_snapshot_conn, in_flight_conn, now_utc_conn, CurrentGrant};
use super::header::ParsedHeader;
use super::signature::MethodSignature;
use crate::commands::jobs::{create_job_conn, get_job_conn};
use crate::error::AppError;
use crate::models::job::{Job, JobOrigin, JobStatus};

/// The only value `jobs.mcp_state` holds (the v24 CHECK).
pub const SUBMITTING: &str = "submitting";

/// One reason on an MCP job, as stored in `jobs.mcp_reasons` (a JSON array) and shown to Anton, and
/// one entry of an `mcp_audit` row's `reasons` (the encoding table is in [`super::audit`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpReason {
    /// What the entry is:
    /// - an ADR row, `"R1"` … `"R9"` or `"P1"` … `"P5"`, or `"internal"` (a REFUSED internal error);
    /// - `"submit"`: step C's refund ("submit refused: …");
    /// - `"interrupted"`: the reconciliation's refund, or its flag clear;
    /// - `"state"`: a job's status after step C or the reconciliation (audit outcome rows only);
    /// - `"step_b"`: step B's own outcome message (audit outcome rows only);
    /// - `"more"`: how many reasons a bounded list left out.
    pub row: String,
    pub message: String,
}

impl McpReason {
    pub fn proposed(r: &ProposedReason) -> McpReason {
        McpReason { row: r.row().into(), message: r.message() }
    }

    /// A REFUSED reason (its row, or `"internal"`).
    pub fn refused(r: &RefusedReason) -> McpReason {
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

    /// The reconciliation's clear of a row that had left `draft`: the charge stays.
    pub fn interrupted_after_hand_over() -> McpReason {
        McpReason {
            row: "interrupted".into(),
            message: "the app stopped after step B handed the job over; the flag is cleared and the charge is kept".into(),
        }
    }

    /// A job's state in an outcome row: its status (`JobStatus::as_str`), or `"gone"`.
    pub fn state(state: &str) -> McpReason {
        McpReason { row: "state".into(), message: state.into() }
    }

    /// Step B's own outcome message in an outcome row.
    pub fn step_b(message: &str) -> McpReason {
        McpReason { row: "step_b".into(), message: message.into() }
    }

    /// How many reasons a bounded list left out.
    pub fn more(n: usize) -> McpReason {
        McpReason { row: "more".into(), message: format!("{n} more reasons not listed") }
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

/// What the P5-only step A ([`charge_and_insert_conn`], M1b) is asked to create: a job already
/// classified SUBMITTED, its `input_content` assembled and its `signature` computed. `create_job`
/// itself goes through [`decide_conn`], which runs the whole classifier inside the transaction; both
/// charge through the one [`charge_in_tx`], so M1b's step-A tests pin the charge `create_job` makes.
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
            charge_in_tx(&tx, &job.id, &grant_id, &snapshot)?;
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

/// The charge, inside the caller's `BEGIN IMMEDIATE` transaction: job `job_id` becomes an MCP
/// `draft` charged to `grant_id` with `mcp_state = 'submitting'`, and the grant's `charged_jobs` is
/// incremented. Shared by both step As ([`charge_and_insert_with`], [`decide_with`]).
///
/// **Post-condition (rule #9), in our terms:** exactly one grant row was charged, and the in-flight
/// cap the P5 rows promised still holds with this job counted. A breach is an error, so the caller's
/// transaction rolls the whole step back.
fn charge_in_tx(tx: &Connection, job_id: &str, grant_id: &str, snapshot: &GrantSnapshot) -> Result<(), AppError> {
    tx.execute(
        "UPDATE jobs SET origin = ?1, grant_id = ?2, mcp_state = ?3 WHERE id = ?4",
        params![JobOrigin::Mcp.as_str(), grant_id, SUBMITTING, job_id],
    )?;
    let charged = tx.execute(
        "UPDATE mcp_grants SET charged_jobs = charged_jobs + 1 WHERE id = ?1 AND revoked_at IS NULL",
        [grant_id],
    )?;
    let in_flight = in_flight_conn(tx)?;
    if charged != 1 || in_flight > snapshot.max_in_flight {
        return Err(AppError::Internal(format!(
            "step A post-condition: {charged} grant rows charged, {in_flight} in flight (cap {})",
            snapshot.max_in_flight
        )));
    }
    Ok(())
}

/// R8's count (ADR-027 decision 3; M3 amendment, item 1): the **open MCP proposals** — every
/// `origin = 'mcp'` job still in `draft` and not charged (`grant_id IS NULL`). A refunded draft
/// counts (it is a proposal now); a charged draft in step B does not (it is a SUBMITTED call), and
/// a proposal Anton ran or deleted no longer does.
pub fn open_proposals_conn(conn: &Connection) -> Result<u32, AppError> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM jobs WHERE origin = 'mcp' AND status = 'draft' AND grant_id IS NULL",
        [],
        |r| r.get(0),
    )?;
    u32::try_from(n).map_err(|_| AppError::Internal(format!("open proposals = {n} is out of range")))
}

/// What `create_job`'s step A decides from: everything computed before it, outside any lock.
#[derive(Debug, Clone, Copy)]
pub struct StepAInput<'a> {
    pub header: &'a ParsedHeader,
    /// `Err` holds why the reference did not resolve (R4).
    pub source: &'a Result<ResolvedSource, String>,
    pub target: &'a Target,
    /// The memory verdict on the assembled input (decision 4), computed before step A.
    pub memory: MemoryVerdict,
    /// The assembled input, or `None` when it could not be attempted: the source did not resolve,
    /// or its atoms, charge or multiplicity are unreadable — each a classifier refusal (R4).
    pub assembled: Option<&'a Result<Assembled, AssemblyError>>,
    pub title: &'a str,
    pub group_id: Option<&'a str>,
    /// The call's bounded argument text, for the audit row.
    pub audit_args: &'a str,
}

/// What step A decided and wrote.
#[derive(Debug, Clone)]
pub enum Decided {
    /// REFUSED: nothing was created; the reasons are in the audit row.
    Refused { reasons: Vec<McpReason> },
    /// PROPOSED: an uncharged MCP `draft` with its reasons; Anton runs it.
    Proposed { job: Job, reasons: Vec<McpReason> },
    /// SUBMITTED: a charged `draft` at `mcp_state = 'submitting'`; step B submits it.
    Charged { job: Job, grant_id: String },
}

/// A disposition with the assembly's result folded in (decision 4): a classifier REFUSED stands as it
/// is; otherwise an input that did not assemble, or was never assembled, is a REFUSED internal error,
/// so a PROPOSED or SUBMITTED job always carries an input that passed the post-condition.
fn with_assembly<'a>(
    disposition: Disposition,
    assembled: Option<&'a Result<Assembled, AssemblyError>>,
) -> Result<(Disposition, &'a Assembled), Vec<RefusedReason>> {
    match (disposition, assembled) {
        (Disposition::Refused(r), _) => Err(r),
        (d, Some(Ok(a))) => Ok((d, a)),
        (_, Some(Err(e))) => Err(vec![e.as_refusal()]),
        (_, None) => Err(vec![RefusedReason::Internal {
            detail: "the input was not assembled, yet no REFUSED row matched".into(),
        }]),
    }
}

/// `create_job`'s step A (ADR-027 decision 5; M3 amendment, items 1 and 3): **one** `BEGIN IMMEDIATE`
/// transaction that reads the time, the grant snapshot with its counters and R8's open-proposal
/// count, classifies the job (`classify_mcp_job`), and writes what the disposition says, with its
/// audit row in the same transaction:
/// - REFUSED → the audit row only;
/// - PROPOSED → an uncharged MCP `draft` with its reasons, and the audit row;
/// - SUBMITTED → a `draft` charged to the grant (`mcp_state = 'submitting'`, `charged_jobs + 1`, the
///   post-condition of [`charge_in_tx`]), and the audit row.
///
/// Every read the decision rests on happens after the write lock is granted, so two step As (in this
/// process, or in two instances on one file) are ordered: with `max_in_flight = 1` the second sees the
/// first's job and is PROPOSED; at 49 open proposals the second of two PROPOSED calls sees 50 and is
/// REFUSED (R8). A revoke that commits first is seen. An unknown `group_id` is an error, before any
/// write and with no audit row (an argument error is not a disposition).
pub fn decide_conn(conn: &Connection, input: StepAInput<'_>) -> Result<Decided, AppError> {
    decide_with(conn, input, || Ok(()), now_utc_conn)
}

/// [`decide_conn`] with the seams of [`charge_and_insert_with`]: `before_begin` runs just before
/// `BEGIN IMMEDIATE` (a test runs a rival step A or a revoke there), and `clock` reads the time
/// inside the transaction.
pub(crate) fn decide_with(
    conn: &Connection,
    input: StepAInput<'_>,
    before_begin: impl FnOnce() -> Result<(), AppError>,
    clock: impl FnOnce(&Connection) -> Result<String, AppError>,
) -> Result<Decided, AppError> {
    before_begin()?;
    let tx = begin_immediate(conn)?;
    let now = clock(&tx)?;
    check_utc_text(&now).map_err(AppError::Internal)?;
    if let Some(group) = input.group_id {
        let exists: bool = tx.query_row("SELECT EXISTS (SELECT 1 FROM groups WHERE id = ?1)", [group], |r| r.get(0))?;
        if !exists {
            return Err(AppError::NotFound(format!("group {group}")));
        }
    }
    let current = grant_snapshot_conn(&tx, &now)?;
    let open_proposals = open_proposals_conn(&tx)?;
    let disposition = classify_mcp_job(&ClassifyInput {
        header: input.header,
        source: input.source,
        target: input.target,
        memory: input.memory,
        grant: current.as_ref().map(|c| &c.snapshot),
        open_proposals,
    });
    let audit = |disposition: AuditDisposition, reasons: &[McpReason], job_id: Option<&str>, grant_id: Option<&str>| {
        append_audit_conn(
            &tx,
            &AuditEntry { tool: CREATE_JOB, args: input.audit_args, disposition: Some(disposition), reasons, job_id, grant_id },
        )
    };
    let insert = |assembled: &Assembled| -> Result<Job, AppError> {
        let job = create_job_conn(&tx, input.title, &assembled.input, None, None)?;
        if let Some(group) = input.group_id {
            tx.execute("UPDATE jobs SET group_id = ?1 WHERE id = ?2", params![group, job.id])?;
        }
        Ok(job)
    };

    let decided = match with_assembly(disposition, input.assembled) {
        Err(refused) => {
            let reasons: Vec<McpReason> = refused.iter().map(McpReason::refused).collect();
            audit(AuditDisposition::Refused, &reasons, None, None)?;
            Decided::Refused { reasons }
        }
        Ok((Disposition::Proposed(proposed), assembled)) => {
            let reasons: Vec<McpReason> = proposed.iter().map(McpReason::proposed).collect();
            let job = insert(assembled)?;
            stamp_proposed(&tx, &job.id, &reasons)?;
            audit(AuditDisposition::Proposed, &reasons, Some(&job.id), None)?;
            Decided::Proposed { job: get_job_conn(&tx, &job.id)?, reasons }
        }
        Ok((Disposition::Submitted, assembled)) => {
            // SUBMITTED means P5 matched nothing, so a grant was read; anything else is a broken invariant.
            let Some(CurrentGrant { id: grant_id, snapshot }) = current else {
                return Err(AppError::Internal("step A: SUBMITTED without a grant".into()));
            };
            let job = insert(assembled)?;
            charge_in_tx(&tx, &job.id, &grant_id, &snapshot)?;
            audit(AuditDisposition::Submitted, &[], Some(&job.id), Some(&grant_id))?;
            Decided::Charged { job: get_job_conn(&tx, &job.id)?, grant_id }
        }
        // `with_assembly` hands every refusal back as `Err`.
        Ok((Disposition::Refused(_), _)) => return Err(AppError::Internal("step A: a refusal passed the assembly fold".into())),
    };
    tx.commit()?;
    Ok(decided)
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
    let outcome = finish_in_tx(&tx, job_id, grant_id, outcome_message)?;
    tx.commit()?;
    Ok(outcome)
}

/// Step C's body inside the caller's transaction (shared by [`finish_submit_conn`] and
/// [`finish_submit_audited_conn`]): re-read the row, refund a `draft` still charged to `grant_id`,
/// else only clear the flag.
fn finish_in_tx(tx: &Connection, job_id: &str, grant_id: &str, outcome_message: &str) -> Result<StepC, AppError> {
    let Some(row) = mcp_row(tx, job_id)? else {
        return Ok(StepC::Gone);
    };
    if row.status == JobStatus::Draft && row.grant_id.as_deref() == Some(grant_id) {
        refund(tx, job_id, row.mcp_reasons.as_deref(), grant_id, McpReason::submit_refused(outcome_message))?;
        Ok(StepC::Refunded)
    } else {
        tx.execute("UPDATE jobs SET mcp_state = NULL WHERE id = ?1", [job_id])?;
        Ok(StepC::Kept { status: row.status })
    }
}

/// Step C's outcome as an audit row's `disposition` and `reasons` (the encoding table in
/// [`super::audit`]): a refund is `proposed` with the reason appended to the job; a kept charge and
/// a gone row are NULL with the job's state and step B's message.
pub fn step_c_audit(outcome: &StepC, step_b_message: &str) -> (Option<AuditDisposition>, Vec<McpReason>) {
    match outcome {
        StepC::Refunded => (Some(AuditDisposition::Proposed), vec![McpReason::submit_refused(step_b_message)]),
        StepC::Kept { status } => (None, vec![McpReason::state(status.as_str()), McpReason::step_b(step_b_message)]),
        StepC::Gone => (None, vec![McpReason::state("gone"), McpReason::step_b(step_b_message)]),
    }
}

/// `create_job`'s step C (ADR-027 decision 5; M3 amendment, item 3): [`finish_submit_conn`]'s body
/// and step C's outcome row ([`step_c_audit`]) in **one** `BEGIN IMMEDIATE` transaction, so a refund
/// never commits without its row and the log never ends at SUBMITTED for a job step C finished.
pub fn finish_submit_audited_conn(
    conn: &Connection,
    job_id: &str,
    grant_id: &str,
    step_b_message: &str,
    audit_args: &str,
) -> Result<StepC, AppError> {
    let tx = begin_immediate(conn)?;
    let outcome = finish_in_tx(&tx, job_id, grant_id, step_b_message)?;
    let (disposition, reasons) = step_c_audit(&outcome, step_b_message);
    append_audit_conn(
        &tx,
        &AuditEntry { tool: CREATE_JOB, args: audit_args, disposition, reasons: &reasons, job_id: Some(job_id), grant_id: Some(grant_id) },
    )?;
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
/// reason "interrupted before submit" — and every other row with the flag only has it cleared. Each
/// row it touches gets the outcome row step C would have written (`mcp_audit`, the encoding table in
/// [`super::audit`]), in the same transaction.
///
/// Only the instance holding the `mcp.lock` `flock` may run it, right after taking the lock and
/// before serving (M2): an instance without the lock could refund a job the serving instance is
/// still submitting. Its one caller is the MCP lifecycle's start (`mcp::server::lifecycle`), once per
/// acquisition, with a 250 ms busy timeout.
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
        let (disposition, reasons) = match (row.status, row.grant_id.as_deref()) {
            (JobStatus::Draft, Some(g)) => {
                refund(&tx, &id, row.mcp_reasons.as_deref(), g, McpReason::interrupted())?;
                done.refunded += 1;
                (Some(AuditDisposition::Proposed), vec![McpReason::interrupted()])
            }
            (status, _) => {
                tx.execute("UPDATE jobs SET mcp_state = NULL WHERE id = ?1", [&id])?;
                done.cleared += 1;
                (None, vec![McpReason::state(status.as_str()), McpReason::interrupted_after_hand_over()])
            }
        };
        // The outcome row step C would have written (M3 amendment, item 3), in this same transaction.
        append_audit_conn(
            &tx,
            &AuditEntry {
                tool: CREATE_JOB,
                args: RECONCILIATION_ARGS,
                disposition,
                reasons: &reasons,
                job_id: Some(&id),
                grant_id: row.grant_id.as_deref(),
            },
        )?;
    }
    tx.commit()?;
    Ok(done)
}

/// The `args` of a reconciliation's outcome row: it is not a tool call, so it names its writer.
pub const RECONCILIATION_ARGS: &str = r#"{"outcome_of":"reconciliation"}"#;

/// The `submit_job` command's guard (ADR-027 decision 5, "the window"): a job whose `mcp_state` is
/// set is between step A and step C, and the MCP server is submitting it; Anton cannot submit it by
/// hand meanwhile. Any non-NULL value refuses (the CHECK admits only `submitting`; fail closed).
/// The commands call it through [`refuse_while_mcp_submitting`]; the backends do not.
pub fn submit_refusal_for_mcp_state(job: &Job) -> Option<AppError> {
    job.mcp_state.as_ref().map(|state| {
        AppError::Conflict(format!(
            "Claude is submitting job {} (mcp_state = {state}); it can be run by hand once that ends",
            job.id
        ))
    })
}

/// [`submit_refusal_for_mcp_state`] as a `Result`, for the commands that submit a job by hand:
/// `submit_job` (through `commands::jobs::submittable_job_conn`, both arms) and
/// `retry_remote_submit`.
///
/// **A read outside the job's lock is enough.** `mcp_state` is set only by step A, in the
/// transaction that inserts the row, and only ever cleared after that. A job read with `mcp_state`
/// NULL can never become `submitting` later, so a check that passes stays true for the rest of the
/// command.
pub fn refuse_while_mcp_submitting(job: &Job) -> Result<(), AppError> {
    submit_refusal_for_mcp_state(job).map_or(Ok(()), Err)
}
