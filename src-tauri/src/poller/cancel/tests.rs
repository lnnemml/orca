//! Delivery (ADR-024 o22.4), the cancel core and Undo (o22.3), the last contact (o22.8) and the
//! retry predicate, over a **fake** server. The fake asserts on every call that the database lock
//! is free, and records the pending cancel as each `cancel.sh` call found it — the mark must
//! already be there.

use std::cell::Cell;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use super::super::plan::{Step, MAX_CANCEL_FAILURES};
use super::super::remote_state::{CancelSends, Known};
use super::super::run::{run_step, Planned, StepRan};
use super::super::tests::{completed, download, indeterminate, listing, log_reply, lost, pending_cancel, record, Remote, OUTPUT};
use super::super::{Event, LogStep};
use super::*;
use crate::commands::jobs::{
    cancel_guarded_or_record, cancel_job_core, record_intent, revoke_cancel_core, CancelResult, Intent, UndoResult,
};
use crate::remote::classify::FailReason;
use crate::remote::run::{MKJOB_HEADER, RUN_HEADER};
use crate::remote::scripts::{INSTALL, LABEL, LIST, MKJOB, POLL_LOG, PREPARE, RUN};
use crate::remote::ssh::{CommandRunner, TransportError, SSH_PROGRAM};
use crate::remote::sync::RSYNC_PROGRAM;
use crate::remote::wire::HEADER as SNAPSHOT_HEADER;
use crate::ssh_backend::resubmit_remote;
use crate::ssh_backend::tests::{
    echo, exited, expected_coords, install_ok, label_reply, mkjob_ok, ok, prepare_fresh, prepare_ready, prepare_symlinked_bin,
    ran, rec, snapshot, Reply, CLEAN_TAIL, ENQUEUED, JOB, NOT_ON_SERVER,
};

// ---- the fake server -------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Call {
    Prepare,
    Install,
    Label,
    Mkjob,
    /// `cancel.sh` through the trampoline.
    Cancel,
    /// `collect.sh` through the trampoline.
    Collect,
    Download,
    List,
    PollLog,
}
use Call::*;

fn call_of(program: &str, stdin: &[u8]) -> (Call, Vec<String>) {
    if program == RSYNC_PROGRAM {
        return (Download, Vec::new());
    }
    assert_eq!(program, SSH_PROGRAM);
    for (call, script) in [(Prepare, PREPARE), (Install, INSTALL), (Label, LABEL), (Mkjob, MKJOB), (Collect, RUN), (List, LIST), (PollLog, POLL_LOG)] {
        if let Some(rest) = stdin.strip_prefix(script.as_bytes()) {
            let mut values: Vec<String> = rest.split(|b| *b == 0).map(|v| String::from_utf8(v.to_vec()).unwrap()).collect();
            assert_eq!(values.pop().as_deref(), Some(""));
            if call == Collect {
                let coords = expected_coords();
                return match values[1].as_str() {
                    "cancel" => {
                        assert_eq!(&values[3..], ["cancel", coords.job_dir.as_str(), crate::ssh_backend::tests::ROOT], "the recorded coordinates");
                        (Cancel, values)
                    }
                    "collect" => (Collect, values),
                    other => panic!("the trampoline never runs {other:?}"),
                };
            }
            return (call, values);
        }
    }
    panic!("a script delivery never sends");
}

/// Answers each call with `respond`, records its kind, checks the database lock is free on every
/// call, and keeps the pending cancel each `cancel.sh` call found.
struct Fake<'a> {
    r: &'a Remote,
    respond: Box<dyn Fn(Call, &[String]) -> Reply + 'a>,
    calls: Mutex<Vec<Call>>,
    at_cancel: Mutex<Vec<(Option<String>, Option<String>)>>,
}

impl<'a> Fake<'a> {
    fn new(r: &'a Remote, respond: impl Fn(Call, &[String]) -> Reply + 'a) -> Self {
        Fake { r, respond: Box::new(respond), calls: Mutex::default(), at_cancel: Mutex::default() }
    }

    fn calls(&self) -> Vec<Call> {
        self.calls.lock().unwrap().clone()
    }

    fn sends(&self) -> usize {
        self.calls().iter().filter(|c| **c == Cancel).count()
    }
}

impl CommandRunner for Fake<'_> {
    fn run(&self, program: &str, _args: &[String], stdin: &[u8], _timeout: Duration) -> Reply {
        let (call, values) = call_of(program, stdin);
        assert!(self.r.w.db.0.try_lock().is_ok(), "the database lock is held across a {call:?} call");
        if call == Cancel {
            self.at_cancel.lock().unwrap().push(pending_cancel(self.r));
        }
        self.calls.lock().unwrap().push(call);
        (self.respond)(call, &values)
    }
}

/// Nothing of the job ever ran and no daemon holds it: row 11 `ReEnqueue` — an outcome a delivery
/// goes on from.
fn waiting() -> Vec<u8> {
    snapshot(false, None, None, (None, None))
}

/// `.cancelled`, nothing ran: row 4.
fn cancelled() -> Vec<u8> {
    snapshot(true, None, None, (None, None))
}

/// `.exit_code` 1, no `.cancelled`: row 6.
fn failed_exit() -> Vec<u8> {
    snapshot(false, Some("1\n"), Some("x"), (None, None))
}

const BOOT: &str = "0f6e3c1a-5b7d-4c2e-9a8b-1d2e3f4a5b6c";

/// `.cancelled`, `.started` in this boot, the wrapper gone, one member still in the job dir: row 3
/// `Cancelling { session: 1 }`.
fn cancelling() -> Vec<u8> {
    let started = format!("pid=4242\npgid=4242\nsid=4242\nboot_id={BOOT}\nstarttime=777\nstarted_at=1\n");
    let coords = expected_coords();
    format!(
        "{SNAPSHOT_HEADER}\n{}{}proc collected\nwrapper_stat -\nwrapper_cmdline -\nsid_stat -\nmembers 1\nmember 70001\n{}\
         sockets 1\n{}{}nodaemon\nexit_code -\ncancelled yes\ntail -\n{}end\n",
        rec("boot_id", &format!("{BOOT}\n")),
        rec("started", &started),
        rec("cwd", &coords.job_dir),
        rec("socket", &coords.socket),
        rec("net_unix", "Num       RefCount Protocol Flags    Type St Inode Path\n"),
        rec("started", &started),
    )
    .into_bytes()
}

