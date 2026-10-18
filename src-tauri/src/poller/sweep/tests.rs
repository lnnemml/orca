//! The sweep of a `cancelling` job (ADR-024 o22.5, o22.13.3–4) over the delivery tests' fake
//! server, which knows every call a sweep makes (prepare, install, `cancel.sh`, collect) and checks
//! the database lock is free on each.

use std::cell::Cell;
use std::sync::Mutex;

use crate::models::job::JobStatus;
use crate::poller::cancel::tests::{cancelled, cancelling_session, not_installed, server, timeout, Call::*, Fake};
use crate::poller::plan::MAX_SWEEPS_WITHOUT_PROGRESS;
use crate::poller::remote_state::Sweeps;
use crate::poller::tests::{indeterminate, pending_cancel, record, Remote};
use crate::poller::{cancelling_message, StatusStep};
use crate::remote::classify::Outcome;
use crate::ssh_backend::tests::{install_ok, ok, prepare_fresh, ran, Reply, JOB};

/// A `cancelling` row whose sweep baseline is `baseline` (as the write of `cancelling` leaves it).
fn cancelling_row(baseline: usize) -> Remote {
    let r = Remote::new();
    r.w.sql("UPDATE jobs SET status = 'cancelling'");
    r.memory.note_cancelling(JOB, baseline);
    r
}

fn swept(session: usize, error: Option<&str>) -> StatusStep {
    StatusStep::Swept { outcome: Some(Outcome::Cancelling { session }), error: error.map(String::from) }
}

fn installs(fake: &Fake<'_>) -> usize {
    fake.calls().iter().filter(|c| **c == Install).count()
}

/// o22.13.3: a sweep readies the server as delivery does — so a `cancel.sh` that reads "not
/// installed" after an app upgrade is reinstalled — then re-runs `cancel.sh`, then collects and
/// classifies. No label call (the job started). The count and the record's bound say one sweep
/// without progress; the delivery's counter is untouched. NEGATIVE CONTROL: skip `ready_server` in
/// the sweep → red (no install; the not-installed `cancel.sh` is never reinstalled).
#[test]
fn a_sweep_readies_the_server_re_sends_and_classifies() {
    let r = cancelling_row(1);
    let installed = Cell::new(false);
    let base = server(&r, vec![cancelling_session(1)]);
    let fake = Fake::new(&r, |call, values| match call {
        Prepare if !installed.get() => prepare_fresh(values),
        Install => {
            installed.set(true);
            install_ok(values)
        }
        Cancel | Collect if !installed.get() => not_installed(values),
        _ => base(call, values),
    });
    assert_eq!(r.poller(&fake).status_step(JOB).unwrap(), swept(1, None));
    assert_eq!(fake.calls(), [Prepare, Install, Prepare, Cancel, Collect]);
    assert_eq!(r.memory.get(JOB).sweeps_without_progress, 1);
    assert_eq!(r.memory.get(JOB).cancel_failures, 0, "a sweep never touches the delivery's counter");
    let state = record(&r).unwrap();
    assert_eq!(state.sweeps, Sweeps { count: 1, stopped: false });
    assert_eq!(state.known, Some(crate::poller::remote_state::Known::Shown(Outcome::Cancelling { session: 1 })));
    assert_eq!(r.row(), (JobStatus::Cancelling, Some(cancelling_message(1, false))));
    assert_eq!(pending_cancel(&r), (None, None));
}

/// o22.5 / o22.9 control: three sweeps without progress stop the automatic sweeps — the third reads
/// stopped with "processes of the job still run after 3 sweeps" — and the 4th step only collects;
/// the click's `restart_sweeps` (o22.12.1) makes the next step sweep again. NEGATIVE CONTROL: drop
/// the bound check in `sweep` → red (a 4th `cancel.sh`).
#[test]
fn three_sweeps_without_progress_stop_the_automatic_sweeps_until_cancel_is_clicked() {
    let r = cancelling_row(1);
    let fake = Fake::new(&r, server(&r, vec![cancelling_session(1)]));
    let poller = r.poller(&fake);
    for n in 1..=MAX_SWEEPS_WITHOUT_PROGRESS {
        assert_eq!(poller.status_step(JOB).unwrap(), swept(1, None), "sweep {n}");
        assert_eq!(record(&r).unwrap().sweeps, Sweeps::from_count(n));
    }
    assert!(record(&r).unwrap().sweeps.stopped, "the third reads stopped");
    assert!(r.row().1.unwrap().contains("processes of the job still run after 3 sweeps"));
    let before = fake.calls().len();
    assert_eq!(poller.status_step(JOB).unwrap(), StatusStep::Shown(Outcome::Cancelling { session: 1 }));
    assert_eq!(&fake.calls()[before..], [Collect], "stopped: collect + classify only");
    assert_eq!(fake.sends(), 3, "no 4th automatic sweep");
    assert_eq!(r.row(), (JobStatus::Cancelling, Some(cancelling_message(1, true))), "the stopped text stays");

    r.memory.restart_sweeps(JOB);
    assert_eq!(poller.status_step(JOB).unwrap(), swept(1, None));
    assert_eq!(fake.sends(), 4, "the click's restart: the next step sweeps");
}

