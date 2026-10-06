//! The remote poller's core (ADR-024 o item 4, o15, o16; unit 5.3 B2 Part A): one loop over every
//! non-terminal remote job, started at launch. **`AppHandle`-free**: every step takes the database,
//! a [`CommandRunner`] and a [`PollerSink`] for its events, so the tests drive it with a fake runner
//! and a recording sink; Part B adds the loop thread, the `AppHandle` sink and the commands.
//!
//! Two steps per job, sequenced per job by the loop, never concurrent ([`plan`]):
//!
//! **The status step** ([`Poller::status_step`]), every 15 s for every job, under the job's
//! in-flight guard — a busy job is **skipped** this tick (o15.3):
//! 1. a `queued` row gets the read-only **label call** first (o 3.4): "not on the server" and
//!    "submit interrupted" end the step (no collect on a missing dir); a `running` row has already
//!    started and goes straight to
//! 2. **collect + `classify`** with the recorded socket (the collector adds the `.enqueued` one);
//! 3. a classifier **`Cancelled`** (row 4) writes the row terminal `cancelled` as a withdraw does
//!    (o17), with no download;
//! 4. any other **non-fetching outcome** is shown — `error_message` says what the server reports, and
//!    "handled in unit 5.4" where it needs an action — and the row stays `queued`/`running`; the
//!    classifier's `Running` moves a `queued` row to `running` (o item 1: a remote job keeps
//!    `queued`/`running` in `jobs.status`);
//! 5. a **fetching outcome** (`Completed`, `Failed{NonZeroExit | BadExitCode |
//!    NoNormalTermination}`) sets the in-memory fetching flag (the live log stops, o16.6), then
//!    **fetches**: rsync down with the shared filter, then the server's listing must equal the local
//!    copy, hash for hash (`ssh_backend::fetch_remote`). Only then does **`detect_completion` on the
//!    downloaded files** decide the status (rule #6) — never the classifier's verdict, never a
//!    hard-coded `Completed` — and one locked transaction writes the terminal status, the energy and
//!    wall time, and the parsed results, in the local finish path's order. Then the live log is
//!    drained from the local copy (o16.6), and `job:status` is emitted last.
//! 6. A failed fetch changes no status: a strike is counted and shown; after
//!    [`MAX_FETCH_STRIKES`] in a row, automatic fetches stop until [`Poller::retry_fetch`].
//!
//! **The log step** ([`log_step`]), every 2 s while the job is watched (o16): no in-flight guard
//! (o16.5), no database; see [`live_log`].
//!
//! **The database lock is never held across an ssh or rsync call**: each write re-reads the row
//! under the lock and writes only if it is still this non-terminal remote job (same recorded job
//! dir), so a concurrent change wins and the step reports it.

pub mod live_log;
pub mod plan;

use std::io;
use std::path::{Path, PathBuf};

use rusqlite::params;

use crate::commands::jobs::{finalize_job_conn, get_job_conn, set_job_results_conn};
use crate::commands::remote_jobs::status_to_emit;
use crate::commands::settings::DbState;
use crate::convergence::ConvergenceEvent;
use crate::error::AppError;
use crate::execution_backend::{FetchPolicy, LogChunk, POLL_LOG_MAX_BYTES};
use crate::in_flight::{InFlight, InFlightGuard};
use crate::local_backend::{
    detect_completion, parse_results_after_completion, read_log_chunk, read_tail, RESULT_TAIL_BYTES,
};
use crate::models::job::{Job, JobStatus};
use crate::remote::classify::{FailReason, Outcome};
use crate::remote::markers::parse_exit_code;
use crate::remote::ssh::CommandRunner;
use crate::remote::submit::Label;
use crate::ssh_backend::{
    collect_and_classify, coordinates, fetch_remote, label_remote, mark_cancelled_conn, poll_log_remote, recorded_root,
    RemoteCoordinates,
};
use live_log::{Applied, Drained, LiveLog};
use plan::{PollerMemory, MAX_FETCH_STRIKES};

