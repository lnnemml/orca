//! `create_job`'s core (ADR-027 decisions 3–5; M0 amendment, item 6; M3 amendment, items 1 and 3;
//! unit M3a): the whole call as one blocking pipeline, which the server runs as **one tracked task**
//! (`server::create_job_task`). Every seam is injected ([`CreateJobEnv`]): the app's are the real
//! step B (`super::step_b`), the `job:status` emit and the sidecar's SMILES call
//! (`server::wiring`).
//!
//! **The order** ([`run_create_job`], two halves so the order is visible in code):
//! 1. [`decide`]: the argument bounds; the source resolved ([`resolve_source`]: the database locked
//!    only for a DB-backed reference, a SMILES through the sidecar seam with no lock); the app's
//!    `%maxcore` and the assembly ([`assemble`]); the **memory verdict** on the assembled input;
//!    then **step A** ([`decide_conn`]: one `BEGIN IMMEDIATE` that classifies against the grant,
//!    its counters and R8's count read inside it, and writes the disposition with its audit row);
//!    then the `job:status` emit for a job it created.
//! 2. [`submit_charged`], for a SUBMITTED job only: **step B** with no lock held; **step C**
//!    ([`finish_submit_audited_conn`]: the row re-read, refunded or not by its state, the outcome
//!    row, one transaction), retried a bounded number of times ([`STEP_C_RETRY_WAITS`]); the
//!    `job:status` emit.
//!
//! **Nothing after step A's commit can stop steps B and C.** Nothing here reads the connection's
//! `closed` flag or can be cancelled (M0 amendment, item 6), and every call into foreign code after
//! [`decide_conn`] is isolated from a panic: a panicking emit is reported and skipped ([`emit`]), a
//! panicking step B is a refusal, and a panicking step C attempt is a failed attempt. Only a crash
//! (or every step C attempt failing) leaves `mcp_state = 'submitting'`, which the next lock holder's
//! reconciliation finishes.
//!
//! **Argument bounds** (M3 amendment, item 1; refused before anything is read or written, with no
//! audit row): `header` ≤ [`HEADER_MAX_BYTES`], `source` ≤ [`SOURCE_MAX_BYTES`] (room for
//! `smiles:` and the resolver's 1000-character SMILES), `target` ≤ [`TARGET_MAX_BYTES`], `group_id`
//! ≤ [`GROUP_ID_MAX_BYTES`], `title` ≤ [`TITLE_MAX_BYTES`]. The result is bounded by construction
//! (at most [`AUDIT_REASONS_MAX`](super::audit::AUDIT_REASONS_MAX) reasons of [`REASON_MESSAGE_MAX_BYTES`](super::audit::REASON_MESSAGE_MAX_BYTES), the source label, step B's
//! message cut to [`STEP_B_MESSAGE_MAX_BYTES`]) and checked against [`RESULT_CAP_BYTES`] as its
//! post-condition.

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::Arc;
use std::time::Duration;

use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use super::assemble::{assemble, Assembled, AssemblyError};
use super::audit::bounded_reasons;
use super::classify::{MemoryVerdict, ResolvedSource, Target};
use super::grant::target_of;
use super::header::parse_header;
use super::preflight::{mcp_maxcore, memory_verdict_for, MaxcoreSource, McpMaxcore};
use super::resolve::{resolve_source, SmilesTo3d};
use super::server::service::HostPaths;
use super::server::DbSource;
use super::steps::{decide_conn, finish_submit_audited_conn, Decided, McpReason, StepAInput, StepC};
use super::tools::{clip, within_cap, ToolError, RESULT_CAP_BYTES};
use crate::commands::settings::DbState;
use crate::error::AppError;
use crate::models::job::JobStatus;

