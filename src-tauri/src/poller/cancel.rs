//! Delivering a pending cancel (ADR-024 o22.4, unit 5.4a): one function, [`Poller::deliver_cancel`],
//! called by the cancel command (o22.3) and — from 5.4a Part A2b — by the poller instead of a job's
//! ordinary status step while the job's cancel is pending. It always runs **holding the job's
//! in-flight guard**, and the database lock is never held across an ssh call.
//!
//! The steps, exactly as o22.4 orders them:
//! 1. **re-read** the row under the lock: no longer remote-live → `NotLive`; nothing pending (an
//!    Undo won) → `RowChanged`;
//! 2. **ready the server** (prepare → install if needed → prepare, `ssh_backend::ready_server`):
//!    the uploaded scripts are sha-named, so an app upgrade needs them reinstalled. A refusal or a
//!    not-installed script is reported as itself (`CancelNotSent`, counted toward the bound), a
//!    transport failure as `CheckFailed` — never one for the other;
//! 3. a **`queued`** row gets the **label call**: "not on the server" / "submit interrupted" →
//!    `mkdir -p` (`ssh_backend::mkjob_call`, the withdraw's tail; a refusal counted like step 2's)
//!    and straight to step 5 (there is nothing to classify in a dir that did not exist);
//! 4. **one collect + `classify` before any cancel**: a job that already ended without
//!    `.cancelled` (row 5 `Completed`, row 6 `Failed`) clears **both** columns — a marked but unsent
//!    cancel may precede this — and is `EndedBeforeCancel`: nothing is sent, the caller fetches. A
//!    snapshot that already holds `.cancelled` (rows 2, 3, 4, row 1 with `cancelled`; an earlier send
//!    whose reply was lost) means the cancel **is delivered**: both columns cleared as after rc 0,
//!    `CancelDelivered(outcome)`, nothing re-sent (o22.11.4; row 3's re-sweep is the poller's).
//!    Every other verdict goes on, `Indeterminate` and row 1 without `.cancelled` included
//!    (`cancel.sh` publishes `.cancelled` first and skips tsp when it cannot read the queue);
//! 5. **mark, then send**: one locked write sets `cancel_sent_at` only while `cancel_requested_at`
//!    is still set, and it **completes before `cancel.sh` is spawned** — so an Undo either wins
//!    before the mark (nothing is sent) or is refused after it (a lost reply can never be "undone"
//!    while `.cancelled` sits on the server). Then `cancel.sh cancel`:
//!    - **rc 0** — delivered: one collect + `classify` decides (never a hard-coded `Cancelled`,
//!      Decision c), and one locked write clears both columns and, for row 3, writes `cancelling`
//!      → `CancelDelivered(outcome)`;
//!    - **a transport failure** — not delivered, both columns stay, `CheckFailed`; the next step
//!      re-sends (step 6);
//!    - **anything the server answered that is not rc 0** (rc 2/3, a refused or not-installed
//!      script, a broken reply) — not delivered, both columns stay, `CancelNotSent(why)`, and one
//!      failure counted;
//!
//!    **The bound** (o22.11.3–4): every answered failure of a delivery counts — steps 2 and 3's
//!    refusals and the send's — a transport failure never does. After [`MAX_CANCEL_FAILURES`] in a
//!    row a delivery is **stopped**: it still runs steps 1–4 (so an ended job and a cancel already
//!    on the server are still seen) but writes nothing on the server — its ready is prepare-only (no
//!    install), no `mkjob`, no send — and returns `CancelStopped { seen, unready }`: what it saw
//!    (the outcome, or the label on the not-on-server path), or, when the prepare was answered with
//!    a failure, `None` with its reason (o22.12.2–3); a prepare that got no answer is `CheckFailed`. A successful ready does not reset the bound (o22.12.4); a Cancel
//!    click restarts it (the cancel core restarts the count);
//! 6. re-sending is safe: `cancel.sh cancel` is idempotent on a dir that holds `.cancelled` (A1's
//!    script tests).
//!
//! Downloads are not delivery's: an `EndedBeforeCancel` or a delivered fetching / evidence verdict
//! is acted on by the caller (the poller's run order, 5.4a Part A2b).

use rusqlite::params;

use super::plan::MAX_CANCEL_FAILURES;
use super::{is_fetching, is_live, live_row, shown_message, Poller, Seen, StatusStep};
use crate::commands::jobs::{get_job_conn, pending_cancel_conn, PendingCancel};
use crate::commands::remote_jobs::status_to_emit;
use crate::error::AppError;
use crate::in_flight::InFlightGuard;
use crate::models::job::{Job, JobStatus};
use crate::remote::classify::{FailReason, Outcome};
use crate::remote::contact::{Contact, ContactRunner};
use crate::remote::submit::Label;
use crate::ssh_backend::{
    cancel_call, check_ready, collect_and_classify, coordinates, label_call, mkjob_call, ready_server, recorded_root, tail,
    RemoteCoordinates,
};

