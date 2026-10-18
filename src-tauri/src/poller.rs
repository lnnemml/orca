//! The remote poller's core (ADR-024 o item 4, o15, o16; unit 5.3 B2 Part A): one loop over every
//! non-terminal remote job, started at launch. **`AppHandle`-free**: every step takes the database,
//! a [`CommandRunner`] and a [`PollerSink`] for its events, so the tests drive it with a fake runner
//! and a recording sink. The loop's body is [`run`]; its `AppHandle` shell (the loop thread, the
//! sink that emits the app's events) is [`app`]; the commands are `commands::remote_jobs`
//! (`watch_job_log`, `retry_remote_fetch`).
//!
//! Two steps per job, sequenced per job by the loop, never concurrent ([`plan`]):
//!
//! **The status step** ([`Poller::status_step`]), every 15 s for every job, under the job's
//! in-flight guard — a busy job is **skipped** this tick (o15.3). **One read decides** (o22.13.1):
//! the step's first locked read takes the row and its pending cancel together, and
//! - while a cancel is **pending** (`cancel_requested_at` set), the step is the cancel's
//!   **delivery** ([`cancel`], o22.4) instead of everything below — before the fetch bound's
//!   short-circuit, so delivery runs first on reconnect (Decision i). Delivery never downloads: its
//!   `EndedBeforeCancel` / `CancelDelivered` is the step's result, both columns are cleared, and the
//!   **next ordinary step** finishes from the server's facts (o22.13.2);
//! - a **`cancelling`** row is **swept** ([`sweep`], o22.5, o22.13.3–4): ready the server, re-run
//!   `cancel.sh`, collect + classify, bounded by [`plan::MAX_SWEEPS_WITHOUT_PROGRESS`]; a stopped job
//!   is only collected and classified;
//! - otherwise:
//! 1. a `queued` row gets the read-only **label call** first (o 3.4): "not on the server" and
//!    "submit interrupted" end the step (no collect on a missing dir); a `running` row has already
//!    started and goes straight to
//! 2. **collect + `classify`** with the recorded socket (the collector adds the `.enqueued` one);
//! 3. a classifier **`Cancelled`** (row 4) is terminal (o17): without `.started` nothing ran, and the
//!    row is written `cancelled` at once with no download; with `.started` — and row 1 with
//!    `.cancelled` — the job's files are fetched as **evidence** first ([`evidence`], o22.6,
//!    o22.13.5): downloaded and listed, then written `cancelled` (row 1: `failed`), then drained —
//!    never decided by `detect_completion`, never parsed;
//! 4. a **row 3** `Cancelling` on a `queued`/`running` row writes status `cancelling` with its text
//!    and clears both pending-cancel columns in that one write (o22.12.1, o22.13.5);
//! 5. any other **non-fetching outcome** is shown — `error_message` says what the server reports, and
//!    "handled in unit 5.4b" where it needs an action of 5.4b — and the row stays `queued`/`running`;
//!    the classifier's `Running` moves a `queued` row to `running` (o item 1: a remote job keeps
//!    `queued`/`running` in `jobs.status`). `Indeterminate` writes nothing: the row keeps the text
//!    of the last known state, and "could not be decided on this check" goes only to the job's
//!    remote record (o19.3);
//! 6. a **fetching outcome** (`Completed`, `Failed{NonZeroExit | BadExitCode |
//!    NoNormalTermination}`) sets the in-memory fetching flag (the live log stops, o16.6), then
//!    **fetches**: rsync down with the shared filter, then the server's listing must equal the local
//!    copy, hash for hash (`ssh_backend::fetch_remote`). Only then does **`detect_completion` on the
//!    downloaded files** decide the status (rule #6) — never the classifier's verdict, never a
//!    hard-coded `Completed` — and one locked transaction writes the terminal status, the energy and
//!    wall time, and the parsed results, in the local finish path's order. Then the live log is
//!    drained from the local copy (o16.6), and `job:status` is emitted last. When a delivery found
//!    the job ended before the cancel reached the server, that `finalise` adds the note (o22.13.2).
//! 7. A failed fetch changes no status: a strike is counted and shown; after
//!    [`MAX_FETCH_STRIKES`] in a row, automatic fetches stop until [`Poller::retry_fetch`].
//!
//! Every step's [`StatusStep`] then updates the job's **remote record** ([`remote_state`], o19) —
//! the labels, the last outcome, a failed check, the fetch bound, the cancel re-send bound, and the
//! **last contact** (o22.8: whether the step's last ssh/rsync call got an answer, recorded by a
//! `ContactRunner` around the step's runner) — while the guard is still held, and the change
//! reaches the view as `job:remote-state`.
//!
//! **Delivering a pending cancel** ([`cancel`], o22.4): ready the server, label, classify, mark,
//! then send `cancel.sh`, and let the server's verdict decide. The cancel core calls it
//! (`commands::jobs::cancel_job_core`, through [`Poller::deliver_and_record`]; the `cancel_job`
//! command runs that core in `spawn_blocking`), and the status step runs it instead of the ordinary
//! step while a cancel is pending, with the step's own read (o22.13.1). **Every write of the two
//! pending-cancel columns emits `job:status`** once its lock is released (o22.14.5) — the mark, the
//! ended-first and the delivered writes here, the record and Undo writes in the cancel core — so
//! the view's row follows a pending cancel without a status change.
//!
//! **The log step** ([`log_step`]), every 2 s while the job is watched (o16): no in-flight guard
//! (o16.5), no database; see [`live_log`].
//!
//! **The database lock is never held across an ssh or rsync call**: each write re-reads the row
//! under the lock and writes only if it is still this non-terminal remote job (same recorded job
//! dir), so a concurrent change wins and the step reports it.

