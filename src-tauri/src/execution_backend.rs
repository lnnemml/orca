//! The `ExecutionBackend` trait (ADR-003) and its local implementation.
//!
//! ORCA jobs run **locally today, remotely soon** (SSH — ADR-005/ADR-023),
//! possibly under SLURM later. Everything above the backend must not care where a
//! job ran, so all execution flows through one trait. This module is the trait's
//! home; the running machinery it drives still lives in [`crate::local_backend`]
//! (the queue, the process tree, cancellation) — each trait method **delegates**
//! there rather than reimplementing it.
//!
//! **Scope note (unit 5.0 Part B).** The trait exists; `LocalBackend` implements
//! it; the Tauri command layer now **dispatches through it** — `submit_job` and
//! `cancel_job` (`commands::jobs`) construct a `LocalBackend` from their
//! `AppHandle` and call `submit` / `cancel` on the trait, so the trait is the real
//! execution seam. Dispatch is a single concrete type today — the
//! `enum Backend { Local, Ssh }` static-dispatch selector and the
//! `jobs.backend_id` column are deferred to the `SshBackend` unit (ADR-023), where
//! the trait's still-maturing `poll_log(offset)` / `fetch_results(policy)` shapes
//! are forced by a second implementation. `poll_log` / `status` / `fetch_results`
//! are wired-but-quiet: the live UI still uses the **push** `job:log` event, so the
//! pull path is exercised by tests until the push→pull flip (a later unit).
//!
//! **Tauri-free signatures.** No method takes an `AppHandle`: the trait must be
//! implementable by `SshBackend` (which has no `AppHandle` to reach app state).
//! The `AppHandle` a local run needs is held **inside** [`LocalBackend`], not
//! threaded through the signatures.

use tauri::{AppHandle, Manager};

use crate::commands::jobs::get_job_conn;
use crate::commands::settings::DbState;
use crate::error::AppError;
use crate::models::job::{Job, JobStatus};

/// A backend-opaque reference to a submitted job. Wraps the job id — the id is the
/// stable key both backends key their state on (the DB row locally, the remote
/// scratch dir over SSH). Returned by [`ExecutionBackend::submit`] and passed back
/// to every other method so a caller never re-derives the handle from a bare string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobHandle(pub String);

/// One incremental slice of a job's log, read from a byte offset.
///
/// `offset` is the **new** byte offset *after* this chunk — the caller stores it
/// and passes it back on the next [`ExecutionBackend::poll_log`] call, so polling
/// resumes exactly where it left off (and survives an app restart once the offset
/// is persisted — ADR-003). `data` is the bytes between the requested offset and
/// this new one, lossily decoded as UTF-8.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogChunk {
    /// The new byte offset after `data`. Feed this back on the next poll.
    pub offset: u64,
    /// The log bytes read from the requested offset up to `offset`.
    pub data: String,
}

/// Which artifacts [`ExecutionBackend::fetch_results`] retrieves.
///
/// For the local backend this is **degenerate** — every artifact is already on
/// disk in the job dir, nothing is transferred. It exists now because it shapes
/// the `SshBackend` (ADR-023): output/xyz/hess come back on every fetch, the large
/// `.gbw` is opt-in (it dominates transfer time), cubes are generated on demand.
/// Designed in here, exercised there.
//
// Not-yet-routed after Part B: `FetchPolicy` is only meaningful over SSH, and
// `fetch_results` is a no-op locally with no live caller. Routed when `SshBackend`
// lands (ADR-023). Targeted allow (not the removed crate-level one) so unrelated
// dead code still warns.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FetchPolicy {
    /// Pull the large `.gbw` wavefunction back. Always the small artifacts
    /// (output/xyz/hess) come regardless; the `.gbw` is the opt-in cost.
    pub include_gbw: bool,
}

#[allow(dead_code)] // consts land with the SshBackend caller (ADR-023); see FetchPolicy above.
impl FetchPolicy {
    /// The default fetch: small artifacts only, no `.gbw`.
    pub const SMALL_ONLY: FetchPolicy = FetchPolicy { include_gbw: false };
    /// Everything, including the `.gbw` wavefunction.
    pub const WITH_GBW: FetchPolicy = FetchPolicy { include_gbw: true };
}

/// The execution abstraction (ADR-003). Every calculation runs through an
/// implementation of this trait; code above it never knows where a job ran.
///
/// Signatures are **Tauri-type-free** on purpose (see the module doc): `SshBackend`
/// must be able to implement them without an `AppHandle`. All methods return
/// `Result<_, AppError>`.
///
/// **Routing status after Part B.** `submit` / `cancel` are routed live —
/// `commands::jobs::{submit_job, cancel_job}` dispatch through them. `poll_log` /
/// `status` / `fetch_results` are implemented and tested but have no live caller
/// yet, so each carries a targeted `#[allow(dead_code)]` naming where it gets
/// routed (the push→pull flip and `SshBackend`), rather than the removed
/// crate-level allow — a genuinely-unrouted item stays visible, an *accidentally*
/// dead one still warns.
pub trait ExecutionBackend {
    /// Submit a job for execution and return its handle. Locally: enqueue on the
    /// single-slot SQLite queue (domain rule #4) and try to start it.
    fn submit(&self, job: &Job) -> Result<JobHandle, AppError>;

