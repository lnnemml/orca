use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::Duration;

use super::gate::*;
use super::test_support::eventually;

/// A task that runs until `release` is dropped or sent to.
fn blocked_task(gate: &AdmissionGate) -> (mpsc::Sender<()>, Arc<AtomicBool>) {
    let (release, wait) = mpsc::channel::<()>();
    let ended = Arc::new(AtomicBool::new(false));
    let done = ended.clone();
    gate.spawn(async move {
        tauri::async_runtime::spawn_blocking(move || {
            let _ = wait.recv();
        })
        .await
        .ok();
        done.store(true, Ordering::SeqCst);
    })
    .expect("the gate is open");
    (release, ended)
}

/// `wait_all` on a background thread; the flag turns true when it returns.
fn wait_all_in_background(gate: Arc<AdmissionGate>) -> Arc<AtomicBool> {
    let returned = Arc::new(AtomicBool::new(false));
    let flag = returned.clone();
    thread::spawn(move || {
        tauri::async_runtime::block_on(gate.wait_all());
        flag.store(true, Ordering::SeqCst);
    });
    returned
}

#[test]
fn a_new_gate_is_closed_and_its_wait_is_immediate() {
    let gate = AdmissionGate::default();
    assert!(!gate.is_open());
    assert_eq!(gate.spawn(async {}).err(), Some(GateClosed));
    tauri::async_runtime::block_on(gate.wait_all());
}

/// Control (M2 amendment item 1): work admitted after the gate is closed.
#[test]
fn work_is_refused_once_the_gate_is_closed() {
    let gate = AdmissionGate::default();
    gate.open();
    assert!(gate.spawn(async {}).is_ok());
    gate.close();
    assert_eq!(gate.spawn(async {}).err(), Some(GateClosed));
    assert_eq!(GateClosed.to_string(), "MCP is shutting down");
}

/// Control (M2 amendment item 1): the gate's check and the spawn not done under one lock. The hook
/// sits between them and starts a `close` on another thread: that close must stay blocked until
/// the spawn is done, so the task it admitted is one `close` (and so `wait_all`) covers.
#[test]
fn a_close_cannot_slip_between_the_check_and_the_spawn() {
    let gate_cell: Arc<Mutex<Option<Arc<AdmissionGate>>>> = Arc::new(Mutex::new(None));
    let close_finished_during_hook = Arc::new(AtomicBool::new(false));
    let (cell, flag) = (gate_cell.clone(), close_finished_during_hook.clone());
    let hook: Hook = Arc::new(move || {
        let gate = cell.lock().unwrap().clone().unwrap();
        let closer = thread::spawn(move || gate.close());
        thread::sleep(Duration::from_millis(200));
        flag.store(closer.is_finished(), Ordering::SeqCst);
        // Detach: it finishes once the spawn releases the gate's mutex.
    });
    let gate = Arc::new(AdmissionGate::new(Some(hook)));
    *gate_cell.lock().unwrap() = Some(gate.clone());
    gate.open();

    let (release, ended) = blocked_task(&gate);
    assert!(!close_finished_during_hook.load(Ordering::SeqCst), "close ran between the check and the spawn");
    assert!(eventually(Duration::from_secs(2), || !gate.is_open()), "the close completes after the spawn");
    assert_eq!(gate.task_count(), 1, "the admitted task is tracked");

    let waited = wait_all_in_background(gate.clone());
    thread::sleep(Duration::from_millis(100));
    assert!(!waited.load(Ordering::SeqCst), "wait_all covers the admitted task");
    drop(release);
    assert!(eventually(Duration::from_secs(2), || waited.load(Ordering::SeqCst)));
    assert!(ended.load(Ordering::SeqCst));
}

/// Control (M2 amendment item 1): a task of an older generation outliving the wait. A task of the
/// first generation still runs after a second open and close; `wait_all` waits for it.
#[test]
fn every_generation_is_kept_until_its_tasks_end() {
    let gate = Arc::new(AdmissionGate::default());
    gate.open();
    let (release, ended) = blocked_task(&gate);
    gate.close();
    gate.open(); // a fresh generation
    gate.close();
    assert_eq!(gate.task_count(), 1);

    let waited = wait_all_in_background(gate.clone());
    thread::sleep(Duration::from_millis(150));
    assert!(!waited.load(Ordering::SeqCst), "wait_all returned while an older generation's task runs");
    drop(release);
    assert!(eventually(Duration::from_secs(2), || waited.load(Ordering::SeqCst)));
    assert!(ended.load(Ordering::SeqCst));
    assert_eq!(gate.task_count(), 0);
}
