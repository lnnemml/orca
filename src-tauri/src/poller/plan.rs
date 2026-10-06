//! What the poller does next (ADR-024 o item 4, o16.2, o16.5), as pure decisions over what it
//! remembers, and that memory itself.
//!
//! **The tick planner** ([`plan`]) gets the non-terminal remote jobs ([`candidates`], read from the
//! database) and, for each, what the poller remembers and whether a view watches it, and decides the
//! one step each job is due:
//! - the **status step** every `periods.status` (15 s) for every job, watched or not;
//! - the **log step** every `periods.log` (2 s) only while the job is watched and no fetching outcome
//!   has been classified for it — or at once while the last chunk was exactly the cap (catch-up);
//! - **status first** when both are due, and **never a second step** for a job whose step is still
//!   running: per job the two are sequenced, never concurrent (o16.5), so a log poll can never make
//!   the status step skip.
//!
//! Draft and local jobs are not candidates, so a watched local job gets no ssh call and a watched
//! draft is polled once it has coordinates (o16.2).
//!
//! **The memory** ([`PollerMemory`]) lives for one launch and starts empty (o item 4): the
//! fetching-outcome flag, the consecutive failed fetches (the strikes), and when each step last ran.

use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use rusqlite::Connection;

use super::live_log::LiveLog;
use crate::error::AppError;
use crate::models::job::{Job, JobStatus};
use crate::ssh_backend::{coordinates, RemoteCoordinates};

/// After this many consecutive failed fetches of a fetching outcome, automatic fetches stop until a
/// manual retry (o item 4).
pub const MAX_FETCH_STRIKES: u32 = 3;

/// The poller's periods (o item 4: initial values, tunable, not a contract).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Periods {
    /// Label/collect/fetch, for every non-terminal remote job.
    pub status: Duration,
    /// The live log of a watched job.
    pub log: Duration,
}

impl Periods {
    pub const INITIAL: Periods = Periods { status: Duration::from_secs(15), log: Duration::from_secs(2) };
}

/// A step the poller runs for one job.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// Label (a `queued` row) → collect + classify → fetch for a fetching outcome.
    Status,
    /// One `poll_log` chunk into the live view.
    Log,
}

/// A non-terminal remote job, as the database has it now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub id: String,
    pub status: JobStatus,
    pub coords: RemoteCoordinates,
}

/// Every non-terminal remote job (`queued`/`running` with coordinates, o item 1). A row with
/// partial coordinates is an error, never skipped as local.
pub fn candidates(conn: &Connection) -> Result<Vec<Candidate>, AppError> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {} FROM jobs WHERE status IN ('queued', 'running') AND remote_host IS NOT NULL ORDER BY created_at, id",
        Job::COLUMNS
    ))?;
    let jobs = stmt.query_map([], Job::from_row)?.collect::<Result<Vec<Job>, _>>()?;
    jobs.into_iter()
        .map(|job| {
            let coords = coordinates(&job)?
                .ok_or_else(|| AppError::Internal(format!("job {}: selected as remote without coordinates", job.id)))?;
            Ok(Candidate { id: job.id, status: job.status, coords })
        })
        .collect()
}

/// What the planner knows about one candidate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanJob {
    pub id: String,
    /// A view is open on it (o16.2).
    pub watched: bool,
    /// Its last chunk was exactly the cap (o16.3).
    pub catch_up: bool,
    /// A fetching outcome was classified for it this launch: its remote log is final (o16.6).
    pub fetching: bool,
    pub last_status: Option<Instant>,
    pub last_log: Option<Instant>,
    /// One of its steps is still running.
    pub in_progress: bool,
}

/// The step `job` is due at `now`, if any.
pub fn due(job: &PlanJob, periods: &Periods, now: Instant) -> Option<Step> {
    if job.in_progress {
        return None;
    }
    let elapsed = |last: Option<Instant>, period: Duration| last.is_none_or(|t| now.saturating_duration_since(t) >= period);
    if elapsed(job.last_status, periods.status) {
        return Some(Step::Status);
    }
    let log_due = job.catch_up || elapsed(job.last_log, periods.log);
    (job.watched && !job.fetching && log_due).then_some(Step::Log)
}