/// The longest header `create_job` takes, in bytes (the grammar's input; the same order as
/// `memory_estimate`'s 20,000-character header).
pub const HEADER_MAX_BYTES: usize = 20_000;
/// The longest source reference, in bytes: `smiles:` plus the resolver's 1000-character SMILES
/// (printable ASCII, so one byte each), with room to spare.
pub const SOURCE_MAX_BYTES: usize = 1_024;
/// The longest target, in bytes: `local` or a profile id (the app mints v4 UUIDs, 36 bytes).
pub const TARGET_MAX_BYTES: usize = 64;
/// The longest group id, in bytes (a v4 UUID is 36).
pub const GROUP_ID_MAX_BYTES: usize = 64;
/// The longest title, in bytes.
pub const TITLE_MAX_BYTES: usize = 200;
/// The header as the audit row keeps it, in serialised bytes (the column takes 8192 characters).
pub const AUDIT_HEADER_MAX_BYTES: usize = 6_000;
/// Step B's message in the result, in serialised bytes.
pub const STEP_B_MESSAGE_MAX_BYTES: usize = 600;
/// A default title quotes at most this many bytes of the source.
const DEFAULT_TITLE_SOURCE_BYTES: usize = 100;
/// Step C's retries (decided by Anton, M3a): after a failed first attempt, one more after each wait
/// here, so at most three attempts. Each attempt is one `BEGIN IMMEDIATE` transaction under the
/// connection's busy timeout (`rusqlite`'s 5 s, pinned by a test) and waits at most one busy timeout at
/// `BEGIN IMMEDIATE` and one at `COMMIT` (rollback journal; each wait measured, `wiki/architecture/sqlite-commit-wait.md`),
/// so step C's **SQLite lock waits** total ≈ 31.25 s (nominal 3 × (5 s + 5 s) + 0.25 s + 1 s), derived
/// from those measured per-statement waits (each ≈ the busy timeout + ~10–15 ms, so ≈ 31.3 s strictly).
/// The factor 2 assumes no statement inside the transaction waits (it is a few rows, far below the
/// page cache, so no mid-transaction spill needs EXCLUSIVE). That figure
/// is **not** wall time (fsync time on a loaded disk comes on top) and does **not** include the wait for
/// the in-process `DbState` mutex before each attempt, which no timeout bounds: it lasts as long as
/// another holder in this process keeps it (a UI command, the poller, the reconciliation — each one
/// transaction or read, normally milliseconds). The exit flow waits for step C (its MCP wait) inside
/// the backstop's 60 s (`server::wiring::EXIT_FLOW_DEADLINE`), which also holds `terminate_on_exit`
/// (≈ 14.5 s worst) and the sidecar stop (≈ 0.3 s): 31.25 + 14.5 + 0.3 ≈ 46 s, leaving ≈ 14 s for
/// step B and the mutex waits. Every attempt failing is the crash case: `step_c_error` is reported and
/// the lock holder's reconciliation recovers the job.
pub const STEP_C_RETRY_WAITS: [Duration; 2] = [Duration::from_millis(250), Duration::from_secs(1)];

/// The tool's arguments (decision 4): no coordinates, charge or multiplicity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateJobArgs {
    /// The `!` lines and `%` blocks (decision 3's closed grammar).
    pub header: String,
    /// The geometry source: `job:<id>`, its `/scan:<k>`, `/neb:<k|ts>`, `/goat:<k>`, `molecule:<id>`
    /// or `smiles:<s>`.
    pub source: String,
    /// `local` or a server profile id.
    pub target: String,
    pub group_id: Option<String>,
    pub title: Option<String>,
}

/// What step B did, in words. Step C does not read it: it judges the row (decision 5). The message
/// goes into the refund's reason ("submit refused: …"), the outcome row and the result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StepBOutcome {
    /// The existing submit path took the job (queued, pending, enqueued on the server, …).
    Handed(String),
    /// The path refused before it took the job.
    Refused(String),
}

impl StepBOutcome {
    pub fn message(&self) -> &str {
        match self {
            StepBOutcome::Handed(m) | StepBOutcome::Refused(m) => m,
        }
    }
}

/// Step B (decision 5): submit a charged draft through the app's existing path — local `submit`, or
/// `submit_to_server` under its in-flight guard. Called with **no lock held**; it takes its own.
/// Part B implements it over `local_backend::submit` and `submit_to_server`.
pub trait StepB: Send + Sync {
    fn submit(&self, job_id: &str, target: &Target) -> StepBOutcome;
}

/// The write tools' emit seam: the service has no `AppHandle`, so every event a tool owes the UI goes
/// through this trait. [`job_status`](StatusEmit::job_status) is `create_job`'s (decision 9: step A
/// emits, so Jobs refreshes); [`groups_changed`](StatusEmit::groups_changed) is `create_group`'s (M4
/// amendment, item 7). The app implements both in `server::wiring::AppEmit`.
pub trait StatusEmit: Send + Sync {
    fn job_status(&self, job_id: &str, status: JobStatus);

    /// `groups:changed`, payload-free: a group was created behind the UI's back, so the Jobs screen
    /// reloads its groups. The `create_group` tool calls it only after its transaction committed and
    /// `DbState` was released, and never for a refused or failed insert.
    fn groups_changed(&self);
}

