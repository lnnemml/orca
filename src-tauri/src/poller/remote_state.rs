//! The poller's per-job remote state as the view sees it (ADR-024 o19; unit 5.3 B3 Part A).
//!
//! The row's `error_message` carries the **words** of the last check; this record carries the
//! **structure** the view needs to decide badges and actions (o19.2): the label of a `queued` job
//! that is not in the server's hands (retry vs withdraw), the last non-fetching outcome, a check
//! that could not be done (the host is away), and the fetch strikes. It is in memory only, one
//! record per non-terminal remote job, and starts empty on every launch like [`PollerMemory`]
//! (o19.1, o19.8): the next status step re-derives it from the server's facts.
//!
//! - **One mapping** ([`next`]): what each [`StatusStep`] does to the record, a `match` with no
//!   wildcard (o19.3).
//! - **Every write holds the job's in-flight guard** (o19.4): the status step applies its change
//!   ([`RemoteStates::apply`], which takes the guard) before the guard is released; the retry and
//!   withdraw commands remove the record inside their guarded work. The one unguarded writer is the
//!   tick's sweep ([`RemoteStates::retain`]), which touches only jobs that left the candidates and
//!   have no step in progress.
//! - **Every change is numbered and emitted under the lock** (o19.5, o19.6): a set or a removal
//!   takes the next value of one launch-wide sequence and is emitted as `job:remote-state` while the
//!   record map's mutex is held, so emit order equals change order. A change that leaves the record
//!   equal — or removes an absent one — emits nothing. [`RemoteStates::read`] returns the record
//!   with the sequence's current value, so any later event has a greater `seq`.
//! - **Lock order** (o19.5): this map's mutex (R) before `PollerMemory`'s (M). Nothing takes R while
//!   holding M or the database lock, and the sink never calls back into R.

use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard};

use serde::Serialize;

use super::plan::{PollerMemory, MAX_FETCH_STRIKES};
use super::{shown_message, PollerSink, StatusStep};
use crate::in_flight::InFlightGuard;
use crate::remote::classify::Outcome;
use crate::remote::submit::Label;

/// What the last status step learned about the job (o19.1, o19.11): `{ "kind": "labelled",
/// "detail": "not_on_server" }`, `{ "kind": "shown", "detail": { "outcome": "running" } }` or
/// `{ "kind": "fetching" }`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", content = "detail", rename_all = "snake_case")]
pub enum Known {
    /// The label call says the job is not in the server's hands (o 3.4).
    Labelled(Label),
    /// A non-fetching outcome the classifier decided (never `Indeterminate`, o19.3).
    Shown(Outcome),
    /// The server reports a fetching outcome and the download is failing (o19.11): the job's fetch
    /// state says how far the strikes have gone.
    Fetching,
}

/// The fetch bound as the view sees it, re-read from `PollerMemory`'s strike counter on every
/// apply (o19.1): the counter the bound acts on stays the one source.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct FetchState {
    /// Consecutive failed fetches this launch.
    pub strikes: u32,
    /// `strikes >= MAX_FETCH_STRIKES`: automatic fetches stopped until `retry_remote_fetch`.
    pub stopped: bool,
}

impl FetchState {
    pub fn from_strikes(strikes: u32) -> Self {
        FetchState { strikes, stopped: strikes >= MAX_FETCH_STRIKES }
    }
}

/// One job's record (o19.1).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct RemoteState {
    /// The last state the server's facts decided; `None` before the first decided check of a launch.
    pub known: Option<Known>,
    /// Why the last check decided nothing (the host is away, or the server's state could not be
    /// decided); `known` stays what was last seen.
    pub check_error: Option<String>,
    pub fetch: FetchState,
}

/// What `remote_job_state` returns: the job's record (`None`: no record) and the sequence's
/// current value, read together under the lock (o19.6).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RemoteStateRead {
    pub seq: u64,
    pub state: Option<RemoteState>,
}

