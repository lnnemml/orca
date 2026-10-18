//! The poller's per-job remote state as the view sees it (ADR-024 o19; unit 5.3 B3 Part A).
//!
//! The row's `error_message` carries the **words** of the last check; this record carries the
//! **structure** the view needs to decide badges and actions (o19.2): the label of a `queued` job
//! that is not in the server's hands (retry vs withdraw), the last non-fetching outcome, `Fetching`
//! once the server reports a fetching outcome whose download is failing (o19.11), a check that
//! could not be done (the host is away), and the fetch strikes. It is in memory only, one
//! record per non-terminal remote job, and starts empty on every launch like [`PollerMemory`]
//! (o19.1, o19.8): the next status step re-derives it from the server's facts.
//!
//! - **One mapping** ([`next`]): what each [`StatusStep`] does to the record, a `match` with no
//!   wildcard (o19.3). [`next_with`] adds what every step's calls and bounds say: the **last
//!   contact** (o22.8, the transport result of the step's last ssh/rsync call — `poll_log` is never
//!   a writer, it holds no guard), the cancel re-send bound (o22.4.5) and the sweep bound of a
//!   `cancelling` job (o22.13.3), each re-read on every apply. A retry's outcome has its own mapping ([`after_retry`], o22.8: it no longer removes the
//!   record).
//! - **Every write holds the job's in-flight guard** (o19.4): the status step applies its change
//!   ([`RemoteStates::apply_with_contact`], which takes the guard) before the guard is released;
//!   the retry command writes its contact ([`RemoteStates::note_retry`]) and the withdraw command
//!   removes the record, each inside its guarded work. The one unguarded writer is the
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

use super::plan::{PollerMemory, MAX_CANCEL_FAILURES, MAX_FETCH_STRIKES, MAX_SWEEPS_WITHOUT_PROGRESS};
use super::{is_fetching, shown_message, PollerSink, Seen, StatusStep};
use crate::in_flight::InFlightGuard;
use crate::remote::classify::Outcome;
use crate::remote::contact::Contact;
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

/// The cancel re-send bound as the view sees it (o22.4.5), re-read from `PollerMemory`'s counter
/// on every apply, like [`FetchState`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct CancelSends {
    /// Consecutive deliveries whose `cancel.sh` the server answered with a failure, this launch.
    pub failures: u32,
    /// `failures >= MAX_CANCEL_FAILURES`: automatic re-sends stopped until Cancel is clicked again.
    pub stopped: bool,
}

impl CancelSends {
    pub fn from_failures(failures: u32) -> Self {
        CancelSends { failures, stopped: failures >= MAX_CANCEL_FAILURES }
    }
}

/// The sweep bound of a `cancelling` job as the view sees it (o22.5, o22.13.3), re-read from
/// `PollerMemory`'s counter on every apply, like [`FetchState`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct Sweeps {
    /// Sweeps without progress this launch.
    pub count: u32,
    /// `count >= MAX_SWEEPS_WITHOUT_PROGRESS`: "processes of the job still run after 3 sweeps";
    /// automatic sweeps stopped until Cancel is clicked again (o22.12.1).
    pub stopped: bool,
}

impl Sweeps {
    pub fn from_count(count: u32) -> Self {
        Sweeps { count, stopped: count >= MAX_SWEEPS_WITHOUT_PROGRESS }
    }
}

/// The bounds a record shows, read from `PollerMemory` at the apply.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Bounds {
    pub fetch: FetchState,
    pub cancel: CancelSends,
    pub sweeps: Sweeps,
}