/// The default server: ready, the job enqueued (label → the classifier), mkjob ok, `cancel.sh`
/// rc 0; the collects answer `collects` in turn (the last repeats); download and listing over the
/// server dir.
fn server<'a>(r: &'a Remote, collects: Vec<Vec<u8>>) -> impl Fn(Call, &[String]) -> Reply + 'a {
    let collects = Mutex::new(collects);
    move |call, values| match call {
        Prepare => prepare_ready(values),
        Install => install_ok(values),
        Label => label_reply(values, ENQUEUED),
        Mkjob => mkjob_ok(values),
        Cancel => ran(values, 0, b"", b""),
        Collect => {
            let mut c = collects.lock().unwrap();
            let snap = if c.len() > 1 { c.remove(0) } else { c[0].clone() };
            ran(values, 0, &snap, b"")
        }
        Download => download(&r.server(), &r.local()),
        List => listing(values, &r.server()),
        PollLog => panic!("no log poll expected"),
    }
}

fn timeout() -> Reply {
    Err(TransportError::Timeout { program: "ssh".into(), secs: 60 })
}

/// The trampoline's `not-installed` reply.
fn not_installed(values: &[String]) -> Reply {
    ok(format!("{}not-installed\nend\n", echo(RUN_HEADER, values)).into_bytes())
}

fn request(r: &Remote) {
    r.w.sql("UPDATE jobs SET cancel_requested_at = '2026-10-17 10:00:00'");
}

/// Requested and marked, as a delivery whose reply was lost leaves it.
fn request_and_mark(r: &Remote) {
    r.w.sql("UPDATE jobs SET cancel_requested_at = '2026-10-17 10:00:00', cancel_sent_at = '2026-10-17 10:00:01'");
}

/// One delivery as the cancel command runs it: the guard claimed, delivery and its record applied.
fn deliver(r: &Remote, fake: &Fake<'_>) -> StatusStep {
    let guard = r.in_flight.acquire(JOB).unwrap();
    r.poller(fake).deliver_and_record(&guard).unwrap()
}

/// A stopped delivery that classified `outcome`.
fn stopped_seeing(outcome: Outcome) -> StatusStep {
    StatusStep::CancelStopped { seen: Some(Seen::Outcome(outcome)), unready: None }
}

/// Count `MAX_CANCEL_FAILURES` answered failures: the delivery is stopped.
fn stop(r: &Remote) {
    for _ in 0..MAX_CANCEL_FAILURES {
        r.memory.cancel_failed(JOB);
    }
}

fn never(_: Job) -> Result<(), AppError> {
    panic!("a remote cancel is never dispatched to a backend")
}

// ---- delivery: the happy path, mark-then-send, Undo -------------------------------------------

/// o22.4 steps 1–5: ready, label, classify, mark, send, classify again; the verdict decides
/// (`Cancelled`, never hard-coded), and one write clears both columns. `cancel.sh` found the send
/// already marked. The row stays `queued` (row 4's terminal write is the caller's, A2b) and the
/// record shows the verdict and an answered contact.
#[test]
fn a_delivery_marks_the_send_before_cancel_sh_and_records_the_servers_verdict() {
    let r = Remote::new();
    request(&r);
    let fake = Fake::new(&r, server(&r, vec![waiting(), cancelled()]));
    assert_eq!(deliver(&r, &fake), StatusStep::CancelDelivered(Outcome::Cancelled { started: false }));
    assert_eq!(fake.calls(), [Prepare, Label, Collect, Cancel, Collect]);
    let at_cancel = fake.at_cancel.lock().unwrap().clone();
    assert_eq!(at_cancel.len(), 1);
    assert!(at_cancel[0].0.is_some() && at_cancel[0].1.is_some(), "cancel.sh ran after the mark: {at_cancel:?}");
    assert_eq!(pending_cancel(&r), (None, None), "delivered: both columns cleared");
    assert_eq!(r.row().0, JobStatus::Queued, "row 4's terminal write is not delivery's");
    let state = record(&r).unwrap();
    assert_eq!(state.known, Some(Known::Shown(Outcome::Cancelled { started: false })));
    assert_eq!(state.last_contact, Some(Contact::Ok));
    assert!(!r.in_flight.is_busy(JOB));
}

/// o22.3: an Undo that lands before the mark wins — the mark's re-check fails and nothing is sent.
#[test]
fn an_undo_before_the_mark_wins_and_nothing_is_sent() {
    let r = Remote::new();
    request(&r);
    let undone = Cell::new(None);
    let base = server(&r, vec![waiting()]);
    let fake = Fake::new(&r, |call, values| {
        if call == Collect {
            undone.set(Some(revoke_cancel_core(&r.w.db, JOB).unwrap()));
        }
        base(call, values)
    });
    assert_eq!(deliver(&r, &fake), StatusStep::RowChanged);
    assert_eq!(undone.get(), Some(UndoResult::Undone), "Undo takes no guard: it ran while the delivery held it");
    assert_eq!(fake.sends(), 0, "nothing is sent after an Undo");
    assert_eq!(pending_cancel(&r), (None, None));
}

/// MAIN RISK 1 (o22.3 round 1 H1): a lost reply after `cancel.sh` ran cannot be undone. While the
/// script runs the send is already marked, so an Undo then is refused; the transport failure keeps
/// both columns (not delivered, re-sent next step) and an Undo afterwards is refused too.
/// NEGATIVE CONTROL: mark the send after `cancel_call` instead of before → red (the Undo during the
/// script succeeds).
#[test]
fn a_lost_reply_after_the_mark_cannot_be_undone() {
    let r = Remote::new();
    request(&r);
    let undo_during_send = Cell::new(None);
    let base = server(&r, vec![waiting()]);
    let fake = Fake::new(&r, |call, values| {
        if call == Cancel {
            // The script "ran" on the server; then the reply is lost.
            undo_during_send.set(Some(revoke_cancel_core(&r.w.db, JOB).unwrap()));
            return timeout();
        }
        base(call, values)
    });
    let step = deliver(&r, &fake);
    assert!(matches!(&step, StatusStep::CheckFailed(why) if why.contains("did not finish")), "{step:?}");
    assert_eq!(undo_during_send.get(), Some(UndoResult::SendMarked), "an Undo while cancel.sh runs is refused");
    let (requested, sent) = pending_cancel(&r);
    assert!(requested.is_some() && sent.is_some(), "not delivered: both columns stay");
    assert_eq!(revoke_cancel_core(&r.w.db, JOB).unwrap(), UndoResult::SendMarked, "and after it");
    assert_eq!(r.memory.get(JOB).cancel_failures, 0, "a transport failure is not a counted failure");
    assert!(matches!(record(&r).unwrap().last_contact, Some(Contact::Failed(_))));
}