/// o22.13.3 / control: a stopped step makes no ready, install or `cancel.sh` call, and its classify
/// changes neither the count nor the baseline — a shrink seen while stopped does not resume the
/// sweeps. NEGATIVE CONTROL: ready the server before the "stopped" check → red (a prepare call).
#[test]
fn a_stopped_step_only_collects_and_changes_no_count() {
    let r = cancelling_row(5);
    r.memory.set_sweeps_without_progress(JOB, MAX_SWEEPS_WITHOUT_PROGRESS);
    let fake = Fake::new(&r, server(&r, vec![cancelling_session(2)]));
    let poller = r.poller(&fake);
    assert_eq!(poller.status_step(JOB).unwrap(), StatusStep::Shown(Outcome::Cancelling { session: 2 }));
    assert_eq!(fake.calls(), [Collect]);
    let m = r.memory.get(JOB);
    assert_eq!((m.sweeps_without_progress, m.min_session), (MAX_SWEEPS_WITHOUT_PROGRESS, Some(5)));
    poller.status_step(JOB).unwrap();
    assert_eq!(fake.calls(), [Collect, Collect], "still stopped");
}

/// o22.13.3 / control (first half): a step whose calls got **no answer at all** is `CheckFailed`
/// and counts nothing — three of them in a row do not stop the sweeps, and the next step that
/// reaches the server sweeps. NEGATIVE CONTROL: count transport-only failures → red (the 4th step
/// sends nothing).
#[test]
fn sweeps_without_any_answer_count_nothing() {
    let r = cancelling_row(1);
    let away = Cell::new(true);
    let base = server(&r, vec![cancelling_session(1)]);
    let fake = Fake::new(&r, |call, values| if away.get() { timeout() } else { base(call, values) });
    let poller = r.poller(&fake);
    for _ in 0..3 {
        assert!(matches!(poller.status_step(JOB).unwrap(), StatusStep::CheckFailed(_)));
    }
    assert_eq!(fake.calls(), [Prepare, Prepare, Prepare], "no answer to the ready: nothing more is tried");
    assert_eq!(r.memory.get(JOB).sweeps_without_progress, 0);
    away.set(false);
    assert_eq!(poller.status_step(JOB).unwrap(), swept(1, None));
    assert_eq!(fake.sends(), 1, "the sweep runs once the server answers");
}

/// o22.13.3 / control (second half): failures the server **answered** count — a `cancel.sh` that
/// exits 3 (the step still collects and classifies, and shows the failure), and a collect whose
/// reply is broken after an rc-0 `cancel.sh` (nothing classified) — so a 4th sweep does not run.
/// NEGATIVE CONTROLS: count nothing when the step has an `error` → red (the rc-3 case sweeps a 4th
/// time); count nothing when nothing was classified → red (the broken-collect case).
#[test]
fn answered_sweep_failures_count_toward_the_bound() {
    let r = cancelling_row(1);
    let base = server(&r, vec![cancelling_session(1)]);
    let fake = Fake::new(&r, |call, values| if call == Cancel { ran(values, 3, b"", b"cancel: not a directory") } else { base(call, values) });
    let poller = r.poller(&fake);
    for _ in 0..3 {
        let step = poller.status_step(JOB).unwrap();
        assert!(
            matches!(&step, StatusStep::Swept { outcome: Some(Outcome::Cancelling { session: 1 }), error: Some(why) } if why.contains("exited 3")),
            "{step:?}"
        );
    }
    assert!(record(&r).unwrap().check_error.unwrap().contains("exited 3"), "the answered failure is the check error");
    poller.status_step(JOB).unwrap();
    assert_eq!(fake.sends(), 3, "no 4th sweep after three answered cancel.sh failures");
    assert_eq!(r.memory.get(JOB).cancel_failures, 0, "never the delivery's counter");

    let r = cancelling_row(1);
    let base = server(&r, vec![cancelling_session(1)]);
    let fake = Fake::new(&r, |call, values| if call == Collect { ok(b"garbage\n".to_vec()) } else { base(call, values) });
    let poller = r.poller(&fake);
    for _ in 0..3 {
        let step = poller.status_step(JOB).unwrap();
        assert!(matches!(&step, StatusStep::Swept { outcome: None, error: Some(_) }), "{step:?}");
    }
    poller.status_step(JOB).unwrap();
    assert_eq!(fake.sends(), 3, "no 4th sweep after three broken collects");
}

