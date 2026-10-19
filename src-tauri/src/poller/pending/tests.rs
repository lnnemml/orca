//! The pending submit's re-check (ADR-024 o23) over a **fake** server: the order of the calls, what
//! each outcome does to the pending flag, the counter, the record and the emits — and the pure
//! decision table. The fake asserts on every call that the database lock is free. The real scripts
//! run the same flow end to end in `remote::backend_e2e_tests`.
//!
//! Every test names the 23.11 / 23.13.9 control it is (NEGATIVE CONTROL: the one-line break that
//! turns it red, demonstrated in the unit's log entry).

use std::sync::Mutex;
use std::time::Duration;

use super::super::tests::{record, Remote};
use super::super::Event;
use super::*;
use crate::models::job::JobStatus;
use crate::remote::classify::Outcome;
use crate::remote::scripts::{INSTALL, LABEL, MKJOB, PREPARE, RUN, SUBMIT};
use crate::remote::ssh::{CommandRunner, TransportError, SSH_PROGRAM};
use crate::remote::submit::ProcessRole;
use crate::remote::sync::RSYNC_PROGRAM;
use crate::ssh_backend::tests::{
    blocked_by, exited, expected_coords, install_ok, label_reply, mkjob_ok, ok, prepare_ready, ran, rec, snapshot, submit_reply,
    Reply, JOB, NOT_ON_SERVER, ROOT,
};

// ---- the fake server -------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Call {
    Prepare,
    Install,
    Upload,
    Submit,
    Label,
    Mkjob,
    /// `cancel.sh` through the trampoline.
    Cancel,
    /// `collect.sh` through the trampoline.
    Collect,
}
use Call::*;

fn call_of(program: &str, args: &[String], stdin: &[u8]) -> (Call, Vec<String>) {
    if program == RSYNC_PROGRAM {
        assert_eq!(args.last().map(String::as_str), Some(format!("uni:{}/", expected_coords().job_dir).as_str()), "only uploads");
        return (Upload, Vec::new());
    }
    assert_eq!(program, SSH_PROGRAM);
    for (call, script) in [(Prepare, PREPARE), (Install, INSTALL), (Submit, SUBMIT), (Label, LABEL), (Mkjob, MKJOB), (Collect, RUN)] {
        if let Some(rest) = stdin.strip_prefix(script.as_bytes()) {
            let mut values: Vec<String> = rest.split(|b| *b == 0).map(|v| String::from_utf8(v.to_vec()).unwrap()).collect();
            assert_eq!(values.pop().as_deref(), Some(""));
            if call == Collect && values[1] == "cancel" {
                return (Cancel, values);
            }
            return (call, values);
        }
    }
    panic!("a script the re-check never sends");
}

/// Answers each call with `respond`, records its kind and the values of each `submit.sh` call, and
/// checks on every call that the database lock is free.
struct Fake<'a> {
    r: &'a Remote,
    respond: Box<dyn Fn(Call, &[String]) -> Reply + 'a>,
    calls: Mutex<Vec<Call>>,
    submits: Mutex<Vec<Vec<String>>>,
}

impl<'a> Fake<'a> {
    fn new(r: &'a Remote, respond: impl Fn(Call, &[String]) -> Reply + 'a) -> Self {
        Fake { r, respond: Box::new(respond), calls: Mutex::default(), submits: Mutex::default() }
    }

    fn calls(&self) -> Vec<Call> {
        self.calls.lock().unwrap().clone()
    }
}

impl CommandRunner for Fake<'_> {
    fn run(&self, program: &str, args: &[String], stdin: &[u8], _timeout: Duration) -> Reply {
        let (call, values) = call_of(program, args, stdin);
        assert!(self.r.w.db.0.try_lock().is_ok(), "the database lock is held across a {call:?} call");
        if call == Submit {
            self.submits.lock().unwrap().push(values.clone());
        }
        self.calls.lock().unwrap().push(call);
        (self.respond)(call, &values)
    }
}

/// A server whose `submit.sh` answers `outcome` (a record line plus its bytes); everything else
/// ready, the label "not on the server", the collect nothing ran.
fn answering(outcome: &'static str) -> impl Fn(Call, &[String]) -> Reply {
    answering_with(move |_| outcome.to_string())
}

/// [`answering`] whose `n`-th `submit.sh` call (from 0) answers `outcome(n)`.
fn answering_with(outcome: impl Fn(usize) -> String) -> impl Fn(Call, &[String]) -> Reply {
    let n = Mutex::new(0);
    move |call, values| match call {
        Prepare => prepare_ready(values),
        Install => install_ok(values),
        Upload => ok(Vec::new()),
        Submit => {
            let mut i = n.lock().unwrap();
            *i += 1;
            submit_reply(values, &outcome(*i - 1))
        }
        Label => label_reply(values, NOT_ON_SERVER),
        Mkjob => mkjob_ok(values),
        Cancel => ran(values, 0, b"", b""),
        Collect => ran(values, 0, &snapshot(true, None, None, (None, None)), b""),
    }
}

/// The ssh timeout of the runner: a call that got no answer.
fn timeout() -> Reply {
    Err(TransportError::Timeout { program: "ssh".into(), secs: 60 })
}