/// MAIN RISK 3: a transport failure on the send keeps the cancel — the next delivery re-sends it
/// (idempotent, A1) and it is delivered. NEGATIVE CONTROL: clear `cancel_requested_at` (and the
/// mark) in the mark step, before the send's rc is known → red (the cancel is lost).
#[test]
fn a_transport_failure_on_the_send_keeps_the_cancel_and_the_next_delivery_sends_again() {
    let r = Remote::new();
    request(&r);
    let lose = Cell::new(true);
    let base = server(&r, vec![waiting(), waiting(), cancelled()]);
    let fake = Fake::new(&r, |call, values| if call == Cancel && lose.replace(false) { timeout() } else { base(call, values) });
    assert!(matches!(deliver(&r, &fake), StatusStep::CheckFailed(_)));
    let (requested, sent) = pending_cancel(&r);
    assert!(requested.is_some() && sent.is_some(), "the cancel is kept: {requested:?} {sent:?}");
    assert_eq!(deliver(&r, &fake), StatusStep::CancelDelivered(Outcome::Cancelled { started: false }));
    assert_eq!(fake.sends(), 2, "sent again");
    assert_eq!(pending_cancel(&r), (None, None));
}

/// MAIN RISK 3: only rc 0 is "delivered". rc 3 (fail-closed) is answered but not delivered: both
/// columns stay, the script's stderr is the reason, no post-send collect, one failure counted.
/// NEGATIVE CONTROL: treat any rc as delivered (`Ok((_, _))` for `Ok((0, _))`) → red.
#[test]
fn rc_3_is_not_delivered() {
    let r = Remote::new();
    request(&r);
    let base = server(&r, vec![waiting()]);
    let fake = Fake::new(&r, |call, values| if call == Cancel { ran(values, 3, b"", b"cancel: not a directory") } else { base(call, values) });
    let step = deliver(&r, &fake);
    assert!(matches!(&step, StatusStep::CancelNotSent(why) if why.contains("exited 3") && why.contains("not a directory")), "{step:?}");
    assert_eq!(fake.calls(), [Prepare, Label, Collect, Cancel], "no post-send collect");
    let (requested, sent) = pending_cancel(&r);
    assert!(requested.is_some() && sent.is_some());
    assert_eq!(r.memory.get(JOB).cancel_failures, 1);
    let state = record(&r).unwrap();
    assert!(state.check_error.unwrap().contains("exited 3"));
    assert_eq!((state.last_contact, state.cancel_sends), (Some(Contact::Ok), CancelSends { failures: 1, stopped: false }));
}

/// MAIN RISK 3, the bound (o22.4.5): three failed sends in a row; the 4th automatic delivery
/// sends nothing (`CancelStopped`) — its pre-classify still runs; a Cancel click restarts the count
/// and sends once more. NEGATIVE CONTROL: drop the `MAX_CANCEL_FAILURES` check → red (a 4th send).
#[test]
fn three_failed_sends_stop_the_automatic_re_sends_until_cancel_is_clicked() {
    let r = Remote::new();
    request(&r);
    let healthy = Cell::new(false);
    let base = server(&r, vec![waiting(), waiting(), waiting(), waiting(), waiting(), cancelled()]);
    let fake = Fake::new(&r, |call, values| {
        if call == Cancel && !healthy.get() {
            return ran(values, 3, b"", b"usage");
        }
        base(call, values)
    });
    for n in 1..=3 {
        assert!(matches!(deliver(&r, &fake), StatusStep::CancelNotSent(_)), "send {n}");
    }
    assert_eq!(record(&r).unwrap().cancel_sends, CancelSends { failures: 3, stopped: true }, "the third reads stopped");
    assert_eq!(deliver(&r, &fake), stopped_seeing(Outcome::ReEnqueue));
    assert_eq!(fake.sends(), 3, "no 4th automatic send");
    assert!(record(&r).unwrap().cancel_sends.stopped);

    healthy.set(true);
    let result = cancel_job_core(&r.poller(&fake), JOB, never).unwrap();
    assert_eq!(result, CancelResult::Delivered { already_pending: true, step: StatusStep::CancelDelivered(Outcome::Cancelled { started: false }) });
    assert_eq!(fake.sends(), 4, "the click is the manual retry");
    assert_eq!(record(&r).unwrap().cancel_sends, CancelSends::default());
}

/// o22.4.2: the scripts are sha-named, so delivery readies the server first — prepare, install,
/// prepare — and the cancel then runs. NEGATIVE CONTROL: skip `ready_server` → red (`cancel.sh`
/// is not installed: `CancelNotSent`).
#[test]
fn a_delivery_readies_a_server_whose_cancel_sh_is_not_installed() {
    let r = Remote::new();
    request(&r);
    let installed = Cell::new(false);
    let base = server(&r, vec![waiting(), cancelled()]);
    let fake = Fake::new(&r, |call, values| match call {
        Prepare if !installed.get() => prepare_fresh(values),
        Install => {
            installed.set(true);
            install_ok(values)
        }
        Cancel | Collect if !installed.get() => not_installed(values),
        _ => base(call, values),
    });
    assert_eq!(deliver(&r, &fake), StatusStep::CancelDelivered(Outcome::Cancelled { started: false }));
    assert_eq!(fake.calls(), [Prepare, Install, Prepare, Label, Collect, Cancel, Collect]);
}