/// Everything `create_job` reaches outside its arguments.
pub struct CreateJobEnv {
    pub db: Arc<dyn DbSource>,
    /// The SMILES seam (`SidecarSmiles` in the app), called off the main thread with no lock.
    pub smiles: Arc<dyn SmilesTo3d + Send + Sync>,
    /// `/proc/meminfo` and the data dir, for `%maxcore` and the memory verdict.
    pub host: Arc<HostPaths>,
    pub step_b: Arc<dyn StepB>,
    pub emit: Arc<dyn StatusEmit>,
}

/// The call's final disposition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Refused,
    Proposed,
    Submitted,
}

/// What the app set in the input (decision 4: "set by the app", never silently).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SetByApp {
    pub maxcore_mib: u64,
    pub maxcore_source: MaxcoreSource,
    /// Dimensionless integers, from `charge_mult_from`.
    pub charge: i64,
    pub multiplicity: i64,
    /// The source reference they came from.
    pub charge_mult_from: String,
    pub atom_count: usize,
}

/// Steps B and C of a SUBMITTED call.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SubmitReport {
    /// Step B's message, cut to [`STEP_B_MESSAGE_MAX_BYTES`].
    pub step_b: String,
    /// Step C refunded the charge: the job is a PROPOSED draft now.
    pub refunded: bool,
    /// The job's status step C read; `None` when the row was gone or step C failed.
    pub job_status: Option<String>,
    /// Step C's failure, if it failed: the job keeps `mcp_state = 'submitting'` until the next lock
    /// holder's reconciliation.
    pub step_c_error: Option<String>,
}

/// What `create_job` returns.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CreateJobResult {
    /// The final disposition: a SUBMITTED call that step C refunded is `proposed`.
    pub disposition: Outcome,
    pub job_id: Option<String>,
    /// At most [`AUDIT_REASONS_MAX`](super::audit::AUDIT_REASONS_MAX), each cut to [`REASON_MESSAGE_MAX_BYTES`](super::audit::REASON_MESSAGE_MAX_BYTES); a last `"more"`
    /// reason counts the rest.
    pub reasons: Vec<McpReason>,
    /// Every reason, listed or not.
    pub reasons_count: usize,
    pub set_by_app: Option<SetByApp>,
    pub submit: Option<SubmitReport>,
}

/// A charged job between step A and step B.
#[derive(Debug, Clone)]
pub struct Charged {
    pub job_id: String,
    pub grant_id: String,
    pub target: Target,
    pub audit_args: String,
    pub set_by_app: Option<SetByApp>,
}

/// The first half's result.
#[derive(Debug, Clone)]
pub enum Decision {
    /// REFUSED or PROPOSED: the call is over.
    Done(CreateJobResult),
    /// SUBMITTED: steps B and C follow.
    Charged(Charged),
}

/// The whole call: [`decide`], then for a SUBMITTED job [`submit_charged`]. Blocking; the server
/// runs it inside the tracked task, never in the handler.
pub fn run_create_job(env: &CreateJobEnv, args: CreateJobArgs) -> Result<CreateJobResult, ToolError> {
    match decide(env, args)? {
        Decision::Done(result) => fit(result),
        Decision::Charged(charged) => fit(submit_charged(env, charged)),
    }
}

/// An argument over its bound, or empty where it must not be. Sizes only, never the text.
fn check_args(args: &CreateJobArgs) -> Result<(), ToolError> {
    let bound = |name: &str, value: &str, max: usize, may_be_empty: bool| {
        if value.len() > max {
            return Err(ToolError::refused(format!("`{name}` is {} bytes; at most {max}", value.len())));
        }
        if value.is_empty() && !may_be_empty {
            return Err(ToolError::refused(format!("`{name}` is empty")));
        }
        Ok(())
    };
    bound("header", &args.header, HEADER_MAX_BYTES, false)?;
    bound("source", &args.source, SOURCE_MAX_BYTES, false)?;
    bound("target", &args.target, TARGET_MAX_BYTES, false)?;
    if let Some(g) = &args.group_id {
        bound("group_id", g, GROUP_ID_MAX_BYTES, false)?;
    }
    if let Some(t) = &args.title {
        bound("title", t, TITLE_MAX_BYTES, false)?;
    }
    Ok(())
}

/// The call's argument text for the audit row: a JSON object, every field cut in serialised bytes,
/// the header to [`AUDIT_HEADER_MAX_BYTES`] with its full size and whether it was cut. Its byte count
/// stays below the column's 8192-character CHECK.
pub fn audit_args(args: &CreateJobArgs) -> String {
    let (header, header_cut) = clip(&args.header, AUDIT_HEADER_MAX_BYTES);
    let value = serde_json::json!({
        "header": header,
        "header_bytes": args.header.len(),
        "header_cut": header_cut,
        "source": clip(&args.source, SOURCE_MAX_BYTES).0,
        "target": clip(&args.target, TARGET_MAX_BYTES).0,
        "group_id": args.group_id.as_deref().map(|g| clip(g, GROUP_ID_MAX_BYTES).0),
        "title": args.title.as_deref().map(|t| clip(t, TITLE_MAX_BYTES).0),
    });
    value.to_string()
}