/// What one status step does to a job's record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    /// Leave it as it is.
    Nothing,
    /// Drop it: the job left the poller's hands (the terminal status arrives as `job:status`).
    Remove,
    /// Make it this.
    Set(RemoteState),
}

/// The record after `step`, from the record before (`None`: none yet) and the fetch bound read now
/// (o19.3). No wildcard: a new `StatusStep` variant fails to compile here until it is decided.
pub fn next(old: Option<&RemoteState>, step: &StatusStep, fetch: FetchState) -> Change {
    let mut state = old.cloned().unwrap_or_default();
    match step {
        StatusStep::Labelled(label) => {
            state.known = Some(Known::Labelled(*label));
            state.check_error = None;
        }
        // "Could not be decided on this check" is a check error, not a new state (Anton, LOW-2):
        // what was last seen stays.
        StatusStep::Shown(Outcome::Indeterminate) => state.check_error = shown_message(&Outcome::Indeterminate),
        StatusStep::Shown(outcome) => {
            state.known = Some(Known::Shown(outcome.clone()));
            state.check_error = None;
        }
        // The server's state is decided (a fetching outcome); the download is what fails (o19.11).
        StatusStep::FetchFailed { .. } | StatusStep::FetchStopped => {
            state.known = Some(Known::Fetching);
            state.check_error = None;
        }
        // A host away does not erase what was last seen.
        StatusStep::CheckFailed(why) => state.check_error = Some(why.clone()),
        StatusStep::Busy | StatusStep::RowChanged => return Change::Nothing,
        StatusStep::NotLive | StatusStep::Cancelled | StatusStep::Finalised { .. } => return Change::Remove,
    }
    state.fetch = fetch;
    Change::Set(state)
}

/// Every job's record and the launch-wide sequence, behind one mutex (R). Managed app state.
#[derive(Debug, Default)]
pub struct RemoteStates {
    inner: Mutex<Records>,
}

#[derive(Debug, Default)]
struct Records {
    jobs: HashMap<String, RemoteState>,
    /// The last change's number; never reset within a launch, 0 before the first change.
    seq: u64,
}

impl Records {
    /// The one place a record changes (o19.5): numbered and emitted with the caller holding R, and
    /// only if it really changes — an equal record or the removal of an absent one emits nothing.
    fn change(&mut self, job_id: &str, new: Option<RemoteState>, sink: &dyn PollerSink) {
        if self.jobs.get(job_id) == new.as_ref() {
            return;
        }
        self.seq += 1;
        match &new {
            Some(state) => self.jobs.insert(job_id.to_string(), state.clone()),
            None => self.jobs.remove(job_id),
        };
        sink.remote_state(job_id, self.seq, new.as_ref());
    }
}

impl RemoteStates {
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> MutexGuard<'_, Records> {
        self.inner.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Apply the status step `step` of the job `guard` holds (o19.3). Taking the guard is the
    /// point: the change is made, and emitted, while the job is still claimed (o19.4), so no later
    /// command on the job can be overtaken by it. The fetch bound is read from `memory` under R —
    /// R before M (o19.5).
    pub fn apply(&self, guard: &InFlightGuard, step: &StatusStep, memory: &PollerMemory, sink: &dyn PollerSink) {
        let job_id = guard.job_id();
        let mut records = self.lock();
        let fetch = FetchState::from_strikes(memory.get(job_id).strikes);
        match next(records.jobs.get(job_id), step, fetch) {
            Change::Nothing => {}
            Change::Remove => records.change(job_id, None, sink),
            Change::Set(state) => records.change(job_id, Some(state), sink),
        }
    }

    /// Drop `job_id`'s record, emitting `null` if there was one. The caller holds the job's
    /// in-flight guard (the retry and withdraw commands, o19.3).
    pub fn remove(&self, job_id: &str, sink: &dyn PollerSink) {
        self.lock().change(job_id, None, sink);
    }