/// o22.4.2: a refused prepare is reported as itself (`CancelNotSent`, the server answered, one
/// failure counted — o22.11.3), a transport failure as `CheckFailed` with a failed contact and
/// nothing counted — never one for the other. Nothing is marked either way.
#[test]
fn a_server_that_cannot_be_readied_is_reported_as_itself() {
    let r = Remote::new();
    request(&r);
    let fake = Fake::new(&r, |call, values| if call == Prepare { prepare_symlinked_bin(values) } else { panic!("{call:?} after a refused prepare") });
    let step = deliver(&r, &fake);
    assert!(matches!(&step, StatusStep::CancelNotSent(why) if why.contains("refused")), "{step:?}");
    assert_eq!(record(&r).unwrap().last_contact, Some(Contact::Ok), "the server answered");
    assert_eq!(r.memory.get(JOB).cancel_failures, 1, "an answered failure counts (o22.11.3)");

    let fake = Fake::new(&r, |_, _| exited(255, Vec::new()));
    let step = deliver(&r, &fake);
    assert!(matches!(&step, StatusStep::CheckFailed(why) if why.contains("exit 255")), "{step:?}");
    assert!(matches!(record(&r).unwrap().last_contact, Some(Contact::Failed(_))));
    assert_eq!(r.memory.get(JOB).cancel_failures, 1, "a transport failure counts nothing");
    assert_eq!(pending_cancel(&r).1, None, "nothing was marked");
}

// ---- delivery: the pre-cancel classify -----------------------------------------------------

/// o22.4.4: a job that failed before the cancel reached the server (row 6, no `.cancelled`) keeps
/// its `failed` — it is `EndedBeforeCancel`, nothing is sent, both columns cleared.
/// NEGATIVE CONTROL: skip the pre-cancel classify → red (the cancel is sent and the verdict is the
/// cancel's).
#[test]
fn a_job_that_failed_before_the_cancel_ended_first_and_is_not_cancelled() {
    let r = Remote::new();
    request(&r);
    let fake = Fake::new(&r, server(&r, vec![failed_exit(), cancelled()]));
    let step = deliver(&r, &fake);
    assert_eq!(step, StatusStep::EndedBeforeCancel(Outcome::Failed { reason: FailReason::NonZeroExit { code: 1 } }));
    assert_eq!(fake.sends(), 0);
    assert_eq!(pending_cancel(&r), (None, None));
    assert_eq!(record(&r).unwrap().known, Some(Known::Fetching));
}

/// o22.4.4 (round 1 M1): `Indeterminate` implies no `.exit_code`, and `cancel.sh` publishes
/// `.cancelled` first — so a delivery goes on and sends. NEGATIVE CONTROL: stop on `Indeterminate`
/// (return it as a failed check) → red (nothing sent).
#[test]
fn indeterminate_before_the_cancel_still_sends() {
    let r = Remote::new();
    request(&r);
    // The first collect and its retake both disagree: Indeterminate; then the post-send collect.
    let fake = Fake::new(&r, server(&r, vec![indeterminate(), indeterminate(), cancelled()]));
    assert_eq!(deliver(&r, &fake), StatusStep::CancelDelivered(Outcome::Cancelled { started: false }));
    assert_eq!(fake.calls(), [Prepare, Label, Collect, Collect, Cancel, Collect]);
}

/// Round 2 M1: a cancel marked but never sent (the send's connection failed) on a job that then
/// completes ends first with **both** columns cleared, and the next ordinary step fetches and
/// finalises it `completed`. NEGATIVE CONTROL: clear only `cancel_requested_at` in the ended-first
/// write → red (the v21 CHECK fails it: `cancel_sent_at` without a request).
#[test]
fn a_marked_but_unsent_cancel_on_a_job_that_completed_ends_first_and_completes() {
    let r = Remote::new();
    request_and_mark(&r);
    r.finished(OUTPUT, "0\n");
    let fake = Fake::new(&r, server(&r, vec![completed()]));
    assert_eq!(deliver(&r, &fake), StatusStep::EndedBeforeCancel(Outcome::Completed { late_cancel: false }));
    assert_eq!(pending_cancel(&r), (None, None));
    assert_eq!(fake.sends(), 0);
    let step = r.poller(&fake).status_step(JOB).unwrap();
    assert!(matches!(step, StatusStep::Finalised { status: JobStatus::Completed, .. }), "{step:?}");
    assert_eq!(r.row(), (JobStatus::Completed, None));
}

/// A late cancel (row 2: `.cancelled`, a clean result) seen before the send is not "ended first":
/// `.cancelled` is already on the server, so the cancel is delivered without a re-send and the
/// verdict keeps the result for the fetch (o22.11.4).
#[test]
fn a_late_cancel_before_the_send_is_delivered_and_keeps_the_result() {
    let late = || snapshot(true, Some("0\n"), Some(CLEAN_TAIL), (None, None));
    assert!(!ended_before_cancel(&Outcome::Completed { late_cancel: true }));
    assert!(cancelled_on_server(&Outcome::Completed { late_cancel: true }));
    let r = Remote::new();
    request_and_mark(&r);
    let fake = Fake::new(&r, server(&r, vec![late()]));
    assert_eq!(deliver(&r, &fake), StatusStep::CancelDelivered(Outcome::Completed { late_cancel: true }));
    assert_eq!(fake.sends(), 0, "nothing re-sent");
    assert_eq!(pending_cancel(&r), (None, None));
    assert_eq!(record(&r).unwrap().known, Some(Known::Fetching));
}

