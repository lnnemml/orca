//! **The one automatic re-enqueue** (ADR-024 Decision d, o25.3–4 as o25.14 settles them): a job the
//! classifier reads as row 11 `ReEnqueue` — nothing of it ever ran, and no queue holds it (a restart
//! or `tsp -K` dropped the queue, or a wrapper crashed before `.started`) — is put back in its slot's
//! queue **once**, by the status step that saw it, under the job's in-flight guard.
//!
//! **The counter is the server's** (o25.3): `submit.sh reenqueue` claims `.reenqueue-1` (no-clobber
//! `ln -sT`) under the account lock, **after** the slot check and **before** the enqueue, and the
//! collector reports the claim, so the classifier's next `ReEnqueue` is a `Failed { WrapperNeverStarted
//! }` (the evidence path, then `failed`). A lost reply, a laptop crash, two racing steps: a second call
//! finds the claim and refuses. No schema version.
//!
//! **The step** ([`Poller::re_enqueue`]):
//! 1. **the plan, decided locally without writing** (`ssh_backend::reenqueue_plan`, o25.14.1–3): the
//!    profile must be a run target — if not, the job **waits** (no ssh call, not counted, "re-test
//!    the server profile to continue"); the input is derived in memory with the profile's current
//!    mask and compared, digest for digest, with the local job dir — different → `failed` at once
//!    ("the core count of the profile changed"), the local dir untouched, **no call**; a local DB or
//!    IO error is a `CheckFailed`; any other plan refusal is `failed`;
//! 2. **ready the server** (prepare → install if needed → prepare); no answer → `CheckFailed`;
//! 3. **`submit.sh reenqueue`** with the local list. [`decide`] reads its outcome:
//!
//! | outcome | the row | counted |
//! |---|---|---|
//! | enqueued → [`StatusStep::ReEnqueued`] | `queued`; the waiting state and the count end | — |
//! | slot busy, slot check error, lock busy → [`StatusStep::ReEnqueueWaiting`] | `queued`; nothing claimed | toward the notice ([`REENQUEUE_NOTICE_AT`]) |
//! | the profile not a run target → [`StatusStep::ReEnqueueWaiting`] | `queued`; no call | no |
//! | a lost reply (the call's own contact `Failed`), a ready with no answer → `CheckFailed` | unchanged | no |
//! | any other answered refusal — an upload mismatch, `refused-kup` (which clears the profile's stamp, o13.3), a marker or a live row, an install refusal, failed after the claim, an unreadable reply → [`StatusStep::ReEnqueueRefused`] | `queued` | **yes**: the [`MAX_REENQUEUE_REFUSALS`]-th is `failed` with its text |
//!
//! The waiting state (the record's `re_enqueue_waiting`) and the counted refusals live in
//! `PollerMemory`, per launch; both end on success and on any decided outcome other than
//! `ReEnqueue` (`poller::step_effect`) — which is how "a refusal the next classify re-decides is not
//! counted" is mechanised (o25.14.4). The 5.4c pending-submit flag is never set here.
//!
//! **The second never-start is confirmed before it is terminal** (o25.15,
//! [`Poller::never_started_again`]): with the claim present, the job is written `failed` only on a
//! second consecutive never-start pass at least [`NEVER_STARTED_CONFIRM`] (45 s, above the 40 s cap
//! on a `submit.sh` call under the lock) after the first, so a re-enqueue still running on the server
//! when its claim was read cannot leave an ORCA run nobody watches.

use serde::Serialize;

use super::pending::Wait;
use super::evidence::Evidence;
use super::plan::{MAX_REENQUEUE_REFUSALS, NEVER_STARTED_CONFIRM, REENQUEUE_NOTICE_AT};
use super::{finalise, shown_message, Poller, StatusStep, Written};
use crate::commands::server_profiles::clear_verified;
use crate::error::AppError;
use crate::models::job::{Job, JobStatus};
use crate::remote::classify::Outcome;
use crate::remote::contact::{Contact, ContactRunner};
use crate::remote::submit::Refusal;
use crate::ssh_backend::{
    not_claimed, ready_server, reenqueue_plan, submit_call, ReEnqueuePlan, ReEnqueueRefusal, RemoteCoordinates,
    SubmitOutcome, SubmitPlan, SubmitStep,
};