/// o22.13.7 control: a permanently failing install is an answered ready failure — it sends
/// nothing, still collects, and counts — so after three steps the 4th makes no install call.
/// NEGATIVE CONTROL: do not count a step whose ready failed → red (a 4th install).
#[test]
fn a_permanently_failing_install_stops_after_three_sweeps() {
    let r = cancelling_row(1);
    let base = server(&r, vec![cancelling_session(1)]);
    let fake = Fake::new(&r, |call, values| if call == Prepare { prepare_fresh(values) } else { base(call, values) });
    let poller = r.poller(&fake);
    for _ in 0..3 {
        let step = poller.status_step(JOB).unwrap();
        assert!(matches!(&step, StatusStep::Swept { outcome: Some(_), error: Some(why) } if why.contains("not ready")), "{step:?}");
    }
    assert_eq!(&fake.calls()[..4], [Prepare, Install, Prepare, Collect], "an answered ready failure sends nothing, then collects");
    assert_eq!(fake.sends(), 0);
    poller.status_step(JOB).unwrap();
    assert_eq!(installs(&fake), 3, "no 4th install");
}

/// o22.13.4 / control: an oscillating session (5, 6, 5, 6 …) after a restart of the app (no
/// baseline) is bounded: the first sweep sets the baseline, three more without progress stop it.
/// NEGATIVE CONTROL: make the baseline the last size seen → red (it never stops).
#[test]
fn an_oscillating_session_stops_after_three_sweeps() {
    let r = Remote::new();
    r.w.sql("UPDATE jobs SET status = 'cancelling'");
    let sessions = [5, 6, 5, 6, 5, 6, 5, 6].map(cancelling_session).to_vec();
    let fake = Fake::new(&r, server(&r, sessions));
    let poller = r.poller(&fake);
    for _ in 0..8 {
        poller.status_step(JOB).unwrap();
    }
    assert_eq!(fake.sends(), 4, "the baseline sweep, then three without progress");
    assert_eq!(r.memory.get(JOB).min_session, Some(5));
}

/// o22.13.3 (13.8.3) / control: a sweep the server answered whose classify gives a
/// non-`Cancelling` verdict that does not end the job (`Indeterminate`: the row stays `cancelling`)
/// still counts one sweep without progress — so the 4th step sends no `cancel.sh`. NEGATIVE CONTROL
/// (X1): count only `Cancelling` or nothing-classified sweeps, not a dispatched other verdict → red
/// (the count stays 0, a 4th sweep runs).
#[test]
fn a_non_cancelling_verdict_after_an_answered_sweep_is_counted() {
    let r = cancelling_row(1);
    let fake = Fake::new(&r, server(&r, vec![indeterminate()]));
    let poller = r.poller(&fake);
    for n in 1..=MAX_SWEEPS_WITHOUT_PROGRESS {
        assert_eq!(poller.status_step(JOB).unwrap(), StatusStep::Shown(Outcome::Indeterminate), "sweep {n}");
        assert_eq!(r.memory.get(JOB).sweeps_without_progress, n, "sweep {n} counted");
        assert_eq!(r.row().0, JobStatus::Cancelling, "the job is not ended");
    }
    assert_eq!(fake.sends(), 3);
    poller.status_step(JOB).unwrap();
    assert_eq!(fake.sends(), 3, "no 4th sweep");
}

/// o22.13.1 (13.8.4) / control: the fetch bound comes before the sweep — a `cancelling` row whose
/// fetching flag is set and whose strikes are at `MAX_FETCH_STRIKES` is `FetchStopped`, with no ready,
/// install or `cancel.sh` call (no call at all). NEGATIVE CONTROL (X2): move the sweep branch before
/// the `FetchStopped` check in `run_status` → red (a sweep runs).
#[test]
fn a_cancelling_row_at_the_fetch_bound_is_fetch_stopped_without_a_sweep() {
    let r = cancelling_row(1);
    r.memory.set_fetching(JOB);
    for _ in 0..crate::poller::plan::MAX_FETCH_STRIKES {
        r.memory.strike(JOB);
    }
    let fake = Fake::new(&r, server(&r, vec![cancelling_session(1)]));
    assert_eq!(r.poller(&fake).status_step(JOB).unwrap(), StatusStep::FetchStopped);
    assert_eq!(fake.calls(), [], "no prepare, install or cancel.sh");
}