const SINCE: &str = "2026-10-19 08:00:00";

/// A [`Remote`] whose queued job is pending since [`SINCE`].
fn pending() -> Remote {
    let r = Remote::new();
    r.w.sql(&format!("UPDATE jobs SET submit_pending_since = '{SINCE}'"));
    r
}

fn flag(r: &Remote) -> Option<String> {
    r.w.job().submit_pending_since
}

fn step(r: &Remote, fake: &Fake<'_>) -> StatusStep {
    r.poller(fake).status_step(JOB).unwrap()
}

fn refusals(r: &Remote) -> u32 {
    r.memory.get(JOB).pending_refusals
}

fn statuses(r: &Remote) -> Vec<JobStatus> {
    r.sink.take().into_iter().filter_map(|e| match e { Event::Status(_, s) => Some(s), _ => None }).collect()
}

fn waiting(r: &Remote) -> Option<PendingSubmit> {
    record(r).and_then(|s| s.pending_submit)
}

const BLOCKED: &str = "blocked 1\nprocess 4242\nkind pinned\ncores 4\n0-23\njob 10\n/elsewhere\n";

/// `r` after a restart: a fresh memory, record map, live log and sink; the database (and the flag in
/// it) kept.
fn restarted(r: Remote) -> Remote {
    let fresh = Remote::new();
    Remote { w: r.w, data_dir: r.data_dir, ..fresh }
}

// ---- the pure decision (o23.6, o23.13.1–6) -----------------------------------------------------

/// Every outcome of a re-check's `submit.sh`, decided outcome-first and never by reason text.
/// NEGATIVE CONTROL: decide `Unknown` by the contact alone the other way (`Ok` → `Lost`) → red.
#[test]
fn every_submit_outcome_has_its_decision() {
    use crate::remote::submit::KupEvidence;
    let blocker = Blocker::Process { pid: 7, role: ProcessRole::Pinned, cores: "0".into(), job_dir: None };
    let refused = |refusal| SubmitOutcome::NotClaimed { step: SubmitStep::Submit, refusal };
    let mismatch = refused(Refusal::UploadMismatch { missing: vec!["input.inp".into()], extra: vec![], differing: vec![] });
    let ok_contact = Some(Contact::Ok);
    let lost = Some(Contact::Failed("ssh exited with 255".into()));
    let unknown = SubmitOutcome::Unknown { reason: "submit: broken".into() };
    let table: Vec<(&str, SubmitOutcome, Option<Contact>, bool, bool, Decision)> = vec![
        ("enqueued", SubmitOutcome::Enqueued { tsp_id: 4 }, ok_contact.clone(), false, false, Decision::Enqueued(4)),
        ("slot busy", refused(Refusal::Blocked { blockers: vec![blocker.clone()] }), ok_contact.clone(), false, false,
            Decision::Wait(Wait::SlotBusy { blockers: vec![ShownBlocker { blocker, title: None }] })),
        ("slot check error", refused(Refusal::SlotCheckFailed("x".into())), ok_contact.clone(), false, false, Decision::Wait(Wait::SlotCheckFailed("x".into()))),
        ("lock busy", refused(Refusal::LockBusy { openers: "9 bash".into() }), ok_contact.clone(), false, false,
            Decision::Wait(Wait::LockBusy { openers: "9 bash".into() })),
        ("upload mismatch", mismatch.clone(), ok_contact.clone(), false, false, Decision::ReUpload),
        ("upload mismatch after a lost reply", mismatch.clone(), ok_contact.clone(), true, false, Decision::Deferred(match &mismatch {
            SubmitOutcome::NotClaimed { refusal, .. } => refusal.to_string(),
            _ => unreachable!(),
        })),
        ("upload mismatch after its one retry", mismatch.clone(), ok_contact.clone(), false, true, Decision::End),
        ("a marker", refused(Refusal::Other("marker: x".into())), ok_contact.clone(), false, false, Decision::End),
        ("a slot row", refused(Refusal::Other("row: x".into())), ok_contact.clone(), false, false, Decision::End),
        ("prepare refused", crate::ssh_backend::not_claimed(SubmitStep::Prepare, "x".into()), ok_contact.clone(), false, false, Decision::End),
        ("KillUserProcesses", SubmitOutcome::KillUserProcesses { evidence: KupEvidence { rc: 0, stdout: "b true\n".into(), stderr: String::new() } },
            ok_contact.clone(), false, false, Decision::End),
        ("failed after the claim", SubmitOutcome::FailedAfterClaim { reason: "x".into() }, ok_contact.clone(), false, false, Decision::End),
        ("unreadable: answered, did not parse", unknown.clone(), ok_contact.clone(), false, false, Decision::Wait(Wait::Unreadable("submit: broken".into()))),
        ("lost: no answer", unknown.clone(), lost, false, false, Decision::Lost("submit: broken".into())),
        ("no call made", unknown, None, false, false, Decision::End),
    ];
    for (case, outcome, contact, lost_before, reuploaded, want) in table {
        assert_eq!(decide(&outcome, contact.as_ref(), lost_before, reuploaded), want, "{case}");
    }
}