/// Why a re-enqueue waits: `{ "kind": "slot", "detail": <Wait> }` (slot busy, slot check error, lock
/// busy — o23's [`Wait`] kinds, the blockers titled the same way) or `{ "kind": "unverified",
/// "detail": "<why>" }` (o25.14.3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", content = "detail", rename_all = "snake_case")]
pub enum ReEnqueueWait {
    /// A transient refusal of the server's slot check or account lock: nothing was claimed.
    Slot(Wait),
    /// The profile is not a run target: no call was made.
    Unverified(String),
}

impl ReEnqueueWait {
    /// Whether this wait counts toward the notice: the transient refusals do; the unverified wait is
    /// not counted and is told at once (it needs a person, o25.14.3).
    pub(crate) fn is_counted(&self) -> bool {
        matches!(self, ReEnqueueWait::Slot(_))
    }

    /// The record's notice for this wait.
    pub fn notice(&self) -> String {
        match self {
            ReEnqueueWait::Slot(wait) => format!("The re-enqueue of a job that never started waits. {}", wait.notice()),
            ReEnqueueWait::Unverified(why) => {
                format!("The re-enqueue of a job that never started waits: re-test the server profile to continue ({why}).")
            }
        }
    }
}

/// The row's text for a never-start with the claim that is not yet confirmed (o25.15): neutral —
/// the job is not ended, and nothing says it failed.
pub const NEVER_STARTED_UNCONFIRMED: &str = "Nothing of the job has run on the server since its one re-enqueue, and no \
     queue holds it; a check at least 45 s later confirms this before the job is ended.";

/// The record's view of a waiting re-enqueue (o25.4): the transient refusals this launch, and — from
/// the [`REENQUEUE_NOTICE_AT`]-th, or at once for an unverified profile — the notice, and the wait
/// it names (`last`), so the view can list a busy slot's blockers as it does a pending submit's.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReEnqueueWaiting {
    pub refusals: u32,
    pub notified: bool,
    pub notice: Option<String>,
    /// The most recent wait.
    pub last: ReEnqueueWait,
}

impl ReEnqueueWaiting {
    /// From the memory's count and last wait; `None` when the re-enqueue does not wait.
    pub(crate) fn from_memory(waits: u32, last: Option<&ReEnqueueWait>) -> Option<ReEnqueueWaiting> {
        let last = last?;
        let notified = !last.is_counted() || waits >= REENQUEUE_NOTICE_AT;
        Some(ReEnqueueWaiting { refusals: waits, notified, notice: notified.then(|| last.notice()), last: last.clone() })
    }
}

/// The pure decision over one `submit.sh reenqueue` outcome, given the call's own contact (o25.4,
/// o25.14.4). Outcome first, never a reason's text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Decision {
    Enqueued(u64),
    /// Transient: nothing claimed; counted toward the notice only.
    Wait(Wait),
    /// The reply was lost (no answer): not counted, the next step re-decides.
    Lost(String),
    /// An answered refusal outside the transient set: counted toward [`MAX_REENQUEUE_REFUSALS`].
    Refused(String),
}

pub(crate) fn decide(outcome: &SubmitOutcome, contact: Option<&Contact>) -> Decision {
    match outcome {
        SubmitOutcome::Enqueued { tsp_id } => Decision::Enqueued(*tsp_id),
        SubmitOutcome::NotClaimed { step: SubmitStep::Submit, refusal } => match refusal {
            Refusal::Blocked { blockers } => Decision::Wait(Wait::SlotBusy {
                blockers: blockers.iter().map(|b| super::pending::ShownBlocker { blocker: b.clone(), title: None }).collect(),
            }),
            Refusal::SlotCheckFailed(why) => Decision::Wait(Wait::SlotCheckFailed(why.clone())),
            Refusal::LockBusy { openers } => Decision::Wait(Wait::LockBusy { openers: openers.clone() }),
            // The plan binding already proved the local dir is the current derivation, so the server's
            // copy differing is not "the profile changed" (o25.14.1): an ordinary counted refusal.
            Refusal::UploadMismatch { .. } | Refusal::Other(_) => Decision::Refused(refusal.to_string()),
        },
        SubmitOutcome::NotClaimed { refusal, .. } => Decision::Refused(refusal.to_string()),
        SubmitOutcome::KillUserProcesses { .. } | SubmitOutcome::FailedAfterClaim { .. } => {
            Decision::Refused(outcome.failure().unwrap_or_default())
        }
        SubmitOutcome::Unknown { reason } => match contact {
            // No answer: the next step re-decides from the server (the claim tells).
            Some(Contact::Failed(_)) => Decision::Lost(reason.clone()),
            // Answered but unreadable, or no call made (a value refused before ssh): counted.
            Some(Contact::Ok) | None => Decision::Refused(reason.clone()),
        },
    }
}