/// o22.11.4 / control (b): a pre-classify that already shows `.cancelled` (row 4; an earlier send
/// whose reply was lost) means the cancel is delivered — both columns cleared in the step's write,
/// the verdict returned as after rc 0, and **no** `cancel.sh` call. The other `.cancelled` rows
/// (2, 3, 1 with `cancelled`) are the same predicate. NEGATIVE CONTROL: drop the
/// `cancelled_on_server` branch → red (a re-send: one `cancel.sh` call).
#[test]
fn a_cancel_already_on_the_server_is_delivered_without_a_re_send() {
    let r = Remote::new();
    request_and_mark(&r);
    let fake = Fake::new(&r, server(&r, vec![cancelled()]));
    assert_eq!(deliver(&r, &fake), StatusStep::CancelDelivered(Outcome::Cancelled { started: false }));
    assert_eq!(fake.sends(), 0, "nothing re-sent");
    assert_eq!(fake.calls(), [Prepare, Label, Collect]);
    assert_eq!(pending_cancel(&r), (None, None));
    for outcome in [
        Outcome::Cancelling { session: 1 },
        Outcome::Failed { reason: FailReason::CorruptStarted { detail: String::new(), cancelled: true } },
    ] {
        assert!(cancelled_on_server(&outcome), "{outcome:?}");
    }
    for outcome in [
        Outcome::Failed { reason: FailReason::CorruptStarted { detail: String::new(), cancelled: false } },
        Outcome::Indeterminate,
        Outcome::Running,
        Outcome::Completed { late_cancel: false },
    ] {
        assert!(!cancelled_on_server(&outcome), "{outcome:?}");
    }
}

/// o22.11.4 / control (a): a **stopped** delivery still runs steps 1–4; a pre-classify that shows
/// row 4 ends `CancelDelivered` with both columns cleared and no `cancel.sh` call; the bound
/// restarts. NEGATIVE CONTROL: return `CancelStopped` before the classify when stopped → red.
#[test]
fn a_stopped_delivery_still_classifies_and_sees_a_cancel_already_delivered() {
    let r = Remote::new();
    request_and_mark(&r);
    for _ in 0..MAX_CANCEL_FAILURES {
        r.memory.cancel_failed(JOB);
    }
    let fake = Fake::new(&r, server(&r, vec![cancelled()]));
    assert_eq!(deliver(&r, &fake), StatusStep::CancelDelivered(Outcome::Cancelled { started: false }));
    assert_eq!(fake.calls(), [Prepare, Label, Collect], "classified, nothing sent");
    assert_eq!(pending_cancel(&r), (None, None));
    assert_eq!(r.memory.get(JOB).cancel_failures, 0);

    // Stopped, and the server shows nothing of the cancel: classified, nothing sent.
    let r = Remote::new();
    request_and_mark(&r);
    for _ in 0..MAX_CANCEL_FAILURES {
        r.memory.cancel_failed(JOB);
    }
    let fake = Fake::new(&r, server(&r, vec![waiting()]));
    assert_eq!(deliver(&r, &fake), stopped_seeing(Outcome::ReEnqueue));
    assert_eq!(fake.calls(), [Prepare, Label, Collect]);
}

/// o22.11.3 / control (c): an `mkjob` refusal is an answered failure of the delivery and counts
/// toward the bound; after three, the 4th automatic attempt makes no `mkjob` call (nothing is
/// sent while stopped, `mkjob` included). A transport failure of `mkjob` counts nothing.
/// NEGATIVE CONTROL: report an answered `mkjob` failure without counting it → red (a 4th mkjob).
#[test]
fn an_mkjob_refusal_counts_toward_the_bound() {
    let r = Remote::new();
    request(&r);
    let base = server(&r, vec![cancelled()]);
    let fake = Fake::new(&r, |call, values| match call {
        Label => label_reply(values, NOT_ON_SERVER),
        // The job dir resolves elsewhere: refused after the mkdir.
        Mkjob => ok(format!("{}{}{}end\n", echo(MKJOB_HEADER, values), rec("job", "/data/j1"), rec("parent", &format!("{}/jobs", values[0]))).into_bytes()),
        _ => base(call, values),
    });
    for n in 1..=3 {
        assert!(matches!(deliver(&r, &fake), StatusStep::CancelNotSent(_)), "attempt {n}");
        assert_eq!(r.memory.get(JOB).cancel_failures, n);
    }
    assert_eq!(deliver(&r, &fake), StatusStep::CancelStopped { seen: Some(Seen::Label(crate::remote::submit::Label::NotOnServer)), unready: None });
    assert_eq!(fake.calls().iter().filter(|c| **c == Mkjob).count(), 3, "no 4th mkjob");
    assert_eq!(record(&r).unwrap().known, Some(Known::Labelled(crate::remote::submit::Label::NotOnServer)), "the label is shown (o22.12.2)");
    assert_eq!(fake.sends(), 0);

    let r = Remote::new();
    request(&r);
    let base = server(&r, vec![cancelled()]);
    let fake = Fake::new(&r, |call, values| match call {
        Label => label_reply(values, NOT_ON_SERVER),
        Mkjob => timeout(),
        _ => base(call, values),
    });
    assert!(matches!(deliver(&r, &fake), StatusStep::CheckFailed(_)));
    assert_eq!(r.memory.get(JOB).cancel_failures, 0, "no answer, nothing counted");
}

/// rc 0 with row 3: the row becomes `cancelling` (with its text) in the write that clears both
/// columns, and `job:status` says so.
#[test]
fn a_delivered_cancelling_verdict_writes_cancelling_and_clears_the_columns() {
    let r = Remote::new();
    r.w.sql("UPDATE jobs SET status = 'running'");
    request(&r);
    let fake = Fake::new(&r, server(&r, vec![waiting(), cancelling()]));
    let step = deliver(&r, &fake);
    assert_eq!(step, StatusStep::CancelDelivered(Outcome::Cancelling { session: 1 }));
    assert_eq!(fake.calls(), [Prepare, Collect, Cancel, Collect], "a running row: no label call");
    assert_eq!(r.row(), (JobStatus::Cancelling, shown_message(&Outcome::Cancelling { session: 1 })));
    assert_eq!(pending_cancel(&r), (None, None));
    assert_eq!(r.sink.events(), [Event::Status(JOB.into(), JobStatus::Cancelling)]);
}

/// o22.4.3: a `queued` job not on the server — the withdraw's tail: mkjob, then straight to the
/// send (no classify of a dir that did not exist).
#[test]
fn a_job_not_on_the_server_gets_its_dir_and_is_cancelled_without_a_first_classify() {
    let r = Remote::new();
    request(&r);
    let base = server(&r, vec![cancelled()]);
    let fake = Fake::new(&r, |call, values| if call == Label { label_reply(values, NOT_ON_SERVER) } else { base(call, values) });
    assert_eq!(deliver(&r, &fake), StatusStep::CancelDelivered(Outcome::Cancelled { started: false }));
    assert_eq!(fake.calls(), [Prepare, Label, Mkjob, Cancel, Collect]);
}

