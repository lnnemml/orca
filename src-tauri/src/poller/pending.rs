//! The pending submit's re-check (ADR-024 o23, unit 5.4c Part A): a submit the user started that
//! the slot check refused (slot busy, slot check error) is **pending** (`jobs.submit_pending_since`,
//! v22) and is re-checked on every status step until the slot frees — and then it is submitted, by
//! itself. Nothing is ever freed by force (m2, rule #8).
//!
//! **Where it runs** (o23.10): in `run_status`, inside `record_step` (so every call writes the
//! record's `last_contact`, o23.9), **after** the pending-cancel branch — a cancel is delivered
//! first (o22.13.1), and delivery's not-on-server branch ends the pending state — and **before** the
//! fetch bound's short-circuit and the label call. It applies only to a row with the flag set.
//!
//! **The re-check is the submit itself** (o23.3):
//! 1. the plan derived exactly as a retry's (`ssh_backend::pending_plan`), minus the label call and
//!    the upload; the local dir rewritten only where the bytes differ (o23.13.5);
//! 2. the ready step (prepare → install if needed → prepare);
//! 3. `submit.sh`. Its step-4 post-condition proves the server copy is the plan; only on an **upload
//!    mismatch** does the step upload once and run `submit.sh` once more — and not in the step right
//!    after a lost reply (o23.13.6). While blocked, `submit.sh` refuses before `.tsp-out` and the
//!    claim, so it writes nothing in the job dir.
//!
//! **What each outcome does** (o23.6, o23.13.1–4), decided by [`decide`] — outcome first; "no
//! answer" only for the calls before `submit.sh` and the one re-upload, from that call's **own**
//! contact (`Failed`, or none for rsync 12):
//!
//! | outcome | the pending state | counted |
//! |---|---|---|
//! | no answer before `submit.sh` or from the re-upload; a DB/IO error here (`CheckFailed`) | stays | no |
//! | `submit.sh` lost (timeout, ssh 255: `Unknown` with a `Failed` contact) → [`Recheck::Lost`] | stays; no upload next step | no |
//! | slot busy, slot check error (a bounded child's timeout too), lock busy, an unreadable answer → [`Recheck::Waiting`] | stays | **yes** |
//! | an upload mismatch in the step after a lost reply → [`Recheck::Deferred`] | stays | no |
//! | enqueued → [`Recheck::Enqueued`] | **ends**: an ordinary queued job | — |
//! | a local refusal of the plan (the profile unverified, host/root/socket moved), any other answered refusal (prepare, install, the upload or an upload mismatch after its one retry, values, markers, a slot row, `KillUserProcesses`, failed after the claim) → [`Recheck::Ended`] | **ends**: the row shows why, with the manual Retry submit | — |
//!
//! **The counter** (o23.4) is `PollerMemory`'s, per launch: one atomic `count_refusal` per counted
//! answer, reset only when the pending state ends (and by a restart). From the
//! [`PENDING_NOTICE_AT`]-th refusal the record's notice names why it waits, in the words of the most
//! recent kind ([`Wait::notice`]).
//!
//! **Writes** (rule #9: each acts on its row count; each emits `job:status` after its lock): the
//! attempt's `record_attempt` (the flag `Set`, kept or cleared with the message, in one statement),
//! and, for a local refusal of the plan, its own guarded NULL before it. Lock order: the database is
//! never held across a call, nor under the memory lock.

use rusqlite::{params, OptionalExtension};
use serde::Serialize;

use super::plan::PENDING_NOTICE_AT;
use super::{Poller, StatusStep};
use crate::commands::jobs::get_job_conn;
use crate::error::AppError;
use crate::models::job::Job;
use crate::remote::contact::{Contact, ContactRunner};
use crate::remote::submit::{Blocker, Refusal};
use crate::ssh_backend::{
    not_claimed, pending_plan, ready_server, record_attempt, submit_call, upload, PendingWrite, RemoteCoordinates,
    SubmitOutcome, SubmitPlan, SubmitStep,
};

/// A blocker as the record shows it (o23.7): by the title of the local job whose recorded job dir it
/// names — only a dir `<root>/jobs/<id>` of that job — and by its pid and cores, or its tsp row and
/// socket, otherwise.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ShownBlocker {
    pub blocker: Blocker,
    /// The local job's title; `None`: the blocker is no job of this database.
    pub title: Option<String>,
}

impl ShownBlocker {
    /// The words the notice uses for it.
    pub fn shown(&self) -> String {
        match &self.title {
            Some(title) => format!("\"{title}\""),
            None => self.blocker.fallback(),
        }
    }
}

