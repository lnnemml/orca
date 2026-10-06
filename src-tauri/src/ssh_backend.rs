//! The remote backend's core (ADR-024 o, unit 5.3 B1): submit a job to a server profile, retry
//! it, label it, withdraw it. **Tauri-free**: every function takes the database as `&DbState`,
//! locked only around its own reads and writes — **never across an ssh call** (a submit can take a
//! minute) — and a [`CommandRunner`]: the real `SystemRunner` in the app, a fake or a local loop in
//! the tests. `execution_backend::SshBackend` is the thin `AppHandle` wrapper over it.
//!
//! **A job is remote iff its coordinates are non-NULL** ([`coordinates`], ADR-024 o item 1): the
//! profile host it was submitted to, its job dir `<root>/jobs/<id>` and its slot socket, written
//! once, before any ssh, and never rewritten from the profile. Every later call uses them, never
//! the profile's current values (n 6b). `backend_id` only names the profile.
//!
//! **The submit sequence** ([`submit_remote`]) — what each step leaves behind if it fails:
//!
//! | step | what | the row after a failure here |
//! |---|---|---|
//! | 1 | checks, no ssh: a draft without coordinates; the profile a run target (n item 2); the input's `%pal` aligned **downward** to the mask ([`align_remote_pal`], o 14.2) and written with the aux files into the local job dir, which is listed — an unreadable `%pal`, over 1000 files or a name outside the path rule refuses | **row unchanged** (`draft`, no coordinates); the local dir may be written, and is rewritten on the next attempt (o 14.4) |
//! | 2 | **persist** (o 3.1): one transaction sets `queued`, `backend_id`, the coordinates and the local `job_dir` | the transaction rolls back: `draft` |
//! | 3 | **prepare** (o 3.2, 13.1): read-only; every existing component of the root a directory at its own realpath, `<root>/bin` included | `queued` + coordinates; label: "not on the server" |
//! | 4 | **install** when `bin/`, `tsp/` or any uploaded script (wrapper, cancel, collect) is missing, then **prepare again**: every script must now hash right (post-condition, rule #9) | same |
//! | 5 | **upload**: `rsync -a --checksum --mkpath`, never `--delete` | same |
//! | 6 | **the submit call** (o 3.3): `Enqueued`, `Refused`, `RefusedKup`, `FailedAfterClaim`, or no readable reply | `queued` + coordinates; the label call decides from the server |
//!
//! Steps 3–6 **never change the status**: the row stays `queued` with its coordinates whatever
//! happens, because only the server knows what happened (Decision c), and the label call (o 3.4)
//! resolves every such row. They write two things only:
//! - `error_message` = the attempt's failure, for people (cleared on `Enqueued`);
//! - the profile's stamp is cleared **only** for [`SubmitOutcome::KillUserProcesses`], i.e. only for
//!   `SubmitReply::RefusedKup` — never by reading a refusal's text (n item 7, o 13.3).
//!
//! **Retry** ([`resubmit_remote`]) re-derives the local input with the profile's *current* mask
//! (step 1 without the persist), then steps 3–6 from the recorded coordinates — only for a job the
//! label call finds "not on the server" (o 3.4: "submit interrupted" is never retried).
//! **Withdraw** ([`withdraw_remote`], o 2, 14.1) readies the server like steps 3–4, makes the job
//! dir (re-asserting its shape), runs `cancel.sh` and `collect.sh` through the trampoline, and lets
//! `classify` decide the status — never a hard-coded `Cancelled` (Decision c).

use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::params;
use serde::Serialize;

use crate::commands::jobs::get_job_conn;
use crate::commands::server_profiles::{clear_verified, get_profile_conn};
use crate::commands::settings::DbState;
use crate::error::AppError;
use crate::local_backend::{align_pal_nprocs, prepare_job_dir, read_aux_files, read_pal_nprocs};
use crate::models::job::{Job, JobStatus};
use crate::models::server_profile::{distinct_cpus, is_run_target, parse_core_mask, validate_host, ServerProfile};
use crate::remote::classify::{classify, Classification, Outcome};
use crate::remote::prepare::{
    check_prepare, parse_install_reply, parse_prepare_reply, InstallArgs, PrepareArgs, Prepared,
};
use crate::remote::run::{parse_mkjob_reply, parse_run_reply, JobScript, MkjobArgs, RunArgs, RunReply};
use crate::remote::scripts::{stdin_with_values, INSTALL, LABEL, MKJOB, PREPARE, RUN, SUBMIT};
use crate::remote::slot_socket_path;
use crate::remote::snapshot::{Attempt, JobIdentity};
use crate::remote::ssh::{ssh_bash_argv, CommandRunner, ProcessOutput, SSH_PROGRAM};
use crate::remote::submit::{
    label, parse_label_reply, parse_submit_reply, remote_job_dir, KupEvidence, Label, LabelArgs,
    LabelFacts, SubmitArgs, SubmitReply,
};
use crate::remote::sync::{upload_argv, upload_expected, RSYNC_PROGRAM};
use crate::remote::wire::parse_snapshot;