/// One tick: the due step of every job, in the candidates' order.
pub fn plan(jobs: &[PlanJob], periods: &Periods, now: Instant) -> Vec<(String, Step)> {
    jobs.iter().filter_map(|job| due(job, periods, now).map(|step| (job.id.clone(), step))).collect()
}

/// The planner's input for `candidates`, from the live log and the memory.
pub fn plan_inputs(candidates: &[Candidate], live: &LiveLog, memory: &PollerMemory) -> Vec<PlanJob> {
    candidates
        .iter()
        .map(|c| {
            let m = memory.get(&c.id);
            PlanJob {
                id: c.id.clone(),
                watched: live.open_count(&c.id) > 0,
                catch_up: live.catch_up(&c.id),
                fetching: m.fetching,
                last_status: m.last_status,
                last_log: m.last_log,
                in_progress: m.in_progress,
            }
        })
        .collect()
}

/// Forget every job that is no longer a candidate (terminal, or its row vanished): its live state
/// (the open count stays, o16.3) and its memory.
pub fn sweep(candidates: &[Candidate], live: &LiveLog, memory: &PollerMemory) {
    let live_ids = |id: &str| candidates.iter().any(|c| c.id == id);
    live.retain(live_ids);
    memory.retain(live_ids);
}

/// What the poller remembers about one job during a launch.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct JobMemory {
    /// A fetching outcome was classified: the log is final and is no longer polled (o16.6).
    pub fetching: bool,
    /// Consecutive failed fetches; [`MAX_FETCH_STRIKES`] stops automatic fetches.
    pub strikes: u32,
    pub last_status: Option<Instant>,
    pub last_log: Option<Instant>,
    pub in_progress: bool,
}

/// The poller's per-launch memory (o item 4: it resets on launch, so the bounds hold per launch).
#[derive(Debug, Default)]
pub struct PollerMemory {
    jobs: Mutex<HashMap<String, JobMemory>>,
}

impl PollerMemory {
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> MutexGuard<'_, HashMap<String, JobMemory>> {
        self.jobs.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub fn get(&self, job_id: &str) -> JobMemory {
        self.lock().get(job_id).cloned().unwrap_or_default()
    }

    /// A fetching outcome was classified for `job_id`.
    pub fn set_fetching(&self, job_id: &str) {
        self.lock().entry(job_id.to_string()).or_default().fetching = true;
    }

    /// Count a failed fetch; returns the count now.
    pub fn strike(&self, job_id: &str) -> u32 {
        let mut jobs = self.lock();
        let m = jobs.entry(job_id.to_string()).or_default();
        m.strikes += 1;
        m.strikes
    }

    /// A manual retry: the bound starts over.
    pub fn clear_strikes(&self, job_id: &str) {
        if let Some(m) = self.lock().get_mut(job_id) {
            m.strikes = 0;
        }
    }

    /// Mark a step of `job_id` started at `now`: `false` (and nothing changed) if one is running.
    pub fn begin_step(&self, job_id: &str, step: Step, now: Instant) -> bool {
        let mut jobs = self.lock();
        let m = jobs.entry(job_id.to_string()).or_default();
        if m.in_progress {
            return false;
        }
        m.in_progress = true;
        match step {
            Step::Status => m.last_status = Some(now),
            Step::Log => m.last_log = Some(now),
        }
        true
    }

    /// The step of `job_id` ended.
    pub fn end_step(&self, job_id: &str) {
        if let Some(m) = self.lock().get_mut(job_id) {
            m.in_progress = false;
        }
    }

    /// Forget `job_id` (it turned terminal).
    pub fn forget(&self, job_id: &str) {
        self.lock().remove(job_id);
    }