/// Where the poller's events go. Part B's `AppHandle` sink maps each to exactly the local event and
/// payload — `job:log` (`local_backend::emit_log`), `job:convergence` (`emit_convergence`),
/// `job:status` (`emit_status`) — plus the one new event `job:log-reset { job_id }` (o16.3). The
/// poller calls `log`/`convergence` only with a non-empty batch.
pub trait PollerSink: Send + Sync {
    fn log(&self, job_id: &str, lines: Vec<String>);
    fn convergence(&self, job_id: &str, events: Vec<ConvergenceEvent>);
    /// Drop every line and convergence point held for the job: the stream restarts at 0.
    fn log_reset(&self, job_id: &str);
    fn status(&self, job_id: &str, status: JobStatus);
}

/// The outcomes that need `.exit_code` and so download the job's files (o item 4): `Completed`
/// (rows 2, 5) and `Failed{NonZeroExit | BadExitCode | NoNormalTermination}` (row 6). A failed job's
/// files are its debugging evidence. Every other outcome is shown and the row stays live.
pub fn is_fetching(outcome: &Outcome) -> bool {
    matches!(
        outcome,
        Outcome::Completed { .. }
            | Outcome::Failed {
                reason: FailReason::NonZeroExit { .. } | FailReason::BadExitCode { .. } | FailReason::NoNormalTermination
            }
    )
}

/// What `error_message` shows for a non-fetching outcome (o item 4: "shown, with 'handled in unit
/// 5.4' where it needs an action"); `None` clears it — the job is plainly queued or running.
pub fn shown_message(outcome: &Outcome) -> Option<String> {
    const LATER: &str = "handled in unit 5.4";
    match outcome {
        Outcome::Queued | Outcome::Running => None,
        Outcome::Indeterminate => Some(
            "The server's state could not be decided on this check (a queue could not be read, or the job started \
             while it was read); the next check tries again."
                .into(),
        ),
        Outcome::Lost { orphans } => Some(format!(
            "The job started on the server, and its wrapper is gone without an exit code ({} orphan process(es) \
             still hold its cores); {LATER}.",
            orphans.len()
        )),
        Outcome::Cancelling => Some(format!("The job was cancelled on the server and some of its processes still run; {LATER}.")),
        // Not shown: the status step writes it terminal (o17); the text stays for completeness.
        Outcome::Cancelled => Some("The server reports the job cancelled (.cancelled, nothing of it runs).".into()),
        Outcome::ReEnqueue => Some(format!("Nothing of the job ever ran on the server; re-enqueueing it is {LATER}.")),
        Outcome::Failed { reason } => Some(format!("The server reports the job failed: {reason}; {LATER}.")),
        Outcome::Completed { .. } => None,
    }
}

/// What one status step did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StatusStep {
    /// Another operation holds the job's in-flight guard: skipped this tick (o15.3).
    Busy,
    /// The row is no longer a non-terminal remote job (finished, vanished, or local): its live state
    /// and memory are dropped, nothing was called.
    NotLive,
    /// [`MAX_FETCH_STRIKES`] fetches in a row failed: no automatic fetch until a manual retry.
    FetchStopped,
    /// The label call says the job is not in the server's hands ("not on the server", "submit
    /// interrupted"): nothing to collect (o 3.4).
    Labelled(Label),
    /// The label call or the collect could not be done (transport, a broken reply): nothing changed.
    CheckFailed(String),
    /// A non-fetching outcome: shown, the row stays `queued`/`running`.
    Shown(Outcome),
    /// The classifier says `Cancelled` (row 4: `.cancelled`, nothing of the job runs): the row is
    /// terminal `cancelled`, as a withdraw records it (o17). Nothing is downloaded.
    Cancelled,
    /// A fetching outcome whose fetch failed: no status change, a strike counted.
    FetchFailed { strikes: u32, reason: String },
    /// Fetched, verified, decided from the downloaded files and written; `status` is what was written.
    /// `drain_error`: the live view could not be drained (the row is final either way).
    Finalised { outcome: Outcome, status: JobStatus, drain_error: Option<String> },
    /// The row changed under the step (another writer won): nothing was written.
    RowChanged,
}