/// The bound on each small call (prepare, install, label, mkjob), connect included. Derived from the
/// scripts' own bounds with margin: ssh's 10 s connect plus, at worst, the install's six
/// `sha256sum`s at `timeout -k 1 5` (36 s) = 46 s; prepare's three = 28 s; label's `tsp -l` 4 s.
pub const CALL_TIMEOUT: Duration = Duration::from_secs(60);

/// The bound on the submit call (o item 3.3.1): above the server's 20 s lock wait plus the 37 s
/// its bounded children can take at worst.
pub const SUBMIT_TIMEOUT: Duration = Duration::from_secs(60);

/// The upload's fixed allowance; the timeout grows with the bytes (o item 6).
const UPLOAD_BASE: Duration = Duration::from_secs(60);

/// The slowest upload rate the timeout allows for, in bytes per second. **Not measured** — a floor
/// chosen so a slow link is not killed mid-transfer; a killed upload only means a retry.
const UPLOAD_MIN_RATE: u64 = 256 * 1024;

/// The upload's timeout for `bytes` of input: [`UPLOAD_BASE`] plus `bytes` at [`UPLOAD_MIN_RATE`].
pub fn upload_timeout(bytes: u64) -> Duration {
    UPLOAD_BASE + Duration::from_secs(bytes / UPLOAD_MIN_RATE)
}

// --- Coordinates and refusals ---------------------------------------------------------------

/// Where a remote job lives (schema v20, ADR-024 o item 1). Recorded at submit, never rewritten.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RemoteCoordinates {
    /// The profile's `~/.ssh/config` alias at submit time.
    pub host: String,
    /// `<root>/jobs/<job id>` on the server.
    pub job_dir: String,
    /// The slot's `tsp` socket, `<root>/tsp/slot<N>.sock`.
    pub socket: String,
}

/// A job's coordinates: `None` for a local job, all three for a remote one. A partial set is an
/// error, never "local" (the v20 CHECK forbids it; this is the Rust side of the same rule).
pub fn coordinates(job: &Job) -> Result<Option<RemoteCoordinates>, AppError> {
    match (&job.remote_host, &job.remote_job_dir, &job.remote_socket) {
        (None, None, None) => Ok(None),
        (Some(host), Some(job_dir), Some(socket)) => Ok(Some(RemoteCoordinates {
            host: host.clone(),
            job_dir: job_dir.clone(),
            socket: socket.clone(),
        })),
        _ => Err(AppError::Internal(format!("job {}: partial remote coordinates", job.id))),
    }
}

/// The root a remote job was submitted under, read from its recorded job dir `<root>/jobs/<id>` —
/// never the profile's current root (n 6b).
pub fn recorded_root(coords: &RemoteCoordinates, job_id: &str) -> Result<String, AppError> {
    let bad = || AppError::Internal(format!("job {job_id}: recorded job dir {:?} is not <root>/jobs/<id>", coords.job_dir));
    let root = coords.job_dir.strip_suffix(&format!("/jobs/{job_id}")).ok_or_else(bad)?;
    match remote_job_dir(root, job_id) {
        Ok(dir) if dir == coords.job_dir => Ok(root.to_string()),
        _ => Err(bad()),
    }
}

/// Refuse to act locally on a remote job that is not terminal (ADR-024 o item 2): a cancel or a
/// delete here would set a local state the server contradicts — never a local `Cancelled` for a
/// job that may be on the server (Decision i). Withdraw is the one exit until unit 5.4.
pub fn refuse_if_remote_live(job: &Job) -> Result<(), AppError> {
    if job.is_remote() && matches!(job.status, JobStatus::Queued | JobStatus::Running) {
        return Err(AppError::Backend(format!(
            "job {} is {} on server {}: remote cancel arrives in unit 5.4 \
             (a job that is not on the server can be withdrawn)",
            job.id,
            job.status.as_str(),
            job.remote_host.as_deref().unwrap_or_default()
        )));
    }
    Ok(())
}

/// A cancel of remote job `job` (the `SshBackend` arm of `cancel_job`): always refused until remote
/// cancel lands (unit 5.4). A live job gets [`refuse_if_remote_live`]'s reason, checked **first**,
/// so it is never told there is "nothing to cancel"; a terminal one has nothing to cancel.
pub fn cancel_remote(job: &Job) -> Result<(), AppError> {
    refuse_if_remote_live(job)?;
    Err(AppError::Backend(format!("job {} is '{}': there is nothing to cancel", job.id, job.status.as_str())))
}

// --- Calls ------------------------------------------------------------------------------------

/// The last 500 characters of a stream, trimmed, for a message.
fn tail(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    let text = text.trim();
    let skip = text.chars().count().saturating_sub(500);
    text.chars().skip(skip).collect()
}

fn exit_text(out: &ProcessOutput) -> String {
    out.code.map_or_else(|| "a signal".to_string(), |c| c.to_string())
}