/// Why a pending submit still waits, counted toward its notice (o23.4, o23.13.1): `{ "kind":
/// "<snake_case>", "detail": … }`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", content = "detail", rename_all = "snake_case")]
pub enum Wait {
    /// The slot check found live work on the mask.
    SlotBusy { blockers: Vec<ShownBlocker> },
    /// The slot could not be checked.
    SlotCheckFailed(String),
    /// Another submit holds the account lock; the lock file's openers.
    LockBusy { openers: String },
    /// The server answered, but its reply could not be read.
    Unreadable(String),
}

impl Wait {
    /// The record's notice from the [`PENDING_NOTICE_AT`]-th refusal, in this kind's words (o23.7,
    /// o23.13.1).
    pub fn notice(&self) -> String {
        match self {
            Wait::SlotBusy { blockers } => {
                let held: Vec<String> = blockers.iter().map(ShownBlocker::shown).collect();
                format!("Waiting for the slot: held by {}", held.join(", "))
            }
            Wait::SlotCheckFailed(why) => format!("Waiting: the slot could not be checked: {why}"),
            Wait::LockBusy { openers } => format!("Waiting: the account lock is held by {openers}"),
            Wait::Unreadable(why) => format!("Waiting: the server's reply could not be read: {why}"),
        }
    }

    /// Whether this kind is the slot check's own answer — the one that sets the pending flag (o23.5).
    fn is_slot_answer(&self) -> bool {
        matches!(self, Wait::SlotBusy { .. } | Wait::SlotCheckFailed(_))
    }
}

/// What one re-check did (the `StatusStep::SubmitRechecked` detail): `{ "kind": "<snake_case>",
/// "detail": … }`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", content = "detail", rename_all = "snake_case")]
pub enum Recheck {
    /// Enqueued with this tsp id: the pending state ended; the row is an ordinary queued job.
    Enqueued { tsp_id: u64 },
    /// Still pending, one refusal counted.
    Waiting(Wait),
    /// Still pending, nothing counted: `submit.sh` got no reply (the next re-check uploads nothing).
    Lost(String),
    /// Still pending, nothing counted: an upload mismatch in the step right after a lost reply.
    Deferred(String),
    /// The pending state ended: a refusal that needs a person (the row's message says which).
    Ended(String),
}

/// The pure decision over one `submit.sh` outcome of a re-check (o23.6, o23.13.1–6).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Decision {
    Enqueued(u64),
    /// Stays pending, counted (titles not mapped yet).
    Wait(Wait),
    /// Stays pending, not counted, no upload in the next re-check.
    Lost(String),
    /// Upload once, then `submit.sh` once more.
    ReUpload,
    /// Stays pending, not counted.
    Deferred(String),
    /// The pending state ends.
    End,
}

/// What a re-check does with `outcome`, the outcome of its `submit.sh` call, given that call's own
/// `contact` (o23.13.4: a reply that did not parse while the server answered is an unreadable
/// answer; one that never came is lost), whether the previous re-check's reply was lost
/// (`reply_lost_before`, o23.13.6) and whether this step already re-uploaded (`reuploaded`). Outcome
/// first, never by reason text.
pub(crate) fn decide(outcome: &SubmitOutcome, contact: Option<&Contact>, reply_lost_before: bool, reuploaded: bool) -> Decision {
    match outcome {
        SubmitOutcome::Enqueued { tsp_id } => Decision::Enqueued(*tsp_id),
        SubmitOutcome::NotClaimed { step: SubmitStep::Submit, refusal } => match refusal {
            Refusal::Blocked { blockers } => Decision::Wait(Wait::SlotBusy {
                blockers: blockers.iter().map(|b| ShownBlocker { blocker: b.clone(), title: None }).collect(),
            }),
            Refusal::SlotCheckFailed(why) => Decision::Wait(Wait::SlotCheckFailed(why.clone())),
            Refusal::LockBusy { openers } => Decision::Wait(Wait::LockBusy { openers: openers.clone() }),
            // Its one retry did not fix it (only extra files: an rsync without --delete cannot, o23.13.8).
            Refusal::UploadMismatch { .. } if reuploaded => Decision::End,
            Refusal::UploadMismatch { .. } if reply_lost_before => Decision::Deferred(refusal.to_string()),
            Refusal::UploadMismatch { .. } => Decision::ReUpload,
            Refusal::Other(_) => Decision::End,
        },
        SubmitOutcome::NotClaimed { .. } | SubmitOutcome::KillUserProcesses { .. } | SubmitOutcome::FailedAfterClaim { .. } => {
            Decision::End
        }
        SubmitOutcome::Unknown { reason } => match contact {
            Some(Contact::Ok) => Decision::Wait(Wait::Unreadable(reason.clone())),
            Some(Contact::Failed(_)) => Decision::Lost(reason.clone()),
            // No call was made (an argv or a value refused before ssh ran): nothing will change by
            // itself, so it needs a person.
            None => Decision::End,
        },
    }
}

