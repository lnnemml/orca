//! Two status-step rules of unit 5.4b over the 5.4b fake server (`poller::fake_tests`): the
//! **`Indeterminate` count** (ADR-024 l row 10, o25.6, o25.14.6) and **ready on a not-installed
//! collect** (o25.7). Each test names the o25.11 gate it is; its negative control is a source
//! mutation in `run_54b_controls.sh`, named at the test.

use std::cell::Cell;

use crate::models::job::JobStatus;
use crate::poller::fake_tests::{timeout, Call::*, Fake, ID};
use crate::poller::plan::UNDECIDED_NOTICE_AT;
use crate::poller::tests::{indeterminate, record, Remote};
use crate::poller::{step_effect, StatusStep};
use crate::remote::classify::{Outcome, STARTED_DISAGREED};
use crate::remote::run::RUN_HEADER;
use crate::ssh_backend::tests::{echo, install_ok, ok, prepare_fresh, prepare_ready, ran, snapshot_with_row};

fn undecided() -> Outcome {
    Outcome::Indeterminate { why: STARTED_DISAGREED.into() }
}

/// o25.6, o25.11 (`Indeterminate`): the record's `undecided` notice appears at the 3rd consecutive
/// pass and names the why; a check without an answer neither counts nor resets the run; a decided
/// verdict resets it. No row write, no action. NEGATIVE CONTROLS: `undecided-no-reset` (a decided
/// verdict keeps the run → red); `undecided-at-first` (the notice from the 1st pass → red);
/// `undecided-checkfailed-resets` (a check without an answer resets it → red).
#[test]
fn the_undecided_notice_comes_at_the_third_pass_and_a_decided_verdict_resets_it() {
    let r = Remote::new();
    r.w.sql("UPDATE jobs SET status = 'running'");
    let mode = Cell::new("undecided");
    let fake = Fake::new(&r, |call, values| match (call, mode.get()) {
        (Collect, "undecided") => ran(values, 0, &indeterminate(), b""),
        (Collect, "away") => timeout(),
        (Collect, _) => ran(values, 0, &snapshot_with_row("running"), b""),
        (other, _) => panic!("a running row: only the collector, got {other:?}"),
    });
    let poller = r.poller(&fake);
    for n in 1..UNDECIDED_NOTICE_AT {
        assert_eq!(poller.status_step(ID).unwrap(), StatusStep::Shown(undecided()));
        assert_eq!(record(&r).unwrap().undecided, None, "pass {n}: no notice yet");
        assert_eq!(r.memory.get(ID).undecided_passes, n);
    }
    assert_eq!(poller.status_step(ID).unwrap(), StatusStep::Shown(undecided()));
    let notice = record(&r).unwrap().undecided.expect("the 3rd pass");
    assert_eq!(notice.passes, UNDECIDED_NOTICE_AT);
    assert_eq!(
        notice.notice,
        format!("The server's state of this job could not be decided 3 times in a row: {STARTED_DISAGREED}")
    );
    assert_eq!(r.row(), (JobStatus::Running, None), "no row write");

    mode.set("away");
    assert!(matches!(poller.status_step(ID).unwrap(), StatusStep::CheckFailed(_)));
    assert_eq!(record(&r).unwrap().undecided.map(|u| u.passes), Some(UNDECIDED_NOTICE_AT), "a check without an answer changes nothing");

    mode.set("undecided");
    poller.status_step(ID).unwrap();
    assert_eq!(record(&r).unwrap().undecided.map(|u| u.passes), Some(UNDECIDED_NOTICE_AT + 1), "consecutive across it");

    mode.set("decided");
    assert_eq!(poller.status_step(ID).unwrap(), StatusStep::Shown(Outcome::Queued));
    assert_eq!(record(&r).unwrap().undecided, None, "a decided verdict resets the run");
    assert_eq!(r.memory.get(ID).undecided_passes, 0);
}