/// The app's `%maxcore` and the assembled input, when the source can be assembled at all: it
/// resolved, and its atoms, charge and multiplicity read. Otherwise `None`; the classifier refuses
/// such a source (R4).
fn assemble_for(
    env: &CreateJobEnv,
    header: &str,
    source: &Result<ResolvedSource, String>,
    target: &Target,
) -> Result<Option<(McpMaxcore, Result<Assembled, AssemblyError>)>, ToolError> {
    let Ok(src) = source else { return Ok(None) };
    let Ok(cm) = &src.charge_mult else { return Ok(None) };
    if src.atoms.is_empty() {
        return Ok(None);
    }
    let maxcore = mcp_maxcore(env.db.db(), &env.host.meminfo, &env.host.data_dir, header, target).map_err(ToolError::from_app)?;
    Ok(Some((maxcore, assemble(header, maxcore.mib, &src.atoms, *cm, &src.label))))
}

fn set_by_app(assembled: &Option<(McpMaxcore, Result<Assembled, AssemblyError>)>) -> Option<SetByApp> {
    let (maxcore, Ok(a)) = assembled.as_ref()? else { return None };
    Some(SetByApp {
        maxcore_mib: a.app_set.maxcore_mib,
        maxcore_source: maxcore.source,
        charge: a.app_set.charge,
        multiplicity: a.app_set.multiplicity,
        charge_mult_from: clip(&a.app_set.charge_mult_from, SOURCE_MAX_BYTES).0,
        atom_count: a.app_set.atom_count,
    })
}

fn result(disposition: Outcome, job_id: Option<String>, reasons: &[McpReason], set_by_app: Option<SetByApp>) -> CreateJobResult {
    CreateJobResult {
        disposition,
        job_id,
        reasons: bounded_reasons(reasons),
        reasons_count: reasons.len(),
        set_by_app,
        submit: None,
    }
}

/// The first half: the bounds, the source, `%maxcore` and the assembly, the memory verdict, step A,
/// and the `job:status` emit for a job step A created.
pub fn decide(env: &CreateJobEnv, args: CreateJobArgs) -> Result<Decision, ToolError> {
    check_args(&args)?;
    let target = target_of(args.target.clone());
    let audit = audit_args(&args);
    let title = match &args.title {
        Some(t) => t.clone(),
        None => format!("Claude: {}", clip(&args.source, DEFAULT_TITLE_SOURCE_BYTES).0),
    };
    let db = env.db.db();
    let header = parse_header(&args.header);
    let source = resolve_source(db, &args.source, &*env.smiles);
    let assembled = assemble_for(env, &args.header, &source, &target)?;
    let memory = match &assembled {
        Some((_, Ok(a))) => memory_verdict_for(db, &env.host.meminfo, &env.host.data_dir, &a.input, &target)
            .map_err(ToolError::from_app)?,
        _ => MemoryVerdict::Unknown,
    };

    let decided = {
        let conn = db.lock().map_err(ToolError::from_app)?;
        decide_conn(
            &conn,
            StepAInput {
                header: &header,
                source: &source,
                target: &target,
                memory,
                assembled: assembled.as_ref().map(|(_, a)| a),
                title: &title,
                group_id: args.group_id.as_deref(),
                audit_args: &audit,
            },
        )
        .map_err(ToolError::from_app)?
    };

    // Step A has committed: from here on nothing may stop a charged job from reaching steps B and C.
    let set_by_app = set_by_app(&assembled);
    Ok(match decided {
        Decided::Refused { reasons } => Decision::Done(result(Outcome::Refused, None, &reasons, None)),
        Decided::Proposed { job, reasons } => {
            emit(env, &job.id, job.status);
            Decision::Done(result(Outcome::Proposed, Some(job.id), &reasons, set_by_app))
        }
        Decided::Charged { job, grant_id } => {
            emit(env, &job.id, job.status);
            Decision::Charged(Charged { job_id: job.id, grant_id, target, audit_args: audit, set_by_app })
        }
    })
}