// ---- 23.11 / 23.13.9 over the fake ---------------------------------------------------------------

/// o23.2, o23.3: a free slot submits at the next step — no label call, no upload (the post-condition
/// passed) — and the pending state ends: the flag NULLed with the message, `job:status` emitted, the
/// counter reset, the record's pending submit gone. NEGATIVE CONTROLS: "a free slot that does not
/// enqueue at the next step" — run the label path instead of the re-check (drop the branch in
/// `run_status`) → red; "a re-check that uploads when the post-condition passes" → red (the calls).
#[test]
fn a_free_slot_enqueues_at_the_next_step_and_ends_the_pending_state() {
    let r = pending();
    r.w.sql("UPDATE jobs SET error_message = 'not submitted: slot busy'");
    r.memory.count_refusal(JOB);
    let fake = Fake::new(&r, answering("enqueued 5\n"));
    let s = step(&r, &fake);
    assert_eq!(s, StatusStep::SubmitRechecked { since: SINCE.into(), recheck: Recheck::Enqueued { tsp_id: 5 } });
    assert_eq!(fake.calls(), [Prepare, Submit], "the ready step, then submit.sh — no label, no upload");
    let job = r.w.job();
    assert_eq!((job.submit_pending_since, job.error_message, job.status), (None, None, JobStatus::Queued));
    assert_eq!(statuses(&r), [JobStatus::Queued], "the flag's write emits job:status");
    assert_eq!(refusals(&r), 0, "the counter ends with the pending state");
    assert_eq!(waiting(&r), None);
}

/// The re-check's plan is a retry's (o23.3.1): the profile's **current** mask, the input from
/// `input_content` aligned to it, the recorded coordinates — and the local dir as the upload source.
#[test]
fn the_recheck_submits_the_plan_a_retry_would() {
    let r = pending();
    r.w.sql("UPDATE jobs SET input_content = '! HF\n%pal nprocs 48 end\n'; UPDATE server_profiles SET core_mask = '0-5'");
    let fake = Fake::new(&r, answering("enqueued 1\n"));
    step(&r, &fake);
    let sent = fake.submits.lock().unwrap()[0].clone();
    let coords = expected_coords();
    assert_eq!(&sent[..4], [coords.job_dir.as_str(), ROOT, coords.socket.as_str(), "0-5"], "recorded coordinates, current mask");
    assert_eq!(std::fs::read_to_string(r.local().join("input.inp")).unwrap(), "! HF\n%pal nprocs 6 end\n", "o14.2: nprocs ≤ the mask's CPUs");
}

/// o23.6: slot busy stays pending — the first time kept, one refusal counted, the blockers in the
/// record, nothing in the row's status. NEGATIVE CONTROLS: "a slot-busy answer that does not make the
/// row pending" (record it as `Clear`) → red; "a later refusal overwriting submit_pending_since"
/// (`datetime('now')` without the COALESCE in `record_attempt`) → red.
#[test]
fn a_blocked_recheck_stays_pending_keeps_its_first_time_and_counts() {
    let r = pending();
    let fake = Fake::new(&r, answering(BLOCKED));
    let s = step(&r, &fake);
    let StatusStep::SubmitRechecked { recheck: Recheck::Waiting(Wait::SlotBusy { blockers }), .. } = &s else { panic!("{s:?}") };
    assert_eq!(blockers.len(), 1);
    assert_eq!(fake.calls(), [Prepare, Submit]);
    assert_eq!(flag(&r).as_deref(), Some(SINCE), "a later refusal keeps the first time");
    assert_eq!(refusals(&r), 1);
    let p = waiting(&r).expect("the record shows the pending submit");
    assert_eq!((p.since.as_str(), p.refusals, p.notified), (SINCE, 1, false));
    assert!(r.w.job().error_message.is_some_and(|m| m.contains("slot busy")));
}

/// MED-5, o23.13.2: a slot check error — step 3's socket `Error` or a bounded child's timeout — stays
/// pending and counts. NEGATIVE CONTROL: "a step-3 socket Error that ends it instead" (decide
/// `SlotCheckFailed` as `End`) → red.
#[test]
fn a_slot_check_error_stays_pending_and_counts() {
    let r = pending();
    let fake = Fake::new(&r, answering("slot-check-error 39\nslot socket: tsp -l on /r exited 1 boom\n"));
    let s = step(&r, &fake);
    assert!(matches!(&s, StatusStep::SubmitRechecked { recheck: Recheck::Waiting(Wait::SlotCheckFailed(_)), .. }), "{s:?}");
    assert_eq!((flag(&r).as_deref(), refusals(&r)), (Some(SINCE), 1));
}

/// o23.6, o23.13.4: no answer before `submit.sh` keeps the pending state and counts nothing — not
/// even after earlier refusals (a transport failure never resets the counter). NEGATIVE CONTROLS: "a
/// transport failure that ends the pending state or counts" → red.
#[test]
fn a_transport_failure_neither_ends_nor_counts() {
    let r = pending();
    r.memory.count_refusal(JOB);
    let fake = Fake::new(&r, |call, values| if call == Prepare { timeout() } else { answering("enqueued 1\n")(call, values) });
    let s = step(&r, &fake);
    assert!(matches!(s, StatusStep::CheckFailed(_)), "{s:?}");
    assert_eq!(fake.calls(), [Prepare]);
    assert_eq!((flag(&r).as_deref(), refusals(&r)), (Some(SINCE), 1), "pending, nothing counted, nothing reset");
    assert!(statuses(&r).is_empty(), "nothing written");
    let record = record(&r).expect("a record");
    assert_eq!(record.check_error.map(|e| e.answered), Some(false), "no answer");
}