/// Everything a status step works with.
pub struct Poller<'a> {
    pub db: &'a DbState,
    pub runner: &'a dyn CommandRunner,
    pub in_flight: &'a InFlight,
    pub memory: &'a PollerMemory,
    pub live: &'a LiveLog,
    pub sink: &'a dyn PollerSink,
    /// What the fetch downloads. There is no per-job `.gbw` opt-in yet, so Part B passes
    /// `FetchPolicy::SMALL_ONLY`.
    pub policy: FetchPolicy,
}

impl Poller<'_> {
    /// The loop's status step for `job_id`: claim the in-flight guard without waiting — a busy job is
    /// skipped (o15.3) — and run it.
    pub fn status_step(&self, job_id: &str) -> Result<StatusStep, AppError> {
        match self.in_flight.try_acquire(job_id) {
            None => Ok(StatusStep::Busy),
            Some(guard) => self.run_status(&guard),
        }
    }

    /// The manual retry after the strikes stopped automatic fetches (o item 4): the bound starts
    /// over and one status step runs. The caller (a command) holds `guard`, the job's in-flight
    /// guard, as every command does; the guard names the job.
    pub fn retry_fetch(&self, guard: &InFlightGuard) -> Result<StatusStep, AppError> {
        self.memory.clear_strikes(guard.job_id());
        self.run_status(guard)
    }

    /// One status step of the job `guard` holds.
    fn run_status(&self, guard: &InFlightGuard) -> Result<StatusStep, AppError> {
        let job_id = guard.job_id();
        let Some((job, coords)) = self.live_remote(job_id)? else {
            self.live.drop_state(job_id);
            self.memory.forget(job_id);
            return Ok(StatusStep::NotLive);
        };
        let m = self.memory.get(job_id);
        if m.fetching && m.strikes >= MAX_FETCH_STRIKES {
            return Ok(StatusStep::FetchStopped);
        }
        let root = recorded_root(&coords, job_id)?;

        if job.status == JobStatus::Queued {
            match label_remote(self.runner, &job) {
                Err(e) => return Ok(StatusStep::CheckFailed(e.to_string())),
                Ok(report) if report.label != Label::Classifier => return Ok(StatusStep::Labelled(report.label)),
                Ok(_) => {}
            }
        }
        let outcome = match collect_and_classify(self.runner, &coords, &root) {
            Ok(outcome) => outcome,
            Err(e) => return Ok(StatusStep::CheckFailed(e)),
        };
        if outcome == Outcome::Cancelled {
            return self.record_cancelled(job_id, &coords);
        }
        if !is_fetching(&outcome) {
            return Ok(match self.record_shown(job_id, &coords, &outcome)? {
                Written::No => StatusStep::RowChanged,
                Written::Yes(emit) => {
                    if let Some(status) = emit {
                        self.sink.status(job_id, status);
                    }
                    StatusStep::Shown(outcome)
                }
            });
        }
        self.memory.set_fetching(job_id);
        self.fetch_and_finalise(&job, &coords, outcome)
    }

    /// The job as a non-terminal remote job, or `None`.
    fn live_remote(&self, job_id: &str) -> Result<Option<(Job, RemoteCoordinates)>, AppError> {
        let job = match get_job_conn(&*self.db.lock()?, job_id) {
            Ok(job) => job,
            Err(AppError::NotFound(_)) => return Ok(None),
            Err(e) => return Err(e),
        };
        Ok(match coordinates(&job)? {
            Some(coords) if is_live(job.status) => Some((job, coords)),
            _ => None,
        })
    }

    /// Steps 5–6 of the module doc: fetch, verify, decide from the downloaded files, write, drain,
    /// emit. A failed fetch is a strike and changes no status.
    fn fetch_and_finalise(&self, job: &Job, coords: &RemoteCoordinates, outcome: Outcome) -> Result<StatusStep, AppError> {
        let fetched = match &job.job_dir {
            None => Err("the job has no local job dir to download into".to_string()),
            Some(dir) => fetch_remote(self.runner, coords, Path::new(dir), self.policy).map(|()| PathBuf::from(dir)),
        };
        let local_dir = match fetched {
            Ok(dir) => dir,
            Err(reason) => return self.strike(&job.id, coords, reason),
        };

        // Rule #6 over the verified copy: the marker file and the normal-termination line.
        let out = local_dir.join("output.out");
        let exit_code = std::fs::read(local_dir.join(".exit_code"))
            .ok()
            .and_then(|raw| parse_exit_code(&raw).ok())
            .map(i32::from);
        let (status, message) = detect_completion(&out, &local_dir.join("stderr.log"), exit_code);
        let (energy, wall_time) = if status == JobStatus::Completed {
            let tail = read_tail(&out, RESULT_TAIL_BYTES).unwrap_or_default();
            (
                crate::result_extraction::extract_final_energy(&tail),
                crate::result_extraction::extract_wall_time(&tail),
            )
        } else {
            (None, None)
        };

        let Some(status) = finalise(self.db, &job.id, coords, status, message.as_deref(), energy, wall_time)? else {
            return Ok(StatusStep::RowChanged);
        };
        self.memory.forget(&job.id);
        let drain_error = self.drain(&job.id, &out).err().map(|e| e.to_string());
        self.live.drop_state(&job.id);
        self.sink.status(&job.id, status);
        Ok(StatusStep::Finalised { outcome, status, drain_error })
    }

    /// The live view's tail, from the downloaded copy (o16.6): only a watched job's copy is read.
    fn drain(&self, job_id: &str, out: &Path) -> io::Result<Drained> {
        let mut read = |offset: u64| match read_log_chunk(out, offset, POLL_LOG_MAX_BYTES) {
            // No output.out on the server either (the wrapper failed before ORCA): nothing to stream.
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(LogChunk::unchanged(offset)),
            other => other,
        };
        self.live.drain(job_id, &mut read, POLL_LOG_MAX_BYTES, self.sink)
    }

    /// A classifier `Cancelled` (o17): the terminal write of `record_withdraw`
    /// (`ssh_backend::mark_cancelled_conn`) under one lock with the live-row re-check, no download
    /// (o6 fetches only `Completed` and the `Failed` variants); then the live state is dropped and
    /// `job:status` emitted.
    fn record_cancelled(&self, job_id: &str, coords: &RemoteCoordinates) -> Result<StatusStep, AppError> {
        {
            let conn = self.db.lock()?;
            if live_row(&conn, job_id, coords)?.is_none() {
                return Ok(StatusStep::RowChanged);
            }
            mark_cancelled_conn(
                &conn,
                job_id,
                coords,
                "Cancelled: the server confirms .cancelled and nothing of the job runs.",
                &[JobStatus::Queued, JobStatus::Running],
            )?;
        }
        self.memory.forget(job_id);
        self.live.drop_state(job_id);
        self.sink.status(job_id, JobStatus::Cancelled);
        Ok(StatusStep::Cancelled)
    }

    /// A failed fetch: count it, show it, and stop the automatic fetches at the bound.
    /// A row that changed meanwhile (`Written::No`) is `RowChanged` and counts no strike.
    fn strike(&self, job_id: &str, coords: &RemoteCoordinates, reason: String) -> Result<StatusStep, AppError> {
        let strikes = self.memory.get(job_id).strikes + 1;
        let message = if strikes >= MAX_FETCH_STRIKES {
            format!(
                "Fetching the results failed {strikes} times in a row; automatic fetches stopped until a manual retry. \
                 Last failure: {reason}"
            )
        } else {
            format!("Fetching the results failed (attempt {strikes} of {MAX_FETCH_STRIKES}); the next check retries: {reason}")
        };
        match self.write_message(job_id, coords, Some(&message))? {
            Written::No => return Ok(StatusStep::RowChanged),
            Written::Yes(emit) => {
                self.memory.strike(job_id);
                if let Some(status) = emit {
                    self.sink.status(job_id, status);
                }
            }
        }
        Ok(StatusStep::FetchFailed { strikes, reason })
    }

    /// Show a non-fetching outcome: the classifier's `Running` moves a `queued` row to `running`;
    /// every outcome sets `error_message` to [`shown_message`].
    fn record_shown(&self, job_id: &str, coords: &RemoteCoordinates, outcome: &Outcome) -> Result<Written, AppError> {
        let conn = self.db.lock()?;
        let Some(before) = live_row(&conn, job_id, coords)? else {
            return Ok(Written::No);
        };
        if *outcome == Outcome::Running && before.status == JobStatus::Queued {
            conn.execute(
                "UPDATE jobs SET status = 'running', started_at = datetime('now'), error_message = NULL \
                 WHERE id = ?1 AND status = 'queued' AND remote_job_dir = ?2",
                params![job_id, coords.job_dir],
            )?;
        } else {
            set_message(&conn, job_id, coords, shown_message(outcome).as_deref())?;
        }
        Ok(Written::Yes(status_to_emit(&before, &get_job_conn(&conn, job_id)?)))
    }

    /// Set `error_message` on the row if it is still this live remote job.
    fn write_message(&self, job_id: &str, coords: &RemoteCoordinates, message: Option<&str>) -> Result<Written, AppError> {
        let conn = self.db.lock()?;
        let Some(before) = live_row(&conn, job_id, coords)? else {
            return Ok(Written::No);
        };
        set_message(&conn, job_id, coords, message)?;
        Ok(Written::Yes(status_to_emit(&before, &get_job_conn(&conn, job_id)?)))
    }
}