/// Run one stdin-fed script on `host` (`ssh … -- <host> bash -s`, its values as a NUL list after
/// it, n item 11) and parse its reply. The reply is parsed first (an `error` record is the best
/// message); exit 255 is ssh's own failure (connect, auth, host key); a complete reply with any
/// exit status but 0 is not trusted (fail closed).
fn call<T, E: std::fmt::Display>(
    runner: &dyn CommandRunner,
    host: &str,
    what: &str,
    script: &str,
    values: &[String],
    timeout: Duration,
    parse: impl FnOnce(&[u8]) -> Result<T, E>,
) -> Result<T, String> {
    let argv = ssh_bash_argv(host).map_err(|e| format!("{what}: {e}"))?;
    let stdin = stdin_with_values(script, values).map_err(|e| format!("{what}: {e}"))?;
    let out = runner.run(SSH_PROGRAM, &argv, &stdin, timeout).map_err(|e| format!("{what}: {e}"))?;
    if out.code == Some(255) {
        return Err(format!("{what}: ssh failed (exit 255): {}", tail(&out.stderr)));
    }
    let parsed = parse(&out.stdout)
        .map_err(|e| format!("{what}: {e} (ssh exit {}; stderr: {:?})", exit_text(&out), tail(&out.stderr)))?;
    if out.code != Some(0) {
        return Err(format!("{what}: ssh exited with {} after a complete reply; not trusted", exit_text(&out)));
    }
    Ok(parsed)
}

/// The read-only prepare call and Rust's decision over its facts.
fn prepare_call(runner: &dyn CommandRunner, host: &str, sent: &PrepareArgs) -> Result<Prepared, String> {
    let facts = call(runner, host, "prepare", PREPARE, &sent.values(), CALL_TIMEOUT, |out| {
        parse_prepare_reply(out, sent)
    })?;
    check_prepare(&facts, sent).map_err(|refusal| format!("prepare: refused: {refusal}"))
}

/// The install call: `bin/`, `tsp/` and the wrapper. Its answer is not the post-condition.
fn install_call(runner: &dyn CommandRunner, host: &str, root: &str) -> Result<(), String> {
    let sent = InstallArgs::new(root).map_err(|e| format!("install: {e}"))?;
    call(runner, host, "install", INSTALL, &sent.values(), CALL_TIMEOUT, |out| parse_install_reply(out, &sent))
        .map(|_| ())
}

/// The read-only label call for a remote job, by its recorded coordinates (o item 3.4).
fn label_call(runner: &dyn CommandRunner, coords: &RemoteCoordinates) -> Result<LabelReport, String> {
    let sent = LabelArgs::new(&coords.job_dir, &coords.socket).map_err(|e| format!("label: {e}"))?;
    let facts = call(runner, &coords.host, "label", LABEL, &sent.values(), CALL_TIMEOUT, |out| {
        parse_label_reply(out, &sent)
    })?;
    Ok(LabelReport { facts, label: label(&facts) })
}

// --- Submit -------------------------------------------------------------------------------------

/// The step of a submit that refused, before the server claimed anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SubmitStep {
    /// The read-only pre-upload call (o 3.2): a transport failure or a component's wrong shape.
    Prepare,
    /// `bin/`, `tsp/` or the wrapper could not be readied, or the prepare call after it did not
    /// find the wrapper hashing right (o 13.1).
    Install,
    /// The rsync upload (o 3.2).
    Upload,
    /// The submit call's own `refused` (o 3.3).
    Submit,
}

/// What one submit attempt did. The row is `queued` with its coordinates in every case; this says
/// what the user is offered next, and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case", tag = "outcome")]
pub enum SubmitOutcome {
    /// In the slot's `tsp` queue with this id; `.enqueued` is published.
    Enqueued { tsp_id: u64 },
    /// Nothing was claimed on the server: the job is "not on the server" (retry or withdraw).
    NotClaimed { step: SubmitStep, reason: String },
    /// `KillUserProcesses` is not `b false` (o 13.3): nothing claimed, and the profile's stamp was
    /// cleared — a run target no longer, until a connection test passes.
    KillUserProcesses { evidence: KupEvidence },
    /// Claimed (`.submitting`) but not enqueued: "submit interrupted", withdraw only (o 3.3.9).
    FailedAfterClaim { reason: String },
    /// The submit call's reply was lost or unreadable (a timeout, ssh exit 255, a broken reply):
    /// the server may have done anything up to the enqueue, so only the label call can tell.
    Unknown { reason: String },
}

impl SubmitOutcome {
    /// The attempt's failure for people; `None` for `Enqueued`.
    pub fn failure(&self) -> Option<String> {
        match self {
            SubmitOutcome::Enqueued { .. } => None,
            SubmitOutcome::NotClaimed { step, reason } => {
                Some(format!("not submitted ({step:?} step; nothing is on the server): {reason}"))
            }
            SubmitOutcome::KillUserProcesses { evidence } => Some(format!(
                "not submitted: KillUserProcesses on the server is not `b false` (busctl rc {}, stdout {:?}, stderr {:?}); \
                 the profile is no longer verified",
                evidence.rc, evidence.stdout, evidence.stderr
            )),
            SubmitOutcome::FailedAfterClaim { reason } => {
                Some(format!("submit interrupted on the server after its claim: {reason}"))
            }
            SubmitOutcome::Unknown { reason } => {
                Some(format!("submit outcome unknown (the server decides on the next check): {reason}"))
            }
        }
    }
}