impl Poller<'_> {
    /// The re-check of `job`'s pending submit (the module doc), `since` the flag the step's own read
    /// found, `coords` its recorded coordinates.
    pub(super) fn recheck_submit(&self, job: &Job, coords: &RemoteCoordinates, since: &str) -> Result<StatusStep, AppError> {
        let job_id = job.id.as_str();

        // 1. The plan, as a retry derives it (o23.3.1). A DB or IO error here is this machine's:
        // pending, not counted (o23.13.2); anything else is a refusal of the plan, which ends it.
        let plan = {
            let conn = self.db.lock()?;
            pending_plan(&conn, self.data_dir, job_id, coords)
        };
        let plan = match plan {
            Ok(plan) => plan,
            Err(e @ (AppError::Io(_) | AppError::Database(_))) => {
                return Ok(StatusStep::CheckFailed(format!("the pending submit could not be prepared on this machine: {e}")))
            }
            Err(e) => return self.end_on_plan_refusal(job_id, coords, since, &e),
        };

        // 2. Ready the server. The failing call is the last one made: its own contact says whether
        // the server answered (o23.13.4).
        let ready = ContactRunner::new(self.runner);
        if let Err((step, why)) = ready_server(&ready, &coords.host, &plan.root, &coords.job_dir) {
            return match ready.last() {
                Some(Contact::Ok) => self.record_decision(&plan, since, &not_claimed(step, why), Decision::End),
                _ => Ok(StatusStep::CheckFailed(why)),
            };
        }

        // 3. `submit.sh`; on an upload mismatch one upload and one more `submit.sh` (o23.3.3).
        let reply_lost_before = self.memory.get(job_id).submit_reply_lost;
        let (mut outcome, contact) = self.submit_once(&plan);
        let mut decision = decide(&outcome, contact.as_ref(), reply_lost_before, false);
        if decision == Decision::ReUpload {
            // A fresh tracker: rsync's 12 writes no contact, so `last()` is `None` for it — never an
            // earlier call's answer.
            let up = ContactRunner::new(self.runner);
            if let Err(why) = upload(&up, &plan) {
                return match up.last() {
                    Some(Contact::Ok) => self.record_decision(&plan, since, &not_claimed(SubmitStep::Upload, why), Decision::End),
                    _ => Ok(StatusStep::CheckFailed(why)),
                };
            }
            let (again, contact) = self.submit_once(&plan);
            decision = decide(&again, contact.as_ref(), false, true);
            outcome = again;
        }
        self.record_decision(&plan, since, &outcome, decision)
    }

    /// One `submit.sh` call through its own tracker: the outcome and the call's own contact.
    fn submit_once(&self, plan: &SubmitPlan) -> (SubmitOutcome, Option<Contact>) {
        let call = ContactRunner::new(self.runner);
        let outcome = submit_call(&call, plan);
        (outcome, call.last())
    }

    /// Record a re-check's decision: the blockers' titles (one read), then `record_attempt` with the
    /// flag the decision implies, then the counter — memory last, with no lock held. A write that
    /// matched no row is `RowChanged` and counts nothing.
    fn record_decision(&self, plan: &SubmitPlan, since: &str, outcome: &SubmitOutcome, decision: Decision) -> Result<StatusStep, AppError> {
        let job_id = plan.job_id.as_str();
        let (pending, recheck) = match decision {
            Decision::Enqueued(tsp_id) => (PendingWrite::Clear, Recheck::Enqueued { tsp_id }),
            Decision::Wait(wait) => {
                let write = if wait.is_slot_answer() { PendingWrite::Set } else { PendingWrite::Keep };
                (write, Recheck::Waiting(self.with_titles(wait, &plan.root)?))
            }
            Decision::Lost(why) => (PendingWrite::Keep, Recheck::Lost(why)),
            Decision::Deferred(why) => (PendingWrite::Keep, Recheck::Deferred(why)),
            // `ReUpload` never reaches here (a re-uploaded step decides with `reuploaded`); were it to,
            // the honest reading is a refusal that needs a person, not a silent wait.
            Decision::End | Decision::ReUpload => {
                (PendingWrite::Clear, Recheck::Ended(outcome.failure().unwrap_or_else(|| "the pending submit ended".into())))
            }
        };
        if !record_attempt(self.db, self.sink, plan, outcome, pending)? {
            return Ok(StatusStep::RowChanged);
        }
        match &recheck {
            Recheck::Enqueued { .. } | Recheck::Ended(_) => self.memory.end_pending(job_id),
            Recheck::Waiting(_) => {
                self.memory.count_refusal(job_id);
                self.memory.note_submit_reply(job_id, false);
            }
            Recheck::Lost(_) => self.memory.note_submit_reply(job_id, true),
            Recheck::Deferred(_) => self.memory.note_submit_reply(job_id, false),
        }
        Ok(StatusStep::SubmitRechecked { since: since.to_string(), recheck })
    }

    /// The blockers of a slot-busy wait, each with the title of the local job whose recorded job dir
    /// it names (o23.7). One locked read; every other wait is returned as it is.
    fn with_titles(&self, wait: Wait, root: &str) -> Result<Wait, AppError> {
        let Wait::SlotBusy { blockers } = wait else { return Ok(wait) };
        let conn = self.db.lock()?;
        let blockers = blockers
            .into_iter()
            .map(|shown| {
                let title = local_title(&conn, root, &shown.blocker)?;
                Ok(ShownBlocker { title, ..shown })
            })
            .collect::<Result<Vec<_>, AppError>>()?;
        Ok(Wait::SlotBusy { blockers })
    }

    /// A local refusal of the plan (o23.6, o23.13.3: the profile unverified — an edit of a target
    /// field or another job's `KillUserProcesses` cleared its stamp —, host/root/socket moved, the
    /// input refused): its own guarded write NULLs the flag and says why (o23.13.9), then
    /// `job:status` after the lock. 0 rows: the row changed under the step.
    fn end_on_plan_refusal(&self, job_id: &str, coords: &RemoteCoordinates, since: &str, e: &AppError) -> Result<StatusStep, AppError> {
        let why = format!("The pending submit ended: {e}. Retry submit once this is fixed.");
        let status = {
            let conn = self.db.lock()?;
            let written = conn.execute(
                "UPDATE jobs SET submit_pending_since = NULL, error_message = ?1 \
                 WHERE id = ?2 AND status = 'queued' AND remote_job_dir = ?3 AND submit_pending_since IS NOT NULL",
                params![why, job_id, coords.job_dir],
            )?;
            if written == 0 {
                return Ok(StatusStep::RowChanged);
            }
            get_job_conn(&conn, job_id)?.status
        };
        self.memory.end_pending(job_id);
        self.sink.status(job_id, status);
        Ok(StatusStep::SubmitRechecked { since: since.to_string(), recheck: Recheck::Ended(why) })
    }
}

