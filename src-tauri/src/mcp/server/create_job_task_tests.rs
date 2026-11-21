//! Tests for the `create_job` task (ADR-027 M0 amendment, items 6 and 9; M3 amendment, item 1, M3a):
//! the whole call runs in one task admitted through the gate, and nothing — a hang-up, a service
//! cancel, a switch-off, an exit — abandons steps B and C once step A has committed. The permit goes
//! with the work. Every test runs on `tauri::async_runtime`, as the app does.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

use super::create_job_task::create_job_call;
use super::gate::{AdmissionGate, Hook};
use super::lifecycle::{BlockingPool, ExitSteps, Hooks};
use super::service::ConnState;
use super::test_support::{lifecycle, Scratch};
use crate::mcp::create_job::Outcome;
use crate::mcp::create_job_test_support::{eventually, mcp_jobs, FakeStepB, Hand, World};
use crate::mcp::tools::ToolError;

const SOON: Duration = Duration::from_secs(5);

/// The world's one MCP job's `mcp_state`, read on a fresh connection (any thread).
fn mcp_state(world_dir: &std::path::Path) -> Option<Option<String>> {
    let conn = rusqlite::Connection::open(world_dir.join("orcastudio.db")).unwrap();
    mcp_jobs(&conn).first().map(|j| j.3.clone())
}

/// Step C's outcome row exists (the second `create_job` row).
fn step_c_row(w: &World) -> bool {
    w.audit().len() == 2
}

/// Control (M0 amendment, item 9): `create_job` abandoning step B or C when `closed` is set after step
/// A. The connection's `closed` flag is set at step A's emit (right after its commit); the call still
/// runs steps B and C, and the job ends with its flag cleared and its outcome row. NEGATIVE CONTROL:
/// a handler that runs steps B and C itself after checking `closed` → they never run → red.
#[test]
fn closed_after_step_a_does_not_abandon_steps_b_and_c() {
    let w = World::new("task-closed", 1);
    let step_b = Arc::new(FakeStepB::new(w.state.clone(), Hand::Over));
    let conn = Arc::new(ConnState::new());
    let c = conn.clone();
    *w.emit.on_emit.lock().unwrap() = Some(Box::new(move || c.closed.store(true, Ordering::SeqCst)));
    let gate = AdmissionGate::default();
    gate.open();

    let result = tauri::async_runtime::block_on(create_job_call(&conn, &gate, w.env(step_b.clone()), w.args()));
    assert!(conn.is_closed(), "the hang-up was played");
    let finished = (step_b.calls(), mcp_state(&w.db.dir), step_c_row(&w));
    assert_eq!(finished, (1, Some(None), true), "ABANDON-CLOSED: steps B and C were abandoned on `closed`: {result:?}");
}

/// Control (M0 amendment, item 9): any step of `create_job` running inside the handler instead of the
/// tracked task. The gate's test hook sits between its check and its spawn (under the gate's mutex):
/// when it runs, nothing of the call has been read or written yet. NEGATIVE CONTROL: run the pipeline
/// in the handler before the spawn → the hook sees the job and its audit row → red.
#[test]
fn no_step_runs_in_the_handler_before_the_spawn() {
    let w = World::new("task-in-handler", 1);
    let step_b = Arc::new(FakeStepB::new(w.state.clone(), Hand::Over));
    let dir = w.db.dir.clone();
    let seen: Arc<Mutex<Vec<(i64, i64)>>> = Arc::default();
    let s = seen.clone();
    let hook: Hook = Arc::new(move || {
        let conn = rusqlite::Connection::open(dir.join("orcastudio.db")).unwrap();
        let count = |sql: &str| conn.query_row(sql, [], |r| r.get::<_, i64>(0)).unwrap();
        s.lock().unwrap().push((count("SELECT COUNT(*) FROM jobs WHERE origin = 'mcp'"), count("SELECT COUNT(*) FROM mcp_audit")));
    });
    let gate = AdmissionGate::new(Some(hook));
    gate.open();
    let conn = ConnState::new();

    let result = tauri::async_runtime::block_on(create_job_call(&conn, &gate, w.env(step_b), w.args()));
    assert_eq!(*seen.lock().unwrap(), vec![(0, 0)], "IN-HANDLER: a step ran in the handler before the spawn");
    assert_eq!(result.unwrap().disposition, Outcome::Submitted);
}

