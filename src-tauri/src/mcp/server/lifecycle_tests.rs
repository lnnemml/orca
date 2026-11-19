//! The lifecycle: the tick table, the lock and the reconciliation, the switch, the stop, and the
//! exit flow. Every async piece runs on `tauri::async_runtime`.

use std::os::unix::fs::MetadataExt;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Condvar, Mutex};
use std::thread::{self, ThreadId};
use std::time::Duration;

use super::lifecycle::*;
use super::lock::{try_acquire, Acquire};
use super::test_support::{counted_env, eventually, lifecycle, set_row, Client, Counts, Scratch};
use super::tick::{run_ticks, Interval};
use crate::mcp::steps::reconcile_submitting_conn;
use crate::mcp::switch::read_enabled_conn;

const SOON: Duration = Duration::from_secs(3);

fn observed(row_on: Option<bool>, holds_lock: bool, serving: bool, stop_running: bool) -> Observed {
    Observed { exiting: false, row_on, holds_lock, serving, stop_running }
}

/// The tick table (M2 amendment item 2), row by row.
#[test]
fn decide_follows_the_tick_table() {
    use Action::*;
    let exiting = Observed { exiting: true, ..observed(Some(true), true, false, false) };
    assert_eq!(decide(&exiting), Nothing(Why::Exiting));
    assert_eq!(decide(&observed(None, true, true, false)), Nothing(Why::RowUnreadable));
    assert_eq!(decide(&observed(Some(true), true, true, false)), Nothing(Why::Serving));
    assert_eq!(decide(&observed(Some(true), true, false, true)), Nothing(Why::StopRunning));
    assert_eq!(decide(&observed(Some(true), false, false, true)), Nothing(Why::StopRunning));
    assert_eq!(decide(&observed(Some(true), true, false, false)), Start);
    assert_eq!(decide(&observed(Some(true), false, false, false)), Acquire);
    assert_eq!(decide(&observed(Some(false), true, true, false)), Stop);
    assert_eq!(decide(&observed(Some(false), true, false, false)), Nothing(Why::Off));
    assert_eq!(decide(&observed(Some(false), false, false, false)), Nothing(Why::Off));
}

#[test]
fn pass_through_is_keyed_on_exit_ready_alone() {
    assert!(pass_through(true));
    assert!(!pass_through(false));
}

/// Two instances on one DB and one lock path.
fn two_instances(tag: &str) -> (Scratch, Arc<McpLifecycle>, Arc<Counts>, Arc<McpLifecycle>, Arc<Counts>) {
    let scratch = Scratch::new(tag);
    let paths = scratch.paths();
    let (a, ca) = lifecycle(&paths, scratch.db(), Hooks::default());
    let (b, cb) = lifecycle(&paths, scratch.db(), Hooks::default());
    (scratch, a, ca, b, cb)
}

fn ino(path: &std::path::Path) -> u64 {
    std::fs::symlink_metadata(path).unwrap().ino()
}

/// Wait until the latest stop has completed (the status shows no stop running).
fn stop_completes(lc: &McpLifecycle) -> bool {
    eventually(SOON, || lc.status().stopping_tasks.is_none())
}

/// Controls (M2 amendment item 1): a second instance binding without the lock, and an instance
/// without the lock reconciling. B reads "on" but A holds the lock: B neither reconciles nor binds,
/// says why, and A's socket is untouched.
#[test]
fn a_second_instance_without_the_lock_neither_reconciles_nor_binds() {
    let (scratch, a, ca, b, cb) = two_instances("second");
    assert!(a.set_enabled(true).unwrap().serving);
    let socket = scratch.paths().socket;
    let a_ino = ino(&socket);

    assert_eq!(b.tick_step(), Action::Acquire);
    let status = b.status();
    assert!(!status.holds_lock && !status.serving, "{status:?}");
    assert_eq!(status.reason.as_deref(), Some(ANOTHER_INSTANCE));
    assert_eq!((cb.reconciles(), cb.binds()), (0, 0));
    assert_eq!(ino(&socket), a_ino, "A's socket untouched");
    assert_eq!((ca.reconciles(), ca.binds()), (1, 1));
    Client::connect(&socket).initialize(1);
}