pub mod app;
pub mod cancel;
mod evidence;
pub mod live_log;
pub mod plan;
pub mod remote_state;
pub mod run;
mod sweep;

use std::io;
use std::path::{Path, PathBuf};

use rusqlite::params;

use crate::commands::jobs::{
    finalize_job_conn, get_job_conn, pending_cancel_conn, refuse_while_cancel_pending, set_job_results_conn, PendingCancel,
};
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
use crate::remote::contact::ContactRunner;
use crate::remote::markers::parse_exit_code;
use crate::remote::ssh::CommandRunner;
use crate::remote::submit::Label;
use crate::ssh_backend::{
    collect_and_classify, coordinates, fetch_remote, label_remote, mark_cancelled_conn, poll_log_remote, recorded_root,
    RemoteCoordinates,
};
use evidence::Evidence;
use live_log::{Applied, Drained, LiveLog};
use plan::{PollerMemory, MAX_FETCH_STRIKES};
use remote_state::{RemoteState, RemoteStates};

/// Where the poller's events go. The app's sink (`app::AppSink`) maps each to exactly the local event and
/// payload — `job:log` (`local_backend::emit_log`), `job:convergence` (`emit_convergence`),
/// `job:status` (`emit_status`) — plus two events of its own: `job:log-reset { job_id }` (o16.3) and
/// `job:remote-state { job_id, seq, state }` (o19.6). The poller calls `log`/`convergence` only
/// with a non-empty batch.
pub trait PollerSink: Send + Sync {
    fn log(&self, job_id: &str, lines: Vec<String>);
    fn convergence(&self, job_id: &str, events: Vec<ConvergenceEvent>);
    /// Drop every line and convergence point held for the job: the stream restarts at 0.
    fn log_reset(&self, job_id: &str);
    fn status(&self, job_id: &str, status: JobStatus);
    /// The job's remote record changed: change number `seq`, the record now (`None`: removed).
    /// Called with the record map's lock held ([`RemoteStates`]); it must not call back into it.
    fn remote_state(&self, job_id: &str, seq: u64, state: Option<&RemoteState>);
}

/// The outcomes that need `.exit_code` and so download the job's files (o item 4): `Completed`
/// (rows 2, 5) and `Failed{NonZeroExit | BadExitCode | NoNormalTermination}` (row 6). A failed job's
/// files are its debugging evidence. Every other outcome is shown and the row stays live — but row 4,
/// row 1 with `.cancelled` (terminal, o17/o22.6) and row 3 (`cancelling`, o22.13.5).
pub fn is_fetching(outcome: &Outcome) -> bool {
    matches!(
        outcome,
        Outcome::Completed { .. }
            | Outcome::Failed {
                reason: FailReason::NonZeroExit { .. } | FailReason::BadExitCode { .. } | FailReason::NoNormalTermination
            }
    )
}