    /// Keep only the records `keep` accepts; each dropped one emits `null` (the tick's sweep,
    /// o19.5). The caller must not hold `PollerMemory`'s lock.
    pub fn retain(&self, keep: impl Fn(&str) -> bool, sink: &dyn PollerSink) {
        let mut records = self.lock();
        let gone: Vec<String> = records.jobs.keys().filter(|id| !keep(id)).cloned().collect();
        for job_id in gone {
            records.change(&job_id, None, sink);
        }
    }

    /// `job_id`'s record and the sequence's current value, read together (o19.6).
    pub fn read(&self, job_id: &str) -> RemoteStateRead {
        let records = self.lock();
        RemoteStateRead { seq: records.seq, state: records.jobs.get(job_id).cloned() }
    }

    /// Whether some thread holds R now (the recording sink checks every emit is made under it).
    #[cfg(test)]
    pub(crate) fn is_locked(&self) -> bool {
        matches!(self.inner.try_lock(), Err(std::sync::TryLockError::WouldBlock))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::in_flight::InFlight;
    use crate::models::job::JobStatus;
    use crate::poller::RecordingSink;
    use crate::remote::classify::FailReason;

    const NO_STRIKES: FetchState = FetchState { strikes: 0, stopped: false };

    fn with(known: Option<Known>, check_error: Option<&str>) -> RemoteState {
        RemoteState { known, check_error: check_error.map(String::from), fetch: NO_STRIKES }
    }

    fn set(state: RemoteState) -> Change {
        Change::Set(state)
    }

    /// o19.3, every variant: from a record that holds a label and a check error, and from none.
    #[test]
    fn each_status_step_maps_to_its_change() {
        let running = Some(Known::Shown(Outcome::Running));
        let old = with(Some(Known::Labelled(Label::NotOnServer)), Some("ssh timed out"));
        let indeterminate = shown_message(&Outcome::Indeterminate);
        let cases: Vec<(StatusStep, Change, Change)> = vec![
            (
                StatusStep::Labelled(Label::SubmitInterrupted),
                set(with(Some(Known::Labelled(Label::SubmitInterrupted)), None)),
                set(with(Some(Known::Labelled(Label::SubmitInterrupted)), None)),
            ),
            (StatusStep::Shown(Outcome::Running), set(with(running.clone(), None)), set(with(running, None))),
            (
                StatusStep::Shown(Outcome::Lost { orphans: vec![7] }),
                set(with(Some(Known::Shown(Outcome::Lost { orphans: vec![7] })), None)),
                set(with(Some(Known::Shown(Outcome::Lost { orphans: vec![7] })), None)),
            ),
            (
                StatusStep::Shown(Outcome::Indeterminate),
                set(RemoteState { check_error: indeterminate.clone(), ..old.clone() }),
                set(RemoteState { check_error: indeterminate, ..RemoteState::default() }),
            ),
            (
                StatusStep::CheckFailed("host away".into()),
                set(with(Some(Known::Labelled(Label::NotOnServer)), Some("host away"))),
                set(with(None, Some("host away"))),
            ),
            (
                StatusStep::FetchFailed { strikes: 1, reason: "rsync".into() },
                set(with(Some(Known::Fetching), None)),
                set(with(Some(Known::Fetching), None)),
            ),
            (StatusStep::FetchStopped, set(with(Some(Known::Fetching), None)), set(with(Some(Known::Fetching), None))),
            (StatusStep::Busy, Change::Nothing, Change::Nothing),
            (StatusStep::RowChanged, Change::Nothing, Change::Nothing),
            (StatusStep::NotLive, Change::Remove, Change::Remove),
            (StatusStep::Cancelled, Change::Remove, Change::Remove),
            (
                StatusStep::Finalised {
                    outcome: Outcome::Failed { reason: FailReason::NonZeroExit { code: 1 } },
                    status: JobStatus::Failed,
                    drain_error: None,
                },
                Change::Remove,
                Change::Remove,
            ),
        ];
        for (step, from_old, from_none) in cases {
            assert_eq!(next(Some(&old), &step, NO_STRIKES), from_old, "{step:?} over a record");
            assert_eq!(next(None, &step, NO_STRIKES), from_none, "{step:?} over none");
        }
    }

    /// o19.9: a host away keeps the label the view offers retry/withdraw from. NEGATIVE CONTROL: make
    /// `CheckFailed` clear `known` → red.
    #[test]
    fn a_failed_check_keeps_the_label() {
        let labelled = with(Some(Known::Labelled(Label::NotOnServer)), None);
        let Change::Set(after) = next(Some(&labelled), &StatusStep::CheckFailed("ssh: timeout".into()), NO_STRIKES) else {
            panic!("a failed check sets the check error")
        };
        assert_eq!(after.known, Some(Known::Labelled(Label::NotOnServer)));
        assert_eq!(after.check_error.as_deref(), Some("ssh: timeout"));
    }

    /// o19.9: `Indeterminate` keeps the last seen `Running` and says why in `check_error` (LOW-2).
    /// NEGATIVE CONTROL: drop the `Shown(Indeterminate)` arm (it falls into `Shown(outcome)`) → red.
    #[test]
    fn indeterminate_keeps_what_was_seen_and_sets_the_check_error() {
        let running = with(Some(Known::Shown(Outcome::Running)), None);
        let Change::Set(after) = next(Some(&running), &StatusStep::Shown(Outcome::Indeterminate), NO_STRIKES) else {
            panic!("Indeterminate sets the check error")
        };
        assert_eq!(after.known, Some(Known::Shown(Outcome::Running)));
        assert_eq!(after.check_error, shown_message(&Outcome::Indeterminate));
    }

    /// o19.11.1: a job last seen running whose fetch then fails reads `Fetching`, never "running"
    /// beside the row's "fetch failed" text; a later failed check keeps `Fetching`. NEGATIVE
    /// CONTROL: make `FetchFailed`/`FetchStopped` keep `known` → red.
    #[test]
    fn a_failed_fetch_after_running_reads_fetching() {
        let running = with(Some(Known::Shown(Outcome::Running)), Some("away"));
        let step = StatusStep::FetchFailed { strikes: 1, reason: "rsync exited with 23".into() };
        let Change::Set(after) = next(Some(&running), &step, FetchState::from_strikes(1)) else { panic!("a failed fetch sets") };
        assert_eq!(after.known, Some(Known::Fetching));
        assert_eq!((after.check_error.as_deref(), after.fetch), (None, FetchState::from_strikes(1)));
        let Change::Set(away) = next(Some(&after), &StatusStep::CheckFailed("ssh: timeout".into()), FetchState::from_strikes(1)) else {
            panic!("a failed check sets")
        };
        assert_eq!(away.known, Some(Known::Fetching), "a failed check keeps Fetching");
    }

    /// The fetch bound is whatever the counter says now, on every step that sets the record.
    #[test]
    fn the_fetch_bound_is_read_from_the_counter() {
        assert_eq!(FetchState::from_strikes(2), FetchState { strikes: 2, stopped: false });
        assert_eq!(FetchState::from_strikes(MAX_FETCH_STRIKES), FetchState { strikes: 3, stopped: true }, "the third strike reads stopped");
        let stopped = RemoteState { fetch: FetchState::from_strikes(3), ..RemoteState::default() };
        assert_eq!(
            next(Some(&stopped), &StatusStep::CheckFailed("x".into()), NO_STRIKES),
            set(with(None, Some("x"))),
            "a cleared counter reads not stopped"
        );
    }

    fn remote_events(sink: &RecordingSink) -> Vec<(String, u64, Option<RemoteState>)> {
        sink.remote_states()
    }

    /// o19.6/o19.9: one event per change, numbered by one launch-wide sequence that strictly
    /// increases across jobs and removals; an unchanged repeat (a `Running` every 15 s) and the
    /// removal of an absent record emit nothing; a read returns the sequence's current value.
    /// NEGATIVE CONTROL: drop the equality check in `Records::change` → red (an event for the repeat).
    #[test]
    fn one_event_per_change_with_strictly_increasing_seqs() {
        let (remote, memory, in_flight) = (std::sync::Arc::new(RemoteStates::new()), PollerMemory::new(), InFlight::default());
        let sink = RecordingSink::default().under_remote(remote.clone());
        assert_eq!(remote.read("a"), RemoteStateRead { seq: 0, state: None });
        let (a, b) = (in_flight.acquire("a").unwrap(), in_flight.acquire("b").unwrap());
        remote.apply(&a, &StatusStep::Shown(Outcome::Running), &memory, &sink);
        remote.apply(&a, &StatusStep::Shown(Outcome::Running), &memory, &sink);
        remote.apply(&b, &StatusStep::Labelled(Label::NotOnServer), &memory, &sink);
        remote.apply(&a, &StatusStep::CheckFailed("away".into()), &memory, &sink);
        remote.apply(&a, &StatusStep::CheckFailed("away".into()), &memory, &sink);
        remote.remove("b", &sink);
        remote.remove("b", &sink);
        remote.apply(&a, &StatusStep::Busy, &memory, &sink);

        let events = remote_events(&sink);
        let seqs: Vec<u64> = events.iter().map(|(_, seq, _)| *seq).collect();
        assert_eq!(seqs, [1, 2, 3, 4], "one per change, strictly increasing");
        assert_eq!(
            events.iter().map(|(id, _, s)| (id.as_str(), s.is_some())).collect::<Vec<_>>(),
            [("a", true), ("b", true), ("a", true), ("b", false)]
        );
        assert_eq!(remote.read("b"), RemoteStateRead { seq: 4, state: None }, "a read carries the current seq, whatever the job");
        assert_eq!(remote.read("a").state, events[2].2);
        assert!(sink.events().is_empty(), "no stream or status event");
    }

    /// o19.5: the sweep drops what `keep` refuses, each with a `null` event under the lock.
    #[test]
    fn retain_removes_with_an_event_each() {
        let (remote, memory, in_flight) = (std::sync::Arc::new(RemoteStates::new()), PollerMemory::new(), InFlight::default());
        let sink = RecordingSink::default().under_remote(remote.clone());
        for id in ["a", "b", "c"] {
            remote.apply(&in_flight.acquire(id).unwrap(), &StatusStep::Shown(Outcome::Queued), &memory, &sink);
        }
        sink.take_remote_states();
        remote.retain(|id| id == "b", &sink);
        let mut gone: Vec<(String, Option<RemoteState>)> =
            sink.remote_states().into_iter().map(|(id, _, state)| (id, state)).collect();
        gone.sort_by(|x, y| x.0.cmp(&y.0));
        assert_eq!(gone, [("a".to_string(), None), ("c".to_string(), None)]);
        assert!(remote.read("b").state.is_some());
    }

    /// The wire shape B3's TypeScript mirrors.
    #[test]
    fn the_record_wire_shape() {
        let state = RemoteState {
            known: Some(Known::Labelled(Label::NotOnServer)),
            check_error: Some("x".into()),
            fetch: FetchState::from_strikes(3),
        };
        assert_eq!(
            serde_json::to_value(RemoteStateRead { seq: 7, state: Some(state) }).unwrap(),
            serde_json::json!({
                "seq": 7,
                "state": {
                    "known": { "kind": "labelled", "detail": "not_on_server" },
                    "check_error": "x",
                    "fetch": { "strikes": 3, "stopped": true }
                }
            })
        );
        assert_eq!(
            serde_json::to_value(Known::Shown(Outcome::Lost { orphans: vec![4] })).unwrap(),
            serde_json::json!({ "kind": "shown", "detail": { "outcome": "lost", "orphans": [4] } })
        );
        assert_eq!(serde_json::to_value(Known::Fetching).unwrap(), serde_json::json!({ "kind": "fetching" }));
        assert_eq!(serde_json::to_value(RemoteStateRead { seq: 0, state: None }).unwrap(), serde_json::json!({ "seq": 0, "state": null }));
    }
}