/// What became of the input's `%pal` on one attempt (ADR-024 o item 14.2): the input's own
/// `nprocs` (`None`: it had no `%pal`), the `nprocs` uploaded, and whether the text was rewritten.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct PalAlignment {
    pub input_nprocs: Option<u32>,
    pub nprocs: u32,
    /// The distinct CPUs of the attempt's core mask, the cap.
    pub mask_cpus: u32,
    pub rewritten: bool,
}

impl PalAlignment {
    /// The visible notice of a change, worded like the local run's log line; `None` when the
    /// input was uploaded as it is.
    pub fn notice(&self) -> Option<String> {
        self.rewritten.then(|| {
            let was = self.input_nprocs.map_or("no %pal".to_string(), |n| format!("nprocs {n}"));
            format!(
                "[OrcaStudio] %pal nprocs aligned to {} (the server profile's core mask has {} CPUs; the input had {was})",
                self.nprocs, self.mask_cpus
            )
        })
    }
}

/// The input to upload for `mask` (ADR-024 o item 14.2): `%pal nprocs` aligned **downward only** —
/// `min(the input's nprocs, the distinct CPUs of the mask)`; no `%pal` → the distinct-CPU count, as a
/// local run. A small `%pal` is never raised (per-rank `%maxcore` multiplies with it). The text is
/// rewritten by the local run's own [`align_pal_nprocs`]. Post-condition (rule #9): exactly one
/// `%pal` directive, stating that `nprocs`. An unreadable `%pal` is an `Err`, never a guess.
pub fn align_remote_pal(input: &str, mask: &str) -> Result<(String, PalAlignment), String> {
    let ranges = parse_core_mask(mask).map_err(|why| format!("core mask {mask:?}: {why}"))?;
    let mask_cpus = u32::try_from(distinct_cpus(&ranges)).map_err(|_| format!("core mask {mask:?} names too many CPUs"))?;
    let input_nprocs = read_pal_nprocs(input)?;
    let nprocs = input_nprocs.map_or(mask_cpus, |n| n.min(mask_cpus));
    let (aligned, rewritten) = align_pal_nprocs(input, nprocs);
    let directives = aligned.lines().filter(|l| l.trim_start().to_ascii_lowercase().starts_with("%pal")).count();
    if directives != 1 || read_pal_nprocs(&aligned)? != Some(nprocs) {
        return Err(format!("after aligning %pal to {nprocs}, the input holds {directives} %pal directive(s), not exactly one stating it"));
    }
    Ok((aligned, PalAlignment { input_nprocs, nprocs, mask_cpus, rewritten }))
}

/// One submit or retry attempt: what the server did, and what became of the input's `%pal`.
/// `submit_job` and `retry_remote_submit` return `pal.notice()` with the outcome.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SubmitAttempt {
    pub outcome: SubmitOutcome,
    pub pal: PalAlignment,
}

/// Everything steps 3–6 need, fixed by the persist step (or read back for a retry).
struct SubmitPlan {
    job_id: String,
    profile_id: String,
    coords: RemoteCoordinates,
    root: String,
    local_dir: PathBuf,
    upload_bytes: u64,
    args: SubmitArgs,
    pal: PalAlignment,
}

/// Submit draft `job_id` to profile `profile_id` (ADR-024 o item 3; the table in the module doc).
/// `data_dir` holds the local job dirs (`<data_dir>/jobs/<id>`, rule #3). An `Err` is a refusal
/// before anything was written, or a database failure; every outcome after the persist step is
/// an `Ok`.
pub fn submit_remote(
    db: &DbState,
    runner: &dyn CommandRunner,
    data_dir: &Path,
    job_id: &str,
    profile_id: &str,
) -> Result<SubmitAttempt, AppError> {
    let plan = persist_submit(db, data_dir, job_id, profile_id)?;
    upload_and_submit(db, runner, &plan)
}

/// The run-target gate (n item 2): `verified_at` gates new submits and retries only (n 6a).
fn run_target(profile: &ServerProfile) -> Result<&str, AppError> {
    is_run_target(profile)
        .map_err(|why| AppError::Backend(format!("server profile {} is not a run target: {why}", profile.name)))?;
    if profile.slot_count != 1 {
        return Err(AppError::Backend(format!("server profile {}: {} slots, only 1 is supported (n item 1)", profile.name, profile.slot_count)));
    }
    validate_host(&profile.host).map_err(|e| AppError::Invalid(e.to_string()))?;
    profile
        .core_mask
        .as_deref()
        .ok_or_else(|| AppError::Backend(format!("server profile {} has no core mask", profile.name)))
}