/// Whether a write found the row still this live remote job, and the `job:status` it owes.
enum Written {
    No,
    Yes(Option<JobStatus>),
}

fn is_live(status: JobStatus) -> bool {
    matches!(status, JobStatus::Queued | JobStatus::Running)
}

/// The row, if it is still the non-terminal remote job at `coords`.
fn live_row(conn: &rusqlite::Connection, job_id: &str, coords: &RemoteCoordinates) -> Result<Option<Job>, AppError> {
    let job = match get_job_conn(conn, job_id) {
        Ok(job) => job,
        Err(AppError::NotFound(_)) => return Ok(None),
        Err(e) => return Err(e),
    };
    Ok((is_live(job.status) && coordinates(&job)?.as_ref() == Some(coords)).then_some(job))
}

fn set_message(conn: &rusqlite::Connection, job_id: &str, coords: &RemoteCoordinates, message: Option<&str>) -> Result<(), AppError> {
    conn.execute(
        "UPDATE jobs SET error_message = ?1 WHERE id = ?2 AND status IN ('queued', 'running') AND remote_job_dir = ?3",
        params![message, job_id, coords.job_dir],
    )?;
    Ok(())
}

/// The terminal write, in the local finish path's order — the status (`finalize_job_conn`), then
/// on `Completed` the energy and wall time, then the parsed results (`parse_results_after_completion`,
/// which may advance to `parsed`) — but in **one** locked transaction, so no reader (and no delete)
/// sees a terminal row without its results. Written only if the row is still this live remote job;
/// `None` otherwise. Returns the status written.
fn finalise(
    db: &DbState,
    job_id: &str,
    coords: &RemoteCoordinates,
    status: JobStatus,
    message: Option<&str>,
    energy: Option<f64>,
    wall_time: Option<f64>,
) -> Result<Option<JobStatus>, AppError> {
    let conn = db.lock()?;
    let tx = conn.unchecked_transaction()?;
    if live_row(&tx, job_id, coords)?.is_none() {
        return Ok(None);
    }
    finalize_job_conn(&tx, job_id, status, message)?;
    let mut status = status;
    if status == JobStatus::Completed {
        if energy.is_some() || wall_time.is_some() {
            set_job_results_conn(&tx, job_id, energy, wall_time)?;
        }
        status = parse_results_after_completion(&tx, job_id);
    }
    tx.commit()?;
    Ok(Some(status))
}