/// o23.6 (Anton), o23.13.6: a lost submit reply stays pending, counts nothing, and the next step
/// uploads nothing — an upload mismatch there is deferred, not retried. NEGATIVE CONTROLS: "an
/// `Unknown` that ends it" → red; "an upload in the step after an `Unknown`" (ignore
/// `submit_reply_lost`) → red (an `Upload` call).
#[test]
fn a_lost_reply_stays_pending_and_the_next_step_uploads_nothing() {
    let r = pending();
    let lost = Mutex::new(true);
    let fake = Fake::new(&r, |call, values| match call {
        Submit if std::mem::replace(&mut *lost.lock().unwrap(), false) => timeout(),
        _ => answering("upload-mismatch\nmissing 1\nname 9\ninput.inp\nextra 0\ndiffering 0\n")(call, values),
    });
    let s = step(&r, &fake);
    assert!(matches!(&s, StatusStep::SubmitRechecked { recheck: Recheck::Lost(_), .. }), "{s:?}");
    assert_eq!((flag(&r).as_deref(), refusals(&r)), (Some(SINCE), 0));
    assert!(r.memory.get(JOB).submit_reply_lost);
    let s = step(&r, &fake);
    assert!(matches!(&s, StatusStep::SubmitRechecked { recheck: Recheck::Deferred(_), .. }), "{s:?}");
    assert_eq!(fake.calls(), [Prepare, Submit, Prepare, Submit], "no upload in the step after the lost reply");
    assert_eq!((flag(&r).as_deref(), refusals(&r)), (Some(SINCE), 0));
    assert!(!r.memory.get(JOB).submit_reply_lost, "an answered submit.sh clears the mark");
    let s = step(&r, &fake);
    assert!(matches!(&s, StatusStep::SubmitRechecked { recheck: Recheck::Ended(_), .. }), "the step after re-uploads once, then ends: {s:?}");
    assert_eq!(fake.calls()[4..], [Prepare, Submit, Upload, Submit]);
}

/// o23.14.5 (verifier F1): the lost-reply mark survives a step without an answer — a transport
/// failure before `submit.sh` clears nothing — so the first step that reaches `submit.sh` after the
/// lost reply still uploads nothing and defers a mismatch. NEGATIVE CONTROL (V2): clear
/// `submit_reply_lost` on the ready step's `CheckFailed` → red (an `Upload` call).
#[test]
fn the_lost_reply_mark_survives_a_transport_failure() {
    let r = pending();
    let calls = Mutex::new(0);
    let fake = Fake::new(&r, |call, values| {
        let n = {
            let mut n = calls.lock().unwrap();
            *n += 1;
            *n
        };
        match (n, call) {
            // Step 1: prepare, then `submit.sh` without a reply.
            (2, Submit) => timeout(),
            // Step 2: the prepare without an answer.
            (3, Prepare) => timeout(),
            _ => answering("upload-mismatch\nmissing 1\nname 9\ninput.inp\nextra 0\ndiffering 0\n")(call, values),
        }
    });
    assert!(matches!(step(&r, &fake), StatusStep::SubmitRechecked { recheck: Recheck::Lost(_), .. }));
    assert!(r.memory.get(JOB).submit_reply_lost);
    assert!(matches!(step(&r, &fake), StatusStep::CheckFailed(_)));
    assert!(r.memory.get(JOB).submit_reply_lost, "a step without an answer keeps the mark");
    let s = step(&r, &fake);
    assert!(matches!(&s, StatusStep::SubmitRechecked { recheck: Recheck::Deferred(_), .. }), "{s:?}");
    assert_eq!(fake.calls(), [Prepare, Submit, Prepare, Prepare, Submit], "no upload after the lost reply");
    assert_eq!((flag(&r).as_deref(), refusals(&r)), (Some(SINCE), 0));
}

/// o23.3.3: an upload mismatch uploads once and runs `submit.sh` once more in the same step; a
/// second mismatch ends the pending state. NEGATIVE CONTROL: "never re-uploads on an upload
/// mismatch" (decide `ReUpload` as `End`) → red.
#[test]
fn an_upload_mismatch_reuploads_once_then_submits() {
    let r = pending();
    let mismatch = "upload-mismatch\nmissing 0\nextra 0\ndiffering 1\nname 9\ninput.inp\n";
    let fake = Fake::new(&r, answering_with(move |n| if n == 0 { mismatch.to_string() } else { "enqueued 2\n".to_string() }));
    assert_eq!(step(&r, &fake), StatusStep::SubmitRechecked { since: SINCE.into(), recheck: Recheck::Enqueued { tsp_id: 2 } });
    assert_eq!(fake.calls(), [Prepare, Submit, Upload, Submit]);
    assert_eq!(flag(&r), None);

    let r = pending();
    let fake = Fake::new(&r, answering(mismatch));
    let s = step(&r, &fake);
    assert!(matches!(&s, StatusStep::SubmitRechecked { recheck: Recheck::Ended(why), .. } if why.contains("differing [input.inp]")), "{s:?}");
    assert_eq!(fake.calls(), [Prepare, Submit, Upload, Submit], "one upload, never two");
    assert_eq!(flag(&r), None);
}