/// Control (M2 amendment item 1): binding or accepting before `reconcile_submitting_conn` returns.
/// The reconciliation looks for a socket and a listener while it runs.
#[test]
fn nothing_is_bound_or_accepted_until_the_reconciliation_returns() {
    let scratch = Scratch::new("reconcile-first");
    let paths = scratch.paths();
    let (mut env, _) = counted_env(&paths, scratch.db(), Hooks::default());
    let seen_early = Arc::new(AtomicBool::new(false));
    let (flag, socket) = (seen_early.clone(), paths.socket.clone());
    env.reconcile = Box::new(move |conn| {
        let bound = socket.exists() || std::os::unix::net::UnixStream::connect(&socket).is_ok();
        flag.store(bound, Ordering::SeqCst);
        reconcile_submitting_conn(conn)
    });
    let lc = McpLifecycle::new(env);
    assert!(lc.set_enabled(true).unwrap().serving);
    assert!(!seen_early.load(Ordering::SeqCst), "a socket existed while the reconciliation ran");
}

/// Control (M2 amendment item 1): a reconcile error (`SQLITE_BUSY`) followed by serving. Required:
/// no serve, the reason shown, a new attempt on the next tick.
#[test]
fn a_reconcile_error_serves_nothing_shows_why_and_is_retried_on_the_next_tick() {
    let scratch = Scratch::new("reconcile-busy");
    let db = scratch.db();
    // rusqlite's default busy timeout is 5 s per attempt; the BUSY is the same with 50 ms.
    db.lock().unwrap().busy_timeout(Duration::from_millis(50)).unwrap();
    let (lc, counts) = lifecycle(&scratch.paths(), db, Hooks::default());
    let other = scratch.another_connection();
    set_row(&other, true);
    other.execute_batch("BEGIN IMMEDIATE").unwrap(); // another instance's write transaction

    assert_eq!(lc.tick_step(), Action::Acquire);
    let status = lc.status();
    assert!(status.holds_lock && !status.serving, "{status:?}");
    let reason = status.reason.unwrap_or_default();
    assert!(reason.contains("reconciliation failed"), "{reason}");
    assert!(!scratch.paths().socket.exists());
    assert_eq!(counts.binds(), 0);

    lc.tick_step();
    assert!(!lc.status().serving, "still busy: still no serve");
    other.execute_batch("COMMIT").unwrap();
    lc.tick_step();
    assert!(lc.status().serving);
    assert_eq!((counts.reconciles(), counts.binds()), (3, 1));
}

/// Control (M2 amendment item 1): `set_enabled(false)`, or a tick reading "off", releasing the
/// `flock`.
#[test]
fn switch_off_and_an_off_tick_keep_the_flock() {
    let scratch = Scratch::new("keep-flock");
    let paths = scratch.paths();
    let (lc, _) = lifecycle(&paths, scratch.db(), Hooks::default());
    lc.set_enabled(true).unwrap();
    lc.set_enabled(false).unwrap();
    assert!(stop_completes(&lc));
    assert!(matches!(try_acquire(&paths.lock).unwrap(), Acquire::HeldElsewhere), "switch-off released the lock");

    lc.set_enabled(true).unwrap();
    set_row(&scratch.another_connection(), false);
    assert_eq!(lc.tick_step(), Action::Stop);
    assert!(stop_completes(&lc));
    assert!(matches!(try_acquire(&paths.lock).unwrap(), Acquire::HeldElsewhere), "an off tick released the lock");
    assert!(lc.status().holds_lock);
}