/// What one log step did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LogStep {
    /// Nobody watches the job: no call.
    NotWatched,
    /// The poll failed (transport, a broken reply): state and offset unchanged, nothing emitted
    /// (o16.5).
    Failed(String),
    Applied(Applied),
}

/// One log poll of remote job `job_id` (o16): begin a step (only for a watched job), read the chunk
/// over ssh **without** the live-log mutex and without the in-flight guard (o16.5), then apply it
/// under the mutex with the generation re-checked. The planner decides whether it is due.
pub fn log_step(live: &LiveLog, runner: &dyn CommandRunner, sink: &dyn PollerSink, job_id: &str, coords: &RemoteCoordinates) -> LogStep {
    let Some(ticket) = live.begin(job_id) else {
        return LogStep::NotWatched;
    };
    match poll_log_remote(runner, coords, ticket.offset) {
        Err(reason) => LogStep::Failed(reason),
        Ok(chunk) => LogStep::Applied(live.apply(job_id, ticket, &chunk, POLL_LOG_MAX_BYTES, sink)),
    }
}

/// A sink that records every event, for the tests.
#[cfg(test)]
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Event {
    Log(String, Vec<String>),
    Convergence(String, Vec<ConvergenceEvent>),
    Reset(String),
    Status(String, JobStatus),
}