/// o23.13.4, o23.13.9: the re-upload's no answer comes from its **own** contact — rsync's 12 writes
/// none, so it is no answer (pending, not counted), even after the answered `submit.sh` before it;
/// an rsync the server answered with a failure (23) ends it. NEGATIVE CONTROL: "a re-upload ended by
/// rsync 12 treated as an answered refusal" (decide the upload from the step tracker's `last()`) →
/// red.
#[test]
fn a_reupload_without_an_answer_stays_pending() {
    let mismatch = "upload-mismatch\nmissing 1\nname 9\ninput.inp\nextra 0\ndiffering 0\n";
    for (code, ends) in [(12, false), (255, false), (23, true)] {
        let r = pending();
        let fake = Fake::new(&r, move |call, values| if call == Upload { exited(code, Vec::new()) } else { answering(mismatch)(call, values) });
        let s = step(&r, &fake);
        assert_eq!(fake.calls(), [Prepare, Submit, Upload], "rsync {code}");
        if ends {
            assert!(matches!(&s, StatusStep::SubmitRechecked { recheck: Recheck::Ended(_), .. }), "rsync {code}: {s:?}");
            assert_eq!(flag(&r), None, "rsync {code}");
        } else {
            assert!(matches!(s, StatusStep::CheckFailed(_)), "rsync {code}: {s:?}");
            assert_eq!((flag(&r).as_deref(), refusals(&r)), (Some(SINCE), 0), "rsync {code}");
        }
    }
}

/// o23.13.1: lock busy stays pending, counts, and reaches its own notice at the third refusal.
/// NEGATIVE CONTROLS: "a lock-busy loop that reaches its notice" — lock busy not counted → red; "the
/// notice before the 3rd refusal" (`PENDING_NOTICE_AT` 2) → red.
#[test]
fn lock_busy_stays_pending_counts_and_reaches_its_notice() {
    let r = pending();
    let fake = Fake::new(&r, answering("lock-busy 18\n77 bash -s (other)\n"));
    // The literal 3 (l, m2: "after 3 refusals"), never the constant: a changed constant must go red.
    for n in 1..=3 {
        let s = step(&r, &fake);
        assert!(matches!(&s, StatusStep::SubmitRechecked { recheck: Recheck::Waiting(Wait::LockBusy { .. }), .. }), "{s:?}");
        let p = waiting(&r).unwrap();
        assert_eq!(p.refusals, n);
        if n < 3 {
            assert_eq!((p.notified, p.notice), (false, None), "no notice before the third refusal");
        } else {
            assert!(p.notified);
            assert_eq!(p.notice.as_deref(), Some("Waiting: the account lock is held by 77 bash -s (other)"));
        }
    }
    assert_eq!(flag(&r).as_deref(), Some(SINCE));
}

/// o23.13.1: an unreadable answer (the server answered, the reply did not parse) stays pending,
/// counts and notifies, in its own words. NEGATIVE CONTROL: "an unreadable answer that ends the
/// pending state or never notifies" (decide it as `Lost`) → red.
#[test]
fn an_unreadable_answer_stays_pending_counts_and_notifies() {
    let r = pending();
    let fake = Fake::new(&r, |call, values| if call == Submit { ok(b"orcastudio-submit 1\nargc 1\n".to_vec()) } else { answering("enqueued 1\n")(call, values) });
    for _ in 0..3 {
        let s = step(&r, &fake);
        assert!(matches!(&s, StatusStep::SubmitRechecked { recheck: Recheck::Waiting(Wait::Unreadable(_)), .. }), "{s:?}");
    }
    assert_eq!(flag(&r).as_deref(), Some(SINCE));
    let notice = waiting(&r).and_then(|p| p.notice).expect("the notice is due");
    assert!(notice.starts_with("Waiting: the server's reply could not be read: submit: "), "{notice}");
}

/// The notice uses the most recent kind (o23.13.1): two slot-busy refusals then a slot check error
/// read "the slot could not be checked".
#[test]
fn the_notice_uses_the_most_recent_kind() {
    let r = pending();
    let fake = Fake::new(&r, answering_with(|n| if n < 2 { BLOCKED.to_string() } else { "slot-check-error 3\nwhy\n".to_string() }));
    for _ in 0..3 {
        step(&r, &fake);
    }
    assert_eq!(waiting(&r).and_then(|p| p.notice).as_deref(), Some("Waiting: the slot could not be checked: why"));
}

