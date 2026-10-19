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
//! fetching-outcome flag, the consecutive failed fetches (the strikes), the consecutive answered
//! failures of a cancel's delivery (o22.4.5, o22.11.3), a `cancelling` job's sweeps without progress
//! and the smallest session size seen for it (o22.5, o22.13.3–4), whether a delivery found the job
//! ended first (o22.13.2), a pending submit's refusals and whether its last submit call's reply was
//! lost (o23.4, o23.13.6), and when each step last ran.

use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use rusqlite::Connection;

use super::live_log::LiveLog;
use super::remote_state::RemoteStates;
use super::PollerSink;
use crate::error::AppError;
use crate::models::job::{Job, JobStatus};
use crate::ssh_backend::{coordinates, RemoteCoordinates};

/// After this many consecutive failed fetches of a fetching outcome, automatic fetches stop until a
/// manual retry (o item 4).
pub const MAX_FETCH_STRIKES: u32 = 3;

/// After this many consecutive answered failures of a delivery (ADR-024 o22.11.3: a ready or `mkjob`
/// refusal, `cancel.sh` rc ≠ 0, a not-installed trampoline, a broken reply), automatic re-sends of a
/// pending cancel stop; Retry cancel is the manual retry (o22.4.5, o22.14.1). Transport failures do not count: they
/// retry on every step, like any check while the host is away.
pub const MAX_CANCEL_FAILURES: u32 = 3;

/// After this many sweeps of a `cancelling` job without progress (ADR-024 l row 3, o22.5,
/// o22.13.3–4), automatic sweeps stop: status steps then only collect and classify, and Retry cancel
/// is the manual retry (o22.12.1, o22.14.2).
pub const MAX_SWEEPS_WITHOUT_PROGRESS: u32 = 3;

/// At this many refusals of a pending submit this launch (slot busy, slot check error, lock busy, an
/// unreadable answer — ADR-024 l, m2, o23.4, o23.13.1), the record's notice names why it waits. The
/// job keeps waiting; Cancel is the exit.
pub const PENDING_NOTICE_AT: u32 = 3;

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

/// The non-terminal remote jobs, and the rows that could not be read as one.
#[derive(Debug, Default)]
pub struct Candidates {
    pub jobs: Vec<Candidate>,
    /// `(job id, why)` for every selected row that does not read as a remote job (a row with partial
    /// coordinates, which the v20 CHECK forbids; a row `Job::from_row` refuses). It is not polled —
    /// never treated as local, never guessed — and the others still are.
    pub unreadable: Vec<(String, String)>,
}

/// Every non-terminal remote job (`queued`/`running`/`cancelling` with coordinates, o item 1, o22.5).
/// One unreadable row is set aside in `unreadable`, so it cannot stop the polling of every other
/// job; only a failed query is an error.
pub fn candidates(conn: &Connection) -> Result<Candidates, AppError> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {} FROM jobs WHERE status IN ('queued', 'running', 'cancelling') AND remote_host IS NOT NULL \
         ORDER BY created_at, id",
        Job::COLUMNS
    ))?;
    let rows = stmt.query_map([], |row| Ok((row.get::<_, String>(0)?, Job::from_row(row))))?;
    let mut out = Candidates::default();
    for row in rows {
        let (id, job) = row?;
        let read = job.map_err(AppError::from).and_then(|job| {
            let coords = coordinates(&job)?
                .ok_or_else(|| AppError::Internal(format!("job {}: selected as remote without coordinates", job.id)))?;
            Ok(Candidate { id: job.id, status: job.status, coords })
        });
        match read {
            Ok(candidate) => out.jobs.push(candidate),
            Err(e) => out.unreadable.push((id, e.to_string())),
        }
    }
    Ok(out)
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
/// (the open count stays, o16.3), its memory, and its remote record — a job that left the
/// candidates without a step on it (another writer, a `RowChanged`) loses its record with a `null`
/// event (o19.5). A job with a step in progress keeps its memory and its record: its step decides.
/// The records come last and outside `PollerMemory`'s lock: the in-progress ids are snapshotted,
/// the lock released, then the records filtered (R is never taken under M).
pub fn sweep(candidates: &[Candidate], live: &LiveLog, memory: &PollerMemory, remote: &RemoteStates, sink: &dyn PollerSink) {
    let live_ids = |id: &str| candidates.iter().any(|c| c.id == id);
    live.retain(live_ids);
    memory.retain(live_ids);
    let in_progress = memory.in_progress();
    remote.retain(|id| live_ids(id) || in_progress.contains(id), sink);
}