/// Control (M2 amendment item 1): a second switch-on after a completed stop, on the same
/// acquisition, reconciling again.
#[test]
fn a_second_switch_on_on_the_same_acquisition_does_not_reconcile_again() {
    let scratch = Scratch::new("once-per-acq");
    let (lc, counts) = lifecycle(&scratch.paths(), scratch.db(), Hooks::default());
    lc.set_enabled(true).unwrap();
    lc.set_enabled(false).unwrap();
    assert!(stop_completes(&lc));
    assert!(lc.set_enabled(true).unwrap().serving);
    assert_eq!((counts.reconciles(), counts.binds()), (1, 2));
}

/// Control (M2 amendment item 1): a socket on disk once a stop has completed.
#[test]
fn a_completed_stop_leaves_no_socket() {
    let scratch = Scratch::new("no-socket");
    let (lc, _) = lifecycle(&scratch.paths(), scratch.db(), Hooks::default());
    lc.set_enabled(true).unwrap();
    assert!(scratch.paths().socket.exists());
    lc.set_enabled(false).unwrap();
    assert!(stop_completes(&lc));
    assert!(!scratch.paths().socket.exists(), "the socket outlived its completed stop");
}

/// A two-party rendezvous with a timeout: inside a correct transition the second party is blocked
/// on the lifecycle mutex, so the first waits out the timeout alone.
fn rendezvous(timeout: Duration) -> Arc<dyn Fn(Action) + Send + Sync> {
    let state = Arc::new((Mutex::new(0usize), Condvar::new()));
    Arc::new(move |_| {
        let (count, cv) = &*state;
        let mut n = count.lock().unwrap();
        *n += 1;
        cv.notify_all();
        let _ = cv.wait_timeout_while(n, timeout, |n| *n < 2).unwrap();
    })
}

/// Control (M2 amendment item 1): two concurrent tick steps (the background tick and
/// `mcp_set_enabled`) with the row "on" giving more than one reconcile or more than one bind. The
/// hook sits between each transition's decision and its action; if the mutex were not held across
/// both, both would decide to start.
#[test]
fn two_concurrent_transitions_start_once() {
    let scratch = Scratch::new("concurrent");
    let paths = scratch.paths();
    let armed = Arc::new(AtomicBool::new(false));
    let (gate_flag, meet) = (armed.clone(), rendezvous(Duration::from_millis(300)));
    let hooks = Hooks {
        after_decide: Some(Arc::new(move |a| {
            if gate_flag.load(Ordering::SeqCst) {
                meet(a)
            }
        })),
        ..Hooks::default()
    };
    let (lc, counts) = lifecycle(&paths, scratch.db(), hooks);
    // The holder, switched off after one start: both transitions below decide on a holder.
    lc.set_enabled(true).unwrap();
    lc.set_enabled(false).unwrap();
    assert!(stop_completes(&lc));
    set_row(&scratch.another_connection(), true);
    armed.store(true, Ordering::SeqCst);

    let (l1, l2) = (lc.clone(), lc.clone());
    let tick = thread::spawn(move || l1.tick_step());
    let toggle = thread::spawn(move || l2.set_enabled(true).unwrap());
    tick.join().unwrap();
    toggle.join().unwrap();
    assert!(lc.status().serving);
    assert_eq!((counts.reconciles(), counts.binds()), (1, 2), "one start for the two transitions");
}

/// A tracked task (M3's kind) that runs until released.
fn blocked_task(lc: &McpLifecycle) -> (mpsc::Sender<()>, Arc<AtomicBool>) {
    let (release, wait) = mpsc::channel::<()>();
    let ended = Arc::new(AtomicBool::new(false));
    let done = ended.clone();
    lc.gate()
        .spawn(async move {
            let _ = tauri::async_runtime::spawn_blocking(move || wait.recv()).await;
            done.store(true, Ordering::SeqCst);
        })
        .expect("the gate is open while serving");
    (release, ended)
}