/// The `job:status` emit, isolated from a panic: one stderr line, and the call goes on. After step A
/// the emit is the only foreign code before step B, and a panic escaping it would leave a charged job
/// `submitting` until the next reconciliation.
fn emit(env: &CreateJobEnv, job_id: &str, status: JobStatus) {
    if catch_unwind(AssertUnwindSafe(|| env.emit.job_status(job_id, status))).is_err() {
        eprintln!("[OrcaStudio] mcp: the job:status emit for job {job_id} panicked; create_job goes on");
    }
}

/// The second half, for a SUBMITTED job: step B with no lock (a panic is a refusal), step C with its
/// outcome row (retried, [`STEP_C_RETRY_WAITS`]), and the `job:status` emit. It returns a result on
/// every path: once step A has committed, the call reports what became of the job.
pub fn submit_charged(env: &CreateJobEnv, charged: Charged) -> CreateJobResult {
    submit_charged_with(env, charged, &STEP_C_RETRY_WAITS, finish_submit_audited_conn)
}

/// [`submit_charged`] with step C's waits and its transaction as seams (`finish_submit_audited_conn`
/// in the app); a test injects failing attempts and short waits.
pub(crate) fn submit_charged_with(
    env: &CreateJobEnv,
    charged: Charged,
    waits: &[Duration],
    mut step_c_tx: impl FnMut(&Connection, &str, &str, &str, &str) -> Result<StepC, AppError>,
) -> CreateJobResult {
    let step_b = catch_unwind(AssertUnwindSafe(|| env.step_b.submit(&charged.job_id, &charged.target)))
        .unwrap_or_else(|_| StepBOutcome::Refused("step B panicked".into()));
    let message = step_b.message();
    let step_c = step_c_retried(env.db.db(), waits, |conn| {
        step_c_tx(conn, &charged.job_id, &charged.grant_id, message, &charged.audit_args)
    });

    let mut report =
        SubmitReport { step_b: clip(message, STEP_B_MESSAGE_MAX_BYTES).0, refunded: false, job_status: None, step_c_error: None };
    let (disposition, reasons) = match step_c {
        Ok(StepC::Refunded) => {
            report.refunded = true;
            report.job_status = Some(JobStatus::Draft.as_str().into());
            emit(env, &charged.job_id, JobStatus::Draft);
            (Outcome::Proposed, vec![McpReason::submit_refused(message)])
        }
        Ok(StepC::Kept { status }) => {
            report.job_status = Some(status.as_str().into());
            emit(env, &charged.job_id, status);
            (Outcome::Submitted, Vec::new())
        }
        Ok(StepC::Gone) => (Outcome::Submitted, vec![McpReason::state("gone")]),
        Err(e) => {
            report.step_c_error = Some(clip(&e.to_string(), STEP_B_MESSAGE_MAX_BYTES).0);
            (Outcome::Submitted, Vec::new())
        }
    };
    let mut out = result(disposition, Some(charged.job_id), &reasons, charged.set_by_app);
    out.submit = Some(report);
    out
}

/// Step C, retried: one attempt at once, then one after each of `waits` while they fail. A failed
/// attempt (an error, or a panic) rolled its own transaction back, so the next one re-reads the same
/// row, and only the first success has written. Every attempt failing returns the last error: the job
/// keeps `mcp_state = 'submitting'` for the lock holder's reconciliation, as after a crash.
fn step_c_retried(
    db: &DbState,
    waits: &[Duration],
    mut attempt: impl FnMut(&Connection) -> Result<StepC, AppError>,
) -> Result<StepC, AppError> {
    let mut once = || {
        catch_unwind(AssertUnwindSafe(|| db.lock().and_then(|conn| attempt(&conn))))
            .unwrap_or_else(|_| Err(AppError::Internal("step C panicked".into())))
    };
    let mut outcome = once();
    for wait in waits {
        if outcome.is_ok() {
            break;
        }
        std::thread::sleep(*wait);
        outcome = once();
    }
    outcome
}

/// The size post-condition (M2 amendment, item 7): the result fits [`RESULT_CAP_BYTES`]. It is
/// bounded by construction; a breach is refused with the disposition and the job id, which the
/// audit log and `get_job` also hold.
fn fit(result: CreateJobResult) -> Result<CreateJobResult, ToolError> {
    let (disposition, job_id) = (result.disposition, result.job_id.clone());
    within_cap(result, RESULT_CAP_BYTES).map_err(|e| match e {
        ToolError::TooLarge { bytes, cap } => ToolError::refused(format!(
            "create_job's result is {bytes} bytes, over the cap of {cap}; the call ended {disposition:?}, job {}",
            job_id.as_deref().unwrap_or("none")
        )),
        other => other,
    })
}