impl Bounds {
    fn read(memory: &PollerMemory, job_id: &str) -> Bounds {
        let m = memory.get(job_id);
        Bounds {
            fetch: FetchState::from_strikes(m.strikes),
            cancel: CancelSends::from_failures(m.cancel_failures),
            sweeps: Sweeps::from_count(m.sweeps_without_progress),
        }
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
    /// Whether the job's last guarded ssh/rsync call got an answer (o22.8); `None`: no call yet
    /// this launch. Retry is offered only while it is not `Failed`.
    pub last_contact: Option<Contact>,
    /// The cancel re-send bound (o22.4.5).
    pub cancel_sends: CancelSends,
    /// The sweep bound of a `cancelling` job (o22.13.3).
    pub sweeps: Sweeps,
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
        // Delivery (o22.4.5): the server answered and the cancel is not delivered — like a failed
        // check, what was last seen stays.
        StatusStep::CancelNotSent(why) => state.check_error = Some(why.clone()),
        // Nothing was sent (the bound, re-read in `next_with`, says why); what the stopped
        // delivery saw is shown like an ordinary step's (o22.12.2).
        StatusStep::CancelStopped { seen: Some(Seen::Label(label)), .. } => {
            state.known = Some(Known::Labelled(*label));
            state.check_error = None;
        }
        StatusStep::CancelStopped { seen: Some(Seen::Outcome(Outcome::Indeterminate)), .. } => {
            state.check_error = shown_message(&Outcome::Indeterminate)
        }
        StatusStep::CancelStopped { seen: Some(Seen::Outcome(outcome)), .. } => {
            state.known = Some(if is_fetching(outcome) { Known::Fetching } else { Known::Shown(outcome.clone()) });
            state.check_error = None;
        }
        // Nothing could be classified: the prepare's reason, what was last seen kept (o22.12.3).
        StatusStep::CancelStopped { seen: None, unready } => state.check_error = unready.clone(),
        // The job ended first: a fetching outcome, the normal fetch path follows (o22.4.4).
        StatusStep::EndedBeforeCancel(_) => {
            state.known = Some(Known::Fetching);
            state.check_error = None;
        }
        // Delivered: the post-send verdict, read as an ordinary step reads it.
        StatusStep::CancelDelivered(Outcome::Indeterminate) => state.check_error = shown_message(&Outcome::Indeterminate),
        StatusStep::CancelDelivered(outcome) => {
            state.known = Some(if is_fetching(outcome) { Known::Fetching } else { Known::Shown(outcome.clone()) });
            state.check_error = None;
        }
        // A sweep of a `cancelling` job (o22.13.3): what it classified, when it could, and the
        // failure the server answered with, if any (the sweep bound is re-read in `next_with`).
        StatusStep::Swept { outcome, error } => {
            if let Some(outcome) = outcome {
                state.known = Some(Known::Shown(outcome.clone()));
            }
            state.check_error = error.clone();
        }
        StatusStep::Busy | StatusStep::RowChanged => return Change::Nothing,
        StatusStep::NotLive | StatusStep::Cancelled | StatusStep::Finalised { .. } => return Change::Remove,
    }
    state.fetch = fetch;
    Change::Set(state)
}

/// [`next`], plus what the step's calls and the cancel and sweep bounds say (o22.4.5, o22.8,
/// o22.13.3): a step that sets the record also takes `bounds.cancel`, `bounds.sweeps` and, when it
/// made a call, `contact` (no call keeps
/// the last one); a step that changes nothing else (`RowChanged` after a call) still updates an
/// existing record's `last_contact` — every guarded call writes it — but creates none.
pub fn next_with(old: Option<&RemoteState>, step: &StatusStep, bounds: Bounds, contact: Option<Contact>) -> Change {
    match next(old, step, bounds.fetch) {
        Change::Remove => Change::Remove,
        Change::Set(mut state) => {
            state.cancel_sends = bounds.cancel;
            state.sweeps = bounds.sweeps;
            if let Some(contact) = contact {
                state.last_contact = Some(contact);
            }
            Change::Set(state)
        }
        Change::Nothing => match (old, contact) {
            (Some(old), Some(contact)) => Change::Set(RemoteState { last_contact: Some(contact), ..old.clone() }),
            _ => Change::Nothing,
        },
    }
}

/// What a `retry_remote_submit` outcome does to the record (o22.8, amending o19.3's removal): no
/// call (a refusal before any ssh) changes nothing; a call that got no answer is a failed check —
/// `known` stays (so a `NotOnServer` keeps its Retry, disabled while `last_contact` is `Failed`),
/// `check_error` says why; a call the server answered — whatever it answered — leaves the job in a
/// state only the next status step can name, so `known` and `check_error` are cleared, as the
/// removal did, and the record keeps its contact and bounds. A job with no record gets one.
pub fn after_retry(old: Option<&RemoteState>, bounds: Bounds, contact: Option<Contact>) -> Change {
    let Some(contact) = contact else { return Change::Nothing };
    let mut state = old.cloned().unwrap_or_default();
    match &contact {
        Contact::Failed(why) => state.check_error = Some(why.clone()),
        Contact::Ok => {
            state.known = None;
            state.check_error = None;
        }
    }
    state.last_contact = Some(contact);
    state.fetch = bounds.fetch;
    state.cancel_sends = bounds.cancel;
    state.sweeps = bounds.sweeps;
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

    fn apply(&mut self, job_id: &str, change: Change, sink: &dyn PollerSink) {
        match change {
            Change::Nothing => {}
            Change::Remove => self.change(job_id, None, sink),
            Change::Set(state) => self.change(job_id, Some(state), sink),
        }
    }
}

