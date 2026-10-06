//! The loop's body, `AppHandle`-free (unit 5.3 B2 Part B): what one tick decides and how one step
//! runs. The thin shell that sleeps, spawns and emits through the app is `poller::app`.
//!
//! - [`tick`] reads the candidates (one DB query), sweeps what is no longer live, plans, and marks
//!   every due step **in progress before it is handed out** (`PollerMemory::begin_step`), so a later
//!   tick can never start a second step for the same job while one runs.
//! - [`run_step`] runs one step and **always** ends it: the in-progress mark is cleared by a drop
//!   guard, on return, on an early return and while a panic unwinds — a job never stays "in
//!   progress" for the rest of the launch.

use std::time::Instant;

use super::live_log::LiveLog;
use super::plan::{candidates, plan, plan_inputs, sweep, Candidates, Periods, PollerMemory, Step};
use super::{log_step, LogStep, Poller, StatusStep};
use crate::commands::settings::DbState;
use crate::error::AppError;
use crate::ssh_backend::RemoteCoordinates;

/// One step a tick handed out, already marked in progress.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Planned {
    pub job_id: String,
    pub step: Step,
    pub coords: RemoteCoordinates,
}

/// One tick at `now`: the due steps, each marked in progress. The database is locked only for the
/// candidates query. A row that does not read as a remote job is skipped (and reported once per
/// launch, `eprintln!`), so it cannot stop the other jobs' polling.
pub fn tick(db: &DbState, live: &LiveLog, memory: &PollerMemory, periods: &Periods, now: Instant) -> Result<Vec<Planned>, AppError> {
    let Candidates { jobs: candidates, unreadable } = candidates(&*db.lock()?)?;
    for (id, why) in unreadable {
        if memory.first_report(&id) {
            eprintln!("[OrcaStudio] remote poller: job {id} is not polled: {why}");
        }
    }
    sweep(&candidates, live, memory);
    let due = plan(&plan_inputs(&candidates, live, memory), periods, now);
    Ok(due
        .into_iter()
        .filter_map(|(job_id, step)| {
            // The coordinates first: a step is marked in progress only once it can be handed out.
            let coords = candidates.iter().find(|c| c.id == job_id)?.coords.clone();
            memory.begin_step(&job_id, step, now).then_some(Planned { job_id, step, coords })
        })
        .collect())
}

/// What a step did, for the log line and the tests.
#[derive(Debug)]
pub enum StepRan {
    Status(Result<StatusStep, AppError>),
    Log(LogStep),
}

/// Clears a job's in-progress mark when dropped.
pub struct StepEnd<'a> {
    memory: &'a PollerMemory,
    job_id: &'a str,
}

impl<'a> StepEnd<'a> {
    /// Takes over the mark `tick` set for `job_id`.
    pub fn new(memory: &'a PollerMemory, job_id: &'a str) -> Self {
        StepEnd { memory, job_id }
    }
}

impl Drop for StepEnd<'_> {
    fn drop(&mut self) {
        self.memory.end_step(self.job_id);
    }
}

/// Run one planned step with `poller` and end it, whatever happens.
pub fn run_step(poller: &Poller<'_>, planned: &Planned) -> StepRan {
    let _end = StepEnd::new(poller.memory, &planned.job_id);
    match planned.step {
        Step::Status => StepRan::Status(poller.status_step(&planned.job_id)),
        Step::Log => StepRan::Log(log_step(poller.live, poller.runner, poller.sink, &planned.job_id, &planned.coords)),
    }
}