/// Control (M0 amendment, item 9): a service cancel right after step A's commit leaving the job at
/// `mcp_state = 'submitting'`. The handler's task is aborted at step A's emit (the service cancel
/// drops the handler); the tracked task runs steps B and C anyway. NEGATIVE CONTROL: steps B and C
/// awaited in the handler instead of the task → they die with the handler → red.
#[test]
fn a_service_cancel_right_after_step_a_leaves_nothing_submitting() {
    let w = World::new("task-cancel", 1);
    let (step_b, entered, release) = FakeStepB::gated(w.state.clone(), Hand::Over);
    let (after_a, wait_a) = mpsc::channel::<()>();
    let after_a = Mutex::new(after_a);
    *w.emit.on_emit.lock().unwrap() = Some(Box::new(move || {
        let _ = after_a.lock().unwrap().send(());
    }));
    let conn = Arc::new(ConnState::new());
    let gate = Arc::new(AdmissionGate::default());
    gate.open();

    let (c, g, env, args) = (conn.clone(), gate.clone(), w.env(step_b.clone()), w.args());
    let handler = tauri::async_runtime::spawn(async move { create_job_call(&c, &g, env, args).await });
    wait_a.recv_timeout(SOON).expect("step A committed");
    handler.abort();
    drop(release); // step B may finish whenever it is reached
    let _ = entered.recv_timeout(Duration::from_secs(1));

    let done = eventually(SOON, || mcp_state(&w.db.dir) == Some(None) && step_c_row(&w));
    assert!(done, "CANCEL-AFTER-A: a service cancel after step A left the job {:?}", mcp_state(&w.db.dir));
    assert!(eventually(SOON, || gate.task_count() == 0));
}

/// Control (M0 amendment, item 9): a `create_job` task starting after the gate has closed. The call
/// is refused "MCP is shutting down" with nothing read or written and its permit back. NEGATIVE
/// CONTROL: spawn the task without the gate → the job is created → red.
#[test]
fn a_closed_gate_refuses_with_nothing_written() {
    let w = World::new("task-gate", 1);
    let step_b = Arc::new(FakeStepB::new(w.state.clone(), Hand::Over));
    let gate = AdmissionGate::default(); // closed
    let conn = ConnState::new();
    let result = tauri::async_runtime::block_on(create_job_call(&conn, &gate, w.env(step_b.clone()), w.args()));
    std::thread::sleep(Duration::from_millis(200)); // a task that slipped through would have written by now
    let written = (w.mcp_jobs().len(), w.audit().len(), step_b.calls());
    assert!(
        matches!(&result, Err(ToolError::Refused(m)) if m == "MCP is shutting down") && written == (0, 0, 0),
        "GATE-CLOSED: a task started after the gate closed: {result:?}, written {written:?}"
    );
    assert_eq!(conn.in_flight(), 0, "the refused call's permit is back");
}

/// Control (M0 amendment, item 9): the permit returned when the `create_job` handler returns instead
/// of when step C ends. The handler is cut off during step B; the permit stays held until step C has
/// written its row. NEGATIVE CONTROL: drop the permit as the task starts → it is back during step B →
/// red.
#[test]
fn the_permit_returns_when_step_c_ends_not_when_the_handler_returns() {
    let w = World::new("task-permit", 1);
    let (step_b, entered, release) = FakeStepB::gated(w.state.clone(), Hand::Over);
    let conn = Arc::new(ConnState::new());
    let gate = Arc::new(AdmissionGate::default());
    gate.open();

    let (c, g, env, args) = (conn.clone(), gate.clone(), w.env(step_b), w.args());
    let handler = tauri::async_runtime::spawn(async move { create_job_call(&c, &g, env, args).await });
    entered.recv_timeout(SOON).expect("step B entered");
    handler.abort();
    std::thread::sleep(Duration::from_millis(100));
    let held_during_b = conn.in_flight();
    release.send(()).unwrap();
    assert!(eventually(SOON, || conn.in_flight() == 0), "the permit never came back");
    assert!(
        held_during_b == 1 && step_c_row(&w),
        "PERMIT: the permit was back before step C ended (held during step B: {held_during_b})"
    );
}

