//! `wait_for_jobs` (ADR-027 decision 8; M0 amendment, items 5, 6 and 10; M3 amendment, item 1; unit
//! M3b; the edges decided by Anton, 2026-11-21): one bounded long-poll over job statuses. It is a
//! read: it reads **any** job (not only Claude's), writes nothing, and has no audit row (decision 9).
//!
//! **The loop** ([`wait_for_jobs`]), in this order on every tick:
//! 1. **`closed`**: if the connection's flag is set, the wait ends ([`WaitEnd::ConnectionClosed`]:
//!    the client has gone, so the result goes nowhere). An abandoned wait would otherwise keep one
//!    of the connection's 4 permits for up to 600 s (M0 amendment, items 5 and 6).
//! 2. **read**: the named jobs' current statuses, through the [`StatusReader`] seam.
//! 3. **compare** with `known`: any job whose status differs — or that no longer exists (`"gone"`) —
//!    ends the wait with the changes. The first read happens before any sleep, so a change between
//!    the caller's last read and this call is answered **at once**.
//! 4. **sleep** one tick ([`WAIT_TICK`], 2 s; never past the deadline). When the deadline has
//!    passed, the wait answers `timed_out` instead.
//!
//! **No lock is held while it sleeps, by construction.** The core never sees `DbState` or a
//! `Connection`: it gets a [`StatusReader`] whose answer is owned data ([`StatusReading`]) and whose
//! future is `Send + 'static`, and the reader itself is `Send + Sync`. A `std::sync::MutexGuard` is
//! neither `Send` nor (over a `Connection`) `Sync`, so neither the reader nor its future can keep the
//! `DbState` guard: such code does not compile. The app's reader ([`DbStatusReader`]) locks
//! `DbState` for one query on the blocking pool, and the guard drops when that closure returns.
//!
//! **Arguments** ([`check_args`], refused before anything is read): `known` not empty, at most
//! [`KNOWN_MAX`] entries, each id not empty and at most [`JOB_ID_MAX_BYTES`], each status one of the
//! job statuses (`draft`, `queued`, `running`, `cancelling`, `completed`, `parsed`, `failed`,
//! `cancelled`, `lost`); `"gone"` is an answer, never an argument. `timeout_s` omitted is
//! [`WAIT_MAX_S`]; above it, it is clamped to it and the result says so (`timeout_clamped`,
//! `effective_timeout_s`); `0` is a one-shot check.

use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use rusqlite::{params_from_iter, Connection};
use serde::{Deserialize, Serialize};

use super::{within_cap, ToolError, RESULT_CAP_BYTES};
use crate::error::AppError;
use crate::mcp::server::read_tools::on_blocking_pool;
use crate::mcp::server::DbSource;
use crate::models::job::JobStatus;

/// The longest wait, in seconds (decision 8's cap), and the default.
pub const WAIT_MAX_S: u64 = 600;
/// The re-read period (decision 8).
pub const WAIT_TICK: Duration = Duration::from_secs(2);
/// The most jobs one wait may name.
pub const KNOWN_MAX: usize = 50;
/// The longest job id, in bytes (a v4 UUID is 36; the same bound as `create_job`'s ids).
pub const JOB_ID_MAX_BYTES: usize = 64;
/// The current status of a job that no longer exists.
pub const GONE: &str = "gone";

/// A future the seams return: `Send` and `'static`, so it can hold no borrow and no
/// `std::sync::MutexGuard`.
pub type SeamFuture<T> = Pin<Box<dyn Future<Output = T> + Send + 'static>>;

/// The tool's arguments, as the client sends them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WaitArgs {
    /// Each job id with the status the caller last saw.
    pub known: BTreeMap<String, String>,
    /// The longest wait, in seconds; omitted = [`WAIT_MAX_S`], above it = clamped to it.
    pub timeout_s: Option<u64>,
}

/// The checked arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WaitRequest {
    pub known: BTreeMap<String, JobStatus>,
    pub effective_timeout_s: u64,
    pub timeout_clamped: bool,
}

/// One read's answer: the current status of every named job that exists. A job absent from the map
/// is gone. Owned data: nothing in it can borrow the database.
pub type StatusReading = BTreeMap<String, JobStatus>;

/// The read seam. The app's is [`DbStatusReader`].
pub trait StatusReader: Send + Sync {
    /// The current statuses of `ids` (each id at most once in the answer).
    fn read(&self, ids: Vec<String>) -> SeamFuture<Result<StatusReading, ToolError>>;
}

/// The clock seam: the time since an arbitrary origin, and a sleep. The app's is [`TokioClock`].
pub trait WaitClock: Send + Sync {
    fn now(&self) -> Duration;
    fn sleep(&self, d: Duration) -> SeamFuture<()>;
}

/// One job whose status differs from the caller's.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StatusChange {
    pub job_id: String,
    /// The status the caller passed in `known`.
    pub known_status: String,
    /// The status now, or `"gone"` for a job that no longer exists.
    pub current_status: String,
}

/// The answer the client receives.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WaitResult {
    /// Every named job whose status differs, in job-id order; empty when the wait timed out.
    pub changed: Vec<StatusChange>,
    /// True when no status differed before the deadline.
    pub timed_out: bool,
    /// The timeout actually used, in seconds (at most [`WAIT_MAX_S`]).
    pub effective_timeout_s: u64,
    /// True when the caller's `timeout_s` was above [`WAIT_MAX_S`] and was clamped.
    pub timeout_clamped: bool,
    /// How many jobs the wait watched.
    pub watched_count: usize,
    /// How long the wait took, in whole seconds on its clock.
    pub waited_s: u64,
}