impl Poller<'_> {
    /// The status step's action on a `ReEnqueue` verdict for `job` (the module doc).
    pub(super) fn re_enqueue(&self, job: &Job, coords: &RemoteCoordinates) -> Result<StatusStep, AppError> {
        // 1. The plan, decided locally without writing (o25.14.1).
        let plan = {
            let conn = self.db.lock()?;
            reenqueue_plan(&conn, self.data_dir, &job.id, coords)
        };
        let plan = match plan {
            Ok(ReEnqueuePlan::Send(plan)) => plan,
            Ok(ReEnqueuePlan::Changed(changed)) => return self.fail_reenqueue(job, coords, &changed.message()),
            Err(ReEnqueueRefusal::Unverified(why)) => return self.reenqueue_waits(job, coords, ReEnqueueWait::Unverified(why)),
            Err(ReEnqueueRefusal::Local(e)) => {
                return Ok(StatusStep::CheckFailed(format!("the re-enqueue could not be prepared on this machine: {e}")))
            }
            Err(ReEnqueueRefusal::Refused(e)) => {
                return self.fail_reenqueue(
                    job,
                    coords,
                    &format!("Never started on the server, and it cannot be re-enqueued: {e}; start it again as a new job."),
                )
            }
        };

        // 2. Ready the server; the failing call is the last one made, its own contact says whether
        // the server answered.
        let ready = ContactRunner::new(self.runner);
        if let Err((step, why)) = ready_server(&ready, &coords.host, &plan.root, &coords.job_dir) {
            return match ready.last() {
                Some(Contact::Ok) => self.reenqueue_refused(job, coords, &plan, &not_claimed(step, why.clone()), why),
                _ => Ok(StatusStep::CheckFailed(why)),
            };
        }

        // 3. The one re-enqueue call, through its own tracker.
        let call = ContactRunner::new(self.runner);
        let outcome = submit_call(&call, &plan);
        match decide(&outcome, call.last().as_ref()) {
            Decision::Enqueued(tsp_id) => self.reenqueued(job, coords, tsp_id),
            Decision::Wait(wait) => {
                let wait = self.with_titles(wait, &plan.root)?;
                self.reenqueue_waits(job, coords, ReEnqueueWait::Slot(wait))
            }
            Decision::Lost(why) => Ok(StatusStep::CheckFailed(why)),
            Decision::Refused(why) => self.reenqueue_refused(job, coords, &plan, &outcome, why),
        }
    }

    /// Enqueued again: the row stays `queued` with a note, and the waiting state ends (the step's
    /// effect, `step_effect`).
    fn reenqueued(&self, job: &Job, coords: &RemoteCoordinates, tsp_id: u64) -> Result<StatusStep, AppError> {
        let note = format!(
            "Nothing of the job ran on the server (its queue entry was lost); it was re-enqueued once (tsp id {tsp_id})."
        );
        Ok(match self.write_message(&job.id, coords, Some(&note))? {
            Written::No => StatusStep::RowChanged,
            Written::Yes(emit) => {
                if let Some(status) = emit {
                    self.sink.status(&job.id, status);
                }
                StatusStep::ReEnqueued { tsp_id }
            }
        })
    }

    /// A wait (transient, or the unverified profile): the row shows the `ReEnqueue` text, the memory
    /// remembers the wait — after the write, so a changed row counts nothing.
    fn reenqueue_waits(&self, job: &Job, coords: &RemoteCoordinates, wait: ReEnqueueWait) -> Result<StatusStep, AppError> {
        match self.write_message(&job.id, coords, shown_message(&Outcome::ReEnqueue).as_deref())? {
            Written::No => Ok(StatusStep::RowChanged),
            Written::Yes(emit) => {
                self.memory.note_reenqueue_wait(&job.id, wait.clone());
                if let Some(status) = emit {
                    self.sink.status(&job.id, status);
                }
                Ok(StatusStep::ReEnqueueWaiting(wait))
            }
        }
    }

    /// A counted refusal (o25.14.4). The [`MAX_REENQUEUE_REFUSALS`]-th is terminal `failed` with its
    /// text; before it the row says which refusal it was. A `refused-kup` clears the profile's stamp
    /// in the same locked write (o13.3), so the next step waits for a connection test (o25.14.3).
    fn reenqueue_refused(
        &self,
        job: &Job,
        coords: &RemoteCoordinates,
        plan: &SubmitPlan,
        outcome: &SubmitOutcome,
        why: String,
    ) -> Result<StatusStep, AppError> {
        let refusals = self.memory.get(&job.id).reenqueue_refusals + 1;
        let kup = matches!(outcome, SubmitOutcome::KillUserProcesses { .. });
        if kup {
            clear_verified(&*self.db.lock()?, &plan.profile_id)?;
        }
        if refusals >= MAX_REENQUEUE_REFUSALS {
            return self.fail_reenqueue(
                job,
                coords,
                &format!(
                    "Never started on the server, and its re-enqueue was refused {refusals} times; the last refusal: \
                     {why}. Start it again as a new job."
                ),
            );
        }
        let message = format!(
            "Nothing of the job ever ran on the server; its re-enqueue was refused ({refusals} of \
             {MAX_REENQUEUE_REFUSALS}; the next check tries again): {why}"
        );
        match self.write_message(&job.id, coords, Some(&message))? {
            Written::No => Ok(StatusStep::RowChanged),
            Written::Yes(emit) => {
                let refusals = self.memory.count_reenqueue_refusal(&job.id);
                if let Some(status) = emit {
                    self.sink.status(&job.id, status);
                }
                Ok(StatusStep::ReEnqueueRefused { refusals, why })
            }
        }
    }

    /// A never-start **with the claim** (`Failed { WrapperNeverStarted }`, ADR-024 o25.15): terminal
    /// only when confirmed. The claim precedes the enqueue under the account lock, and the collector
    /// does not take that lock, so a re-enqueue call still running on the server (its reply lost) can
    /// have claimed and not yet enqueued when the collect read the claim. So the first pass of a run
    /// is shown, not terminal, with a neutral text; a later consecutive pass less than
    /// [`NEVER_STARTED_CONFIRM`] after it is shown again; one at least that long after it — when any
    /// such call has ended — takes the evidence path and writes `failed` (o25.5). Any other verdict
    /// ends the run (`poller::step_effect`).
    ///
    /// The 45 s bound the gap between the two **server reads** (o25.15.1): the first pass is
    /// remembered at `read_until` — **after** its collect — and a confirming pass counts only when its
    /// collect **started** (`read_from`, before `collect_readying`) at least 45 s after that.
    pub(super) fn never_started_again(
        &self,
        job: &Job,
        coords: &RemoteCoordinates,
        outcome: Outcome,
        read_from: std::time::Instant,
        read_until: std::time::Instant,
    ) -> Result<StatusStep, AppError> {
        match self.memory.note_never_started(&job.id, read_until) {
            None => self.show_never_started(job, coords, outcome),
            Some(first) if read_from.saturating_duration_since(first) < NEVER_STARTED_CONFIRM => {
                self.show_never_started(job, coords, outcome)
            }
            Some(_) => self.fetch_evidence(job, coords, Evidence::NeverStarted, outcome),
        }
    }

    /// An unconfirmed never-start: the row says so, neutrally, and stays live.
    fn show_never_started(&self, job: &Job, coords: &RemoteCoordinates, outcome: Outcome) -> Result<StatusStep, AppError> {
        Ok(match self.write_message(&job.id, coords, Some(NEVER_STARTED_UNCONFIRMED))? {
            Written::No => StatusStep::RowChanged,
            Written::Yes(emit) => {
                if let Some(status) = emit {
                    self.sink.status(&job.id, status);
                }
                StatusStep::Shown(outcome)
            }
        })
    }

    /// The re-enqueue's terminal `failed`, without evidence (nothing ran; the server's dir is as the
    /// first submit left it): `finalize_job_conn` through `finalise` (its live-row re-check), the job
    /// forgotten, `job:status`. The local job dir is not touched.
    fn fail_reenqueue(&self, job: &Job, coords: &RemoteCoordinates, message: &str) -> Result<StatusStep, AppError> {
        let Some(status) = finalise(self.db, &job.id, coords, JobStatus::Failed, Some(message), None, None, None)? else {
            return Ok(StatusStep::RowChanged);
        };
        self.memory.forget(&job.id);
        self.live.drop_state(&job.id);
        self.sink.status(&job.id, status);
        Ok(StatusStep::Finalised { outcome: Outcome::ReEnqueue, status, drain_error: None })
    }
}

#[cfg(test)]
mod tests;