/// o23.4: the counter is per launch — after a restart (memory gone, the flag kept) the step simply
/// re-checks and the notice comes back only after 3 more refusals. NEGATIVE CONTROL: "after a
/// restart, the notice before the 3rd new refusal" → red.
#[test]
fn after_a_restart_the_notice_comes_back_after_three_more_refusals() {
    let r = pending();
    let fake = Fake::new(&r, answering(BLOCKED));
    for _ in 0..3 {
        step(&r, &fake);
    }
    assert!(waiting(&r).unwrap().notified, "notified in the first launch");
    // A restart: a fresh memory and record map; the flag stays in the database.
    drop(fake);
    let r2 = restarted(r);
    let fake = Fake::new(&r2, answering(BLOCKED));
    for n in 1..3 {
        step(&r2, &fake);
        assert_eq!(waiting(&r2).map(|p| (p.refusals, p.notified)), Some((n, false)), "refusal {n} after the restart");
    }
    step(&r2, &fake);
    assert!(waiting(&r2).unwrap().notified);
    assert_eq!(flag(&r2).as_deref(), Some(SINCE));
}

/// o23.6: an answered refusal that needs a person ends the pending state — the flag NULLed with the
/// message in one write, `job:status` emitted, the counter reset. NEGATIVE CONTROL: "a non-slot
/// answered refusal ... that keeps it" (decide `Other` as a wait) → red.
#[test]
fn an_answered_refusal_ends_the_pending_state() {
    let kup = format!("refused-kup {}\nrc 0\n{}{}\n", "rc 0\n".len() + rec("stdout", "b true\n").len() + rec("stderr", "").len(), rec("stdout", "b true\n"), rec("stderr", ""));
    let cases: Vec<(&str, String)> = vec![
        ("a marker", rec("refused", "marker: /r/jobs/j1/.submitting exists")),
        ("a slot row", rec("refused", "row: a tsp row already holds it")),
        ("failed after the claim", rec("failed-after-claim", "enqueue: tsp exited 1")),
        ("KillUserProcesses", kup),
    ];
    for (case, outcome) in cases {
        let r = pending();
        r.memory.count_refusal(JOB);
        let fake = Fake::new(&r, answering_with(move |_| outcome.clone()));
        let s = step(&r, &fake);
        let StatusStep::SubmitRechecked { recheck: Recheck::Ended(why), .. } = &s else { panic!("{case}: {s:?}") };
        let job = r.w.job();
        assert_eq!(job.submit_pending_since, None, "{case}");
        assert_eq!(job.error_message.as_deref(), Some(why.as_str()), "{case}: the row says why");
        assert_eq!(statuses(&r), [JobStatus::Queued], "{case}");
        assert_eq!(refusals(&r), 0, "{case}");
        assert_eq!(waiting(&r), None, "{case}");
    }
    // A prepare the server refused (answered) ends it too.
    let r = pending();
    let fake = Fake::new(&r, |call, values| match call {
        Prepare => exited(3, format!("{}\n{}", crate::remote::prepare::PREPARE_HEADER, rec("error", "stat: Permission denied")).into_bytes()),
        _ => answering("enqueued 1\n")(call, values),
    });
    assert!(matches!(step(&r, &fake), StatusStep::SubmitRechecked { recheck: Recheck::Ended(_), .. }));
    assert_eq!((fake.calls(), flag(&r)), (vec![Prepare], None));
}

/// o23.6, o23.13.3, o23.13.9: a local refusal of the plan — the profile unverified (a mask edit
/// cleared its stamp, `server_profiles.rs`), or its host or root moved — ends the pending state in
/// its own guarded write, before any ssh, with `job:status`. NEGATIVE CONTROLS: "a local plan refusal
/// that keeps it" (map it to `CheckFailed`) → red; "a re-check after a mask edit that submits instead
/// of ending" (skip `run_target` in the plan) → red (a `Submit` call).
#[test]
fn a_local_plan_refusal_ends_the_pending_state_before_any_ssh() {
    for (case, sql) in [
        ("a mask edit (the stamp cleared with it)", "UPDATE server_profiles SET core_mask = '0-5', verified_at = NULL"),
        ("another host", "UPDATE server_profiles SET host = 'other'"),
        ("another root", "UPDATE server_profiles SET remote_scratch_dir = '/srv/os'"),
    ] {
        let r = pending();
        r.memory.count_refusal(JOB);
        r.w.sql(sql);
        let fake = Fake::new(&r, answering("enqueued 1\n"));
        let s = step(&r, &fake);
        assert!(matches!(&s, StatusStep::SubmitRechecked { recheck: Recheck::Ended(why), .. } if why.starts_with("The pending submit ended: ")), "{case}: {s:?}");
        assert_eq!(fake.calls(), [], "{case}: no ssh");
        assert_eq!(flag(&r), None, "{case}");
        assert!(r.w.job().error_message.is_some_and(|m| m.starts_with("The pending submit ended: ")), "{case}");
        assert_eq!(statuses(&r), [JobStatus::Queued], "{case}: the flag's write emits");
        assert_eq!(refusals(&r), 0, "{case}");
    }
}

