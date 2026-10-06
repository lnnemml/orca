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
//! execution seam. The commands dispatch through the `enum Backend { Local, Ssh }`
//! static-dispatch selector ([`Backend`]): `submit_job` by the run target chosen next to
//! Submit ([`Backend::for_submit`]), `cancel_job` and the remote job commands by the job's
//! coordinates ([`Backend::for_job`], [`backend_kind`]). `poll_log` / `status` / `fetch_results`
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
/// `offset` is the **new** byte offset *after* this chunk — the caller stores it and passes it
/// back on the next [`ExecutionBackend::poll_log`] call, so polling resumes exactly where it left
/// off. `bytes` are the raw log bytes between the requested offset and this new one, **not**
/// decoded: a chunk boundary can fall inside a multi-byte UTF-8 character (measured, probe 5.3a),
/// so only the consumer, holding a byte carry, decodes complete lines ([`LineAssembler`]).
///
/// `reset` means the log is now **shorter than the requested offset** (replaced or truncated):
/// `offset` is 0, `bytes` is empty, and the consumer drops its carry and whatever it built from
/// the old log, then reads again from 0 (ADR-024 o item 7). Both backends share this rule through
/// [`plan_log_read`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogChunk {
    /// The new byte offset after `bytes` (0 after a reset). Feed this back on the next poll.
    pub offset: u64,
    /// The raw log bytes read from the requested offset up to `offset`.
    pub bytes: Vec<u8>,
    /// The log shrank below the requested offset; start over from 0.
    pub reset: bool,
}

impl LogChunk {
    /// Nothing new (no log yet, or no growth); the offset holds.
    pub fn unchanged(offset: u64) -> Self {
        LogChunk { offset, bytes: Vec::new(), reset: false }
    }

    /// The log is shorter than the offset: start over from 0.
    pub fn reset() -> Self {
        LogChunk { offset: 0, bytes: Vec::new(), reset: true }
    }
}

/// What to read for a poll, given the log's current `size`, the caller's `offset` and the
/// per-poll `cap`. The one rule both backends follow (ADR-024 o item 7).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogRead {
    /// `size < offset`: the log was replaced or truncated.
    Reset,
    /// Read `len` bytes from `start` (`len` may be 0: no growth).
    Range { start: u64, len: u64 },
}

/// Plan one poll: `size < offset` ⇒ [`LogRead::Reset`]; otherwise `min(cap, size − offset)` bytes
/// from `offset`. The remote reply's post-condition checks its byte count against this.
pub fn plan_log_read(size: u64, offset: u64, cap: u64) -> LogRead {
    if size < offset {
        LogRead::Reset
    } else {
        LogRead::Range { start: offset, len: (size - offset).min(cap) }
    }
}

/// Turns a stream of [`LogChunk`]s into complete text lines. Keeps the bytes after the last `\n`
/// as a carry and decodes only complete lines, so a UTF-8 character split across two chunks is
/// decoded whole and no byte is lost or repeated. A `reset` chunk drops the carry.
///
/// A line is split at `\n` and a trailing `\r` is dropped (as `BufRead::lines` does for the local
/// tailing thread). A line that is not valid UTF-8 on its own is decoded lossily — it is ORCA's
/// bytes, not a chunk boundary. The carry is bounded by [`MAX_LINE_CARRY`]: past it, the valid
/// UTF-8 prefix is emitted as a line so a log without newlines cannot grow memory without bound.
#[derive(Debug, Default)]
pub struct LineAssembler {
    carry: Vec<u8>,
}

/// The longest incomplete line [`LineAssembler`] holds before emitting it.
pub const MAX_LINE_CARRY: usize = 1 << 20;

// Not routed live yet: the consumer is the 5.3 Part B log poller (remote `poll_log`); the local
// live log is still the push `job:log` event.
#[allow(dead_code)]
impl LineAssembler {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed one chunk; return the lines it completed, in order.
    pub fn push(&mut self, chunk: &LogChunk) -> Vec<String> {
        if chunk.reset {
            self.carry.clear();
        }
        self.carry.extend_from_slice(&chunk.bytes);
        let mut lines = Vec::new();
        let mut start = 0;
        while let Some(pos) = self.carry[start..].iter().position(|b| *b == b'\n') {
            lines.push(decode_line(&self.carry[start..start + pos]));
            start += pos + 1;
        }
        self.carry.drain(..start);
        if self.carry.len() > MAX_LINE_CARRY {
            let cut = match std::str::from_utf8(&self.carry) {
                Err(e) if e.error_len().is_none() => e.valid_up_to(),
                _ => self.carry.len(),
            };
            lines.push(decode_line(&self.carry[..cut]));
            self.carry.drain(..cut);
        }
        lines
    }