/// What the poller remembers about one job during a launch.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct JobMemory {
    /// A fetching outcome was classified: the log is final and is no longer polled (o16.6).
    pub fetching: bool,
    /// Consecutive failed fetches; [`MAX_FETCH_STRIKES`] stops automatic fetches.
    pub strikes: u32,
    /// Consecutive answered failures of a delivery (o22.11.3); [`MAX_CANCEL_FAILURES`] stops
    /// automatic re-sends (o22.4.5).
    pub cancel_failures: u32,
    /// Sweeps of a `cancelling` job without progress (o22.5, o22.13.3): every sweep step the server
    /// answered counts one, unless it shows the session below [`Self::min_session`]
    /// ([`PollerMemory::count_sweep`]). [`MAX_SWEEPS_WITHOUT_PROGRESS`] stops automatic sweeps; a
    /// Retry cancel click restarts it ([`PollerMemory::restart_sweeps`], o22.12.1, o22.14.2).
    pub sweeps_without_progress: u32,
    /// The smallest job-session size seen for the `cancelling` job (o22.13.4): the baseline progress
    /// is measured against. It only falls, so a session that grows and shrinks back is not progress.
    /// `None` until the first write of `cancelling` or the first sweep of this launch.
    pub min_session: Option<usize>,
    /// A delivery found the job ended before the cancel reached the server (o22.13.2): the job's next
    /// `finalise` adds that note to the message it writes.
    pub ended_before_cancel: bool,
    /// A pending submit's refusals this launch (o23.4, o23.13.1): counted by the attempt that got
    /// one ([`PollerMemory::count_refusal`]), never reset by a transport failure — only when the
    /// pending state ends ([`PollerMemory::end_pending`]) and by a restart. [`PENDING_NOTICE_AT`]
    /// shows the notice.
    pub pending_refusals: u32,
    /// The pending re-check's last submit call got no reply (`Unknown`, o23.6): the next re-check
    /// uploads nothing (o23.13.6), so a lagging call cannot have its dir overwritten. Cleared by the
    /// next submit call that is answered.
    pub submit_reply_lost: bool,
    pub last_status: Option<Instant>,
    pub last_log: Option<Instant>,
    pub in_progress: bool,
}

/// The poller's per-launch memory (o item 4: it resets on launch, so the bounds hold per launch).
#[derive(Debug, Default)]
pub struct PollerMemory {
    jobs: Mutex<HashMap<String, JobMemory>>,
    /// Unreadable rows already reported this launch (each is reported once, not every tick).
    reported: Mutex<std::collections::HashSet<String>>,
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

    /// Count an answered failure of a delivery (o22.11.3); returns the count now.
    pub fn cancel_failed(&self, job_id: &str) -> u32 {
        let mut jobs = self.lock();
        let m = jobs.entry(job_id.to_string()).or_default();
        m.cancel_failures += 1;
        m.cancel_failures
    }

    /// The cancel was delivered or overtaken, or Cancel was clicked again (the manual retry): the
    /// re-send bound starts over.
    pub fn clear_cancel_failures(&self, job_id: &str) {
        if let Some(m) = self.lock().get_mut(job_id) {
            m.cancel_failures = 0;
        }
    }

    /// Cancel was clicked on a `cancelling` job — the manual sweep retry (o22.12.1): the sweep
    /// bound starts over; the baseline is kept (o22.13.4).
    pub fn restart_sweeps(&self, job_id: &str) {
        if let Some(m) = self.lock().get_mut(job_id) {
            m.sweeps_without_progress = 0;
        }
    }