/// Whether the job ended **before** a cancel reached the server (o22.4.4): a fetching outcome
/// without `.cancelled` — row 5 `Completed { late_cancel: false }` and row 6's `Failed`s (the
/// classifier returns rows 1–4 whenever `.cancelled` exists, so row 6 never carries it). Row 2's
/// `Completed { late_cancel: true }` already holds `.cancelled`: it is [`cancelled_on_server`].
pub(crate) fn ended_before_cancel(outcome: &Outcome) -> bool {
    is_fetching(outcome) && !matches!(outcome, Outcome::Completed { late_cancel: true })
}

/// Whether the snapshot already holds `.cancelled` (o22.11.4): row 2 `Completed { late_cancel }`,
/// row 3 `Cancelling`, row 4 `Cancelled`, row 1 `CorruptStarted { cancelled: true }` — the classifier
/// returns one of these whenever `.cancelled` exists. A pending cancel seen this way **is delivered**.
pub(crate) fn cancelled_on_server(outcome: &Outcome) -> bool {
    matches!(
        outcome,
        Outcome::Completed { late_cancel: true }
            | Outcome::Cancelling { .. }
            | Outcome::Cancelled { .. }
            | Outcome::Failed { reason: FailReason::CorruptStarted { cancelled: true, .. } }
    )
}