/// Steps 1–2: the checks that need no ssh, then the coordinates persisted with `queued` in one
/// transaction (o 3.1). Holds the database lock throughout, so the profile read, the run-target
/// check and the write are one atomic step: an edit cannot slip between them.
fn persist_submit(db: &DbState, data_dir: &Path, job_id: &str, profile_id: &str) -> Result<SubmitPlan, AppError> {
    let conn = db.lock()?;
    let tx = conn.unchecked_transaction()?;
    let job = get_job_conn(&tx, job_id)?;
    if job.status != JobStatus::Draft {
        return Err(AppError::Backend(format!("job {job_id} is '{}'; only draft jobs can be submitted", job.status.as_str())));
    }
    if coordinates(&job)?.is_some() {
        return Err(AppError::Backend(format!("job {job_id} already has remote coordinates")));
    }
    let profile = get_profile_conn(&tx, profile_id)?;
    let mask = run_target(&profile)?;
    let root = profile.remote_scratch_dir.clone();
    let coords = RemoteCoordinates {
        host: profile.host.clone(),
        job_dir: remote_job_dir(&root, job_id).map_err(|e| AppError::Invalid(e.to_string()))?,
        socket: slot_socket_path(&root, 0),
    };
    let (local_dir, pal) = write_local_dir(&tx, data_dir, &job, mask)?;
    let (args, upload_bytes) = submit_args(&root, job_id, &coords, mask, &profile.remote_orca_path, &local_dir)?;
    let local = local_dir.to_str().ok_or_else(|| AppError::Internal(format!("local job dir {local_dir:?} is not UTF-8")))?;

    let updated = tx.execute(
        "UPDATE jobs SET status = 'queued', backend_id = ?1, remote_host = ?2, remote_job_dir = ?3, \
         remote_socket = ?4, job_dir = ?5, error_message = NULL \
         WHERE id = ?6 AND status = 'draft' AND remote_host IS NULL",
        params![profile_id, coords.host, coords.job_dir, coords.socket, local, job_id],
    )?;
    if updated != 1 {
        return Err(AppError::Conflict(format!("job {job_id} changed while it was being submitted")));
    }
    tx.commit()?;
    Ok(SubmitPlan { job_id: job_id.to_string(), profile_id: profile_id.to_string(), coords, root, local_dir, upload_bytes, args, pal })
}

/// The local job dir of one attempt: the input from `jobs.input_content` (the user's original, never
/// the last upload) with `%pal` aligned to **this attempt's** mask, and the aux files — written
/// before anything is listed, so the local dir = the uploaded bytes = the hashed list (o 14.2).
fn write_local_dir(conn: &rusqlite::Connection, data_dir: &Path, job: &Job, mask: &str) -> Result<(PathBuf, PalAlignment), AppError> {
    let (input, pal) = align_remote_pal(&job.input_content, mask)
        .map_err(|e| AppError::Backend(format!("cannot submit job {}: {e}", job.id)))?;
    let aux = read_aux_files(conn, &job.id)?;
    Ok((prepare_job_dir(data_dir, &job.id, &input, &aux)?, pal))
}

/// The submit call's values for the local dir's files, and the bytes the upload will carry. More
/// than 1000 files, a name outside the path rule, or anything but a regular file refuses here,
/// before any ssh (`upload_expected`).
fn submit_args(
    root: &str,
    job_id: &str,
    coords: &RemoteCoordinates,
    mask: &str,
    orca_path: &str,
    local_dir: &Path,
) -> Result<(SubmitArgs, u64), AppError> {
    let files = upload_expected(local_dir).map_err(|e| AppError::Backend(format!("cannot upload {local_dir:?}: {e}")))?;
    let mut bytes = 0u64;
    for f in &files {
        bytes += std::fs::metadata(local_dir.join(&f.name))?.len();
    }
    let args = SubmitArgs::new(root, job_id, &coords.socket, mask, orca_path, files)
        .map_err(|e| AppError::Backend(format!("cannot submit job {job_id}: {e}")))?;
    if args.job_dir != coords.job_dir {
        return Err(AppError::Internal(format!("job {job_id}: submit values name {:?}, the coordinates {:?}", args.job_dir, coords.job_dir)));
    }
    Ok((args, bytes))
}

/// Steps 3–6, then the one write they allow (`error_message`; the stamp only for `RefusedKup`).
fn upload_and_submit(db: &DbState, runner: &dyn CommandRunner, plan: &SubmitPlan) -> Result<SubmitAttempt, AppError> {
    let outcome = run_submit_steps(runner, plan);
    record_attempt(db, plan, &outcome)?;
    Ok(SubmitAttempt { outcome, pal: plan.pal })
}

/// Steps 3–4, shared by submit, retry and withdraw: the read-only prepare call; when `bin/`,
/// `tsp/` or any uploaded script is missing, the install call and then the prepare call again —
/// every script must now hash right (the post-condition, rule #9). Which step failed, and why.
fn ready_server(runner: &dyn CommandRunner, host: &str, root: &str, job_dir: &str) -> Result<(), (SubmitStep, String)> {
    let sent = PrepareArgs::new(root, job_dir).map_err(|e| (SubmitStep::Prepare, e.to_string()))?;
    if prepare_call(runner, host, &sent).map_err(|r| (SubmitStep::Prepare, r))?.ready() {
        return Ok(());
    }
    install_call(runner, host, root).map_err(|r| (SubmitStep::Install, r))?;
    match prepare_call(runner, host, &sent) {
        Ok(after) if after.ready() => Ok(()),
        Ok(after) => Err((
            SubmitStep::Install,
            format!(
                "after the install the server is not ready: not hashing right {:?}, bin/ {}, tsp/ {}",
                after.missing(),
                after.bin_exists,
                after.tsp_exists
            ),
        )),
        Err(reason) => Err((SubmitStep::Install, format!("after the install: {reason}"))),
    }
}