    /// One sweep step the server answered (o22.13.3–4), in **one** operation under the lock — never
    /// "the count read before the ssh calls + 1", so a `restart_sweeps` click during the sweep is not
    /// overwritten. `session` is the classify's `Cancelling { session }`, `None` when it showed
    /// anything else or nothing could be classified:
    /// - no baseline yet → it is set, nothing counted;
    /// - below the baseline → progress: the baseline falls to it, the count restarts at 0;
    /// - otherwise (equal, larger, or no session) → counted.
    ///
    /// Returns the count now.
    pub fn count_sweep(&self, job_id: &str, session: Option<usize>) -> u32 {
        let mut jobs = self.lock();
        let m = jobs.entry(job_id.to_string()).or_default();
        match (session, m.min_session) {
            (Some(session), None) => m.min_session = Some(session),
            (Some(session), Some(baseline)) if session < baseline => {
                m.min_session = Some(session);
                m.sweeps_without_progress = 0;
            }
            _ => m.sweeps_without_progress += 1,
        }
        m.sweeps_without_progress
    }

    /// A write of `cancelling` (delivery's or the ordinary step's, o22.13.4): the baseline takes this
    /// session size unless a smaller one was already seen — it only falls.
    pub fn note_cancelling(&self, job_id: &str, session: usize) {
        let mut jobs = self.lock();
        let m = jobs.entry(job_id.to_string()).or_default();
        m.min_session = Some(m.min_session.map_or(session, |baseline| baseline.min(session)));
    }

    /// A delivery found the job ended before the cancel reached the server (o22.13.2).
    pub fn set_ended_before_cancel(&self, job_id: &str) {
        self.lock().entry(job_id.to_string()).or_default().ended_before_cancel = true;
    }

    /// One refusal of the pending submit (o23.4), in **one** operation under the lock; returns the
    /// count now.
    pub fn count_refusal(&self, job_id: &str) -> u32 {
        let mut jobs = self.lock();
        let m = jobs.entry(job_id.to_string()).or_default();
        m.pending_refusals += 1;
        m.pending_refusals
    }

    /// Whether the pending re-check's last submit call got no reply (o23.13.6).
    pub fn note_submit_reply(&self, job_id: &str, lost: bool) {
        self.lock().entry(job_id.to_string()).or_default().submit_reply_lost = lost;
    }

    /// The pending state ended (an enqueue, a refusal that needs a person, a delivered cancel, the
    /// job ended): the refusal count and the lost-reply mark start over (o23.4).
    pub fn end_pending(&self, job_id: &str) {
        if let Some(m) = self.lock().get_mut(job_id) {
            m.pending_refusals = 0;
            m.submit_reply_lost = false;
        }
    }

    /// Seed the sweep count, for the tests: a reset of 0 proves nothing.
    #[cfg(test)]
    pub(crate) fn set_sweeps_without_progress(&self, job_id: &str, sweeps: u32) {
        self.lock().entry(job_id.to_string()).or_default().sweeps_without_progress = sweeps;
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

    /// `true` the first time an unreadable row `job_id` is reported this launch, `false` after.
    pub fn first_report(&self, job_id: &str) -> bool {
        self.reported.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).insert(job_id.to_string())
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

    /// The jobs with a step running now (a snapshot; the lock is released on return).
    pub fn in_progress(&self) -> std::collections::HashSet<String> {
        self.lock().iter().filter(|(_, m)| m.in_progress).map(|(id, _)| id.clone()).collect()
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

    /// The body of `fn <signature>` in `src`, up to the first line that closes it at `close`.
    fn body<'a>(src: &'a str, signature: &str, close: &str) -> &'a str {
        let start = src.find(signature).unwrap_or_else(|| panic!("{signature} exists"));
        &src[start..start + src[start..].find(close).expect("it ends")]
    }