/// o22.13.4: a session below the baseline is progress — the baseline falls, the count restarts.
#[test]
fn a_shrinking_session_is_progress() {
    let r = cancelling_row(3);
    r.memory.set_sweeps_without_progress(JOB, 2);
    let fake = Fake::new(&r, server(&r, vec![cancelling_session(2)]));
    assert_eq!(r.poller(&fake).status_step(JOB).unwrap(), swept(2, None));
    let m = r.memory.get(JOB);
    assert_eq!((m.sweeps_without_progress, m.min_session), (0, Some(2)));
}

/// o22.13.4 / control: the count's update is one memory operation — a `restart_sweeps` click
/// that lands while the sweep's ssh calls run is not overwritten by "the count read before + 1".
/// NEGATIVE CONTROL: read the count before the calls and write it + 1 after → red (3, not 1).
#[test]
fn a_restart_during_a_sweep_is_not_overwritten() {
    let r = cancelling_row(1);
    r.memory.set_sweeps_without_progress(JOB, 2);
    let base = server(&r, vec![cancelling_session(1)]);
    let fake = Fake::new(&r, |call, values| {
        if call == Cancel {
            r.memory.restart_sweeps(JOB);
        }
        base(call, values)
    });
    r.poller(&fake).status_step(JOB).unwrap();
    assert_eq!(r.memory.get(JOB).sweeps_without_progress, 1);
}

/// o22.13.3: a sweep whose classify shows the job no longer runs is acted on like an ordinary
/// step's outcome in the same step — row 4 without `.started` ends `cancelled` at once, with
/// `.started` after its evidence fetch.
#[test]
fn a_sweep_that_sees_the_job_cancelled_ends_it_in_the_same_step() {
    let r = cancelling_row(1);
    let fake = Fake::new(&r, server(&r, vec![cancelled()]));
    assert_eq!(r.poller(&fake).status_step(JOB).unwrap(), StatusStep::Cancelled);
    assert_eq!(fake.calls(), [Prepare, Cancel, Collect]);
    assert_eq!(r.row().0, JobStatus::Cancelled);
    assert_eq!(record(&r), None, "terminal: the record is removed");

    let r = cancelling_row(1);
    let fake = Fake::new(&r, server(&r, vec![cancelling_session(0)]));
    assert_eq!(r.poller(&fake).status_step(JOB).unwrap(), StatusStep::Cancelled);
    assert_eq!(fake.calls(), [Prepare, Cancel, Collect, Download, List], "row 4 with .started: evidence first");
    assert_eq!(r.row().0, JobStatus::Cancelled);
    assert_eq!(r.memory.get(JOB), Default::default());
}

/// A sweep's text write is guarded on the row: a row that turned terminal under the sweep is
/// `RowChanged`, its text not overwritten.
#[test]
fn a_sweep_on_a_row_that_changed_is_row_changed() {
    let r = cancelling_row(1);
    let base = server(&r, vec![cancelling_session(1)]);
    let changed = Mutex::new(false);
    let fake = Fake::new(&r, |call, values| -> Reply {
        if call == Collect && !std::mem::replace(&mut *changed.lock().unwrap(), true) {
            r.w.sql("UPDATE jobs SET status = 'cancelled', error_message = 'someone else'");
        }
        base(call, values)
    });
    assert_eq!(r.poller(&fake).status_step(JOB).unwrap(), StatusStep::RowChanged);
    assert_eq!(r.row(), (JobStatus::Cancelled, Some("someone else".into())));
}

/// The wire shape `src/types.ts` mirrors for the sweep step (o22.13.3).
#[test]
fn the_swept_wire_shape() {
    assert_eq!(
        serde_json::to_value(swept(2, Some("cancel.sh exited 3"))).unwrap(),
        serde_json::json!({ "step": "swept", "detail": { "outcome": { "outcome": "cancelling", "session": 2 }, "error": "cancel.sh exited 3" } })
    );
    assert_eq!(
        serde_json::to_value(StatusStep::Swept { outcome: None, error: Some("x".into()) }).unwrap()["detail"],
        serde_json::json!({ "outcome": null, "error": "x" })
    );
}