/// o23.13.2, o23.13.9: a local DB or IO failure of the re-check is `CheckFailed`: pending, not
/// counted, no ssh. NEGATIVE CONTROL: "a DB/IO error that ends it" (send IO errors to the plan
/// refusal) → red.
#[test]
fn a_local_io_error_keeps_the_pending_state() {
    let r = pending();
    let input = r.local().join("input.inp");
    std::fs::remove_file(&input).unwrap();
    std::fs::create_dir_all(&input).unwrap();
    let fake = Fake::new(&r, answering("enqueued 1\n"));
    let s = step(&r, &fake);
    assert!(matches!(&s, StatusStep::CheckFailed(why) if why.contains("on this machine")), "{s:?}");
    assert_eq!(fake.calls(), []);
    assert_eq!((flag(&r).as_deref(), refusals(&r)), (Some(SINCE), 0));
}

/// o23.13.5, o23.13.9: a re-check whose plan is unchanged writes nothing in the local dir. NEGATIVE
/// CONTROL: "a rewrite of an unchanged local dir" (`write_if_changed` → a plain write) → red (the
/// mtime moves).
#[test]
fn a_recheck_never_rewrites_an_unchanged_local_dir() {
    let r = pending();
    let fake = Fake::new(&r, answering(BLOCKED));
    step(&r, &fake);
    let input = r.local().join("input.inp");
    let old = std::time::SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
    std::fs::File::options().write(true).open(&input).unwrap().set_modified(old).unwrap();
    step(&r, &fake);
    assert_eq!(std::fs::metadata(&input).unwrap().modified().unwrap(), old, "the local input was not rewritten");
}

/// o23.10, o22.13.1, o23.6: a pending cancel comes first — delivery's not-on-server branch (mkjob →
/// cancel.sh), no `submit.sh` — and `record_delivered` NULLs the flag; the next ordinary step labels
/// the job. NEGATIVE CONTROL: "the re-check placed before the pending-cancel branch" (swap the two in
/// `run_status`) → red (a `Submit` call).
#[test]
fn a_pending_cancel_is_delivered_before_any_recheck() {
    let r = pending();
    r.memory.count_refusal(JOB);
    r.w.sql("UPDATE jobs SET cancel_requested_at = '2026-10-19 08:01:00'");
    let fake = Fake::new(&r, answering("enqueued 1\n"));
    let s = step(&r, &fake);
    assert_eq!(s, StatusStep::CancelDelivered(Outcome::Cancelled { started: false }));
    assert_eq!(fake.calls(), [Prepare, Label, Mkjob, Cancel, Collect], "delivery, never submit.sh");
    let job = r.w.job();
    assert_eq!((job.submit_pending_since, job.cancel_requested_at), (None, None), "record_delivered ends the wait");
    assert_eq!(refusals(&r), 0);
    assert_eq!(waiting(&r), None);
}

/// o23.6: an Undo resumes the pending submit — the flag is untouched by it. NEGATIVE CONTROL: "an
/// Undo that clears the flag" (NULL it in `revoke_cancel_core`) → red.
#[test]
fn an_undo_leaves_the_pending_submit_to_resume() {
    let r = pending();
    r.w.sql("UPDATE jobs SET cancel_requested_at = '2026-10-19 08:01:00'");
    let undone = crate::commands::jobs::revoke_cancel_core(&r.w.db, &r.memory, &r.remote, &r.sink, JOB).unwrap();
    assert_eq!(undone, crate::commands::jobs::UndoResult::Undone);
    assert_eq!(flag(&r).as_deref(), Some(SINCE));
    let fake = Fake::new(&r, answering("enqueued 1\n"));
    assert!(matches!(step(&r, &fake), StatusStep::SubmitRechecked { recheck: Recheck::Enqueued { .. }, .. }), "the re-check resumes");
}

/// o23.7: a blocker is shown by a local job's title only when its job dir is exactly `<root>/jobs/<id>`
/// and that job's recorded dir; a dir outside `<root>/jobs/` or deeper — even ending in the same id —
/// falls back to pid and cores. NEGATIVE CONTROL: "a blocker mapped to the wrong local job (a job dir
/// outside `<root>/jobs/`)" — take the id as the dir's last component → red.
#[test]
fn a_blocker_is_shown_by_a_local_jobs_title_only_from_its_own_job_dir() {
    let r = pending();
    r.w.sql(&format!(
        "INSERT INTO jobs (id, title, input_content, status, remote_host, remote_job_dir, remote_socket) \
         VALUES ('other', 'benzene opt', '! HF', 'running', 'uni', '{ROOT}/jobs/other', '{ROOT}/tsp/slot0.sock')"
    ));
    let own = format!("{ROOT}/jobs/other");
    let outside = "/elsewhere/jobs/other".to_string();
    let deeper = format!("{ROOT}/jobs/other/sub");
    let outcome = blocked_by(&[(11, &own), (12, &outside), (13, &deeper)]);
    let fake = Fake::new(&r, answering_with(move |_| outcome.clone()));
    let s = step(&r, &fake);
    let StatusStep::SubmitRechecked { recheck: Recheck::Waiting(Wait::SlotBusy { blockers }), .. } = &s else { panic!("{s:?}") };
    let titles: Vec<Option<&str>> = blockers.iter().map(|b| b.title.as_deref()).collect();
    assert_eq!(titles, [Some("benzene opt"), None, None]);
    let shown: Vec<String> = blockers.iter().map(ShownBlocker::shown).collect();
    assert_eq!(shown, ["\"benzene opt\"", "pid 12 (cores 0-23)", "pid 13 (cores 0-23)"]);
}