/// Control (M2 amendment item 1): a row reading "on" while a stop is still running reconciling or
/// binding. And an old stop's unlink never removes the newer socket.
#[test]
fn on_during_a_running_stop_waits_for_the_stop() {
    let scratch = Scratch::new("on-during-stop");
    let paths = scratch.paths();
    let (lc, counts) = lifecycle(&paths, scratch.db(), Hooks::default());
    lc.set_enabled(true).unwrap();
    let (release, ended) = blocked_task(&lc);
    let off = lc.set_enabled(false).unwrap();
    assert_eq!(off.stopping_tasks, Some(1), "{off:?}");

    let on = lc.set_enabled(true).unwrap();
    assert!(!on.serving, "{on:?}");
    assert_eq!(on.reason.as_deref(), Some(STOPPING));
    assert_eq!((counts.reconciles(), counts.binds()), (1, 1), "nothing started during the stop");
    assert!(paths.socket.exists(), "the old socket stays until its stop's unlink");

    drop(release);
    assert!(eventually(SOON, || ended.load(Ordering::SeqCst)));
    assert!(stop_completes(&lc));
    assert!(!paths.socket.exists());
    assert_eq!(lc.tick_step(), Action::Start, "the next tick after the stop starts serving");
    assert!(lc.status().serving);
    assert!(paths.socket.exists());
}

/// Control (M2 amendment item 1): a tick whose row read fails changing any state.
#[test]
fn a_tick_whose_row_read_fails_changes_nothing() {
    let scratch = Scratch::new("row-fails");
    let paths = scratch.paths();
    let (lc, counts) = lifecycle(&paths, scratch.db(), Hooks::default());
    lc.set_enabled(true).unwrap();
    let before = lc.status();
    let other = scratch.another_connection();

    other.execute_batch("ALTER TABLE settings RENAME TO settings_away").unwrap();
    assert_eq!(lc.tick_step(), Action::Nothing(Why::RowUnreadable));
    let during = lc.status();
    assert!(during.serving && during.holds_lock, "{during:?}");
    assert_eq!(during.enabled, None, "an unreadable row is shown as unknown, not as off");
    assert_eq!(during.reason, before.reason);
    assert!(paths.socket.exists());
    other.execute_batch("ALTER TABLE settings_away RENAME TO settings").unwrap();
    assert_eq!(counts.binds(), 1);
}

/// Control (M2 amendment item 1): the holder not starting to serve within one tick after another
/// instance writes `mcp_enabled = "true"`.
#[test]
fn the_holder_serves_one_tick_after_another_instance_turns_the_row_on() {
    let (_scratch, a, _, b, _) = two_instances("row-on");
    a.set_enabled(true).unwrap();
    a.set_enabled(false).unwrap();
    assert!(stop_completes(&a));
    assert!(!b.set_enabled(true).unwrap().serving, "B is not the holder");
    assert_eq!(a.tick_step(), Action::Start);
    assert!(a.status().serving);
}

/// Control (M2 amendment item 1): the holder still serving one tick after the row reads "off".
#[test]
fn the_holder_stops_one_tick_after_another_instance_turns_the_row_off() {
    let (scratch, a, _, b, _) = two_instances("row-off");
    a.set_enabled(true).unwrap();
    b.set_enabled(false).unwrap();
    assert!(a.status().serving, "not yet: A has not ticked");
    assert_eq!(a.tick_step(), Action::Stop);
    assert!(!a.status().serving);
    assert!(stop_completes(&a));
    assert!(!scratch.paths().socket.exists());
}

/// An interval the test drives: each `wait` reports the finished step and blocks for the next go.
struct Driven {
    done: mpsc::Sender<()>,
    go: mpsc::Receiver<()>,
}

impl Interval for Driven {
    fn wait(&mut self) -> bool {
        self.done.send(()).is_ok() && self.go.recv().is_ok()
    }
}