    /// Keep only the jobs `keep` accepts. A job whose step is still running is kept, so its
    /// in-progress mark is not lost under it.
    pub fn retain(&self, keep: impl Fn(&str) -> bool) {
        self.lock().retain(|id, m| m.in_progress || keep(id));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const S: Duration = Duration::from_secs(1);

    fn job(watched: bool) -> PlanJob {
        PlanJob {
            id: "j".into(),
            watched,
            catch_up: false,
            fetching: false,
            last_status: None,
            last_log: None,
            in_progress: false,
        }
    }

    /// Ran each step at `t`.
    fn ran(mut j: PlanJob, t: Instant) -> PlanJob {
        j.last_status = Some(t);
        j.last_log = Some(t);
        j
    }

    /// The periods are parameters; status every 15 s for every job, the log every 2 s only while
    /// watched.
    #[test]
    fn status_for_every_job_and_the_log_only_while_watched() {
        let (p, t0) = (Periods::INITIAL, Instant::now());
        assert_eq!(due(&job(false), &p, t0), Some(Step::Status), "never run: status first");
        let unwatched = ran(job(false), t0);
        for secs in [0, 2, 10] {
            assert_eq!(due(&unwatched, &p, t0 + secs * S), None, "unwatched at {secs} s: no log poll");
        }
        assert_eq!(due(&unwatched, &p, t0 + 15 * S), Some(Step::Status));
        let watched = ran(job(true), t0);
        assert_eq!(due(&watched, &p, t0 + S), None);
        assert_eq!(due(&watched, &p, t0 + 2 * S), Some(Step::Log));
        let fast = Periods { status: 3 * S, log: S };
        assert_eq!(due(&watched, &fast, t0 + S), Some(Step::Log), "the periods are parameters");
        assert_eq!(due(&watched, &fast, t0 + 3 * S), Some(Step::Status));
    }

    /// o16.5/o16.8: a due status step of a watched job is not skipped for its own log poll — status
    /// first when both are due. NEGATIVE CONTROL: test the log before the status in `due` and this
    /// goes red (`Some(Log)`).
    #[test]
    fn status_comes_first_when_both_are_due() {
        let (p, t0) = (Periods::INITIAL, Instant::now());
        let watched = ran(job(true), t0);
        assert_eq!(due(&watched, &p, t0 + 15 * S), Some(Step::Status));
        let catching_up = PlanJob { catch_up: true, ..watched };
        assert_eq!(due(&catching_up, &p, t0 + 15 * S), Some(Step::Status), "also during catch-up");
    }

    /// o16.5: never a second step for a job whose step still runs. NEGATIVE CONTROL: drop the
    /// `in_progress` check and both rows go red.
    #[test]
    fn a_job_with_a_running_step_gets_no_other() {
        let (p, t0) = (Periods::INITIAL, Instant::now());
        let busy = PlanJob { in_progress: true, catch_up: true, ..job(true) };
        assert_eq!(due(&busy, &p, t0 + 60 * S), None);
        assert_eq!(due(&PlanJob { watched: false, ..busy }, &p, t0), None);
    }

    /// o16.3: catch-up polls without the 2 s wait.
    #[test]
    fn catch_up_polls_at_once() {
        let (p, t0) = (Periods::INITIAL, Instant::now());
        let j = PlanJob { catch_up: true, ..ran(job(true), t0) };
        assert_eq!(due(&j, &p, t0), Some(Step::Log));
        assert_eq!(due(&PlanJob { watched: false, ..j }, &p, t0), None, "catch-up needs a watcher");
    }

    /// o16.6/o16.8: a job with a fetching outcome gets no further log poll, watched or catching up;
    /// its status step still runs. NEGATIVE CONTROL: drop `!job.fetching` from `due` and this goes
    /// red (`Some(Log)`).
    #[test]
    fn a_fetching_job_gets_no_log_poll() {
        let (p, t0) = (Periods::INITIAL, Instant::now());
        let j = PlanJob { fetching: true, catch_up: true, ..ran(job(true), t0) };
        assert_eq!(due(&j, &p, t0 + 5 * S), None);
        assert_eq!(due(&j, &p, t0 + 15 * S), Some(Step::Status));
    }

    #[test]
    fn memory_marks_steps_and_strikes() {
        let m = PollerMemory::new();
        let t0 = Instant::now();
        assert!(m.begin_step("j", Step::Log, t0));
        assert!(!m.begin_step("j", Step::Status, t0), "one step at a time");
        m.end_step("j");
        assert!(m.begin_step("j", Step::Status, t0 + S));
        assert_eq!((m.get("j").last_log, m.get("j").last_status), (Some(t0), Some(t0 + S)));
        m.end_step("j");
        assert_eq!((m.strike("j"), m.strike("j")), (1, 2));
        m.clear_strikes("j");
        assert_eq!(m.get("j").strikes, 0);
        m.retain(|_| false);
        assert_eq!(m.get("j"), JobMemory::default());
    }
}