/// Step 1: a row with nothing pending (an Undo won before the delivery began) or no longer live
/// makes no call.
#[test]
fn a_delivery_without_a_pending_cancel_or_a_live_row_makes_no_call() {
    let r = Remote::new();
    let fake = Fake::new(&r, |call, _| panic!("{call:?}"));
    assert_eq!(deliver(&r, &fake), StatusStep::RowChanged);
    r.w.sql("UPDATE jobs SET status = 'completed'");
    assert_eq!(deliver(&r, &fake), StatusStep::NotLive);
}

// ---- the cancel core (o22.3) -----------------------------------------------------------------

/// MAIN RISK 1 / o22.9: with the guard held (a poller step mid-ssh), Cancel still records and
/// answers "pending" — never `Conflict`; a second click finds it already pending. No call, no
/// dispatch. NEGATIVE CONTROL: claim with `acquire(id)?` instead of `try_acquire` → red (Conflict).
#[test]
fn with_the_guard_held_a_cancel_is_recorded_and_pending() {
    let r = Remote::new();
    let fake = Fake::new(&r, |call, _| panic!("{call:?}: a busy job gets no call"));
    let _held = r.in_flight.acquire(JOB).unwrap();
    assert_eq!(cancel_job_core(&r.poller(&fake), JOB, never).unwrap(), CancelResult::Pending { already_pending: false });
    assert!(pending_cancel(&r).0.is_some(), "recorded");
    assert_eq!(cancel_job_core(&r.poller(&fake), JOB, never).unwrap(), CancelResult::Pending { already_pending: true });
    assert_eq!(pending_cancel(&r).1, None, "nothing marked");
}

/// With the guard free, the click delivers now, holding the guard, and releases it after.
#[test]
fn with_the_guard_free_a_cancel_is_delivered_at_once() {
    let r = Remote::new();
    let busy_during_send = Cell::new(false);
    let base = server(&r, vec![waiting(), cancelled()]);
    let fake = Fake::new(&r, |call, values| {
        if call == Cancel {
            busy_during_send.set(r.in_flight.is_busy(JOB));
        }
        base(call, values)
    });
    let result = cancel_job_core(&r.poller(&fake), JOB, never).unwrap();
    assert_eq!(result, CancelResult::Delivered { already_pending: false, step: StatusStep::CancelDelivered(Outcome::Cancelled { started: false }) });
    assert!(busy_during_send.get(), "delivery holds the guard");
    assert!(!r.in_flight.is_busy(JOB), "released after");
}

/// o22.3 / o15.1: the record write has the remote predicate itself — a draft or a local job never
/// gets a pending cancel, whatever routed it there; a finished remote job has "moved on".
/// NEGATIVE CONTROL: drop `remote_host IS NOT NULL AND status IN (…)` from `RECORD_CANCEL_SQL` → red
/// (the write lands, and the v21 CHECK refuses it with a database error instead of `MovedOn`).
#[test]
fn the_record_write_never_records_a_draft_or_a_local_job() {
    let r = Remote::new();
    r.w.sql("UPDATE jobs SET status = 'draft', remote_host = NULL, remote_job_dir = NULL, remote_socket = NULL");
    assert_eq!(record_intent(&r.w.db, JOB).unwrap(), Intent::MovedOn(JobStatus::Draft));
    r.w.sql("UPDATE jobs SET status = 'queued'");
    assert_eq!(record_intent(&r.w.db, JOB).unwrap(), Intent::MovedOn(JobStatus::Queued), "a local queued job");
    assert_eq!(pending_cancel(&r), (None, None));

    let r = Remote::new();
    r.w.sql("UPDATE jobs SET status = 'completed'");
    assert_eq!(record_intent(&r.w.db, JOB).unwrap(), Intent::MovedOn(JobStatus::Completed));
}

/// o22.3 L3: the pre-read saw a draft, then a remote submit persisted it and finished before the
/// guarded read — that read finds a remote-live row and the cancel goes once to the record path,
/// delivered with the guard it holds; nothing is dispatched to a local backend. NEGATIVE CONTROL:
/// dispatch whatever the guarded read finds (drop the re-check) → red.
#[test]
fn a_persist_between_the_two_reads_goes_once_to_the_record_path() {
    let r = Remote::new();
    let fake = Fake::new(&r, server(&r, vec![waiting(), cancelled()]));
    // The job is remote-live now; enter where the pre-read (which saw a draft) hands over.
    let result = cancel_guarded_or_record(&r.poller(&fake), JOB, never).unwrap();
    assert_eq!(result, CancelResult::Delivered { already_pending: false, step: StatusStep::CancelDelivered(Outcome::Cancelled { started: false }) });
    assert_eq!(fake.sends(), 1);
}

/// o21's guarded path is unchanged for a local job: dispatched with the guard held; a busy one is
/// refused with a conflict and never dispatched.
#[test]
fn a_local_job_takes_the_guarded_path() {
    let r = Remote::new();
    r.w.sql("UPDATE jobs SET remote_host = NULL, remote_job_dir = NULL, remote_socket = NULL");
    let fake = Fake::new(&r, |call, _| panic!("{call:?}"));
    let dispatched = Cell::new(false);
    let result = cancel_job_core(&r.poller(&fake), JOB, |job| {
        assert!(r.in_flight.is_busy(JOB), "the guard is held while the local cancel runs");
        assert!(!job.is_remote());
        dispatched.set(true);
        Ok(())
    });
    assert_eq!(result.unwrap(), CancelResult::Dispatched);
    assert!(dispatched.get());
    let _held = r.in_flight.acquire(JOB).unwrap();
    let busy = cancel_job_core(&r.poller(&fake), JOB, |_| panic!("never dispatched while busy"));
    assert!(matches!(busy, Err(AppError::Conflict(_))), "{busy:?}");
    assert_eq!(pending_cancel(&r), (None, None));
}

// ---- Undo (o22.3) ------------------------------------------------------------------------------