/// o25.6, o22.12.2, o25.14.6: the delivery's verdicts feed the same run (a delivered or stopped
/// cancel that read `Indeterminate` counts; a stopped delivery that saw a decided verdict resets),
/// while a label and a pending submit's re-check neither count nor reset — the effect table.
#[test]
fn every_step_feeds_or_keeps_the_runs_as_the_table_says() {
    use crate::poller::plan::{ReEnqueueRun, Undecided};
    use crate::poller::Seen;
    let count = |step: StatusStep| step_effect(&step).undecided;
    assert_eq!(count(StatusStep::CancelDelivered(undecided())), Undecided::Count(STARTED_DISAGREED.into()));
    assert_eq!(count(StatusStep::CancelStopped { seen: Some(Seen::Outcome(undecided())), unready: None }), Undecided::Count(STARTED_DISAGREED.into()));
    assert_eq!(count(StatusStep::CancelStopped { seen: Some(Seen::Outcome(Outcome::Running)), unready: None }), Undecided::Reset);
    assert_eq!(count(StatusStep::Labelled(crate::remote::submit::Label::NotOnServer)), Undecided::Keep);
    assert_eq!(count(StatusStep::CheckFailed("x".into())), Undecided::Keep);
    assert_eq!(count(StatusStep::Shown(Outcome::Running)), Undecided::Reset);
    let reenqueue = |step: StatusStep| step_effect(&step).reenqueue;
    assert_eq!(reenqueue(StatusStep::Shown(Outcome::Running)), ReEnqueueRun::End);
    assert_eq!(reenqueue(StatusStep::ReEnqueued { tsp_id: 1 }), ReEnqueueRun::End);
    for kept in [
        StatusStep::CheckFailed("x".into()),
        StatusStep::CancelNotSent("x".into()),
        StatusStep::CancelStopped { seen: Some(Seen::Outcome(Outcome::Running)), unready: None },
        StatusStep::ReEnqueueRefused { refusals: 1, why: "x".into() },
    ] {
        assert_eq!(reenqueue(kept.clone()), ReEnqueueRun::Keep, "{kept:?}");
    }
}

/// The trampoline's `not-installed` reply.
fn not_installed(values: &[String]) -> crate::ssh_backend::tests::Reply {
    ok(format!("{}not-installed\nend\n", echo(RUN_HEADER, values)).into_bytes())
}

/// o25.7, o25.11 (Ready on `NotInstalled`): after an app upgrade the ordinary step's collect reads
/// "not installed"; the step readies the server once (prepare → install → prepare) and collects
/// again, in the same step. NEGATIVE CONTROL (`no-ready-on-not-installed`): the ordinary step
/// collects without readying → it stays not installed, red.
#[test]
fn a_not_installed_collect_readies_the_server_and_collects_again() {
    let r = Remote::new();
    r.w.sql("UPDATE jobs SET status = 'running'");
    let installed = Cell::new(false);
    let fake = Fake::new(&r, |call, values| match call {
        Prepare if installed.get() => prepare_ready(values),
        Prepare => prepare_fresh(values),
        Install => {
            installed.set(true);
            install_ok(values)
        }
        Collect if installed.get() => ran(values, 0, &snapshot_with_row("running"), b""),
        Collect => not_installed(values),
        other => panic!("{other:?}"),
    });
    assert_eq!(r.poller(&fake).status_step(ID).unwrap(), StatusStep::Shown(Outcome::Queued));
    assert_eq!(fake.calls(), [Collect, Prepare, Install, Prepare, Collect], "one ready, one more collect");
}

/// o25.7: a second not-installed (the ready did not help) is the step's failed check, as before.
#[test]
fn a_second_not_installed_collect_is_a_failed_check() {
    let r = Remote::new();
    r.w.sql("UPDATE jobs SET status = 'running'");
    let fake = Fake::new(&r, |call, values| match call {
        Prepare => prepare_ready(values),
        Collect => not_installed(values),
        other => panic!("{other:?}"),
    });
    let step = r.poller(&fake).status_step(ID).unwrap();
    assert!(matches!(&step, StatusStep::CheckFailed(why) if why.contains("not installed")), "{step:?}");
    assert_eq!(fake.calls(), [Collect, Prepare, Collect]);
    assert!(record(&r).unwrap().check_error.unwrap().answered);
}