    /// The incomplete last line, if any (e.g. when the job has ended), emptying the carry.
    pub fn take_partial(&mut self) -> Option<String> {
        if self.carry.is_empty() {
            None
        } else {
            let line = decode_line(&self.carry);
            self.carry.clear();
            Some(line)
        }
    }
}

fn decode_line(bytes: &[u8]) -> String {
    let bytes = bytes.strip_suffix(b"\r").unwrap_or(bytes);
    String::from_utf8_lossy(bytes).into_owned()
}

/// Which artifacts [`ExecutionBackend::fetch_results`] retrieves.
///
/// For the local backend this is **degenerate** — every artifact is already on
/// disk in the job dir, nothing is transferred. Over SSH every fetch brings down the
/// one shared artifact list ([`crate::artifacts::ARTIFACT_PATTERNS`], also the curated
/// export's) plus `stderr.log`, the job markers and `.tsp-out/`; the large `.gbw` is
/// the opt-in (it dominates transfer time); cubes are generated on demand. The filter
/// is `remote::sync::download_filter_args` (ADR-024 o items 6 and 11).
//
// Not-yet-routed after Part B: `FetchPolicy` is only meaningful over SSH, and
// `fetch_results` is a no-op locally with no live caller. Routed when `SshBackend`
// lands (ADR-023). Targeted allow (not the removed crate-level one) so unrelated
// dead code still warns.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FetchPolicy {
    /// Pull the large `.gbw` wavefunction back. The shared artifact list always comes
    /// regardless; the `.gbw` is the opt-in cost.
    pub include_gbw: bool,
}

#[allow(dead_code)] // consts land with the SshBackend caller (ADR-023); see FetchPolicy above.
impl FetchPolicy {
    /// The default fetch: the shared artifact list, no `.gbw`.
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
            return Ok(LogChunk::unchanged(offset));
        };
        let path = std::path::Path::new(&dir).join("output.out");
        if !path.exists() {
            // Dir exists but ORCA hasn't opened the log yet.
            return Ok(LogChunk::unchanged(offset));
        }
        Ok(crate::local_backend::read_log_chunk(&path, offset, POLL_LOG_MAX_BYTES)?)
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

// --- Dispatch (ADR-023, ADR-024 o item 1) ----------------------------------------------------

/// Which backend an existing job belongs to. **Keyed on the job's coordinates, never on
/// `backend_id`** (ADR-024 o item 1): `backend_id` only names a profile, and a job whose profile
/// was deleted after it finished keeps its coordinates and stays remote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackendKind {
    /// No coordinates: this machine.
    Local,
    /// On a server, at these recorded coordinates.
    Ssh(crate::ssh_backend::RemoteCoordinates),
}

/// The backend of `job`, from its coordinates. A partial set of coordinates is an error, never
/// "local".
pub fn backend_kind(job: &Job) -> Result<BackendKind, AppError> {
    Ok(match crate::ssh_backend::coordinates(job)? {
        None => BackendKind::Local,
        Some(coords) => BackendKind::Ssh(coords),
    })
}

/// Static dispatch over the backends (ADR-023: an `enum`, not `dyn`). An existing job's backend
/// comes from [`backend_kind`]; a draft's from the run target chosen next to Submit
/// ([`Backend::for_submit`], o item 5).
pub enum Backend {
    Local(LocalBackend),
    Ssh(SshBackend),
}

impl Backend {
    /// The backend of an existing job, by its coordinates.
    pub fn for_job(app: AppHandle, job: &Job) -> Result<Backend, AppError> {
        Ok(match backend_kind(job)? {
            BackendKind::Local => Backend::Local(LocalBackend::new(app)),
            BackendKind::Ssh(_) => Backend::Ssh(SshBackend::new(app, None)),
        })
    }

    /// The backend a draft is submitted to: `None` = this machine, `Some(profile id)` = that
    /// server profile (o item 5).
    pub fn for_submit(app: AppHandle, target: Option<String>) -> Backend {
        match target {
            None => Backend::Local(LocalBackend::new(app)),
            Some(profile_id) => Backend::Ssh(SshBackend::new(app, Some(profile_id))),
        }
    }
}