/// Control (M0 amendment, item 9): switching MCP off between step A and step C leaving the job at
/// `mcp_state = 'submitting'`. Switch-off happens during step B and the handler is cut off (the
/// service cancel); the stop waits for the task, so when the stop reaches its unlink step C has
/// already cleared the flag. NEGATIVE CONTROL: spawn the task without the gate → the stop does not
/// wait, and reaches its unlink with the job still `submitting` → red.
#[test]
fn switching_off_between_a_and_c_waits_for_step_c() {
    let w = World::new("task-switch-off", 1);
    let scratch = Scratch::new("cj-off");
    let at_unlink: Arc<Mutex<Option<Option<Option<String>>>>> = Arc::default();
    let (record, dir) = (at_unlink.clone(), w.db.dir.clone());
    let hooks = Hooks { before_unlink: Some(Arc::new(move || *record.lock().unwrap() = Some(mcp_state(&dir)))), ..Hooks::default() };
    let (lc, _) = lifecycle(&scratch.paths(), w.state.clone(), hooks);
    assert!(lc.set_enabled(true).unwrap().serving);

    let (step_b, entered, release) = FakeStepB::gated(w.state.clone(), Hand::Over);
    let conn = Arc::new(ConnState::new());
    let (c, g, env, args) = (conn.clone(), lc.gate().clone(), w.env(step_b), w.args());
    let handler = tauri::async_runtime::spawn(async move { create_job_call(&c, &g, env, args).await });
    entered.recv_timeout(SOON).expect("step B entered");

    lc.set_enabled(false).unwrap();
    handler.abort();
    std::thread::sleep(Duration::from_millis(200));
    let unlinked_during_b = at_unlink.lock().unwrap().clone();
    release.send(()).unwrap();
    assert!(eventually(SOON, || at_unlink.lock().unwrap().is_some()), "the stop never completed");
    let seen = at_unlink.lock().unwrap().clone();
    assert!(
        unlinked_during_b.is_none() && seen == Some(Some(None)),
        "SWITCH-OFF: the stop reached its unlink with the job {seen:?} (during step B: {unlinked_during_b:?})"
    );
}

/// Control (M0 amendment, item 9): a clean exit releasing the lock while a `create_job` task is still
/// running. The exit flow begins during step B; its kill step (after the MCP wait, before the lock is
/// released) runs only after step C, with no task left and the lock still held. NEGATIVE CONTROL:
/// spawn the task without the gate → the exit does not wait for it → red.
#[test]
fn a_clean_exit_holds_the_lock_until_step_c_has_run() {
    let w = World::new("task-exit", 1);
    let scratch = Scratch::new("cj-exit");
    let (lc, _) = lifecycle(&scratch.paths(), w.state.clone(), Hooks::default());
    assert!(lc.set_enabled(true).unwrap().serving);

    let (step_b, entered, release) = FakeStepB::gated(w.state.clone(), Hand::Over);
    let conn = Arc::new(ConnState::new());
    let (c, g, env, args) = (conn.clone(), lc.gate().clone(), w.env(step_b), w.args());
    let _handler = tauri::async_runtime::spawn(async move { create_job_call(&c, &g, env, args).await });
    entered.recv_timeout(SOON).expect("step B entered");

    let at_kill: Arc<Mutex<Option<(usize, Option<Option<String>>, bool)>>> = Arc::default();
    let exited = Arc::new(AtomicBool::new(false));
    let (record, lc2, dir, done) = (at_kill.clone(), lc.clone(), w.db.dir.clone(), exited.clone());
    let steps = ExitSteps {
        pause_queue: Box::new(|| {}),
        terminate: Box::new(move || {
            *record.lock().unwrap() = Some((lc2.gate().task_count(), mcp_state(&dir), lc2.status().holds_lock));
        }),
        sidecar_stop: Box::new(|| {}),
        exit: Box::new(move || done.store(true, Ordering::SeqCst)),
    };
    lc.on_exit_requested(&BlockingPool, steps);
    std::thread::sleep(Duration::from_millis(300));
    let killed_during_b = at_kill.lock().unwrap().clone();
    release.send(()).unwrap();
    assert!(eventually(SOON, || exited.load(Ordering::SeqCst)), "the exit flow never ended");
    let seen = at_kill.lock().unwrap().clone();
    assert!(
        killed_during_b.is_none() && seen == Some((0, Some(None), true)),
        "EXIT-LOCK: the exit went on while a create_job task ran: at the kill {seen:?} (during step B: {killed_during_b:?})"
    );
    assert!(!lc.status().holds_lock, "the lock is released after the kill");
}
