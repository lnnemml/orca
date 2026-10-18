//! The status step of a **`cancelling`** job (ADR-024 l row 3, o22.5, o22.13.3–4): the cancel is
//! on the server and some of the job's processes still run, so every step **sweeps** — until the
//! bound stops it.
//!
//! **A sweep** (while [`MAX_SWEEPS_WITHOUT_PROGRESS`] is not reached):
//! 1. **ready the server** as a delivery does (prepare → install if needed → prepare,
//!    `ssh_backend::ready_server`), so after an app upgrade — whose sha-named scripts read "not
//!    installed" — a sweep reinstalls them, `collect.sh` included. A ready **without an answer** is a
//!    failed check; an **answered** ready failure sends nothing and goes straight to step 3;
//! 2. **`cancel.sh cancel`** (idempotent on a dir that holds `.cancelled`, A1);
//! 3. **collect + `classify`**.
//!
//! **One counting rule** ([`count_sweep`](super::plan::PollerMemory::count_sweep), one operation
//! under the memory's lock): a
//! sweep in which the server answered any call counts one sweep without progress, unless its
//! classify shows the job session **below the smallest size seen** (the baseline, `min_session`) —
//! that is progress: the baseline falls and the count restarts. A step none of whose calls got an
//! answer is `CheckFailed` and counts nothing. So a sweep that may have written to the server is
//! always counted, whether the failure was the script's, the collect's or the classify's. The
//! delivery's `cancel_failures` is never touched here (o22.11.3).
//!
//! **The result** is `Swept { outcome, error }` (`outcome`: the classify's `Cancelling`, `None` when
//! nothing could be classified; `error`: the answered failure). A sweep whose classify shows anything
//! else — row 4, row 1 with `.cancelled`, row 2 … — is acted on like an ordinary step's outcome in the
//! same step (`Poller::act_on`), and that is the step's result.
//!
//! **Stopped** (the bound reached): no ready, no install, no `cancel.sh` — a plain collect + classify
//! whose result is `Shown(outcome)` (a collect failure is `CheckFailed`), changing neither the count
//! nor the baseline, so a shrink seen while stopped does not resume the sweeps; a terminal outcome
//! still ends the job. Cancel clicked on the job restarts the count (o22.12.1), and the next step
//! sweeps and readies again.

use super::plan::MAX_SWEEPS_WITHOUT_PROGRESS;
use super::{cancelling_message, Poller, StatusStep, Written};
use crate::error::AppError;
use crate::models::job::Job;
use crate::remote::classify::Outcome;
use crate::remote::contact::ContactRunner;
use crate::ssh_backend::{cancel_call, collect_and_classify, ready_server, tail, RemoteCoordinates};

impl Poller<'_> {
    /// The status step of the `cancelling` job `job` (the module doc).
    pub(super) fn sweep(&self, job: &Job, coords: &RemoteCoordinates, root: &str) -> Result<StatusStep, AppError> {
        if self.memory.get(&job.id).sweeps_without_progress >= MAX_SWEEPS_WITHOUT_PROGRESS {
            return self.collect_only(job, coords, root);
        }
        let tracker = ContactRunner::new(self.runner);
        let traced = Poller { runner: &tracker, ..*self };

        // 1–2. Ready, then send. An answered ready failure sends nothing; one without any answer is
        // a failed check, counting nothing.
        let mut error = match ready_server(traced.runner, &coords.host, root, &coords.job_dir) {
            Err((_, why)) if !tracker.answered() => return Ok(StatusStep::CheckFailed(why)),
            Err((_, why)) => Some(why),
            Ok(()) => match cancel_call(traced.runner, coords, root) {
                Ok((0, _)) => None,
                Ok((rc, stderr)) => Some(format!("cancel.sh exited {rc}: {}", tail(&stderr))),
                Err(why) => Some(why),
            },
        };

        // 3. Collect + classify, whatever step 2 did: the cancel may have run either way.
        let outcome = match collect_and_classify(traced.runner, coords, root) {
            Ok(outcome) => Some(outcome),
            Err(why) => {
                error = Some(match error {
                    Some(earlier) => format!("{earlier}; then {why}"),
                    None => why,
                });
                None
            }
        };

        // The server answered the ready step, so this sweep counts (o22.13.3).
        let session = match &outcome {
            Some(Outcome::Cancelling { session }) => Some(*session),
            _ => None,
        };
        let count = self.memory.count_sweep(&job.id, session);
        match outcome {
            Some(Outcome::Cancelling { session }) => {
                let stopped = count >= MAX_SWEEPS_WITHOUT_PROGRESS;
                self.show_cancelling(job, coords, session, stopped, StatusStep::Swept { outcome: Some(Outcome::Cancelling { session }), error })
            }
            Some(other) => traced.act_on(job, coords, other),
            None => Ok(StatusStep::Swept { outcome: None, error }),
        }
    }

    /// A stopped job's step: collect + classify only (the module doc).
    fn collect_only(&self, job: &Job, coords: &RemoteCoordinates, root: &str) -> Result<StatusStep, AppError> {
        let outcome = match collect_and_classify(self.runner, coords, root) {
            Ok(outcome) => outcome,
            Err(why) => return Ok(StatusStep::CheckFailed(why)),
        };
        match outcome {
            Outcome::Cancelling { session } => self.show_cancelling(job, coords, session, true, StatusStep::Shown(outcome)),
            other => self.act_on(job, coords, other),
        }
    }

    /// Write a `cancelling` row's text — whether the sweeps have stopped — guarded on the row still
    /// being this live remote job (`RowChanged` otherwise), then return `step`.
    fn show_cancelling(
        &self,
        job: &Job,
        coords: &RemoteCoordinates,
        session: usize,
        stopped: bool,
        step: StatusStep,
    ) -> Result<StatusStep, AppError> {
        match self.write_message(&job.id, coords, Some(&cancelling_message(session, stopped)))? {
            Written::No => Ok(StatusStep::RowChanged),
            Written::Yes(emit) => {
                if let Some(status) = emit {
                    self.sink.status(&job.id, status);
                }
                Ok(step)
            }
        }
    }
}

#[cfg(test)]
mod tests;