/// Undo is one atomic write, takes no guard, and says why it did nothing.
#[test]
fn undo_clears_only_an_unmarked_request() {
    let r = Remote::new();
    let _held = r.in_flight.acquire(JOB).unwrap();
    assert_eq!(revoke_cancel_core(&r.w.db, JOB).unwrap(), UndoResult::NothingPending);
    request(&r);
    assert_eq!(revoke_cancel_core(&r.w.db, JOB).unwrap(), UndoResult::Undone, "with the guard held by another");
    assert_eq!(pending_cancel(&r), (None, None));
    request_and_mark(&r);
    assert_eq!(revoke_cancel_core(&r.w.db, JOB).unwrap(), UndoResult::SendMarked);
    assert!(pending_cancel(&r).0.is_some(), "kept");
    assert!(matches!(revoke_cancel_core(&r.w.db, "nope"), Err(AppError::NotFound(_))));
}

// ---- retry while a cancel is pending (o22.8) ---------------------------------------------------

/// Both retries refuse in their locked read while a cancel is pending — no call is made.
/// NEGATIVE CONTROL: drop the predicate from `retry_checks` (or from `Poller::retry_fetch`) → red.
#[test]
fn retry_is_refused_while_a_cancel_is_pending() {
    let r = Remote::new();
    request(&r);
    let fake = Fake::new(&r, |call, _| panic!("{call:?}: no call while a cancel is pending"));
    let retried = resubmit_remote(&r.w.db, &fake, &r.w.data_dir(), JOB);
    assert!(matches!(&retried, Err(AppError::Backend(m)) if m.contains("undo the cancel first")), "{retried:?}");
    let guard = r.in_flight.acquire(JOB).unwrap();
    let fetched = r.poller(&fake).retry_fetch(&guard);
    assert!(matches!(&fetched, Err(AppError::Backend(m)) if m.contains("undo the cancel first")), "{fetched:?}");
}

// ---- the last contact (o22.8) ----------------------------------------------------------------

/// A status step writes its last call's contact: a host away is `Failed`, an answer `Ok`.
#[test]
fn a_status_steps_last_call_writes_the_contact() {
    let r = Remote::new();
    let away = Cell::new(true);
    let base = server(&r, vec![waiting()]);
    let fake = Fake::new(&r, |call, values| if away.get() { timeout() } else { base(call, values) });
    assert!(matches!(r.poller(&fake).status_step(JOB).unwrap(), StatusStep::CheckFailed(_)));
    assert!(matches!(record(&r).unwrap().last_contact, Some(Contact::Failed(why)) if why.contains("did not finish")));
    away.set(false);
    assert_eq!(r.poller(&fake).status_step(JOB).unwrap(), StatusStep::Shown(Outcome::ReEnqueue));
    assert_eq!(record(&r).unwrap().last_contact, Some(Contact::Ok));
}

/// o22.8: `poll_log` is not a writer — it holds no guard (o16.5). A failing log poll after an
/// answered status step leaves the record as it was. NEGATIVE CONTROL: apply the log poll's
/// contact to the record in `run_step`'s log arm → red.
#[test]
fn a_log_poll_never_writes_the_last_contact() {
    let r = Remote::new();
    let polls_fail = Cell::new(false);
    let base = server(&r, vec![waiting()]);
    let fake = Fake::new(&r, |call, values| match call {
        PollLog if polls_fail.get() => timeout(),
        PollLog => log_reply(values, b"line\n", 5),
        _ => base(call, values),
    });
    let t0 = Instant::now();
    let status = Planned { job_id: JOB.into(), step: Step::Status, coords: expected_coords() };
    assert!(r.memory.begin_step(JOB, Step::Status, t0));
    assert!(matches!(run_step(&r.poller(&fake), &status), StepRan::Status(Ok(_))));
    let before = record(&r).unwrap();
    assert_eq!(before.last_contact, Some(Contact::Ok));

    polls_fail.set(true);
    r.live.open(JOB, &r.sink);
    let log = Planned { step: Step::Log, ..status };
    assert!(r.memory.begin_step(JOB, Step::Log, t0));
    assert!(matches!(run_step(&r.poller(&fake), &log), StepRan::Log(LogStep::Failed(_))));
    assert_eq!(record(&r), Some(before), "the log poll wrote nothing to the record");
}

// ---- the classifier's `Cancelled` acts on the row it wrote (A1 verifier LOW) -------------------

/// `write_cancelled` acts on `mark_cancelled_conn`'s count: a write whose own predicate matched no
/// row (here its `from` list misses the row's status) is `RowChanged` — no `Cancelled`, no
/// `job:status`, the memory kept. NEGATIVE CONTROL: ignore the count → red.
#[test]
fn a_cancelled_write_that_changed_no_row_is_row_changed() {
    let r = Remote::new();
    r.w.sql("UPDATE jobs SET status = 'running'");
    r.memory.set_fetching(JOB);
    let fake = Fake::new(&r, |call, _| panic!("{call:?}"));
    let step = r.poller(&fake).write_cancelled(JOB, &expected_coords(), &[JobStatus::Queued]).unwrap();
    assert_eq!(step, StatusStep::RowChanged);
    assert_eq!(r.row().0, JobStatus::Running);
    assert!(r.sink.events().is_empty());
    assert!(r.memory.get(JOB).fetching, "not forgotten");
}

// ---- o22.12: a `cancelling` click, what a stopped delivery saw, prepare-only while stopped ------

/// o22.12.1 (Anton, D1): a click on a `cancelling` job records **no** request — the cancel is
/// already on the server — so both columns stay NULL and no Undo is ever offered; it answers
/// `AlreadyOnServer`, restarts the sweep bound (seeded non-zero first), and makes no call (the next
/// status step sweeps). NEGATIVE CONTROLS: admit `cancelling` in `RECORD_CANCEL_SQL` again → red (a
/// request is recorded and delivered); drop the `restart_sweeps` call → red (the count stays 2).
#[test]
fn a_click_on_a_cancelling_job_records_nothing_and_restarts_the_sweeps() {
    let r = Remote::new();
    r.w.sql("UPDATE jobs SET status = 'cancelling'");
    r.memory.set_sweeps_without_progress(JOB, 2);
    let fake = Fake::new(&r, server(&r, vec![cancelling()]));
    assert_eq!(cancel_job_core(&r.poller(&fake), JOB, never).unwrap(), CancelResult::AlreadyOnServer);
    assert_eq!(pending_cancel(&r), (None, None), "no request on a cancelling row");
    assert_eq!(fake.calls(), [], "the next status step sweeps, not the click");
    assert_eq!(r.memory.get(JOB).sweeps_without_progress, 0);
    assert_eq!(revoke_cancel_core(&r.w.db, JOB).unwrap(), UndoResult::NothingPending, "no Undo");
    assert_eq!(record_intent(&r.w.db, JOB).unwrap(), Intent::OnServer);
}

