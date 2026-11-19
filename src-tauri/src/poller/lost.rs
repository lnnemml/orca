//! **A `Lost` job** (ADR-024 Decision d, l row 8, o25.1–2): it started on the server and its wrapper
//! is gone without an `.exit_code`. `Lost` is **terminal** and nothing is restarted automatically
//! (Anton): the user starts a new job. Before the terminal write: **sweep, then evidence**.
//!
//! - **The orphans are swept first** ([`Poller::sweep_lost`]). Row 8's `orphans` — the job-session
//!   processes still in the job dir — exist only within the boot that wrote `.started` (an OOM, an
//!   outside kill of the wrapper). They hold the slot's cores, and their result can never pass rule
//!   #6. Each step, while the bound is not reached:
//!   1. **ready the server** (prepare → install if needed → prepare): `cancel.sh sweep` exists only
//!      in the new `cancel` sha. A ready **without any answer** is `CheckFailed`, counting nothing;
//!      an answered ready failure sends nothing and goes on to step 3;
//!   2. **`cancel.sh sweep`**: it re-checks row 8 in shell first (rule #9 at the actor) and never
//!      publishes `.cancelled`. Its "wrapper alive" or "not row 8" exit signals nothing and **is not
//!      counted** (o25.14.6): the step ends and the next classify decides;
//!   3. **collect + `classify`**: an empty session → the evidence path below, in the same step;
//!      orphans left → counted by o22.13.4's rule (`PollerMemory::count_lost_sweep`, its own counter
//!      and baseline, seeded from the first observed orphan count); anything else is acted on like an
//!      ordinary step's verdict (a Cancel that landed meanwhile: d′, `.cancelled` makes it
//!      `cancelled`, never `lost`).
//!
//!   After [`MAX_SWEEPS_WITHOUT_PROGRESS`] sweeps without progress the job is written `lost`
//!   **anyway**, its text naming the survivors; the evidence is then weaker — a survivor may still
//!   write after the LIST check, and the text says so (as o22.6 does).
//! - **The evidence** is o22.6's path (`evidence.rs`): the fetching flag, rsync down + the LIST
//!   check (it never reads `.exit_code`), then the terminal write through `finalize_job_conn` (the
//!   pending columns NULLed), then the drain. With a stale boot, or once the session is empty, the job
//!   dir no longer changes. A failed fetch counts the strikes; the [`super::plan::MAX_FETCH_STRIKES`]-th
//!   writes `lost` without evidence and says so.

use super::evidence::Evidence;
use super::plan::MAX_SWEEPS_WITHOUT_PROGRESS;
use super::remote_state::CheckError;
use super::{Poller, StatusStep, Written};
use crate::error::AppError;
use crate::models::job::Job;
use crate::remote::classify::Outcome;
use crate::remote::contact::ContactRunner;
use crate::ssh_backend::{
    collect_and_classify, ready_server, recorded_root, sweep_call, tail, RemoteCoordinates, SWEEP_NOT_ROW_8,
    SWEEP_WRAPPER_ALIVE,
};

/// The row's text while a `Lost` job's orphans are swept: how many remain, and which sweep this was.
pub fn lost_sweep_message(orphans: usize, sweeps: u32) -> String {
    format!(
        "The job's wrapper died without an exit code, and {orphans} process(es) of the job still run in its \
         directory; they are swept (sweep {sweeps} of {MAX_SWEEPS_WITHOUT_PROGRESS} without progress), then its \
         files are downloaded as evidence and the job is written lost."
    )
}

