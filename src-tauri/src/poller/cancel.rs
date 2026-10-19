//! Delivering a pending cancel (ADR-024 o22.4, unit 5.4a): one body, called by the cancel command
//! (o22.3, [`Poller::deliver_cancel`]) and by the poller's status step **instead of** the job's
//! ordinary step while the job's cancel is pending (o22.13.1, [`Poller::deliver_pending`], with the
//! step's own read). It always runs **holding the job's in-flight guard**, and the database lock is
//! never held across an ssh call.
//!
//! The steps, exactly as o22.4 orders them:
//! 1. **re-read** the row under the lock (the command's path; the status step's own read is this
//!    read): no longer remote-live → `NotLive`; nothing pending (an Undo won) → `RowChanged`;
//! 2. **ready the server** (prepare → install if needed → prepare, `ssh_backend::ready_server`):
//!    the uploaded scripts are sha-named, so an app upgrade needs them reinstalled. A refusal or a
//!    not-installed script is reported as itself (`CancelNotSent`, counted toward the bound), a
//!    transport failure as `CheckFailed` — never one for the other;
//! 3. a **`queued`** row gets the **label call**: "not on the server" / "submit interrupted" →
//!    `mkdir -p` (`ssh_backend::mkjob_call`, o2's tail for a job not on the server; a refusal counted like step 2's)
//!    and straight to step 5 (there is nothing to classify in a dir that did not exist);
//! 4. **one collect + `classify` before any cancel**: a job that already ended without
//!    `.cancelled` (row 5 `Completed`, row 6 `Failed`) clears **both** columns — a marked but unsent
//!    cancel may precede this — and is `EndedBeforeCancel`: nothing is sent or downloaded, and the
//!    note "the job finished before the cancel reached the server" is remembered for the job's next
//!    `finalise` (o22.13.2). A snapshot that already holds `.cancelled` (rows 2, 3, 4, row 1 with
//!    `cancelled`; an earlier send whose reply was lost) means the cancel **is delivered**: both
//!    columns cleared as after rc 0, `CancelDelivered(outcome)`, nothing re-sent (o22.11.4; row 3's
//!    re-sweep is the poller's sweep, o22.13.3).
//!    Every other verdict goes on, `Indeterminate` and row 1 without `.cancelled` included
//!    (`cancel.sh` publishes `.cancelled` first and skips tsp when it cannot read the queue);
//! 5. **mark, then send**: one locked write sets `cancel_sent_at` only while `cancel_requested_at`
//!    is still set, and it **completes before `cancel.sh` is spawned** — so an Undo either wins
//!    before the mark (nothing is sent) or is refused after it (a lost reply can never be "undone"
//!    while `.cancelled` sits on the server). Then `cancel.sh cancel`:
//!    - **rc 0** — delivered: one collect + `classify` decides (never a hard-coded `Cancelled`,
//!      Decision c), and one locked write clears both columns and, for row 3, writes `cancelling`
//!      (and sets the sweep baseline, o22.13.4) → `CancelDelivered(outcome)`;
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
//! **Every column write emits** `job:status` for the job once its lock is released (o22.14.5): the
//! mark (step 5), the ended-first write (step 4) and the delivered write (both its branches —
//! `cancelling` and the unchanged status), so the view's row shows the send marked (no more Undo)
//! and the request gone.
//!
//! **Delivery never downloads** (o22.13.2): its `EndedBeforeCancel` or `CancelDelivered` is the
//! step's result, both columns are already NULL, and the **next ordinary status step** re-collects,
//! re-classifies and finishes — rows 5/6's fetch, row 4's or row 1's evidence path, row 2's fetch,
//! or row 3's sweep — keyed on the snapshot, never on the path that delivered (o22.11.2). The cost
//! is at most one status period.

use rusqlite::params;