/// How a wait ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WaitEnd {
    /// A status differed, or the deadline passed: the result for the client.
    Answered(WaitResult),
    /// The connection's `closed` flag was set: the client has gone, and there is no one to answer.
    ConnectionClosed,
}

/// Step 0: the bounds and the parse. Every refusal names the argument, never echoes it whole.
pub fn check_args(args: &WaitArgs) -> Result<WaitRequest, ToolError> {
    if args.known.is_empty() {
        return Err(ToolError::refused("`known` is empty: name at least one job with the status you last saw"));
    }
    if args.known.len() > KNOWN_MAX {
        return Err(ToolError::refused(format!("`known` names {} jobs; at most {KNOWN_MAX}", args.known.len())));
    }
    let mut known = BTreeMap::new();
    for (id, status) in &args.known {
        if id.is_empty() {
            return Err(ToolError::refused("`known` has an empty job id"));
        }
        if id.len() > JOB_ID_MAX_BYTES {
            return Err(ToolError::refused(format!("a job id in `known` is {} bytes; at most {JOB_ID_MAX_BYTES}", id.len())));
        }
        let parsed = JobStatus::from_db(status).map_err(|_| {
            ToolError::refused(format!(
                "`known` gives job {} the status {}, which is not a job status",
                super::echo(id),
                super::echo(status)
            ))
        })?;
        known.insert(id.clone(), parsed);
    }
    let asked = args.timeout_s.unwrap_or(WAIT_MAX_S);
    Ok(WaitRequest { known, effective_timeout_s: asked.min(WAIT_MAX_S), timeout_clamped: asked > WAIT_MAX_S })
}

/// Step 3: every job of `known` whose status in `reading` differs, or that is absent (gone).
pub fn changes(known: &BTreeMap<String, JobStatus>, reading: &StatusReading) -> Vec<StatusChange> {
    known
        .iter()
        .filter_map(|(id, was)| {
            let current = match reading.get(id) {
                None => GONE,
                Some(now) if now == was => return None,
                Some(now) => now.as_str(),
            };
            Some(StatusChange { job_id: id.clone(), known_status: was.as_str().into(), current_status: current.into() })
        })
        .collect()
}

/// `wait_for_jobs` (the module docs' loop). `tick` is [`WAIT_TICK`] in the app.
pub async fn wait_for_jobs(
    args: &WaitArgs,
    closed: &AtomicBool,
    reader: &dyn StatusReader,
    clock: &dyn WaitClock,
    tick: Duration,
) -> Result<WaitEnd, ToolError> {
    let request = check_args(args)?;
    let ids: Vec<String> = request.known.keys().cloned().collect();
    let start = clock.now();
    let deadline = start + Duration::from_secs(request.effective_timeout_s);
    loop {
        if closed.load(Ordering::SeqCst) {
            return Ok(WaitEnd::ConnectionClosed);
        }
        let reading = reader.read(ids.clone()).await?;
        let changed = changes(&request.known, &reading);
        let now = clock.now();
        if !changed.is_empty() || now >= deadline {
            let result = WaitResult {
                timed_out: changed.is_empty(),
                changed,
                effective_timeout_s: request.effective_timeout_s,
                timeout_clamped: request.timeout_clamped,
                watched_count: request.known.len(),
                waited_s: now.saturating_sub(start).as_secs(),
            };
            return Ok(WaitEnd::Answered(within_cap(result, RESULT_CAP_BYTES)?));
        }
        clock.sleep(tick.min(deadline - now)).await;
    }
}

/// The statuses of `ids` that exist, in one query. A stored status outside the known set is an error
/// (rule #9), never skipped or read as gone.
pub fn read_statuses_conn(conn: &Connection, ids: &[String]) -> Result<StatusReading, AppError> {
    if ids.is_empty() {
        return Ok(StatusReading::new());
    }
    let marks = vec!["?"; ids.len()].join(", ");
    let mut stmt = conn.prepare(&format!("SELECT id, status FROM jobs WHERE id IN ({marks})"))?;
    let rows = stmt
        .query_map(params_from_iter(ids.iter()), |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    rows.into_iter().map(|(id, status)| Ok((id, JobStatus::from_db(&status)?))).collect()
}

/// The app's reader: one query per tick on the blocking pool, with `DbState` locked inside that
/// closure only (the guard drops when it returns, before the future resolves).
pub struct DbStatusReader(pub Arc<dyn DbSource>);

impl StatusReader for DbStatusReader {
    fn read(&self, ids: Vec<String>) -> SeamFuture<Result<StatusReading, ToolError>> {
        let db = self.0.clone();
        Box::pin(on_blocking_pool(move || {
            let conn = db.db().lock().map_err(ToolError::App)?;
            read_statuses_conn(&conn, &ids).map_err(ToolError::App)
        }))
    }
}

/// The app's clock: `Instant` since the clock was made, and `tokio`'s sleep.
pub struct TokioClock(Instant);

impl TokioClock {
    pub fn new() -> TokioClock {
        TokioClock(Instant::now())
    }
}

impl Default for TokioClock {
    fn default() -> Self {
        Self::new()
    }
}

impl WaitClock for TokioClock {
    fn now(&self) -> Duration {
        self.0.elapsed()
    }

    fn sleep(&self, d: Duration) -> SeamFuture<()> {
        Box::pin(tokio::time::sleep(d))
    }
}