impl ExecutionBackend for Backend {
    fn submit(&self, job: &Job) -> Result<JobHandle, AppError> {
        match self {
            Backend::Local(b) => b.submit(job),
            Backend::Ssh(b) => b.submit(job),
        }
    }

    fn poll_log(&self, h: &JobHandle, offset: u64) -> Result<LogChunk, AppError> {
        match self {
            Backend::Local(b) => b.poll_log(h, offset),
            Backend::Ssh(b) => b.poll_log(h, offset),
        }
    }

    fn status(&self, h: &JobHandle) -> Result<JobStatus, AppError> {
        match self {
            Backend::Local(b) => b.status(h),
            Backend::Ssh(b) => b.status(h),
        }
    }

    fn fetch_results(&self, h: &JobHandle, policy: FetchPolicy) -> Result<(), AppError> {
        match self {
            Backend::Local(b) => b.fetch_results(h, policy),
            Backend::Ssh(b) => b.fetch_results(h, policy),
        }
    }

    fn cancel(&self, h: &JobHandle) -> Result<(), AppError> {
        match self {
            Backend::Local(b) => b.cancel(h),
            Backend::Ssh(b) => b.cancel(h),
        }
    }
}

/// The remote backend (ADR-023, ADR-024): a thin `AppHandle` wrapper over the Tauri-free core in
/// [`crate::ssh_backend`], like [`LocalBackend`] over `local_backend`. It runs the real ssh/rsync
/// ([`SystemRunner`](crate::remote::ssh::SystemRunner)). `target` is the profile a draft is
/// submitted to; an existing job's calls use its recorded coordinates instead (n 6b).
///
/// Every method but `status` and `cancel` **blocks on ssh** (each call up to 60 s): the commands
/// run them in `spawn_blocking`, holding the job's [`InFlightGuard`](crate::in_flight::InFlightGuard).
pub struct SshBackend {
    app: AppHandle,
    target: Option<String>,
}

impl SshBackend {
    pub fn new(app: AppHandle, target: Option<String>) -> Self {
        SshBackend { app, target }
    }

    /// Submit draft `job_id` to the target profile and return the whole attempt — the outcome and
    /// the `%pal` alignment the trait's `submit` has no room for (`ssh_backend::submit_remote`).
    pub fn submit_attempt(&self, job_id: &str) -> Result<crate::ssh_backend::SubmitAttempt, AppError> {
        let profile_id = self
            .target
            .as_deref()
            .ok_or_else(|| AppError::Backend("no server profile was chosen to run on".into()))?;
        let db = self.app.state::<DbState>();
        let runner = self.app.state::<crate::local_backend::JobRunner>();
        crate::ssh_backend::submit_remote(&db, &crate::remote::ssh::SystemRunner, runner.data_dir(), job_id, profile_id)
    }

    /// Retry a job the label call finds "not on the server" (`ssh_backend::resubmit_remote`).
    pub fn retry(&self, job_id: &str) -> Result<crate::ssh_backend::SubmitAttempt, AppError> {
        let db = self.app.state::<DbState>();
        let runner = self.app.state::<crate::local_backend::JobRunner>();
        crate::ssh_backend::resubmit_remote(&db, &crate::remote::ssh::SystemRunner, runner.data_dir(), job_id)
    }

    /// Withdraw a job that is not (or not yet) in the server's hands (`ssh_backend::withdraw_remote`).
    pub fn withdraw(&self, job_id: &str) -> Result<crate::ssh_backend::WithdrawReport, AppError> {
        let db = self.app.state::<DbState>();
        crate::ssh_backend::withdraw_remote(&db, &crate::remote::ssh::SystemRunner, job_id)
    }

    /// The read-only label call (`ssh_backend::label_remote`): nothing changes on either side.
    pub fn label(&self, job: &Job) -> Result<crate::ssh_backend::LabelReport, AppError> {
        crate::ssh_backend::label_remote(&crate::remote::ssh::SystemRunner, job)
    }
}

impl ExecutionBackend for SshBackend {
    /// Blocks for the whole sequence (up to the submit call's 60 s after the upload): call it off
    /// the main thread. Only `Enqueued` is an `Ok`; every other outcome is an `Err` carrying it,
    /// while the row stays `queued` with its coordinates for the label call to resolve.
    fn submit(&self, job: &Job) -> Result<JobHandle, AppError> {
        let attempt = self.submit_attempt(&job.id)?;
        // The trait has no room for the %pal notice (`attempt.pal`); `submit_job` calls
        // `submit_attempt` to show it.
        match attempt.outcome.failure() {
            None => Ok(JobHandle(job.id.clone())),
            Some(failure) => Err(AppError::Backend(failure)),
        }
    }