use super::plan::MAX_CANCEL_FAILURES;
use super::{is_fetching, live_row, shown_message, Poller, Seen, StatusStep};
use crate::commands::jobs::get_job_conn;
use crate::error::AppError;
use crate::in_flight::InFlightGuard;
use crate::models::job::{Job, JobStatus};
use crate::remote::classify::{FailReason, Outcome};
use crate::remote::contact::{Contact, ContactRunner};
use crate::remote::submit::Label;
use crate::ssh_backend::{
    cancel_call, check_ready, collect_and_classify, label_call, mkjob_call, ready_server, recorded_root, tail,
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
        let job_id = guard.job_id();
        // 1. Re-read under the lock.
        let Some((job, coords, pending)) = self.live_remote(job_id)? else {
            self.live.drop_state(job_id);
            self.memory.forget(job_id);
            return Ok(StatusStep::NotLive);
        };
        if pending.requested_at.is_none() {
            return Ok(StatusStep::RowChanged);
        }
        self.deliver_pending(&job, &coords)
    }

    /// Steps 2–6 for `job`, read remote-live at `coords` with its cancel requested — by
    /// [`Self::deliver_cancel`]'s step 1, or by the status step's own read (o22.13.1).
    pub(super) fn deliver_pending(&self, job: &Job, coords: &RemoteCoordinates) -> Result<StatusStep, AppError> {
        let tracker = ContactRunner::new(self.runner);
        Poller { runner: &tracker, ..*self }.deliver(job, coords, &tracker)
    }

    /// Steps 2–6. `tracker` is `self.runner`: it tells a call that failed in transport from one the
    /// server answered with a failure.
    fn deliver(&self, job: &Job, coords: &RemoteCoordinates, tracker: &ContactRunner<'_>) -> Result<StatusStep, AppError> {
        let job_id = job.id.as_str();
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
        // and classifies, but sends nothing — `mkjob` included — until Retry cancel (o22.14.1).
        let stopped = || self.memory.get(job_id).cancel_failures >= MAX_CANCEL_FAILURES;

        let root = recorded_root(coords, job_id)?;

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
            match label_call(self.runner, coords) {
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
            let outcome = match collect_and_classify(self.runner, coords, &root) {
                Ok(outcome) => outcome,
                Err(why) => return Ok(StatusStep::CheckFailed(why)),
            };
            if ended_before_cancel(&outcome) {
                return self.record_ended_first(job_id, coords, outcome);
            }
            // `.cancelled` is already on the server (an earlier send whose reply was lost): the
            // cancel is delivered, stopped or not, and nothing is re-sent (o22.11.4).
            if cancelled_on_server(&outcome) {
                return self.record_delivered(job_id, coords, outcome);
            }
            if stopped() {
                return Ok(StatusStep::CancelStopped { seen: Some(Seen::Outcome(outcome)), unready: None });
            }
        }

        // 5. Mark, then send. The mark's lock is released before `cancel.sh` is spawned.
        if !self.mark_sent(job_id, coords)? {
            return Ok(StatusStep::RowChanged);
        }
        match cancel_call(self.runner, coords, &root) {
            Err(why) => return Ok(failed(why)),
            Ok((0, _)) => {}
            Ok((rc, stderr)) => {
                self.memory.cancel_failed(job_id);
                return Ok(StatusStep::CancelNotSent(format!("cancel.sh exited {rc}: {}", tail(&stderr))));
            }
        }

        // Delivered: the server decides what it is now.
        let outcome = match collect_and_classify(self.runner, coords, &root) {
            Ok(outcome) => outcome,
            Err(why) => return Ok(StatusStep::CheckFailed(format!(".cancelled is on the server, but {why}"))),
        };
        self.record_delivered(job_id, coords, outcome)
    }

    /// Step 5's mark: `cancel_sent_at` set in one locked write that re-checks the request — `false`
    /// when an Undo won (or the row is no longer this live remote job), and then nothing is sent.
    /// The column changed, so `job:status` follows once the lock is released (o22.14.5): the view
    /// drops Undo before `cancel.sh` runs.
    fn mark_sent(&self, job_id: &str, coords: &RemoteCoordinates) -> Result<bool, AppError> {
        let status = {
            let conn = self.db.lock()?;
            let marked = conn.execute(
                "UPDATE jobs SET cancel_sent_at = datetime('now') \
                 WHERE id = ?1 AND remote_job_dir = ?2 AND status IN ('queued', 'running', 'cancelling') \
                 AND cancel_requested_at IS NOT NULL",
                params![job_id, coords.job_dir],
            )?;
            if marked == 0 {
                return Ok(false);
            }
            get_job_conn(&conn, job_id)?.status
        };
        self.sink.status(job_id, status);
        Ok(true)
    }

    /// Step 4's "ended first": both columns cleared (the v21 CHECK refuses `cancel_sent_at` without
    /// `cancel_requested_at`) and a pending submit with them (o23.5: the job is not waiting for a
    /// slot any more; this `queued`-keeping write is pinned by a test, the CHECK cannot catch it),
    /// the re-send bound and the refusal counter restarted, and the note remembered for the job's
    /// next `finalise` (o22.13.2) — on the click's path and the poller's alike. The columns changed,
    /// so `job:status` follows once the lock is released (o22.14.5).
    fn record_ended_first(&self, job_id: &str, coords: &RemoteCoordinates, outcome: Outcome) -> Result<StatusStep, AppError> {
        let status = {
            let conn = self.db.lock()?;
            let cleared = conn.execute(
                "UPDATE jobs SET cancel_requested_at = NULL, cancel_sent_at = NULL, submit_pending_since = NULL \
                 WHERE id = ?1 AND remote_job_dir = ?2 AND status IN ('queued', 'running', 'cancelling')",
                params![job_id, coords.job_dir],
            )?;
            if cleared == 0 {
                return Ok(StatusStep::RowChanged);
            }
            get_job_conn(&conn, job_id)?.status
        };
        self.memory.clear_cancel_failures(job_id);
        self.memory.end_pending(job_id);
        self.memory.set_ended_before_cancel(job_id);
        self.sink.status(job_id, status);
        Ok(StatusStep::EndedBeforeCancel(outcome))
    }

    /// rc 0's write (o22.4.5): the delivered verdict recorded and both columns cleared in **one**
    /// locked write, if the row is still this live remote job — with a pending submit (o23.5, both
    /// branches: a delivered cancel ends the wait; the unchanged-status branch keeps `queued`, so a
    /// test pins it, the CHECK cannot) and its refusal counter. Row 3 `Cancelling` becomes status
    /// `cancelling` with its text and sets the sweep baseline (o22.13.4); every other verdict changes
    /// no status here (a terminal verdict's write is the next ordinary step's, and NULLs the columns
    /// again). A write that matched no row is `RowChanged`. Both branches change the columns, so
    /// each emits `job:status` — the status written, or the unchanged one — once the lock is
    /// released (o22.14.5).
    fn record_delivered(&self, job_id: &str, coords: &RemoteCoordinates, outcome: Outcome) -> Result<StatusStep, AppError> {
        let status = {
            let conn = self.db.lock()?;
            if live_row(&conn, job_id, coords)?.is_none() {
                return Ok(StatusStep::RowChanged);
            }
            let written = if matches!(outcome, Outcome::Cancelling { .. }) {
                conn.execute(
                    "UPDATE jobs SET status = 'cancelling', error_message = ?1, cancel_requested_at = NULL, \
                     cancel_sent_at = NULL, submit_pending_since = NULL \
                     WHERE id = ?2 AND remote_job_dir = ?3 AND status IN ('queued', 'running', 'cancelling')",
                    params![shown_message(&outcome), job_id, coords.job_dir],
                )?
            } else {
                conn.execute(
                    "UPDATE jobs SET cancel_requested_at = NULL, cancel_sent_at = NULL, submit_pending_since = NULL \
                     WHERE id = ?1 AND remote_job_dir = ?2 AND status IN ('queued', 'running', 'cancelling')",
                    params![job_id, coords.job_dir],
                )?
            };
            if written == 0 {
                return Ok(StatusStep::RowChanged);
            }
            get_job_conn(&conn, job_id)?.status
        };
        self.memory.clear_cancel_failures(job_id);
        self.memory.end_pending(job_id);
        if let Outcome::Cancelling { session } = outcome {
            self.memory.note_cancelling(job_id, session);
        }
        self.sink.status(job_id, status);
        Ok(StatusStep::CancelDelivered(outcome))
    }
}

#[cfg(test)]
pub(super) mod tests;