/// The title of the local job `blocker` names, if its job dir is exactly `<root>/jobs/<id>` and that
/// job's recorded job dir (o23.7): any other dir — outside `<root>/jobs/`, deeper, or a job this
/// database does not hold there — is no local job.
fn local_title(conn: &rusqlite::Connection, root: &str, blocker: &Blocker) -> Result<Option<String>, AppError> {
    let Some(dir) = blocker.job_dir() else { return Ok(None) };
    let prefix = format!("{root}/jobs/");
    let Some(id) = dir.strip_prefix(&prefix).filter(|id| !id.is_empty() && !id.contains('/')) else {
        return Ok(None);
    };
    Ok(conn
        .query_row("SELECT title FROM jobs WHERE id = ?1 AND remote_job_dir = ?2", params![id, dir], |r| r.get(0))
        .optional()?)
}

/// The record's view of a pending submit (o23.10): when it began, the refusals this launch, the most
/// recent counted kind, and — from the [`PENDING_NOTICE_AT`]-th refusal — the notice.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PendingSubmit {
    /// `jobs.submit_pending_since` as the step read it (UTC, laptop clock).
    pub since: String,
    /// Refusals counted this launch, re-read from `PollerMemory` on every apply.
    pub refusals: u32,
    /// The most recent counted refusal; `None` before the first of this launch.
    pub last: Option<Wait>,
    /// `refusals >= PENDING_NOTICE_AT` with a kind to name: the notice is shown.
    pub notified: bool,
    /// The notice, in the words of `last` (o23.7, o23.13.1); `None` before it is due.
    pub notice: Option<String>,
}

impl PendingSubmit {
    pub(crate) fn new(since: &str, last: Option<Wait>) -> PendingSubmit {
        PendingSubmit { since: since.to_string(), refusals: 0, last, notified: false, notice: None }
    }

    /// The counter's value now, and the notice it makes due.
    pub(crate) fn with_refusals(mut self, refusals: u32) -> PendingSubmit {
        self.refusals = refusals;
        self.notice = (refusals >= PENDING_NOTICE_AT).then(|| self.last.as_ref().map(Wait::notice)).flatten();
        self.notified = self.notice.is_some();
        self
    }
}

#[cfg(test)]
mod tests;