impl RemoteStates {
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> MutexGuard<'_, Records> {
        self.inner.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// [`Self::apply_with_contact`] without a contact: the tests' shorthand. Every production step
    /// applies with the contact of its last call.
    #[cfg(test)]
    pub fn apply(&self, guard: &InFlightGuard, step: &StatusStep, memory: &PollerMemory, sink: &dyn PollerSink) {
        self.apply_with_contact(guard, step, None, memory, sink);
    }

    /// Apply the status step `step` of the job `guard` holds, with the contact of the step's last
    /// call ([`next_with`], o19.3, o22.8). Taking the guard is the point: the change is made, and
    /// emitted, while the job is still claimed (o19.4), so no later command on the job can be
    /// overtaken by it. The bounds are read from `memory` under R — R before M (o19.5).
    pub fn apply_with_contact(
        &self,
        guard: &InFlightGuard,
        step: &StatusStep,
        contact: Option<Contact>,
        memory: &PollerMemory,
        sink: &dyn PollerSink,
    ) {
        let job_id = guard.job_id();
        let mut records = self.lock();
        let bounds = Bounds::read(memory, job_id);
        let change = next_with(records.jobs.get(job_id), step, bounds, contact);
        records.apply(job_id, change, sink);
    }

    /// A `retry_remote_submit` outcome's contact ([`after_retry`], o22.8). The caller holds the
    /// job's in-flight guard (the retry command's guarded work, o19.4). R before M.
    pub fn note_retry(&self, job_id: &str, contact: Option<Contact>, memory: &PollerMemory, sink: &dyn PollerSink) {
        let mut records = self.lock();
        let bounds = Bounds::read(memory, job_id);
        let change = after_retry(records.jobs.get(job_id), bounds, contact);
        records.apply(job_id, change, sink);
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
        RemoteState { known, check_error: check_error.map(String::from), fetch: NO_STRIKES, ..RemoteState::default() }
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
            last_contact: Some(Contact::Failed("ssh: timeout".into())),
            cancel_sends: CancelSends::from_failures(1),
            sweeps: Sweeps::from_count(3),
        };
        assert_eq!(
            serde_json::to_value(RemoteStateRead { seq: 7, state: Some(state) }).unwrap(),
            serde_json::json!({
                "seq": 7,
                "state": {
                    "known": { "kind": "labelled", "detail": "not_on_server" },
                    "check_error": "x",
                    "fetch": { "strikes": 3, "stopped": true },
                    "last_contact": { "kind": "failed", "detail": "ssh: timeout" },
                    "cancel_sends": { "failures": 1, "stopped": false },
                    "sweeps": { "count": 3, "stopped": true }
                }
            })
        );
        assert_eq!(serde_json::to_value(RemoteState::default()).unwrap()["last_contact"], serde_json::Value::Null, "no call yet");
        assert_eq!(
            serde_json::to_value(Known::Shown(Outcome::Lost { orphans: vec![4] })).unwrap(),
            serde_json::json!({ "kind": "shown", "detail": { "outcome": "lost", "orphans": [4] } })
        );
        assert_eq!(serde_json::to_value(Known::Fetching).unwrap(), serde_json::json!({ "kind": "fetching" }));
        assert_eq!(serde_json::to_value(RemoteStateRead { seq: 0, state: None }).unwrap(), serde_json::json!({ "seq": 0, "state": null }));
    }

    // ---- o22: the delivery's steps, the last contact, the cancel bound, a retry's outcome ----------

    fn bounds(strikes: u32, failures: u32) -> Bounds {
        Bounds { fetch: FetchState::from_strikes(strikes), cancel: CancelSends::from_failures(failures), sweeps: Sweeps::default() }
    }

    /// o22.4: what each delivery step does to the record — from a record that holds a label and a
    /// check error, and from none. `CancelNotSent` is a check error (known kept); `CancelStopped`
    /// shows what it saw like `Shown` / `Labelled` (o22.12.2), or the prepare's reason as a check
    /// error with `known` kept when it saw nothing; `EndedBeforeCancel` reads `Fetching`; `CancelDelivered` reads
    /// as an ordinary step reads its verdict (a fetching one as `Fetching`, `Indeterminate` as a
    /// check error).
    #[test]
    fn each_delivery_step_maps_to_its_change() {
        let old = with(Some(Known::Labelled(Label::NotOnServer)), Some("ssh timed out"));
        let completed = Outcome::Completed { late_cancel: true };
        let cancelling = Outcome::Cancelling { session: 2 };
        let cases: Vec<(StatusStep, Change, Change)> = vec![
            (
                StatusStep::CancelNotSent("cancel.sh exited 3".into()),
                set(with(Some(Known::Labelled(Label::NotOnServer)), Some("cancel.sh exited 3"))),
                set(with(None, Some("cancel.sh exited 3"))),
            ),
            (StatusStep::CancelStopped { seen: None, unready: None }, set(RemoteState { check_error: None, ..old.clone() }), set(with(None, None))),
            (
                StatusStep::CancelStopped { seen: None, unready: Some("prepare: refused".into()) },
                set(with(Some(Known::Labelled(Label::NotOnServer)), Some("prepare: refused"))),
                set(with(None, Some("prepare: refused"))),
            ),
            (
                StatusStep::CancelStopped { seen: Some(Seen::Label(Label::SubmitInterrupted)), unready: None },
                set(with(Some(Known::Labelled(Label::SubmitInterrupted)), None)),
                set(with(Some(Known::Labelled(Label::SubmitInterrupted)), None)),
            ),
            (
                StatusStep::CancelStopped { seen: Some(Seen::Outcome(Outcome::Lost { orphans: vec![] })), unready: None },
                set(with(Some(Known::Shown(Outcome::Lost { orphans: vec![] })), None)),
                set(with(Some(Known::Shown(Outcome::Lost { orphans: vec![] })), None)),
            ),
            (
                StatusStep::CancelStopped { seen: Some(Seen::Outcome(Outcome::Indeterminate)), unready: None },
                set(RemoteState { check_error: shown_message(&Outcome::Indeterminate), ..old.clone() }),
                set(with(None, shown_message(&Outcome::Indeterminate).as_deref())),
            ),
            (
                StatusStep::EndedBeforeCancel(Outcome::Completed { late_cancel: false }),
                set(with(Some(Known::Fetching), None)),
                set(with(Some(Known::Fetching), None)),
            ),
            (StatusStep::CancelDelivered(completed), set(with(Some(Known::Fetching), None)), set(with(Some(Known::Fetching), None))),
            (
                StatusStep::CancelDelivered(cancelling.clone()),
                set(with(Some(Known::Shown(cancelling.clone())), None)),
                set(with(Some(Known::Shown(cancelling)), None)),
            ),
            (
                StatusStep::CancelDelivered(Outcome::Cancelled { started: true }),
                set(with(Some(Known::Shown(Outcome::Cancelled { started: true })), None)),
                set(with(Some(Known::Shown(Outcome::Cancelled { started: true })), None)),
            ),
            (
                StatusStep::CancelDelivered(Outcome::Indeterminate),
                set(RemoteState { check_error: shown_message(&Outcome::Indeterminate), ..old.clone() }),
                set(with(None, shown_message(&Outcome::Indeterminate).as_deref())),
            ),
        ];
        for (step, from_old, from_none) in cases {
            assert_eq!(next(Some(&old), &step, NO_STRIKES), from_old, "{step:?} over a record");
            assert_eq!(next(None, &step, NO_STRIKES), from_none, "{step:?} over none");
        }
    }

    /// o22.8: a step that made a call writes its contact; one that made none (`FetchStopped`, a
    /// `CancelStopped` before the send) keeps the last; `RowChanged` after a call updates an existing
    /// record but creates none; the removing steps remove. The cancel bound is the counter's now.
    #[test]
    fn a_steps_contact_and_the_cancel_bound_reach_the_record() {
        let failed = Some(Contact::Failed("ssh: timeout".into()));
        let old = RemoteState { last_contact: Some(Contact::Ok), ..with(Some(Known::Shown(Outcome::Running)), None) };
        let Change::Set(away) = next_with(Some(&old), &StatusStep::CheckFailed("ssh: timeout".into()), bounds(0, 0), failed.clone()) else {
            panic!("a failed check sets")
        };
        assert_eq!((away.known.clone(), away.last_contact.clone()), (Some(Known::Shown(Outcome::Running)), failed.clone()));
        let Change::Set(kept) = next_with(Some(&away), &StatusStep::FetchStopped, bounds(3, 0), None) else { panic!("sets") };
        assert_eq!(kept.last_contact, failed, "no call keeps the last contact");
        let Change::Set(after) = next_with(Some(&away), &StatusStep::RowChanged, bounds(0, 0), Some(Contact::Ok)) else {
            panic!("a call on an existing record writes its contact")
        };
        assert_eq!(after, RemoteState { last_contact: Some(Contact::Ok), ..away.clone() });
        assert_eq!(next_with(None, &StatusStep::RowChanged, bounds(0, 0), Some(Contact::Ok)), Change::Nothing, "creates none");
        assert_eq!(next_with(Some(&away), &StatusStep::Busy, bounds(0, 0), None), Change::Nothing);
        assert_eq!(next_with(Some(&away), &StatusStep::Cancelled, bounds(0, 0), Some(Contact::Ok)), Change::Remove);
        let Change::Set(third) = next_with(Some(&old), &StatusStep::CancelNotSent("rc 3".into()), bounds(0, 3), Some(Contact::Ok)) else {
            panic!("sets")
        };
        assert_eq!(third.cancel_sends, CancelSends { failures: 3, stopped: true }, "the third failure already reads stopped");
    }

    /// o22.13.3: a sweep shows what it classified (`known`) and the failure the server answered with
    /// (`check_error`); one that classified nothing keeps what was seen. The sweep bound is the
    /// counter's now, the third reading stopped. NEGATIVE CONTROL: map `Swept` like `CheckFailed`
    /// (keep `known`) → red.
    #[test]
    fn a_sweep_maps_to_its_change_and_carries_the_sweep_bound() {
        let cancelling = Outcome::Cancelling { session: 2 };
        let old = with(Some(Known::Shown(Outcome::Cancelling { session: 3 })), Some("ssh timed out"));
        let swept = StatusStep::Swept { outcome: Some(cancelling.clone()), error: None };
        assert_eq!(next(Some(&old), &swept, NO_STRIKES), set(with(Some(Known::Shown(cancelling.clone())), None)));
        let failed = StatusStep::Swept { outcome: Some(cancelling.clone()), error: Some("cancel.sh exited 3".into()) };
        assert_eq!(next(None, &failed, NO_STRIKES), set(with(Some(Known::Shown(cancelling)), Some("cancel.sh exited 3"))));
        let blind = StatusStep::Swept { outcome: None, error: Some("collect: broken".into()) };
        assert_eq!(next(Some(&old), &blind, NO_STRIKES), set(with(old.known.clone(), Some("collect: broken"))));
        let third = Bounds { sweeps: Sweeps::from_count(MAX_SWEEPS_WITHOUT_PROGRESS), ..bounds(0, 0) };
        let Change::Set(after) = next_with(Some(&old), &swept, third, Some(Contact::Ok)) else { panic!("sets") };
        assert_eq!(after.sweeps, Sweeps { count: 3, stopped: true }, "the third sweep without progress reads stopped");
        assert_eq!(Sweeps::from_count(2), Sweeps { count: 2, stopped: false });
    }

    /// o22.8 (amends o19.3): a retry no longer removes the record. No call → nothing; no answer →
    /// a failed check with `known` kept (Retry stays, disabled); an answer → `known` and the check
    /// error cleared, contact kept; no record yet → one is made. NEGATIVE CONTROL: make `after_retry`
    /// return `Change::Remove` → red.
    #[test]
    fn a_retrys_outcome_writes_its_contact_and_keeps_the_record() {
        let labelled = with(Some(Known::Labelled(Label::NotOnServer)), None);
        assert_eq!(after_retry(Some(&labelled), bounds(0, 0), None), Change::Nothing);
        let failed = Contact::Failed("ssh exited with 255".into());
        let Change::Set(away) = after_retry(Some(&labelled), bounds(0, 0), Some(failed.clone())) else { panic!("sets") };
        assert_eq!(away.known, Some(Known::Labelled(Label::NotOnServer)), "Retry stays, disabled");
        assert_eq!((away.check_error.as_deref(), away.last_contact), (Some("ssh exited with 255"), Some(failed.clone())));
        let Change::Set(answered) = after_retry(Some(&labelled), bounds(0, 0), Some(Contact::Ok)) else { panic!("sets") };
        assert_eq!(answered, RemoteState { last_contact: Some(Contact::Ok), ..RemoteState::default() });
        let Change::Set(fresh) = after_retry(None, bounds(0, 0), Some(failed.clone())) else { panic!("a record is made") };
        assert_eq!(fresh.last_contact, Some(failed));
    }
}