    /// Read the job's log forward from `offset`, returning the new bytes plus the
    /// updated offset. Pull-based (ADR-003) so the same interface serves a local
    /// file and a remote `tail -c +<offset>` over SSH. Never loads the whole log
    /// (domain rule #5): the read is bounded and seeks to `offset`.
    //
    // Not routed live yet: the UI log is still the push `job:log` event; this pull
    // path is exercised by tests until the push→pull flip (a later unit).
    #[allow(dead_code)]
    fn poll_log(&self, h: &JobHandle, offset: u64) -> Result<LogChunk, AppError>;

    /// The job's current lifecycle state.
    //
    // Not routed live yet: `get_job`/`list_jobs` return the whole `Job` row via
    // `DbState` (no `AppHandle`), so nothing calls the single-status accessor until
    // `SshBackend` needs a backend-uniform status probe.
    #[allow(dead_code)]
    fn status(&self, h: &JobHandle) -> Result<JobStatus, AppError>;

    /// Retrieve the job's result artifacts per `policy`. Locally this is a no-op
    /// (the artifacts are already on disk); remotely it `rsync`s them back.
    //
    // Not routed live yet: degenerate locally; the live caller is `SshBackend` (ADR-023).
    #[allow(dead_code)]
    fn fetch_results(&self, h: &JobHandle, policy: FetchPolicy) -> Result<(), AppError>;

    /// Cancel a queued or running job.
    fn cancel(&self, h: &JobHandle) -> Result<(), AppError>;
}

/// The local execution backend: runs ORCA on this machine.
///
/// Holds an [`AppHandle`] because the local run machinery reaches app-managed state
/// (the `JobRunner` slot, the `DbState` connection) through it — the least-churn way
/// to route the trait to the existing `local_backend` free functions without
/// changing their signatures. `SshBackend` will instead hold a `ServerProfile`
/// (ADR-023); neither is visible in the trait's signatures.
pub struct LocalBackend {
    app: AppHandle,
}

impl LocalBackend {
    /// Wrap an `AppHandle` into a local backend. The handle must already have the
    /// `JobRunner` and `DbState` managed (it does at app setup time).
    pub fn new(app: AppHandle) -> Self {
        LocalBackend { app }
    }
}

impl ExecutionBackend for LocalBackend {
    fn submit(&self, job: &Job) -> Result<JobHandle, AppError> {
        crate::local_backend::submit(&self.app, &job.id)?;
        Ok(JobHandle(job.id.clone()))
    }

    fn poll_log(&self, h: &JobHandle, offset: u64) -> Result<LogChunk, AppError> {
        // The log file is `output.out` inside the job's isolated dir (rule #3).
        // Read forward from `offset`, capped, never whole (rule #5).
        let job_dir = {
            let db = self.app.state::<DbState>();
            let conn = db.lock()?;
            get_job_conn(&conn, &h.0)?.job_dir
        };
        let Some(dir) = job_dir else {
            // No dir yet (job still draft/queued) → nothing to read; offset holds.
            return Ok(LogChunk {
                offset,
                data: String::new(),
            });
        };
        let path = std::path::Path::new(&dir).join("output.out");
        if !path.exists() {
            // Dir exists but ORCA hasn't opened the log yet.
            return Ok(LogChunk {
                offset,
                data: String::new(),
            });
        }
        let (data, new_offset) =
            crate::local_backend::read_log_chunk(&path, offset, POLL_LOG_MAX_BYTES)?;
        Ok(LogChunk {
            offset: new_offset,
            data,
        })
    }

    fn status(&self, h: &JobHandle) -> Result<JobStatus, AppError> {
        let db = self.app.state::<DbState>();
        let conn = db.lock()?;
        Ok(get_job_conn(&conn, &h.0)?.status)
    }

    fn fetch_results(&self, _h: &JobHandle, _policy: FetchPolicy) -> Result<(), AppError> {
        // Degenerate for local: the artifacts are already on disk in the job dir,
        // and the live finish path (`parse_results_after_completion`) already
        // parsed them. Nothing to transfer, so this is an idempotent no-op —
        // `FetchPolicy` only bites over SSH (ADR-023). It exists to satisfy the
        // trait and to keep the caller uniform across backends in Part B.
        Ok(())
    }

    fn cancel(&self, h: &JobHandle) -> Result<(), AppError> {
        crate::local_backend::cancel(&self.app, &h.0)
    }
}

/// Bytes read per `poll_log` call. Bounds memory per poll (rule #5); the pull loop
/// advances the offset so a large log is still fully delivered across polls.
const POLL_LOG_MAX_BYTES: u64 = 256 * 1024;