    fn poll_log(&self, _h: &JobHandle, _offset: u64) -> Result<LogChunk, AppError> {
        Err(AppError::Backend("the remote log arrives with the poller (unit 5.3 B2)".into()))
    }

    fn status(&self, h: &JobHandle) -> Result<JobStatus, AppError> {
        let db = self.app.state::<DbState>();
        let conn = db.lock()?;
        Ok(get_job_conn(&conn, &h.0)?.status)
    }

    fn fetch_results(&self, _h: &JobHandle, _policy: FetchPolicy) -> Result<(), AppError> {
        Err(AppError::Backend("fetching remote results arrives with the poller (unit 5.3 B2)".into()))
    }

    /// Refused for a live remote job (o item 2) — never a local `Cancelled`.
    fn cancel(&self, h: &JobHandle) -> Result<(), AppError> {
        let db = self.app.state::<DbState>();
        let job = get_job_conn(&*db.lock()?, &h.0)?;
        crate::ssh_backend::cancel_remote(&job)
    }
}

/// Bytes read per `poll_log` call, by either backend. Bounds memory per poll (rule #5); the pull
/// loop advances the offset so a large log is still fully delivered across polls. The remote
/// reply carries this many bytes plus a little framing, so it must stay well under the ssh
/// runner's output cap (`remote::ssh::MAX_OUTPUT_BYTES`, 1 MiB) — `remote::poll` asserts it.
pub const POLL_LOG_MAX_BYTES: u64 = 256 * 1024;

#[cfg(test)]
mod tests {
    use super::*;