/// The record across a re-check's steps (o23.10): a failed check keeps the pending submit (and its
/// last kind), a lost reply keeps it with an unanswered check error, an enqueue removes it.
#[test]
fn the_record_keeps_the_pending_submit_across_a_failed_check() {
    let r = pending();
    let prepares = Mutex::new(0);
    let server = answering_with(|n| if n == 0 { BLOCKED.to_string() } else { "enqueued 1\n".to_string() });
    let fake = Fake::new(&r, |call, values| match call {
        Prepare => {
            let mut n = prepares.lock().unwrap();
            *n += 1;
            if *n == 2 { timeout() } else { prepare_ready(values) }
        }
        _ => server(call, values),
    });
    step(&r, &fake);
    let before = waiting(&r).unwrap();
    assert!(matches!(step(&r, &fake), StatusStep::CheckFailed(_)));
    assert_eq!(waiting(&r), Some(before), "a failed check keeps it");
    step(&r, &fake);
    assert_eq!(waiting(&r), None, "the enqueue removes it");
}

// ---- the NULLs (o23.5, o23.13.9) -------------------------------------------------------------

/// A server for a delivery to a job the label hands to the classifier: the collects answer `snaps`
/// in turn (the last repeats), `cancel.sh` exits 0.
fn delivering(snaps: Vec<Vec<u8>>) -> impl Fn(Call, &[String]) -> Reply {
    let snaps = Mutex::new(snaps);
    move |call, values| match call {
        Label => label_reply(values, crate::ssh_backend::tests::ENQUEUED),
        Collect => {
            let mut s = snaps.lock().unwrap();
            let snap = if s.len() > 1 { s.remove(0) } else { s[0].clone() };
            ran(values, 0, &snap, b"")
        }
        _ => answering("enqueued 1\n")(call, values),
    }
}

/// o23.5: every terminal or `cancelling` write NULLs the pending flag in its own statement, so it
/// succeeds on a flagged row and leaves no flag. NEGATIVE CONTROLS (one per site; the v22 CHECK
/// refuses the write): drop `submit_pending_since = NULL` from `finalize_job_conn`,
/// `mark_cancelled_conn`, `record_cancelling` or `record_delivered`'s `cancelling` branch → red.
#[test]
fn every_terminal_or_cancelling_write_clears_the_flag() {
    let r = pending();
    crate::commands::jobs::finalize_job_conn(&r.w.db.lock().unwrap(), JOB, JobStatus::Failed, Some("x")).unwrap();
    assert_eq!((r.w.job().status, flag(&r)), (JobStatus::Failed, None), "finalize_job_conn");

    let r = pending();
    let written = crate::ssh_backend::mark_cancelled_conn(&r.w.db.lock().unwrap(), JOB, &expected_coords(), "x", &[JobStatus::Queued]).unwrap();
    assert_eq!((written, r.w.job().status, flag(&r)), (1, JobStatus::Cancelled, None), "mark_cancelled_conn");

    let r = pending();
    let fake = Fake::new(&r, answering("enqueued 1\n"));
    assert_eq!(r.poller(&fake).record_cancelling(JOB, &expected_coords(), 1).unwrap(), StatusStep::Shown(Outcome::Cancelling { session: 1 }));
    assert_eq!((r.w.job().status, flag(&r)), (JobStatus::Cancelling, None), "record_cancelling");

    let r = pending();
    r.w.sql("UPDATE jobs SET cancel_requested_at = '2026-10-19 08:01:00'");
    let fake = Fake::new(&r, delivering(vec![super::super::cancel::tests::waiting(), super::super::cancel::tests::cancelling()]));
    assert_eq!(step(&r, &fake), StatusStep::CancelDelivered(Outcome::Cancelling { session: 1 }));
    assert_eq!((r.w.job().status, flag(&r)), (JobStatus::Cancelling, None), "record_delivered, the cancelling branch");
}

/// o23.5: the two `queued`-keeping writes that end the wait — `record_delivered`'s unchanged-status
/// branch (pinned by `a_pending_cancel_is_delivered_before_any_recheck`) and `record_ended_first` —
/// NULL the flag; the CHECK cannot catch them, so this does. NEGATIVE CONTROL: drop
/// `submit_pending_since = NULL` from `record_ended_first` → red.
#[test]
fn a_cancel_that_finds_the_job_ended_clears_the_flag() {
    let r = pending();
    r.memory.count_refusal(JOB);
    r.w.sql("UPDATE jobs SET cancel_requested_at = '2026-10-19 08:01:00'");
    let fake = Fake::new(&r, delivering(vec![super::super::tests::completed()]));
    assert!(matches!(step(&r, &fake), StatusStep::EndedBeforeCancel(_)));
    let job = r.w.job();
    assert_eq!((job.status, job.submit_pending_since, job.cancel_requested_at), (JobStatus::Queued, None, None));
    assert_eq!(refusals(&r), 0, "the wait ended with it");
}