/// Control (M2 amendment item 1): a non-holder that stops retrying after the row has been off and
/// then on again. B's tick driver runs on through A's hold, the row's off and on, and takes the
/// lock once A has exited.
#[test]
fn a_non_holder_keeps_retrying_through_off_and_on() {
    let (scratch, a, _, b, _) = two_instances("retry");
    a.set_enabled(true).unwrap();
    let (done_tx, done) = mpsc::channel();
    let (go, go_rx) = mpsc::channel();
    let driver_b = b.clone();
    let driver = thread::spawn(move || run_ticks(&driver_b, &mut Driven { done: done_tx, go: go_rx }));
    let step = || {
        go.send(()).unwrap();
        done.recv_timeout(SOON).expect("the driver ticked")
    };
    done.recv_timeout(SOON).unwrap(); // the first step, at start
    assert_eq!(b.status().reason.as_deref(), Some(ANOTHER_INSTANCE));

    let other = scratch.another_connection();
    set_row(&other, false);
    step();
    set_row(&other, true);
    step();
    assert!(!b.status().serving);

    // A exits: its lock is released (no-op app steps).
    let exited = Arc::new(AtomicBool::new(false));
    let flag = exited.clone();
    let steps = ExitSteps {
        terminate: Box::new(|| {}),
        sidecar_stop: Box::new(|| {}),
        exit: Box::new(move || flag.store(true, Ordering::SeqCst)),
    };
    assert_eq!(a.on_exit_requested(&BlockingPool, steps), ExitDecision::Started);
    assert!(eventually(SOON, || exited.load(Ordering::SeqCst)));
    // A's exit switched A off, but the row is the shared switch and stays "on".
    assert!(read_enabled_conn(&other).unwrap());

    step();
    assert!(b.status().serving, "B took the lock on the next tick: {:?}", b.status());
    drop(go);
    driver.join().unwrap();
}

/// The status is read without the lifecycle mutex: it answers while a transition holds it.
#[test]
fn the_status_does_not_wait_for_the_lifecycle_mutex() {
    let scratch = Scratch::new("status-leaf");
    let (inside_tx, inside) = mpsc::channel();
    let (release_tx, release) = mpsc::channel::<()>();
    let release = Arc::new(Mutex::new(release));
    let hooks = Hooks {
        after_decide: Some(Arc::new(move |_| {
            let _ = inside_tx.send(());
            let _ = release.lock().unwrap().recv_timeout(SOON);
        })),
        ..Hooks::default()
    };
    let (lc, _) = lifecycle(&scratch.paths(), scratch.db(), hooks);
    let busy = lc.clone();
    let transition = thread::spawn(move || busy.tick_step());
    inside.recv_timeout(SOON).unwrap(); // the transition holds the mutex now
    let (answered_tx, answered) = mpsc::channel();
    let reader = lc.clone();
    thread::spawn(move || answered_tx.send(reader.status()).unwrap());
    assert!(answered.recv_timeout(Duration::from_secs(1)).is_ok(), "status waited for the lifecycle mutex");
    drop(release_tx);
    transition.join().unwrap();
}

// ----- exit -----

/// Runs the flow on a new thread and records which.
#[derive(Default)]
struct NewThread(Mutex<Option<ThreadId>>);

impl Executor for NewThread {
    fn execute(&self, job: Box<dyn FnOnce() + Send>) {
        let handle = thread::spawn(job);
        *self.0.lock().unwrap() = Some(handle.thread().id());
    }
}

/// What the exit steps saw.
#[derive(Default)]
struct Seen {
    order: Mutex<Vec<&'static str>>,
    task_ended_at_terminate: AtomicBool,
    socket_gone_at_terminate: AtomicBool,
    lock_free_at_exit: AtomicBool,
}

fn recording_steps(seen: &Arc<Seen>, task_ended: Arc<AtomicBool>, paths: &super::paths::McpPaths) -> ExitSteps {
    let (s1, s2, s3) = (seen.clone(), seen.clone(), seen.clone());
    let (socket, lock) = (paths.socket.clone(), paths.lock.clone());
    ExitSteps {
        terminate: Box::new(move || {
            s1.task_ended_at_terminate.store(task_ended.load(Ordering::SeqCst), Ordering::SeqCst);
            s1.socket_gone_at_terminate.store(!socket.exists(), Ordering::SeqCst);
            s1.order.lock().unwrap().push("terminate");
        }),
        sidecar_stop: Box::new(move || s2.order.lock().unwrap().push("sidecar_stop")),
        exit: Box::new(move || {
            s3.lock_free_at_exit.store(matches!(try_acquire(&lock), Ok(Acquire::Acquired(_))), Ordering::SeqCst);
            s3.order.lock().unwrap().push("exit");
        }),
    }
}