    /// A job with these coordinates and this `backend_id`, through the real `Job::from_row`.
    fn job(coords: Option<(&str, &str, &str)>, backend_id: Option<&str>) -> Job {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE jobs (id TEXT, title TEXT, input_content TEXT, status TEXT, job_dir TEXT, energy REAL,
                 wall_time REAL, error_message TEXT, created_at TEXT, started_at TEXT, completed_at TEXT,
                 scene_json TEXT, scene_log_json TEXT, pathway_id TEXT, group_id TEXT, backend_id TEXT,
                 remote_host TEXT, remote_job_dir TEXT, remote_socket TEXT);",
        )
        .unwrap();
        let (h, d, s) = coords.map_or((None, None, None), |(h, d, s)| (Some(h), Some(d), Some(s)));
        conn.execute(
            "INSERT INTO jobs (id, title, input_content, status, created_at, backend_id, remote_host, remote_job_dir, remote_socket)
             VALUES ('j1', 't', '! HF', 'queued', 'now', ?1, ?2, ?3, ?4)",
            rusqlite::params![backend_id, h, d, s],
        )
        .unwrap();
        conn.query_row(&format!("SELECT {} FROM jobs", Job::COLUMNS), [], Job::from_row).unwrap()
    }

    /// NEGATIVE CONTROL target (c): dispatch keys on the coordinates, never on `backend_id`
    /// (ADR-024 o item 1). Make `backend_kind` look at `job.backend_id` and the first two rows go
    /// red: a remote job whose profile was deleted would run locally, and a job naming a profile
    /// but never submitted would be treated as on a server.
    #[test]
    fn dispatch_keys_on_the_coordinates_never_on_backend_id() {
        let coords = Some(("uni", "/r/jobs/j1", "/r/tsp/slot0.sock"));
        let remote = BackendKind::Ssh(crate::ssh_backend::RemoteCoordinates {
            host: "uni".into(),
            job_dir: "/r/jobs/j1".into(),
            socket: "/r/tsp/slot0.sock".into(),
        });
        for (case, job, want) in [
            ("coordinates, profile deleted", job(coords, None), remote.clone()),
            ("a profile named, never submitted", job(None, Some("p1")), BackendKind::Local),
            ("coordinates and a profile", job(coords, Some("p1")), remote),
            ("neither", job(None, None), BackendKind::Local),
        ] {
            assert_eq!(backend_kind(&job).unwrap(), want, "{case}");
        }
        let mut partial = job(coords, Some("p1"));
        partial.remote_job_dir = None;
        assert!(backend_kind(&partial).is_err(), "partial coordinates are an error, never local");
    }

    fn chunk(bytes: &[u8]) -> LogChunk {
        LogChunk { offset: 0, bytes: bytes.to_vec(), reset: false }
    }

    /// Feed `chunks` to an assembler and collect every line, the way the poller will.
    fn assemble(chunks: &[&[u8]]) -> Vec<String> {
        let mut a = LineAssembler::new();
        chunks.iter().flat_map(|c| a.push(&chunk(c))).collect()
    }

    /// The same, but decoding each chunk on its own before splitting lines — the lossy shape
    /// `LogChunk` had before (a `String` per chunk). Used only by the negative control.
    fn assemble_lossy_per_chunk(chunks: &[&[u8]]) -> Vec<String> {
        let text: String = chunks.iter().map(|c| String::from_utf8_lossy(c).into_owned()).collect();
        text.lines().map(String::from).collect()
    }

    // "Å" is C3 85 in UTF-8; the chunk boundary falls between the two bytes.
    const SPLIT_CHAR: &[&[u8]] = &[b"E(SCF) \xC3", b"\x85 converged\n"];

    #[test]
    fn a_character_split_across_two_chunks_is_decoded_whole() {
        assert_eq!(assemble(SPLIT_CHAR), ["E(SCF) Å converged"]);
    }

    /// NEGATIVE CONTROL: decoding each chunk on its own turns the split `Å` into two U+FFFD.
    #[test]
    #[should_panic(expected = "assertion")]
    fn lossy_per_chunk_decoding_breaks_the_split_character() {
        assert_eq!(assemble_lossy_per_chunk(SPLIT_CHAR), ["E(SCF) Å converged"]);
    }

    #[test]
    fn a_line_split_mid_way_is_emitted_once_complete() {
        let mut a = LineAssembler::new();
        assert!(a.push(&chunk(b"ITER  ")).is_empty());
        assert_eq!(a.push(&chunk(b" 1\nITER")), ["ITER   1"]);
        assert_eq!(a.push(&chunk(b"  2\r\n\n")), ["ITER  2", ""]);
        assert_eq!(a.take_partial(), None);
    }

    #[test]
    fn reset_drops_the_carry() {
        let mut a = LineAssembler::new();
        assert!(a.push(&chunk(b"old partial")).is_empty());
        assert!(a.push(&LogChunk::reset()).is_empty());
        assert_eq!(a.push(&chunk(b"new\n")), ["new"], "nothing of the old log is spliced on");
    }

    /// Any chunking of a log reproduces its lines exactly (probe 5.3a rebuilt a file with Å/ü/→
    /// over 66 polls; here every cap from 1 to 9 bytes).
    #[test]
    fn every_chunk_size_rebuilds_the_lines_exactly() {
        let log: String = (0..40).map(|i| format!("L{i:04} Å ü → partial rest-of-line\n")).collect();
        let want: Vec<String> = log.lines().map(String::from).collect();
        for cap in 1..=9 {
            let pieces: Vec<&[u8]> = log.as_bytes().chunks(cap).collect();
            assert_eq!(assemble(&pieces), want, "cap {cap}");
        }
    }

    #[test]
    fn take_partial_returns_an_unterminated_last_line() {
        let mut a = LineAssembler::new();
        assert_eq!(a.push(&chunk(b"done\nTOTAL RUN TIME")), ["done"]);
        assert_eq!(a.take_partial().as_deref(), Some("TOTAL RUN TIME"));
        assert_eq!(a.take_partial(), None);
    }

    #[test]
    fn the_carry_is_bounded_without_splitting_a_character() {
        let mut a = LineAssembler::new();
        let mut long = vec![b'x'; MAX_LINE_CARRY];
        long.push(0xC3); // first byte of `Å`, its second byte still to come
        let lines = a.push(&chunk(&long));
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].len(), MAX_LINE_CARRY, "the incomplete character stays in the carry");
        assert_eq!(a.push(&chunk(b"\x85\n")), ["Å"]);
    }

    #[test]
    fn plan_log_read_follows_size_and_offset() {
        assert_eq!(plan_log_read(10, 0, 4), LogRead::Range { start: 0, len: 4 });
        assert_eq!(plan_log_read(10, 8, 4), LogRead::Range { start: 8, len: 2 });
        assert_eq!(plan_log_read(10, 10, 4), LogRead::Range { start: 10, len: 0 });
        assert_eq!(plan_log_read(9, 10, 4), LogRead::Reset);
        assert_eq!(plan_log_read(0, 0, 4), LogRead::Range { start: 0, len: 0 });
    }
}