impl Poller<'_> {
    /// The status step of a job whose verdict is `Lost { orphans, stale_boot }` with orphans left
    /// (the module doc). An empty orphan list never comes here: it is evidence at once.
    pub(super) fn sweep_lost(&self, job: &Job, coords: &RemoteCoordinates, orphans: Vec<u32>, stale_boot: bool) -> Result<StatusStep, AppError> {
        let root = recorded_root(coords, &job.id)?;
        self.memory.seed_lost(&job.id, orphans.len());
        if self.memory.get(&job.id).lost_sweeps >= MAX_SWEEPS_WITHOUT_PROGRESS {
            // The bound was reached by an earlier step whose classify then saw no Lost to finalise.
            return self.finalise_lost(job, coords, stale_boot, orphans);
        }
        let tracker = ContactRunner::new(self.runner);
        let traced = Poller { runner: &tracker, ..*self };
        let failure = |why: String| CheckError::from_contact(why, tracker.last().as_ref());

        // 1–2. Ready, then sweep. An answered ready failure sweeps nothing; one without any answer is
        // a failed check, counting nothing.
        let mut error = match ready_server(&tracker, &coords.host, &root, &coords.job_dir) {
            Err((_, why)) if !tracker.answered() => return Ok(StatusStep::CheckFailed(why)),
            Err((_, why)) => Some(failure(why)),
            Ok(_) => match sweep_call(&tracker, coords, &root) {
                Ok((0, _, _)) => None,
                // Row 8 does not hold at the actor: nothing was signalled, nothing is counted, the next
                // step classifies again (o25.14.6).
                Ok((rc @ (SWEEP_WRAPPER_ALIVE | SWEEP_NOT_ROW_8), stdout, _)) => {
                    let why = format!("cancel.sh sweep signalled nothing (exit {rc}): {}", tail(&stdout));
                    return Ok(StatusStep::LostSwept {
                        outcome: Some(Outcome::Lost { orphans, stale_boot }),
                        error: Some(CheckError::answered(why)),
                    });
                }
                Ok((rc, _, stderr)) => Some(CheckError::answered(format!("cancel.sh sweep exited {rc}: {}", tail(&stderr)))),
                Err(why) => Some(failure(why)),
            },
        };

        // 3. Collect + classify, whatever step 2 did. A joined error takes the later call's flag.
        let outcome = match collect_and_classify(&tracker, coords, &root) {
            Ok(outcome) => Some(outcome),
            Err(why) => {
                let later = failure(why);
                error = Some(match error {
                    Some(earlier) => CheckError { text: format!("{}; then {}", earlier.text, later.text), ..later },
                    None => later,
                });
                None
            }
        };
        match outcome {
            Some(Outcome::Lost { orphans: left, stale_boot }) if left.is_empty() => traced.finalise_lost(job, coords, stale_boot, left),
            Some(Outcome::Lost { orphans: left, stale_boot }) => {
                let count = self.memory.count_lost_sweep(&job.id, Some(left.len()));
                if count >= MAX_SWEEPS_WITHOUT_PROGRESS {
                    return traced.finalise_lost(job, coords, stale_boot, left);
                }
                let step = StatusStep::LostSwept { outcome: Some(Outcome::Lost { orphans: left.clone(), stale_boot }), error };
                self.show_lost_sweep(job, coords, left.len(), count, step)
            }
            Some(other) => {
                self.memory.end_lost(&job.id);
                traced.act_on(job, coords, other)
            }
            None => {
                let count = self.memory.count_lost_sweep(&job.id, None);
                if count >= MAX_SWEEPS_WITHOUT_PROGRESS {
                    return traced.finalise_lost(job, coords, stale_boot, orphans);
                }
                self.show_lost_sweep(job, coords, orphans.len(), count, StatusStep::LostSwept { outcome: None, error })
            }
        }
    }

    /// The evidence path of a `Lost` job, `survivors` the orphans a bounded sweep left (empty: the
    /// session is empty, or the boot is stale).
    pub(super) fn finalise_lost(&self, job: &Job, coords: &RemoteCoordinates, stale_boot: bool, survivors: Vec<u32>) -> Result<StatusStep, AppError> {
        let outcome = Outcome::Lost { orphans: survivors.clone(), stale_boot };
        self.fetch_evidence(job, coords, Evidence::Lost { stale_boot, survivors }, outcome)
    }

    /// Write a sweeping `Lost` row's text, guarded on the row still being this live remote job
    /// (`RowChanged` otherwise), then return `step`.
    fn show_lost_sweep(&self, job: &Job, coords: &RemoteCoordinates, orphans: usize, sweeps: u32, step: StatusStep) -> Result<StatusStep, AppError> {
        match self.write_message(&job.id, coords, Some(&lost_sweep_message(orphans, sweeps)))? {
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