/// Controls (M2 amendment item 1): `terminate` running before the MCP wait has ended, and the wait
/// or the lifecycle mutex taken on the caller's thread. Also: the lock released only after the
/// stop is complete, and the second request absorbed until `exit_ready`.
#[test]
fn exit_waits_for_the_stop_off_the_callers_thread_then_terminates_then_releases() {
    let scratch = Scratch::new("exit-order");
    let paths = scratch.paths();
    let lock_threads = Arc::new(Mutex::new(Vec::<ThreadId>::new()));
    let log = lock_threads.clone();
    let hooks = Hooks {
        on_lifecycle_lock: Some(Arc::new(move || log.lock().unwrap().push(thread::current().id()))),
        ..Hooks::default()
    };
    let (lc, _) = lifecycle(&paths, scratch.db(), hooks);
    lc.set_enabled(true).unwrap();
    let (release, ended) = blocked_task(&lc);
    // Released from elsewhere, so even a flow wrongly run inline here would finish.
    thread::spawn(move || {
        thread::sleep(Duration::from_millis(400));
        drop(release);
    });

    let caller = thread::current().id();
    lock_threads.lock().unwrap().clear();
    let seen = Arc::new(Seen::default());
    let executor = NewThread::default();
    let decision = lc.on_exit_requested(&executor, recording_steps(&seen, ended.clone(), &paths));
    assert_eq!(decision, ExitDecision::Started);
    assert!(seen.order.lock().unwrap().is_empty(), "nothing ran before the tracked task ended");

    let second = lc.on_exit_requested(&executor, recording_steps(&seen, ended.clone(), &paths));
    assert_eq!(second, ExitDecision::Absorbed, "a second request during the flow passes through");

    assert!(eventually(SOON, || seen.order.lock().unwrap().len() == 3));
    assert_eq!(*seen.order.lock().unwrap(), vec!["terminate", "sidecar_stop", "exit"]);
    assert!(seen.task_ended_at_terminate.load(Ordering::SeqCst), "terminate ran before the MCP wait ended");
    assert!(seen.socket_gone_at_terminate.load(Ordering::SeqCst));
    assert!(seen.lock_free_at_exit.load(Ordering::SeqCst), "the lock is released before exit");
    assert!(!lock_threads.lock().unwrap().contains(&caller), "the caller's thread took the lifecycle mutex");
    assert_ne!(executor.0.lock().unwrap().unwrap(), caller);

    let after = lc.on_exit_requested(&executor, recording_steps(&seen, ended, &paths));
    assert_eq!(after, ExitDecision::PassThrough, "once exit_ready is set, the exit passes through");
}

/// Controls (M2 amendment item 1): exit releasing the lock while a stop's unlink is still pending,
/// for a stop that began before exit; and a task that stop waits for still running.
#[test]
fn exit_waits_for_a_stop_that_began_before_it_including_its_unlink() {
    let scratch = Scratch::new("exit-prior-stop");
    let paths = scratch.paths();
    let hooks = Hooks {
        before_unlink: Some(Arc::new(|| thread::sleep(Duration::from_millis(300)))),
        ..Hooks::default()
    };
    let (lc, _) = lifecycle(&paths, scratch.db(), hooks);
    lc.set_enabled(true).unwrap();
    let (release, ended) = blocked_task(&lc);
    lc.set_enabled(false).unwrap(); // the stop begins before exit, held by the task

    let seen = Arc::new(Seen::default());
    assert_eq!(lc.on_exit_requested(&NewThread::default(), recording_steps(&seen, ended.clone(), &paths)), ExitDecision::Started);
    thread::sleep(Duration::from_millis(100));
    drop(release);
    assert!(eventually(SOON, || seen.order.lock().unwrap().len() == 3));
    assert!(seen.task_ended_at_terminate.load(Ordering::SeqCst));
    assert!(seen.socket_gone_at_terminate.load(Ordering::SeqCst), "the flow went on while the unlink was pending");
}