impl Poller<'_> {
    /// Deliver the pending cancel of the job `guard` holds (o22.4; the module doc). The caller
    /// holds the guard; `deliver_and_record` applies the result to the job's remote record.
    pub fn deliver_cancel(&self, guard: &InFlightGuard) -> Result<StatusStep, AppError> {
        let tracker = ContactRunner::new(self.runner);
        Poller { runner: &tracker, ..*self }.deliver(guard.job_id(), &tracker)
    }

    /// The steps. `tracker` is `self.runner`: it tells a call that failed in transport from one the
    /// server answered with a failure.
    fn deliver(&self, job_id: &str, tracker: &ContactRunner<'_>) -> Result<StatusStep, AppError> {
        // A failed call: a transport failure is a failed check, never counted; a failure the server
        // answered is a cancel not sent (o22.4.2: reported as itself, never as "unreachable") and
        // counts toward the re-send bound (o22.11.3).
        let failed = |why: String| match tracker.last() {
            Some(Contact::Ok) => {
                self.memory.cancel_failed(job_id);
                StatusStep::CancelNotSent(why)
            }
            _ => StatusStep::CheckFailed(why),
        };
        // The re-send bound (o22.11.4): once reached, a delivery still re-reads, readies, labels
        // and classifies, but sends nothing — `mkjob` included — until Cancel is clicked again.
        let stopped = || self.memory.get(job_id).cancel_failures >= MAX_CANCEL_FAILURES;

        // 1. Re-read under the lock.
        let Some((job, coords, pending)) = self.pending_row(job_id)? else {
            self.live.drop_state(job_id);
            self.memory.forget(job_id);
            return Ok(StatusStep::NotLive);
        };
        if pending.requested_at.is_none() {
            return Ok(StatusStep::RowChanged);
        }
        let root = recorded_root(&coords, job_id)?;

        // 2. Ready the server — prepare-only while stopped (o22.12.3): nothing is installed. A
        // prepare answered with a failure is the stopped delivery that could classify nothing; one
        // that got no answer is a failed check, as everywhere else.
        if stopped() {
            if let Err(why) = check_ready(self.runner, &coords.host, &root, &coords.job_dir) {
                return Ok(match tracker.last() {
                    Some(Contact::Ok) => StatusStep::CancelStopped { seen: None, unready: Some(why) },
                    _ => StatusStep::CheckFailed(why),
                });
            }
        } else if let Err((_, why)) = ready_server(self.runner, &coords.host, &root, &coords.job_dir) {
            return Ok(failed(why));
        }

        // 3. A queued job not in the server's hands: make its dir, then straight to the send.
        let mut classify_first = true;
        if job.status == JobStatus::Queued {
            match label_call(self.runner, &coords) {
                Err(why) => return Ok(StatusStep::CheckFailed(why)),
                Ok(report) if matches!(report.label, Label::NotOnServer | Label::SubmitInterrupted) => {
                    if stopped() {
                        return Ok(StatusStep::CancelStopped { seen: Some(Seen::Label(report.label)), unready: None });
                    }
                    if let Err(why) = mkjob_call(self.runner, &coords.host, &root, &coords.job_dir) {
                        return Ok(failed(why));
                    }
                    classify_first = false;
                }
                Ok(_) => {}
            }
        }

        // 4. One collect + classify before any cancel.
        if classify_first {
            let outcome = match collect_and_classify(self.runner, &coords, &root) {
                Ok(outcome) => outcome,
                Err(why) => return Ok(StatusStep::CheckFailed(why)),
            };
            if ended_before_cancel(&outcome) {
                return self.record_ended_first(job_id, &coords, outcome);
            }
            // `.cancelled` is already on the server (an earlier send whose reply was lost): the
            // cancel is delivered, stopped or not, and nothing is re-sent (o22.11.4).
            if cancelled_on_server(&outcome) {
                return self.record_delivered(job_id, &coords, outcome);
            }
            if stopped() {
                return Ok(StatusStep::CancelStopped { seen: Some(Seen::Outcome(outcome)), unready: None });
            }
        }

        // 5. Mark, then send. The mark's lock is released before `cancel.sh` is spawned.
        if !self.mark_sent(job_id, &coords)? {
            return Ok(StatusStep::RowChanged);
        }
        match cancel_call(self.runner, &coords, &root) {
            Err(why) => return Ok(failed(why)),
            Ok((0, _)) => {}
            Ok((rc, stderr)) => {
                self.memory.cancel_failed(job_id);
                return Ok(StatusStep::CancelNotSent(format!("cancel.sh exited {rc}: {}", tail(&stderr))));
            }
        }

        // Delivered: the server decides what it is now.
        let outcome = match collect_and_classify(self.runner, &coords, &root) {
            Ok(outcome) => outcome,
            Err(why) => return Ok(StatusStep::CheckFailed(format!(".cancelled is on the server, but {why}"))),
        };
        self.record_delivered(job_id, &coords, outcome)
    }

    /// The job, its coordinates and its pending cancel, if it is still a remote-live job.
    fn pending_row(&self, job_id: &str) -> Result<Option<(Job, RemoteCoordinates, PendingCancel)>, AppError> {
        let conn = self.db.lock()?;
        let job = match get_job_conn(&conn, job_id) {
            Ok(job) => job,
            Err(AppError::NotFound(_)) => return Ok(None),
            Err(e) => return Err(e),
        };
        let Some(coords) = coordinates(&job)? else { return Ok(None) };
        if !is_live(job.status) {
            return Ok(None);
        }
        let pending = pending_cancel_conn(&conn, job_id)?;
        Ok(Some((job, coords, pending)))
    }

    /// Step 5's mark: `cancel_sent_at` set in one locked write that re-checks the request — `false`
    /// when an Undo won (or the row is no longer this live remote job), and then nothing is sent.
    fn mark_sent(&self, job_id: &str, coords: &RemoteCoordinates) -> Result<bool, AppError> {
        let conn = self.db.lock()?;
        let marked = conn.execute(
            "UPDATE jobs SET cancel_sent_at = datetime('now') \
             WHERE id = ?1 AND remote_job_dir = ?2 AND status IN ('queued', 'running', 'cancelling') \
             AND cancel_requested_at IS NOT NULL",
            params![job_id, coords.job_dir],
        )?;
        Ok(marked == 1)
    }

    /// Step 4's "ended first": both columns cleared (the v21 CHECK refuses `cancel_sent_at` without
    /// `cancel_requested_at`), the re-send bound restarted.
    fn record_ended_first(&self, job_id: &str, coords: &RemoteCoordinates, outcome: Outcome) -> Result<StatusStep, AppError> {
        let cleared = {
            let conn = self.db.lock()?;
            conn.execute(
                "UPDATE jobs SET cancel_requested_at = NULL, cancel_sent_at = NULL \
                 WHERE id = ?1 AND remote_job_dir = ?2 AND status IN ('queued', 'running', 'cancelling')",
                params![job_id, coords.job_dir],
            )?
        };
        if cleared == 0 {
            return Ok(StatusStep::RowChanged);
        }
        self.memory.clear_cancel_failures(job_id);
        Ok(StatusStep::EndedBeforeCancel(outcome))
    }

    /// rc 0's write (o22.4.5): the delivered verdict recorded and both columns cleared in **one**
    /// locked write, if the row is still this live remote job. Row 3 `Cancelling` becomes status
    /// `cancelling` with its text; every other verdict changes no status here (a terminal verdict's
    /// write is the caller's, and NULLs the columns again). `job:status` when the row changed.
    fn record_delivered(&self, job_id: &str, coords: &RemoteCoordinates, outcome: Outcome) -> Result<StatusStep, AppError> {
        let emit = {
            let conn = self.db.lock()?;
            let Some(before) = live_row(&conn, job_id, coords)? else {
                return Ok(StatusStep::RowChanged);
            };
            if matches!(outcome, Outcome::Cancelling { .. }) {
                conn.execute(
                    "UPDATE jobs SET status = 'cancelling', error_message = ?1, cancel_requested_at = NULL, \
                     cancel_sent_at = NULL \
                     WHERE id = ?2 AND remote_job_dir = ?3 AND status IN ('queued', 'running', 'cancelling')",
                    params![shown_message(&outcome), job_id, coords.job_dir],
                )?;
            } else {
                conn.execute(
                    "UPDATE jobs SET cancel_requested_at = NULL, cancel_sent_at = NULL \
                     WHERE id = ?1 AND remote_job_dir = ?2 AND status IN ('queued', 'running', 'cancelling')",
                    params![job_id, coords.job_dir],
                )?;
            }
            status_to_emit(&before, &get_job_conn(&conn, job_id)?)
        };
        self.memory.clear_cancel_failures(job_id);
        if let Some(status) = emit {
            self.sink.status(job_id, status);
        }
        Ok(StatusStep::CancelDelivered(outcome))
    }
}

#[cfg(test)]
mod tests;