/// Steps 3–6 over ssh; no database access. The first failing step ends the attempt.
fn run_submit_steps(runner: &dyn CommandRunner, plan: &SubmitPlan) -> SubmitOutcome {
    let not_claimed = |step, reason| SubmitOutcome::NotClaimed { step, reason };
    let host = plan.coords.host.as_str();

    // 3–4. Prepare; install what is missing; prepare again (o 3.2, 13.1, 14.1).
    if let Err((step, reason)) = ready_server(runner, host, &plan.root, &plan.coords.job_dir) {
        return not_claimed(step, reason);
    }

    // 5. The upload.
    if let Err(reason) = upload(runner, plan) {
        return not_claimed(SubmitStep::Upload, reason);
    }

    // 6. The one atomic submit call (o 3.3).
    let sent = &plan.args;
    let values = match sent.values() {
        Ok(values) => values,
        Err(e) => return not_claimed(SubmitStep::Submit, e.to_string()),
    };
    match call(runner, host, "submit", SUBMIT, &values, SUBMIT_TIMEOUT, |out| parse_submit_reply(out, sent)) {
        Ok(SubmitReply::Enqueued(tsp_id)) => SubmitOutcome::Enqueued { tsp_id },
        Ok(SubmitReply::Refused(reason)) => not_claimed(SubmitStep::Submit, reason),
        Ok(SubmitReply::RefusedKup(evidence)) => SubmitOutcome::KillUserProcesses { evidence },
        Ok(SubmitReply::FailedAfterClaim(reason)) => SubmitOutcome::FailedAfterClaim { reason },
        Err(reason) => SubmitOutcome::Unknown { reason },
    }
}

/// `rsync -a --checksum --mkpath` of the local job dir into the recorded remote job dir.
fn upload(runner: &dyn CommandRunner, plan: &SubmitPlan) -> Result<(), String> {
    let local = plan.local_dir.to_str().ok_or_else(|| format!("local job dir {:?} is not UTF-8", plan.local_dir))?;
    let argv = upload_argv(local, &plan.coords.host, &plan.coords.job_dir).map_err(|e| format!("upload: {e}"))?;
    let out = runner
        .run(RSYNC_PROGRAM, &argv, b"", upload_timeout(plan.upload_bytes))
        .map_err(|e| format!("upload: {e}"))?;
    if out.code != Some(0) {
        return Err(format!("upload: rsync exited with {}: {}", exit_text(&out), tail(&out.stderr)));
    }
    Ok(())
}

/// The one write after the ssh steps: the attempt's failure in `error_message` (NULL after
/// `Enqueued`), guarded on the row still being this remote `queued` job; and the profile's stamp
/// cleared **only** for `KillUserProcesses` (o 13.3).
fn record_attempt(db: &DbState, plan: &SubmitPlan, outcome: &SubmitOutcome) -> Result<(), AppError> {
    let conn = db.lock()?;
    let tx = conn.unchecked_transaction()?;
    if matches!(outcome, SubmitOutcome::KillUserProcesses { .. }) {
        clear_verified(&tx, &plan.profile_id)?;
    }
    tx.execute(
        "UPDATE jobs SET error_message = ?1 WHERE id = ?2 AND status = 'queued' AND remote_job_dir = ?3",
        params![outcome.failure(), plan.job_id, plan.coords.job_dir],
    )?;
    tx.commit()?;
    Ok(())
}

// --- Label and retry ------------------------------------------------------------------------

/// What the read-only label call found for a remote `queued` job, and its label (o item 3.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct LabelReport {
    pub facts: LabelFacts,
    pub label: Label,
}

/// The label of remote job `job` (o item 3.4), by its recorded coordinates. Read-only: nothing on
/// the server or in the database changes.
pub fn label_remote(runner: &dyn CommandRunner, job: &Job) -> Result<LabelReport, AppError> {
    let coords = coordinates(job)?.ok_or_else(|| AppError::Backend(format!("job {} is not a remote job", job.id)))?;
    label_call(runner, &coords).map_err(AppError::Backend)
}

/// A remote `queued` job, its coordinates and its recorded root.
fn queued_remote(job: &Job) -> Result<(RemoteCoordinates, String), AppError> {
    let coords = coordinates(job)?.ok_or_else(|| AppError::Backend(format!("job {} is not a remote job", job.id)))?;
    if job.status != JobStatus::Queued {
        return Err(AppError::Backend(format!(
            "job {} is '{}': only a queued remote job can be retried or withdrawn",
            job.id,
            job.status.as_str()
        )));
    }
    let root = recorded_root(&coords, &job.id)?;
    Ok((coords, root))
}