/// Controls (M2 amendment item 1): a tick or `set_enabled(true)` after exit began binding or opening
/// the gate, and `set_enabled` writing the row during exit. Placed where no other guard makes them
/// no-ops: after stop complete and before the lock release (in `terminate`), and after the release
/// with the row "on" (in `exit`).
#[test]
fn after_exit_began_nothing_binds_opens_or_writes() {
    let scratch = Scratch::new("exit-noop");
    let paths = scratch.paths();
    let (lc, counts) = lifecycle(&paths, scratch.db(), Hooks::default());
    lc.set_enabled(true).unwrap();
    let probe = scratch.another_connection();

    #[derive(Default)]
    struct Probed {
        binds_before: AtomicUsize,
        binds_after: AtomicUsize,
        gate_opened: AtomicBool,
        row_after_write: AtomicBool,
        held_after_release: AtomicBool,
        exiting_reason: AtomicBool,
        exit_step_done: AtomicBool,
    }
    let probed = Arc::new(Probed::default());
    let (p1, p2, lc1, lc2, c1, c2) = (probed.clone(), probed.clone(), lc.clone(), lc.clone(), counts.clone(), counts.clone());
    let probe = Arc::new(Mutex::new(probe));
    let probe2 = probe.clone();
    let steps = ExitSteps {
        terminate: Box::new(move || {
            // After stop complete, before the lock release; the row reads "on".
            p1.binds_before.store(c1.binds(), Ordering::SeqCst);
            lc1.tick_step();
            let status = lc1.set_enabled(false).unwrap();
            p1.exiting_reason.store(status.reason.as_deref() == Some(EXITING), Ordering::SeqCst);
            p1.row_after_write.store(read_enabled_conn(&probe.lock().unwrap()).unwrap(), Ordering::SeqCst);
            lc1.set_enabled(true).unwrap();
            p1.gate_opened.store(lc1.gate().is_open(), Ordering::SeqCst);
        }),
        sidecar_stop: Box::new(|| {}),
        exit: Box::new(move || {
            // After the release, the row still "on": a non-holder's tick would take the lock.
            assert!(read_enabled_conn(&probe2.lock().unwrap()).unwrap());
            lc2.tick_step();
            lc2.set_enabled(true).unwrap();
            p2.held_after_release.store(lc2.status().holds_lock, Ordering::SeqCst);
            p2.binds_after.store(c2.binds(), Ordering::SeqCst);
            p2.exit_step_done.store(true, Ordering::SeqCst);
        }),
    };
    assert_eq!(lc.on_exit_requested(&NewThread::default(), steps), ExitDecision::Started);
    // `exit_ready` is set just before `exit` runs: wait for the step itself, not for the pass-through.
    assert!(eventually(SOON, || probed.exit_step_done.load(Ordering::SeqCst)));
    assert_eq!(lc.on_exit_requested(&NewThread::default(), noop_steps()), ExitDecision::PassThrough);

    assert_eq!(probed.binds_before.load(Ordering::SeqCst), 1);
    assert_eq!(probed.binds_after.load(Ordering::SeqCst), 1, "a transition after exit began bound a socket");
    assert!(!probed.gate_opened.load(Ordering::SeqCst), "a transition after exit began opened the gate");
    assert!(probed.row_after_write.load(Ordering::SeqCst), "set_enabled wrote the shared row during exit");
    assert!(probed.exiting_reason.load(Ordering::SeqCst));
    assert!(!probed.held_after_release.load(Ordering::SeqCst), "a tick after the release took the lock again");
    assert!(!paths.socket.exists());
}

fn noop_steps() -> ExitSteps {
    ExitSteps { terminate: Box::new(|| {}), sidecar_stop: Box::new(|| {}), exit: Box::new(|| {}) }
}