/// The sweep hook (o22.12.1) resets what A2b's sweeps will count. NEGATIVE CONTROL: make
/// `restart_sweeps` a no-op → red.
#[test]
fn restart_sweeps_resets_the_sweep_count() {
    let m = crate::poller::plan::PollerMemory::new();
    m.set_sweeps_without_progress(JOB, 2);
    assert_eq!(m.get(JOB).sweeps_without_progress, 2, "seeded");
    m.restart_sweeps(JOB);
    assert_eq!(m.get(JOB).sweeps_without_progress, 0);
}

/// o22.12.1's belt: Undo requires `queued`/`running`. A `cancelling` row never holds a request in
/// production (every write of `cancelling` clears both columns), but the CHECK allows one; built
/// directly, Undo refuses it and the request stays. NEGATIVE CONTROL: drop the status condition from
/// Undo's `UPDATE` → red (`Undone`).
#[test]
fn undo_refuses_a_cancelling_row_that_holds_a_request() {
    let r = Remote::new();
    r.w.sql("UPDATE jobs SET status = 'cancelling', cancel_requested_at = '2026-10-17 10:00:00'");
    assert_eq!(revoke_cancel_core(&r.w.db, JOB).unwrap(), UndoResult::SendMarked);
    assert!(pending_cancel(&r).0.is_some(), "the request stays");
}

/// o22.12.3: while stopped, a prepare that gets **no answer** is a failed check (contact `Failed`),
/// not a stopped delivery that "could classify nothing". NEGATIVE CONTROL: return `CancelStopped`
/// for every prepare failure while stopped → red.
#[test]
fn a_stopped_delivery_whose_prepare_gets_no_answer_is_a_failed_check() {
    let r = Remote::new();
    request_and_mark(&r);
    stop(&r);
    let fake = Fake::new(&r, |call, _| if call == Prepare { timeout() } else { panic!("{call:?} after a prepare without an answer") });
    let step = deliver(&r, &fake);
    assert!(matches!(&step, StatusStep::CheckFailed(why) if why.contains("did not finish")), "{step:?}");
    assert!(matches!(record(&r).unwrap().last_contact, Some(Contact::Failed(_))));
}

/// o22.12.2 (D3): a stopped delivery still shows the server's verdict — a job seen `Lost` keeps
/// `Lost` in the record's `known`, with the stopped flag; nothing is sent. NEGATIVE CONTROL: drop the
/// payload (`seen: None` at the stopped return) → red.
#[test]
fn a_stopped_delivery_that_sees_lost_shows_lost() {
    let r = Remote::new();
    r.w.sql("UPDATE jobs SET status = 'running'");
    request_and_mark(&r);
    stop(&r);
    let fake = Fake::new(&r, server(&r, vec![lost()]));
    assert_eq!(deliver(&r, &fake), stopped_seeing(Outcome::Lost { orphans: vec![] }));
    assert_eq!(fake.calls(), [Prepare, Collect]);
    let state = record(&r).unwrap();
    assert_eq!(state.known, Some(Known::Shown(Outcome::Lost { orphans: vec![] })));
    assert!(state.cancel_sends.stopped);
    let (requested, sent) = pending_cancel(&r);
    assert!(requested.is_some() && sent.is_some(), "not delivered: kept");
}

/// o22.12.3 (D4): while stopped, ready is prepare-only — a server whose scripts are missing gets no
/// `Install` call (no server write on any period); the delivery classifies nothing and says why
/// (`CancelStopped { seen: None, unready }`). NEGATIVE CONTROL: call `ready_server` while stopped →
/// red (an `Install` call).
#[test]
fn a_stopped_delivery_installs_nothing() {
    let r = Remote::new();
    request_and_mark(&r);
    stop(&r);
    let fake = Fake::new(&r, |call, values| match call {
        Prepare => prepare_fresh(values),
        Install => install_ok(values),
        other => panic!("{other:?} after a prepare that is not ready"),
    });
    let step = deliver(&r, &fake);
    assert!(
        matches!(&step, StatusStep::CancelStopped { seen: None, unready: Some(why) } if why.contains("not ready")),
        "{step:?}"
    );
    assert_eq!(fake.calls(), [Prepare], "prepare only, no install");
    let state = record(&r).unwrap();
    assert!(state.check_error.unwrap().contains("not ready"));
    assert_eq!(r.memory.get(JOB).cancel_failures, MAX_CANCEL_FAILURES, "a stopped delivery counts nothing more");
}


/// The wire shape `src/types.ts` mirrors for the stopped step (o22.12.2).
#[test]
fn the_cancel_stopped_wire_shape() {
    let lost = StatusStep::CancelStopped { seen: Some(Seen::Outcome(Outcome::Lost { orphans: vec![] })), unready: None };
    assert_eq!(
        serde_json::to_value(&lost).unwrap(),
        serde_json::json!({ "step": "cancel_stopped", "detail": { "seen": { "kind": "outcome", "detail": { "outcome": "lost", "orphans": [] } }, "unready": null } })
    );
    let label = StatusStep::CancelStopped { seen: Some(Seen::Label(crate::remote::submit::Label::NotOnServer)), unready: None };
    assert_eq!(serde_json::to_value(&label).unwrap()["detail"]["seen"], serde_json::json!({ "kind": "label", "detail": "not_on_server" }));
    let unready = StatusStep::CancelStopped { seen: None, unready: Some("prepare: x".into()) };
    assert_eq!(serde_json::to_value(&unready).unwrap()["detail"], serde_json::json!({ "seen": null, "unready": "prepare: x" }));
}