/// Retry a remote job the label call finds **"not on the server"** (o 3.4) — never "submit
/// interrupted": that call may still be running on the server (o item 8). The local input is
/// derived again from `jobs.input_content` with the profile's **current** mask (o 14.2: a retry after
/// a mask change uploads the new `nprocs`), then steps 3–6 run against the recorded coordinates. The
/// profile (by `backend_id`) must still be a run target with the recorded host and root (they cannot
/// change while the job is live, n 6b; checked again here). The label call comes first: until it
/// says "not on the server", nothing is written — not the row, not the local dir.
pub fn resubmit_remote(db: &DbState, runner: &dyn CommandRunner, data_dir: &Path, job_id: &str) -> Result<SubmitAttempt, AppError> {
    // Read-only checks, then the label call; nothing is written until the server says "not on the
    // server" (verifier LOW-2: a refused retry leaves the local dir as it was).
    let coords = {
        let conn = db.lock()?;
        retry_checks(&conn, job_id)?.1
    };
    let report = label_call(runner, &coords).map_err(AppError::Backend)?;
    if report.label != Label::NotOnServer {
        return Err(AppError::Backend(format!(
            "job {job_id} is {:?} on the server: only a job not on the server can be retried",
            report.label
        )));
    }
    let plan = {
        let conn = db.lock()?;
        let (job, now, root, profile_id, profile) = retry_checks(&conn, job_id)?;
        if now != coords {
            return Err(AppError::Conflict(format!("job {job_id} changed during the retry")));
        }
        let mask = run_target(&profile)?.to_string();
        let (local_dir, pal) = write_local_dir(&conn, data_dir, &job, &mask)?;
        if job.job_dir.as_deref() != local_dir.to_str() {
            return Err(AppError::Internal(format!("job {job_id}: recorded local dir {:?} is not {local_dir:?}", job.job_dir)));
        }
        let (args, upload_bytes) = submit_args(&root, job_id, &coords, &mask, &profile.remote_orca_path, &local_dir)?;
        SubmitPlan { job_id: job_id.to_string(), profile_id, coords, root, local_dir, upload_bytes, args, pal }
    };
    upload_and_submit(db, runner, &plan)
}

/// A retry's read-only preconditions: a remote `queued` job, its profile a run target with the
/// recorded host and root. Returns the job, its coordinates and root, the profile id and profile.
fn retry_checks(
    conn: &rusqlite::Connection,
    job_id: &str,
) -> Result<(Job, RemoteCoordinates, String, String, ServerProfile), AppError> {
    let job = get_job_conn(conn, job_id)?;
    let (coords, root) = queued_remote(&job)?;
    let profile_id = job.backend_id.clone().ok_or_else(|| AppError::Backend(format!("job {job_id} names no server profile")))?;
    let profile = get_profile_conn(conn, &profile_id)?;
    run_target(&profile)?;
    if profile.host != coords.host || profile.remote_scratch_dir != root || slot_socket_path(&root, 0) != coords.socket {
        return Err(AppError::Conflict(format!(
            "job {job_id} was submitted to {}:{root}; server profile {} is now {}:{}",
            coords.host, profile.name, profile.host, profile.remote_scratch_dir
        )));
    }
    Ok((job, coords, root, profile_id, profile))
}

// --- Withdraw ---------------------------------------------------------------------------------

/// The bound on one trampoline call. Derived with margin: ssh's 10 s connect + the `realpath` and
/// `sha256sum` checks at `timeout -k 1 5` (6 s each) + the script's budget (`run.sh` `BUDGET`: cancel
/// 20 s, collect 15 s) + its 1 s kill-after = 43 s at worst.
pub const RUN_TIMEOUT: Duration = Duration::from_secs(60);

/// Run an uploaded job script through the trampoline (o 14.1) and return its rc and streams.
/// `Refused` and `NotInstalled` are errors here: the caller readied the server just before.
fn run_call(runner: &dyn CommandRunner, host: &str, sent: &RunArgs) -> Result<(u8, Vec<u8>, Vec<u8>), String> {
    let what = format!("run {}", sent.name);
    match call(runner, host, &what, RUN, &sent.values(), RUN_TIMEOUT, |out| parse_run_reply(out, sent))? {
        RunReply::Ran { rc, stdout, stderr } => Ok((rc, stdout, stderr)),
        RunReply::NotInstalled => Err(format!("{what}: the script is not installed")),
        RunReply::Refused(reason) => Err(format!("{what}: refused: {reason}")),
    }
}

/// `mkdir -p <job dir>` and o item 1's shapes re-asserted after it, in one call (o 14.1).
fn mkjob_call(runner: &dyn CommandRunner, host: &str, root: &str, job_dir: &str) -> Result<(), String> {
    let sent = MkjobArgs::new(root, job_dir).map_err(|e| format!("mkjob: {e}"))?;
    call(runner, host, "mkjob", MKJOB, &sent.values(), CALL_TIMEOUT, |out| parse_mkjob_reply(out, &sent))
}

/// What a withdraw found and left.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WithdrawReport {
    /// The label before the withdraw: "not on the server" or "submit interrupted".
    pub label: Label,
    /// The classifier's verdict after `.cancelled` was published.
    pub outcome: Outcome,
    /// The row's status afterwards: `cancelled` only when the classifier says `Cancelled`.
    pub status: JobStatus,
}