/// What a non-fetching outcome says (o item 4: "shown, with 'handled in unit 5.4b' where it needs an
/// action", o22.14.7): the row's `error_message` for every outcome but `Indeterminate`, whose text is the
/// job's remote record's `check_error` instead and never reaches the row (o19.3); `None` clears the
/// message — the job is plainly queued or running.
pub fn shown_message(outcome: &Outcome) -> Option<String> {
    const LATER: &str = "handled in unit 5.4b";
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
        Outcome::Cancelling { session } => Some(cancelling_message(*session, false)),
        // Not shown: the status step writes it terminal (o17) — at once without `.started`, after
        // the evidence fetch with it (o22.6); the text stays for completeness.
        Outcome::Cancelled { .. } => Some("The server reports the job cancelled (.cancelled, nothing of it runs).".into()),
        Outcome::ReEnqueue => Some(format!("Nothing of the job ever ran on the server; re-enqueueing it is {LATER}.")),
        Outcome::Failed { reason } => Some(format!("The server reports the job failed: {reason}; {LATER}.")),
        Outcome::Completed { .. } => None,
    }
}

/// A `cancelling` row's text (row 3, o22.5): what still runs, and whether the automatic sweeps have
/// stopped at [`plan::MAX_SWEEPS_WITHOUT_PROGRESS`] (o22.13.4).
pub fn cancelling_message(session: usize, stopped: bool) -> String {
    if stopped {
        format!(
            "The job was cancelled on the server, but processes of the job still run after 3 sweeps ({session} in the \
             job's directory); automatic sweeps stopped — Retry cancel sweeps again."
        )
    } else {
        format!(
            "The job was cancelled on the server and some of its processes still run ({session} in the job's \
             directory); the next check sweeps them again."
        )
    }
}

/// The note the job's next `finalise` adds after a delivery found it ended first (o22.13.2).
pub const ENDED_FIRST_NOTE: &str = "The job finished before the cancel reached the server.";

/// What one status step did. `retry_remote_fetch` returns it: `{ "step": "<snake_case>", "detail": … }`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "step", content = "detail", rename_all = "snake_case")]
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
    /// A non-fetching outcome: shown, the row stays live — row 3 `Cancelling` on a `queued`/`running`
    /// row was written `cancelling` (o22.13.5).
    Shown(Outcome),
    /// The classifier says `Cancelled` (row 4: `.cancelled`, nothing of the job runs): the row is
    /// terminal `cancelled` (o17) — at once when nothing ran, after its evidence was downloaded and
    /// listed when it had started (o22.6), or with "evidence not downloaded" at the last strike.
    Cancelled,
    /// A fetching outcome (or an evidence fetch, o22.6) whose fetch failed: no status change, a strike
    /// counted.
    FetchFailed { strikes: u32, reason: String },
    /// Fetched, verified, decided from the downloaded files and written; `status` is what was written.
    /// Row 1 with `.cancelled` is written `failed` after its evidence fetch, never decided from the
    /// files (o22.13.5). `drain_error`: the live view could not be drained (the row is final either
    /// way).
    Finalised { outcome: Outcome, status: JobStatus, drain_error: Option<String> },
    /// The row changed under the step (another writer won): nothing was written.
    RowChanged,
    /// A delivery ([`Poller::deliver_cancel`], o22.4) that the server answered without delivering:
    /// `cancel.sh` exited non-zero, or the server could not be readied or the script run (refused,
    /// not installed, a broken reply). Both pending-cancel columns stay set and the next step tries
    /// again; a failed send counts toward [`plan::MAX_CANCEL_FAILURES`]. Why → `check_error`.
    CancelNotSent(String),
    /// A delivery found the re-send bound reached ([`plan::MAX_CANCEL_FAILURES`] answered failures
    /// in a row): nothing was sent or written on the server; Retry cancel restarts it (o22.14.1). It
    /// still classified (o22.11.4, o22.12.2): `seen` is the server's current verdict (the
    /// pre-classify's outcome, or the label on the not-on-server path), which the record shows like
    /// an ordinary step's. `seen` is `None` only when the prepare-only ready was answered with a
    /// failure (o22.12.3; a prepare without an answer is `CheckFailed`); `unready` is then its
    /// reason, and `None` otherwise.
    CancelStopped { seen: Option<Seen>, unready: Option<String> },
    /// A delivery's pre-cancel classify (o22.4.4) found the job ended without `.cancelled` — row 5
    /// `Completed`, row 6 `Failed{NonZeroExit | BadExitCode | NoNormalTermination}` — before the
    /// cancel reached the server. Nothing was sent or downloaded; both pending-cancel columns were
    /// cleared and the note remembered; the **next ordinary step** re-classifies and takes the normal
    /// fetch path, whose `finalise` adds the note (o22.13.2).
    EndedBeforeCancel(Outcome),
    /// `cancel.sh` exited 0 — or the pre-classify already showed `.cancelled` — the cancel is
    /// **delivered**, and the classify says this. Both pending-cancel columns were cleared in the
    /// write that recorded it; a row 3 `Cancelling` was written `cancelling`. Nothing is downloaded:
    /// the **next ordinary step** re-derives every other verdict from the server (o22.11.2,
    /// o22.13.2) — row 4 and row 1 with `.cancelled` take o22.6's evidence path, row 2
    /// `Completed{late_cancel}` the normal fetch, the rest are shown.
    CancelDelivered(Outcome),
    /// A sweep of a `cancelling` job (o22.5, o22.13.3): the server was readied, `cancel.sh` re-run,
    /// and the job collected and classified. `outcome` is the classify's `Cancelling`, `None` when
    /// nothing could be classified; `error` is a failure the server answered with (a refused ready, a
    /// `cancel.sh` that failed, a broken collect), with the flag of the **last failing call** — a
    /// joined "earlier; then why" takes the later call's (o22.14.3). A sweep whose classify shows
    /// anything else returns that outcome's ordinary result instead.
    Swept { outcome: Option<Outcome>, error: Option<remote_state::CheckError> },
}