    /// o19.5/o19.9, the lock order pinned in the source (deterministic; a looped race test would only
    /// be probabilistic): R (the record map) before M (this memory). `clear_strikes` and
    /// `PollerMemory::retain` run under M and never reach the records, and `sweep` filters the
    /// records only after `memory.retain` has returned and the in-progress snapshot released M.
    /// NEGATIVE CONTROL: filter the records inside `memory.retain`'s closure → red.
    #[test]
    fn the_record_map_is_never_taken_under_the_memory_lock() {
        let src = include_str!("plan.rs");
        let record_map = [concat!("Remote", "States"), concat!("remote", "."), concat!("remote_", "state")];
        for signature in [concat!("pub fn ", "clear_strikes("), concat!("pub fn ", "retain(&self")] {
            let b = body(src, signature, "\n    }\n");
            for r in record_map {
                assert!(!b.contains(r), "{signature} references the record map (`{r}`)");
            }
        }
        let sweep = body(src, concat!("pub fn ", "sweep("), "\n}\n");
        let memory_retain = sweep.find(concat!("memory.", "retain(")).expect("sweep filters the memory");
        let memory_call = &sweep[memory_retain..memory_retain + sweep[memory_retain..].find(";\n").expect("one statement")];
        assert!(!memory_call.contains(concat!("remote", ".")), "the records are filtered inside memory.retain: {memory_call}");
        let snapshot = sweep.find(concat!("memory.", "in_progress()")).expect("sweep snapshots the in-progress ids");
        let records = sweep.find(concat!("remote.", "retain(")).expect("sweep filters the records");
        assert!(memory_retain < snapshot && snapshot < records, "the records last, after M is released");
    }

    /// o22.13.4, the counting rule as one operation: no baseline → set, nothing counted; below it →
    /// progress (baseline falls, count 0); equal, larger or no session → counted.
    #[test]
    fn a_sweep_counts_unless_the_session_falls_below_the_baseline() {
        let m = PollerMemory::new();
        assert_eq!(m.count_sweep("j", Some(4)), 0, "the first sweep of a launch sets the baseline");
        assert_eq!(m.get("j").min_session, Some(4));
        assert_eq!(m.count_sweep("j", Some(4)), 1, "equal: no progress");
        assert_eq!(m.count_sweep("j", Some(5)), 2, "larger: no progress");
        assert_eq!(m.count_sweep("j", None), 3, "nothing classified: counted");
        assert_eq!(m.count_sweep("j", Some(3)), 0, "smaller: progress");
        assert_eq!(m.get("j").min_session, Some(3));
        m.note_cancelling("j", 7);
        assert_eq!(m.get("j").min_session, Some(3), "a write of cancelling never raises the baseline");
        m.note_cancelling("k", 7);
        assert_eq!(m.get("k").min_session, Some(7), "and sets it when there is none");
    }

    /// o22.13.4: a session that oscillates (5, 6, 5, 6 …) is bounded — the baseline is the smallest
    /// size, not the last. NEGATIVE CONTROL: track the last size (`min_session = Some(session)` on
    /// every sweep, progress when it is below the last) → red (the count never reaches the bound).
    #[test]
    fn an_oscillating_session_reaches_the_bound() {
        let m = PollerMemory::new();
        m.note_cancelling("j", 5);
        let counts: Vec<u32> = [6, 5, 6, 5, 6, 5].iter().map(|s| m.count_sweep("j", Some(*s))).collect();
        assert_eq!(&counts[..3], [1, 2, 3], "{counts:?}");
        assert!(counts[2] >= MAX_SWEEPS_WITHOUT_PROGRESS);
    }

    /// o22.12.1 / o22.13.4: `restart_sweeps` zeroes the count and keeps the baseline, so a session
    /// equal to it is counted, not progress. NEGATIVE CONTROL: progress on `session <= baseline` →
    /// red (the count stays 0).
    #[test]
    fn after_a_restart_a_session_equal_to_the_baseline_is_counted() {
        let m = PollerMemory::new();
        m.note_cancelling("j", 5);
        m.set_sweeps_without_progress("j", MAX_SWEEPS_WITHOUT_PROGRESS);
        m.restart_sweeps("j");
        assert_eq!((m.get("j").sweeps_without_progress, m.get("j").min_session), (0, Some(5)), "the baseline is kept");
        assert_eq!(m.count_sweep("j", Some(5)), 1);
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