/// Withdraw a remote `queued` job that the label call finds "not on the server" or "submit
/// interrupted" (ADR-024 o items 2, 14.1). No `verified_at` gate (n 6a). The sequence:
/// 1. the label call (anything else is in the server's hands: remote cancel, unit 5.4);
/// 2. prepare → install if any script is missing → prepare again (the post-condition);
/// 3. one call: `mkdir -p <job dir>`, then `realpath <job> == <job>` and `realpath <job>/.. ==
///    <root>/jobs` re-asserted after the mkdir;
/// 4. `cancel.sh cancel <job> <root>` through the trampoline — it publishes `.cancelled` first, so a
///    late enqueue's wrapper refuses at its step 2; rc 0 required;
/// 5. `collect.sh <job> <recorded socket>` through the trampoline (one retake if `classify` asks),
///    and **`classify` decides**: `Cancelled` (row 4) → the row becomes `cancelled`; anything else
///    (`Completed { late_cancel }`, `Cancelling`, `Failed`, …) → the row stays `queued` for the poller
///    (B2) or unit 5.4, with the verdict in `error_message`. Never a hard-coded `Cancelled`
///    (Decision c). `reenqueue_count` is 0 (the counter is v21's, o item 4).
pub fn withdraw_remote(db: &DbState, runner: &dyn CommandRunner, job_id: &str) -> Result<WithdrawReport, AppError> {
    let (coords, root) = {
        let conn = db.lock()?;
        queued_remote(&get_job_conn(&conn, job_id)?)?
    };
    let fail = |e: String| AppError::Backend(format!("withdraw of job {job_id}: {e}"));
    let report = label_call(runner, &coords).map_err(AppError::Backend)?;
    if !matches!(report.label, Label::NotOnServer | Label::SubmitInterrupted) {
        return Err(AppError::Backend(format!(
            "job {job_id} is already in the server's hands: remote cancel arrives in unit 5.4"
        )));
    }
    ready_server(runner, &coords.host, &root, &coords.job_dir).map_err(|(_, e)| fail(e))?;
    mkjob_call(runner, &coords.host, &root, &coords.job_dir).map_err(fail)?;
    let cancel = RunArgs::new(&root, JobScript::Cancel, vec!["cancel".into(), coords.job_dir.clone(), root.clone()])
        .map_err(|e| fail(e.to_string()))?;
    let (rc, _, stderr) = run_call(runner, &coords.host, &cancel).map_err(fail)?;
    if rc != 0 {
        return Err(fail(format!("cancel.sh exited {rc}: {}", tail(&stderr))));
    }
    let outcome = collect_and_classify(runner, &coords, &root)
        .map_err(|e| fail(format!(".cancelled is published, but {e}")))?;
    let status = record_withdraw(db, job_id, &coords, &outcome)?;
    Ok(WithdrawReport { label: report.label, outcome, status })
}

/// One collect (through the trampoline) + `classify`, with the one retake the classifier may ask
/// for. The snapshot is the trampoline's `stdout` payload, verbatim; a complete snapshot from a
/// collector that exited non-zero is not trusted. The slot socket is the recorded one; the collector
/// adds the `.enqueued` socket itself (o item 4).
fn collect_and_classify(runner: &dyn CommandRunner, coords: &RemoteCoordinates, root: &str) -> Result<Outcome, String> {
    let identity = JobIdentity { job_dir: coords.job_dir.clone(), root: root.to_string() };
    let slots = [coords.socket.clone()];
    let mut argv = vec![coords.job_dir.clone()];
    argv.extend(slots.iter().cloned());
    let sent = RunArgs::new(root, JobScript::Collect, argv).map_err(|e| e.to_string())?;
    for attempt in [Attempt::First, Attempt::Retake] {
        let (rc, wire, stderr) = run_call(runner, &coords.host, &sent)?;
        let snapshot = parse_snapshot(&wire, identity.clone(), attempt, &slots)
            .map_err(|e| format!("collect (rc {rc}): {e}; stderr {:?}", tail(&stderr)))?;
        if rc != 0 {
            return Err(format!("collect exited {rc} after a complete snapshot; not trusted"));
        }
        match classify(&snapshot, 0).map_err(|e| format!("classify: {e}"))? {
            Classification::Decided(outcome) => return Ok(outcome),
            Classification::Retake => continue,
        }
    }
    Err("classify asked for a second retake".to_string())
}

/// The withdraw's one write, guarded on the row still being this remote `queued` job.
fn record_withdraw(db: &DbState, job_id: &str, coords: &RemoteCoordinates, outcome: &Outcome) -> Result<JobStatus, AppError> {
    let conn = db.lock()?;
    if *outcome == Outcome::Cancelled {
        conn.execute(
            "UPDATE jobs SET status = 'cancelled', completed_at = datetime('now'), \
             error_message = 'Withdrawn: the server confirms .cancelled and nothing ran.' \
             WHERE id = ?1 AND status = 'queued' AND remote_job_dir = ?2",
            params![job_id, coords.job_dir],
        )?;
    } else {
        conn.execute(
            "UPDATE jobs SET error_message = ?1 WHERE id = ?2 AND status = 'queued' AND remote_job_dir = ?3",
            params![
                format!("Withdraw published .cancelled; the server reports {outcome:?} (decided by the next status check)"),
                job_id,
                coords.job_dir
            ],
        )?;
    }
    Ok(get_job_conn(&conn, job_id)?.status)
}

#[cfg(test)]
mod tests;