#[cfg(test)]
#[derive(Default)]
pub(crate) struct RecordingSink {
    events: std::sync::Mutex<Vec<Event>>,
    /// When set, every `log`, `convergence` and `log_reset` asserts this live log's mutex is held:
    /// the event is emitted inside the locked step that decided it (o16.3).
    live: Option<std::sync::Arc<LiveLog>>,
}

#[cfg(test)]
impl RecordingSink {
    /// A live log and a sink that checks every stream event is emitted under its mutex.
    pub(crate) fn with_live() -> (std::sync::Arc<LiveLog>, RecordingSink) {
        let live = std::sync::Arc::new(LiveLog::new());
        (live.clone(), RecordingSink { events: Default::default(), live: Some(live) })
    }

    fn under_the_mutex(&self, what: &str) {
        if let Some(live) = &self.live {
            assert!(live.is_locked(), "{what} emitted outside the live-log mutex (o16.3)");
        }
    }

    fn push(&self, event: Event) {
        self.events.lock().unwrap().push(event);
    }

    /// Every event so far, emptying the record.
    pub(crate) fn take(&self) -> Vec<Event> {
        std::mem::take(&mut *self.events.lock().unwrap())
    }

    pub(crate) fn events(&self) -> Vec<Event> {
        self.events.lock().unwrap().clone()
    }
}

#[cfg(test)]
impl PollerSink for RecordingSink {
    fn log(&self, job_id: &str, lines: Vec<String>) {
        assert!(!lines.is_empty(), "the poller never emits an empty batch");
        self.under_the_mutex("job:log");
        self.push(Event::Log(job_id.into(), lines));
    }
    fn convergence(&self, job_id: &str, events: Vec<ConvergenceEvent>) {
        assert!(!events.is_empty(), "the poller never emits an empty batch");
        self.under_the_mutex("job:convergence");
        self.push(Event::Convergence(job_id.into(), events));
    }
    fn log_reset(&self, job_id: &str) {
        self.under_the_mutex("job:log-reset");
        self.push(Event::Reset(job_id.into()));
    }
    fn status(&self, job_id: &str, status: JobStatus) {
        self.push(Event::Status(job_id.into(), status));
    }
}

#[cfg(test)]
mod tests;