/// What a stopped delivery saw before it declined to send (o22.12.2): `{ "kind": "outcome",
/// "detail": <Outcome> }` or `{ "kind": "label", "detail": "not_on_server" }`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "kind", content = "detail", rename_all = "snake_case")]
pub enum Seen {
    /// The pre-cancel classify's verdict.
    Outcome(Outcome),
    /// The label call's "not on the server" / "submit interrupted": nothing to classify.
    Label(Label),
}

/// Everything a status step works with.
pub struct Poller<'a> {
    pub db: &'a DbState,
    pub runner: &'a dyn CommandRunner,
    pub in_flight: &'a InFlight,
    pub memory: &'a PollerMemory,
    pub live: &'a LiveLog,
    /// The per-job records the view reads (o19).
    pub remote: &'a RemoteStates,
    pub sink: &'a dyn PollerSink,
    /// What the fetch downloads. There is no per-job `.gbw` opt-in yet, so the app passes
    /// `FetchPolicy::SMALL_ONLY` (`app::with_poller`).
    pub policy: FetchPolicy,
}

impl Poller<'_> {
    /// The loop's status step for `job_id`: claim the in-flight guard without waiting — a busy job is
    /// skipped (o15.3), its record untouched — and run it.
    pub fn status_step(&self, job_id: &str) -> Result<StatusStep, AppError> {
        match self.in_flight.try_acquire(job_id) {
            None => Ok(StatusStep::Busy),
            Some(guard) => self.run_and_record(&guard),
        }
    }

    /// The manual retry after the strikes stopped automatic fetches (o item 4): the bound starts
    /// over and one status step runs. The caller (a command) holds `guard`, the job's in-flight
    /// guard, as every command does; the guard names the job. `clear_strikes` releases
    /// `PollerMemory`'s lock before the record is applied (R before M, o19.5), and the record re-reads
    /// the cleared counter, so a retry whose own step is `CheckFailed` no longer reads stopped.
    ///
    /// A job with a pending cancel is refused ("undo the cancel first", o22.8): its locked read is
    /// the first thing the retry does.
    pub fn retry_fetch(&self, guard: &InFlightGuard) -> Result<StatusStep, AppError> {
        refuse_while_cancel_pending(&*self.db.lock()?, guard.job_id())?;
        self.memory.clear_strikes(guard.job_id());
        self.run_and_record(guard)
    }

    /// One status step of the job `guard` holds, then its change to the job's remote record — made
    /// and emitted **while the guard is still held** (o19.4), so a later command on the job (a
    /// cancel's delivery, a retry) can never be overtaken by this step's record. A step that fails with an
    /// error changes no record.
    fn run_and_record(&self, guard: &InFlightGuard) -> Result<StatusStep, AppError> {
        self.record_step(guard, |poller, guard| poller.run_status(guard))
    }

    /// A delivery of the job's pending cancel ([`Poller::deliver_cancel`]) and its change to the
    /// record, made while `guard` is held — the cancel command's path (o22.3).
    pub(crate) fn deliver_and_record(&self, guard: &InFlightGuard) -> Result<StatusStep, AppError> {
        self.record_step(guard, |poller, guard| poller.deliver_cancel(guard))
    }

    /// Run `step` with a runner that remembers the contact of its last call, then apply the step and
    /// that contact to the record ([`remote_state::next_with`], o22.8). Every ssh/rsync call of a
    /// status step or a delivery goes through here; the log step does not (o16.5).
    fn record_step(
        &self,
        guard: &InFlightGuard,
        step: impl FnOnce(&Poller<'_>, &InFlightGuard) -> Result<StatusStep, AppError>,
    ) -> Result<StatusStep, AppError> {
        let tracker = ContactRunner::new(self.runner);
        let traced = Poller { runner: &tracker, ..*self };
        let step = step(&traced, guard)?;
        self.remote.apply_with_contact(guard, &step, tracker.last(), self.memory, self.sink);
        Ok(step)
    }

    /// One status step of the job `guard` holds. **One read decides** (o22.13.1): `live_remote`'s
    /// locked read carries the pending cancel, and while one is pending the delivery runs instead
    /// of everything after it — before the fetch bound, so no fetch runs for a job whose cancel is
    /// pending, and delivery comes first on reconnect (Decision i).
    fn run_status(&self, guard: &InFlightGuard) -> Result<StatusStep, AppError> {
        let job_id = guard.job_id();
        let Some((job, coords, pending)) = self.live_remote(job_id)? else {
            self.live.drop_state(job_id);
            self.memory.forget(job_id);
            return Ok(StatusStep::NotLive);
        };
        if pending.requested_at.is_some() {
            return self.deliver_pending(&job, &coords);
        }
        let m = self.memory.get(job_id);
        if m.fetching && m.strikes >= MAX_FETCH_STRIKES {
            return Ok(StatusStep::FetchStopped);
        }
        let root = recorded_root(&coords, job_id)?;

        if job.status == JobStatus::Cancelling {
            return self.sweep(&job, &coords, &root);
        }
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
        self.act_on(&job, &coords, outcome)
    }

    /// What an ordinary step does with the server's verdict (module doc, steps 3–6). A sweep hands
    /// its non-`Cancelling` verdicts here too (o22.13.3).
    fn act_on(&self, job: &Job, coords: &RemoteCoordinates, outcome: Outcome) -> Result<StatusStep, AppError> {
        if let Some(evidence) = Evidence::of(&outcome) {
            return self.fetch_evidence(job, coords, evidence, outcome);
        }
        match outcome {
            Outcome::Cancelled { started: false } => self.record_cancelled(&job.id, coords),
            Outcome::Cancelling { session } => self.record_cancelling(&job.id, coords, session),
            outcome if is_fetching(&outcome) => {
                self.memory.set_fetching(&job.id);
                self.fetch_and_finalise(job, coords, outcome)
            }
            outcome => Ok(match self.record_shown(&job.id, coords, &outcome)? {
                Written::No => StatusStep::RowChanged,
                Written::Yes(emit) => {
                    if let Some(status) = emit {
                        self.sink.status(&job.id, status);
                    }
                    StatusStep::Shown(outcome)
                }
            }),
        }
    }

    /// The job as a non-terminal remote job with its pending cancel, read under **one** lock, or
    /// `None`. The status step decides from it (o22.13.1), and delivery's step 1 re-reads with it.
    fn live_remote(&self, job_id: &str) -> Result<Option<(Job, RemoteCoordinates, PendingCancel)>, AppError> {
        let conn = self.db.lock()?;
        let job = match get_job_conn(&conn, job_id) {
            Ok(job) => job,
            Err(AppError::NotFound(_)) => return Ok(None),
            Err(e) => return Err(e),
        };
        let Some(coords) = coordinates(&job)?.filter(|_| is_live(job.status)) else { return Ok(None) };
        Ok(Some((job, coords, pending_cancel_conn(&conn, job_id)?)))
    }

    /// Row 3 on a `queued`/`running` row (o22.13.5): **one** write sets status `cancelling` and its
    /// text and NULLs both pending-cancel columns (o22.12.1: a `cancelling` row never holds a
    /// request — the v21 CHECK admits one there, so this write is what keeps it so), guarded on the
    /// row still being this live remote job; then the sweep baseline (o22.13.4). 0 rows →
    /// `RowChanged`.
    fn record_cancelling(&self, job_id: &str, coords: &RemoteCoordinates, session: usize) -> Result<StatusStep, AppError> {
        let emit = {
            let conn = self.db.lock()?;
            let Some(before) = live_row(&conn, job_id, coords)? else {
                return Ok(StatusStep::RowChanged);
            };
            let written = conn.execute(
                "UPDATE jobs SET status = 'cancelling', error_message = ?1, cancel_requested_at = NULL, \
                 cancel_sent_at = NULL \
                 WHERE id = ?2 AND remote_job_dir = ?3 AND status IN ('queued', 'running')",
                params![cancelling_message(session, false), job_id, coords.job_dir],
            )?;
            if written == 0 {
                return Ok(StatusStep::RowChanged);
            }
            status_to_emit(&before, &get_job_conn(&conn, job_id)?)
        };
        self.memory.note_cancelling(job_id, session);
        if let Some(status) = emit {
            self.sink.status(job_id, status);
        }
        Ok(StatusStep::Shown(Outcome::Cancelling { session }))
    }

    /// The note `finalise` adds for this job, if a delivery found it ended first (o22.13.2).
    fn ended_first_note(&self, job_id: &str) -> Option<&'static str> {
        self.memory.get(job_id).ended_before_cancel.then_some(ENDED_FIRST_NOTE)
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

        let note = self.ended_first_note(&job.id);
        let Some(status) = finalise(self.db, &job.id, coords, status, message.as_deref(), energy, wall_time, note)? else {
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

    /// A classifier `Cancelled { started: false }` (o17: nothing ran): the terminal write
    /// (`ssh_backend::mark_cancelled_conn`) under one lock with the live-row
    /// re-check, no download (a job that ran is the evidence fetch's, o22.6); then the live state is
    /// dropped and `job:status` emitted.
    fn record_cancelled(&self, job_id: &str, coords: &RemoteCoordinates) -> Result<StatusStep, AppError> {
        self.write_cancelled(job_id, coords, &LIVE)
    }

    /// [`Self::record_cancelled`]'s write from the statuses `from`. A write that changed no row —
    /// the live-row check passed, but `mark_cancelled_conn`'s own predicate did not match — is
    /// `RowChanged`: nothing is announced as cancelled that the database does not hold as such.
    fn write_cancelled(&self, job_id: &str, coords: &RemoteCoordinates, from: &[JobStatus]) -> Result<StatusStep, AppError> {
        if !self.cancel_row(job_id, coords, from, "Cancelled: the server confirms .cancelled and nothing of the job runs.")? {
            return Ok(StatusStep::RowChanged);
        }
        self.memory.forget(job_id);
        self.live.drop_state(job_id);
        self.sink.status(job_id, JobStatus::Cancelled);
        Ok(StatusStep::Cancelled)
    }

    /// The terminal `cancelled` write (`ssh_backend::mark_cancelled_conn`, which NULLs both
    /// pending-cancel columns) under one lock with the live-row re-check: `false` when the row
    /// changed or the write matched no row.
    fn cancel_row(&self, job_id: &str, coords: &RemoteCoordinates, from: &[JobStatus], message: &str) -> Result<bool, AppError> {
        let conn = self.db.lock()?;
        if live_row(&conn, job_id, coords)?.is_none() {
            return Ok(false);
        }
        Ok(mark_cancelled_conn(&conn, job_id, coords, message, from)? > 0)
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
    /// every other outcome but `Indeterminate` sets `error_message` to [`shown_message`].
    /// `Indeterminate` writes nothing (o19.3: the row keeps the last known state's text; the record's
    /// `check_error` says this check decided nothing), but the live row is still re-checked, so a
    /// row that changed under the step is `RowChanged` as for every outcome.
    fn record_shown(&self, job_id: &str, coords: &RemoteCoordinates, outcome: &Outcome) -> Result<Written, AppError> {
        let conn = self.db.lock()?;
        let Some(before) = live_row(&conn, job_id, coords)? else {
            return Ok(Written::No);
        };
        if *outcome == Outcome::Indeterminate {
            return Ok(Written::Yes(None));
        }
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

/// The remote-live statuses (o22.5): the poller keeps stepping such a job, and only the server's
/// verdict ends it. The same set as `plan::candidates`' SQL and `set_message`'s.
const LIVE: [JobStatus; 3] = [JobStatus::Queued, JobStatus::Running, JobStatus::Cancelling];

/// Whether `status` is one of [`LIVE`].
fn is_live(status: JobStatus) -> bool {
    matches!(status, JobStatus::Queued | JobStatus::Running | JobStatus::Cancelling)
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
        "UPDATE jobs SET error_message = ?1 WHERE id = ?2 AND status IN ('queued', 'running', 'cancelling') \
         AND remote_job_dir = ?3",
        params![message, job_id, coords.job_dir],
    )?;
    Ok(())
}

/// The terminal write, in the local finish path's order — the status (`finalize_job_conn`), then
/// on `Completed` the energy and wall time, then the parsed results (`parse_results_after_completion`,
/// which may advance to `parsed`) — but in **one** locked transaction, so no reader (and no delete)
/// sees a terminal row without its results. Written only if the row is still this live remote job;
/// `None` otherwise. Returns the status written.
///
/// `note` (o22.13.2, the ended-first notice) is added **last**, after the parse: the whole message
/// of a `completed` row, appended after `detect_completion`'s for a `failed` one — and after a
/// results-parse failure's, which `parse_results_after_completion` writes over the message
/// (`set_job_parse_error_conn`), so the note survives it.
#[allow(clippy::too_many_arguments)]
fn finalise(
    db: &DbState,
    job_id: &str,
    coords: &RemoteCoordinates,
    status: JobStatus,
    message: Option<&str>,
    energy: Option<f64>,
    wall_time: Option<f64>,
    note: Option<&str>,
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
    if let Some(note) = note {
        let noted = tx.execute(
            "UPDATE jobs SET error_message = CASE WHEN error_message IS NULL OR error_message = '' THEN ?1 \
             ELSE error_message || char(10) || ?1 END WHERE id = ?2",
            params![note, job_id],
        )?;
        // The row was read live in this transaction; a note that lands nowhere is a bug, and the
        // transaction is dropped (rolled back) rather than committed without it.
        if noted != 1 {
            return Err(AppError::Internal(format!("job {job_id}: the ended-first note matched {noted} rows")));
        }
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
    /// `job:remote-state` events `(job_id, seq, state)`, kept apart from `events` so the stream and
    /// status assertions read as before.
    remote_states: std::sync::Mutex<Vec<(String, u64, Option<RemoteState>)>>,
    /// When set, every `remote_state` asserts this record map's mutex is held (o19.6).
    remote: Option<std::sync::Arc<RemoteStates>>,
}

#[cfg(test)]
impl RecordingSink {
    /// A live log and a sink that checks every stream event is emitted under its mutex.
    pub(crate) fn with_live() -> (std::sync::Arc<LiveLog>, RecordingSink) {
        let live = std::sync::Arc::new(LiveLog::new());
        (live.clone(), RecordingSink { live: Some(live), ..Default::default() })
    }

    /// This sink, checking every `job:remote-state` is emitted under `remote`'s mutex (o19.6).
    pub(crate) fn under_remote(self, remote: std::sync::Arc<RemoteStates>) -> RecordingSink {
        RecordingSink { remote: Some(remote), ..self }
    }

    /// Every `job:remote-state` so far.
    pub(crate) fn remote_states(&self) -> Vec<(String, u64, Option<RemoteState>)> {
        self.remote_states.lock().unwrap().clone()
    }

    /// Every `job:remote-state` so far, emptying that record.
    pub(crate) fn take_remote_states(&self) -> Vec<(String, u64, Option<RemoteState>)> {
        std::mem::take(&mut *self.remote_states.lock().unwrap())
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
    fn remote_state(&self, job_id: &str, seq: u64, state: Option<&RemoteState>) {
        if let Some(remote) = &self.remote {
            assert!(remote.is_locked(), "job:remote-state emitted outside the record map's mutex (o19.6)");
        }
        self.remote_states.lock().unwrap().push((job_id.into(), seq, state.cloned()));
    }
}

#[cfg(test)]
mod tests;
