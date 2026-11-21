//! The app's side of the transport (M2a Part B): the production lifecycle under a scratch dir, the
//! exit decision and steps, the backstop, and source pins on `lib.rs`/`main.rs`, which need an
//! `AppHandle` the tests do not have (as the M1b guard pins).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use super::lifecycle::{BlockingPool, ExitDecision, Executor};
use super::test_support::{eventually, no_create_job, set_row, Scratch};
use super::wiring::*;
use crate::mcp::grant::create_grant_conn;
use crate::mcp::steps::{reconcile_submitting_conn, Reconciled, SUBMITTING};
use crate::mcp::test_db::{new_grant, NOW};

const SOON: Duration = Duration::from_secs(3);

/// What the exit steps did, in order.
#[derive(Default, Clone)]
struct Order(Arc<Mutex<Vec<&'static str>>>);

impl Order {
    fn push(&self, step: &'static str) {
        self.0.lock().unwrap().push(step);
    }
    fn get(&self) -> Vec<&'static str> {
        self.0.lock().unwrap().clone()
    }
}

/// The four steps through [`guarded_steps`], each recording itself.
fn recording(order: &Order) -> (super::lifecycle::ExitSteps, mpsc::Receiver<()>) {
    let (p, a, b, c) = (order.clone(), order.clone(), order.clone(), order.clone());
    guarded_steps(
        move || p.push("pause_queue"),
        move || a.push("terminate"),
        move || b.push("sidecar_stop"),
        move || c.push("exit"),
    )
}

/// Short backstop bounds for the tests.
fn timings(flow: Duration, grace: Duration, steps: Duration) -> BackstopTimings {
    BackstopTimings { flow, grace, steps }
}

// ----- the production environment -----

/// Control (Part B): the **wired** lifecycle keeps "a non-holder reconciling" red. Two instances are
/// built as the app builds them (`LifecycleEnv::production_at`: the real reconciliation, uncounted,
/// no hooks) on one database and one lock path. A serves; a job then enters step B (`submitting`).
/// B reads "on" but cannot take the lock: it must not reconcile, which would refund that job under
/// A's feet.
#[test]
fn the_wired_lifecycle_keeps_a_non_holder_from_reconciling() {
    let scratch = Scratch::new("wired-nonholder");
    let paths = scratch.paths();
    let a = wired_lifecycle(scratch.db(), Ok(paths.clone()), scratch.host(), no_create_job());
    let b = wired_lifecycle(scratch.db(), Ok(paths.clone()), scratch.host(), no_create_job());
    assert!(a.set_enabled(true).unwrap().serving);

    // A's job in step B, as step A leaves it: a charged `submitting` draft.
    let other = scratch.another_connection();
    let grant = create_grant_conn(&other, &new_grant(5, 5), NOW).unwrap();
    other
        .execute(
            "INSERT INTO jobs (id, title, input_content, status, origin, grant_id) VALUES ('b', 't', '! SP', 'draft', 'mcp', ?1)",
            [&grant.id],
        )
        .unwrap();
    other.execute("UPDATE jobs SET mcp_state = ?1 WHERE id = 'b'", [SUBMITTING]).unwrap();
    other.execute("UPDATE mcp_grants SET charged_jobs = 1 WHERE id = ?1", [&grant.id]).unwrap();

    b.tick_step();
    let status = b.status();
    assert!(!status.holds_lock && !status.serving, "{status:?}");
    let state: Option<String> = other.query_row("SELECT mcp_state FROM jobs WHERE id = 'b'", [], |r| r.get(0)).unwrap();
    assert_eq!(state.as_deref(), Some(SUBMITTING), "a non-holder reconciled: A's job in step B was refunded");

    // The seed is one a reconciliation does change, so the assertion above can fail.
    let reconciled = reconcile_submitting_conn(&other).unwrap();
    assert_eq!(reconciled, Reconciled { refunded: 1, cleared: 0 }, "the seed must be reconcilable");
}

/// A path refusal does not fail anything: it is kept, and shown once the switch is on.
#[test]
fn a_path_refusal_becomes_the_status_reason() {
    let scratch = Scratch::new("wired-refusal");
    let lc = wired_lifecycle(scratch.db(), Err(super::paths::PathRefusal::NoBaseDir), scratch.host(), no_create_job());
    set_row(&scratch.another_connection(), true);
    lc.tick_step();
    let status = lc.status();
    assert!(!status.serving && !status.holds_lock, "{status:?}");
    assert_eq!(status.socket_path, None);
    assert!(status.reason.unwrap_or_default().contains("neither XDG_RUNTIME_DIR"));
}

// ----- exit -----

/// Control (Part B): `ExitRequested` calls `prevent_exit` exactly when the decision is not
/// `PassThrough`: both arms (and both non-pass-through decisions).
#[test]
fn prevent_exit_exactly_when_the_decision_is_not_pass_through() {
    assert!(!prevents_exit(ExitDecision::PassThrough), "the flow's own app.exit must pass through");
    assert!(prevents_exit(ExitDecision::Started), "the request that starts the flow must be prevented");
    assert!(prevents_exit(ExitDecision::Absorbed), "a request during the flow must be prevented");
}

/// Control (Part B): exit **with MCP never switched on** (no start, no lock) still runs the kill,
/// then the sidecar stop, then `exit`, promptly, through the production lifecycle and Tauri's
/// blocking pool; the flow's own exit then passes through. Nothing of MCP is created on disk.
#[test]
fn exit_with_mcp_never_enabled_kills_then_stops_the_sidecar_then_exits_promptly() {
    let scratch = Scratch::new("wired-exit-off");
    let paths = scratch.paths();
    let lc = wired_lifecycle(scratch.db(), Ok(paths.clone()), scratch.host(), no_create_job());
    lc.tick_step(); // the first tick at app start: the row is missing, so "off"

    let order = Order::default();
    let (steps, heard) = recording(&order);
    let backstop_started = Arc::new(AtomicBool::new(false));
    let flag = backstop_started.clone();
    let (heard_tx, heard_rx) = mpsc::channel();
    let started = Instant::now();
    let prevent = handle_exit_request(&lc, &BlockingPool, steps, heard, move |h| {
        flag.store(true, Ordering::SeqCst);
        heard_tx.send(h).unwrap();
    });
    assert!(prevent, "the request that starts the flow is prevented");
    assert!(backstop_started.load(Ordering::SeqCst), "the backstop starts with the flow");

    assert!(eventually(SOON, || order.get().len() == 4), "the flow did not finish: {:?}", order.get());
    assert!(started.elapsed() < Duration::from_secs(1), "exit with MCP off took {:?}", started.elapsed());
    assert_eq!(order.get(), vec!["pause_queue", "terminate", "sidecar_stop", "exit"]);
    let heard = heard_rx.recv().unwrap();
    assert!(heard.recv_timeout(SOON).is_ok(), "the backstop did not hear exit");

    let (again, again_heard) = recording(&order);
    let prevent = handle_exit_request(&lc, &BlockingPool, again, again_heard, |_| panic!("no second backstop"));
    assert!(!prevent, "the flow's own app.exit passes through");
    assert_eq!(order.get().len(), 4, "a step ran twice");
    assert!(!paths.dir.exists(), "exit with MCP off created {}", paths.dir.display());
}

/// A request during the flow is absorbed: prevented, and it starts nothing.
#[test]
fn a_second_request_during_the_flow_is_prevented_and_starts_nothing() {
    let scratch = Scratch::new("wired-exit-twice");
    let lc = wired_lifecycle(scratch.db(), Ok(scratch.paths()), scratch.host(), no_create_job());
    let order = Order::default();
    let (release, hold) = mpsc::channel::<()>();
    let o = order.clone();
    let (steps, heard) = guarded_steps(
        || {},
        move || {
            let _ = hold.recv();
            o.push("terminate");
        },
        || {},
        || {},
    );
    assert!(handle_exit_request(&lc, &BlockingPool, steps, heard, |_| {}));
    let (again, again_heard) = recording(&order);
    assert!(handle_exit_request(&lc, &BlockingPool, again, again_heard, |_| panic!("no second backstop")));
    drop(release);
    assert!(eventually(SOON, || order.get() == vec!["terminate"]));
}

/// Control (Part B): a panic in a step does not stop the exit: the steps after it run, and `exit`
/// is called.
#[test]
fn a_panicking_step_does_not_stop_the_exit() {
    let scratch = Scratch::new("wired-exit-panic");
    let lc = wired_lifecycle(scratch.db(), Ok(scratch.paths()), scratch.host(), no_create_job());
    let order = Order::default();
    let (b, c) = (order.clone(), order.clone());
    let (steps, heard) = guarded_steps(
        || {},
        || panic!("control: terminate_on_exit panics"),
        move || b.push("sidecar_stop"),
        move || c.push("exit"),
    );
    assert_eq!(lc.on_exit_requested(&BlockingPool, steps), ExitDecision::Started);
    assert!(eventually(SOON, || order.get().len() == 2), "a panicking step stopped the exit: {:?}", order.get());
    assert_eq!(order.get(), vec!["sidecar_stop", "exit"]);
    assert!(heard.recv_timeout(SOON).is_ok(), "the backstop did not hear exit");
}

/// Control (Part B): a step that hangs cannot leave the app un-exitable. The flow never reaches
/// `exit`; the backstop finds it hung after its deadline, runs its own kill, and the caller ends the
/// process (here: records it).
#[test]
fn a_hung_step_is_caught_by_the_backstop() {
    let scratch = Scratch::new("wired-exit-hang");
    let lc = wired_lifecycle(scratch.db(), Ok(scratch.paths()), scratch.host(), no_create_job());
    let (release, hold) = mpsc::channel::<()>();
    let (steps, heard) = guarded_steps(
        || {},
        move || {
            let _ = hold.recv(); // hangs until the test ends
        },
        || {},
        || {},
    );
    let found = Arc::new(Mutex::new(None));
    let backstop_ran_steps = Arc::new(AtomicBool::new(false));
    let (f, ran) = (found.clone(), backstop_ran_steps.clone());
    let started = Instant::now();
    let prevent = handle_exit_request(&lc, &BlockingPool, steps, heard, move |h| {
        thread::spawn(move || {
            let t = timings(Duration::from_millis(300), SOON, Duration::from_millis(300));
            backstop(h, t, move || ran.store(true, Ordering::SeqCst), move |r| *f.lock().unwrap() = Some(r));
        });
    });
    assert!(prevent);
    assert!(eventually(SOON, || found.lock().unwrap().is_some()), "the backstop never fired");
    assert_eq!(*found.lock().unwrap(), Some(Backstop::Hung));
    assert!(backstop_ran_steps.load(Ordering::SeqCst), "the backstop did not run its kill");
    assert!(started.elapsed() >= Duration::from_millis(300));
    drop(release);
}

/// Control (Part B fix round, decided by Anton): after `exit` the backstop does not stand down. A
/// fake `exit` that returns without ending the process (the flow's real steps, through the
/// production lifecycle) → the backstop ends the process after the grace, and not before it.
#[test]
fn a_process_that_outlives_exit_is_ended_after_the_grace() {
    let scratch = Scratch::new("wired-exit-grace");
    let lc = wired_lifecycle(scratch.db(), Ok(scratch.paths()), scratch.host(), no_create_job());
    let order = Order::default();
    let (steps, heard) = recording(&order); // its `exit` only records: the process lives on
    let ended = Arc::new(Mutex::new(None::<(Backstop, Duration)>));
    let backstop_ran_steps = Arc::new(AtomicBool::new(false));
    let (e, ran) = (ended.clone(), backstop_ran_steps.clone());
    let grace = Duration::from_millis(400);
    let started = Instant::now();
    assert!(handle_exit_request(&lc, &BlockingPool, steps, heard, move |h| {
        thread::spawn(move || {
            let t = timings(SOON, grace, SOON);
            backstop(h, t, move || ran.store(true, Ordering::SeqCst), move |r| {
                *e.lock().unwrap() = Some((r, started.elapsed()))
            });
        });
    }));
    assert!(eventually(SOON, || order.get().len() == 4), "{:?}", order.get());
    assert!(ended.lock().unwrap().is_none(), "the backstop ended the process before the grace");
    assert!(eventually(SOON, || ended.lock().unwrap().is_some()), "the process outlived exit and the backstop stood down");
    let (found, at) = ended.lock().unwrap().unwrap();
    assert_eq!(found, Backstop::StillAlive);
    assert!(at >= grace, "ended after {at:?}, before the {grace:?} grace");
    assert!(!backstop_ran_steps.load(Ordering::SeqCst), "the flow already ran the kill and the sidecar stop");
}

/// The backstop's other outcomes, and its own steps bounded.
#[test]
fn the_backstop_fires_on_a_hang_or_a_dead_flow() {
    // the flow keeps its sender but never calls exit: hung, after the deadline
    let (_keep, rx) = mpsc::channel::<()>();
    let started = Instant::now();
    assert_eq!(run_backstop(&rx, timings(Duration::from_millis(200), SOON, SOON), || {}), Backstop::Hung);
    assert!(started.elapsed() >= Duration::from_millis(200));

    // the flow died (its steps dropped unrun): at once, without waiting for the deadline
    let (tx, rx) = mpsc::channel::<()>();
    drop(tx);
    let started = Instant::now();
    assert_eq!(run_backstop(&rx, timings(Duration::from_secs(30), SOON, SOON), || {}), Backstop::Died);
    assert!(started.elapsed() < Duration::from_secs(5), "a dead flow waited for the deadline");

    // the backstop's own steps hang: it still returns after their deadline
    let (_keep, rx) = mpsc::channel::<()>();
    let (_never, block) = mpsc::channel::<()>();
    let started = Instant::now();
    let r = run_backstop(&rx, timings(Duration::from_millis(50), SOON, Duration::from_millis(200)), move || {
        let _ = block.recv();
    });
    assert_eq!(r, Backstop::Hung);
    assert!(started.elapsed() < Duration::from_secs(5));
}

/// Control (Part B, second fix round): when the flow died before its steps ran, the backstop's own
/// steps start with the queue pause, then the kill, then the sidecar stop. Removing the pause → red.
#[test]
fn a_dead_flows_backstop_pauses_the_queue_first() {
    let order = Order::default();
    let (p, a, b) = (order.clone(), order.clone(), order.clone());
    let steps = backstop_steps(move || p.push("pause_queue"), move || a.push("terminate"), move || b.push("sidecar_stop"));
    let (tx, rx) = mpsc::channel::<()>();
    drop(tx); // the flow died: its own pause never ran
    let ended = Arc::new(Mutex::new(None));
    let e = ended.clone();
    backstop(rx, timings(SOON, SOON, SOON), steps, move |r| *e.lock().unwrap() = Some(r));
    assert_eq!(*ended.lock().unwrap(), Some(Backstop::Died));
    assert_eq!(order.get(), vec!["pause_queue", "terminate", "sidecar_stop"], "the backstop did not pause the queue first");
}

/// The production bounds are the decided ones: 60 s to reach `exit`, 10 s of grace after it, 20 s for the backstop's own steps.
#[test]
fn the_production_backstop_bounds_are_the_decided_ones() {
    let t = BackstopTimings::PRODUCTION;
    assert_eq!((t.flow, t.grace, t.steps), (Duration::from_secs(60), Duration::from_secs(10), Duration::from_secs(20)));
}

// ----- the local queue on exit -----

/// Control (Part B fix round, decided by Anton): once the exit flow has begun, no queued local job
/// is started. The killed job's drive thread frees its slot and calls `try_start_next`; that call
/// is played here, inside `terminate`, through the queue's real claim (`JobRunner::claim_next`)
/// against a database holding a queued job. MCP was never switched on. Removing the pause → the job
/// is claimed → red.
#[test]
fn no_queued_job_starts_once_the_exit_flow_has_begun() {
    use crate::local_backend::JobRunner;

    let scratch = Scratch::new("wired-exit-queue");
    let db = scratch.db();
    db.lock()
        .unwrap()
        .execute("INSERT INTO jobs (id, title, input_content, status) VALUES ('next', 't', '! SP', 'queued')", [])
        .unwrap();
    let lc = wired_lifecycle(db.clone(), Ok(scratch.paths()), scratch.host(), no_create_job());
    let runner = Arc::new(JobRunner::new(scratch.dir.clone()));

    let claimed = Arc::new(Mutex::new(None));
    let (r1, r2, d, c) = (runner.clone(), runner.clone(), db.clone(), claimed.clone());
    let (steps, heard) = guarded_steps(
        move || r1.pause_for_exit(),
        move || *c.lock().unwrap() = Some(r2.claim_next(&d).map(|(id, _)| id)),
        || {},
        || {},
    );
    assert!(handle_exit_request(&lc, &BlockingPool, steps, heard, |_| {}));
    assert!(eventually(SOON, || claimed.lock().unwrap().is_some()), "terminate never ran");
    assert_eq!(*claimed.lock().unwrap(), Some(None), "a queued job was started after the exit flow began");

    // The seed is a job the queue does start when nothing paused it.
    let fresh = JobRunner::new(scratch.dir.clone());
    assert_eq!(fresh.claim_next(&db).map(|(id, _)| id).as_deref(), Some("next"), "the seed must be claimable");
}

/// The production executor runs the flow off the caller's thread (the main thread in the app).
#[test]
fn the_blocking_pool_runs_the_flow_off_the_callers_thread() {
    let (tx, rx) = mpsc::channel();
    BlockingPool.execute(Box::new(move || tx.send(thread::current().id()).unwrap()));
    assert_ne!(rx.recv_timeout(SOON).unwrap(), thread::current().id());
}

// ----- the relay invocation -----

#[test]
fn only_a_first_argument_of_exactly_dash_dash_mcp_is_the_relay() {
    use std::ffi::OsStr;
    assert!(is_relay_invocation(Some(OsStr::new("--mcp"))));
    assert!(!is_relay_invocation(None));
    assert!(!is_relay_invocation(Some(OsStr::new("--mcp=1"))));
    assert!(!is_relay_invocation(Some(OsStr::new("mcp"))));
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        assert!(!is_relay_invocation(Some(OsStr::from_bytes(b"--mcp\xff"))), "a non-UTF-8 argument is not --mcp");
    }
}

// ----- source pins (no AppHandle harness exists) -----

const LIB: &str = include_str!("../../lib.rs");
const MAIN: &str = include_str!("../../main.rs");
const WIRING: &str = include_str!("wiring.rs");

/// The `app.run(...)` closure of `lib.rs`.
fn run_closure() -> &'static str {
    let start = LIB.find("app.run(|app_handle, event|").expect("lib.rs has the app.run closure");
    &LIB[start..start + LIB[start..].find("\n    });\n").expect("the closure ends")]
}

/// Control (Part B): the old synchronous `terminate_on_exit` or `sidecar.stop()` left in
/// `ExitRequested`, which would run it on the main thread, before the MCP wait, and twice. The arm is
/// the hand-off and `prevent_exit` only; the steps live in `wiring::app_exit_steps`, once each, in
/// order.
#[test]
fn exit_requested_is_the_hand_off_and_prevent_exit_only() {
    // The code of the closure, without its comments.
    let arm: String = run_closure().lines().filter(|l| !l.trim_start().starts_with("//")).collect::<Vec<_>>().join("\n");
    assert_eq!(arm.matches("ExitRequested").count(), 1, "{arm}");
    assert_eq!(arm.matches("mcp::server::wiring::exit_requested(app_handle)").count(), 1, "{arm}");
    assert_eq!(arm.matches("api.prevent_exit()").count(), 1, "{arm}");
    for old in ["terminate_on_exit(", "SidecarManager", ".stop(", "sidecar", ".exit("] {
        assert!(!arm.contains(old), "ExitRequested still calls `{old}` itself:\n{arm}");
    }

    let steps = &WIRING[WIRING.find("fn app_exit_steps").unwrap()..];
    let steps = &steps[..steps.find("\n}\n").unwrap()];
    assert!(
        steps.contains(
            "guarded_steps(move || pause_queue(&p), move || terminate(&a), move || stop_sidecar(&b), move || c.exit(0))"
        ),
        "{steps}"
    );
    let terminate = &WIRING[WIRING.find("fn terminate(app: &AppHandle)").unwrap()..];
    assert!(terminate[..terminate.find("\n}\n").unwrap()].contains("local_backend::terminate_on_exit(app)"));
    let pause = &WIRING[WIRING.find("fn pause_queue(app: &AppHandle)").unwrap()..];
    assert!(pause[..pause.find("\n}\n").unwrap()].contains("runner.pause_for_exit()"));
    let start = &WIRING[WIRING.find("fn start_backstop(").unwrap()..];
    let start = &start[..start.find("\n}\n").unwrap()];
    assert!(
        start.contains("backstop(exit_heard, BackstopTimings::PRODUCTION, steps, hard_end)"),
        "the production backstop must end with hard_end"
    );
    assert!(
        start.contains("backstop_steps(move || pause_queue(&p), move || terminate(&a), move || stop_sidecar(&b))"),
        "the production backstop's steps must pause the queue first:\n{start}"
    );
    let end = &WIRING[WIRING.find("pub fn hard_end(").unwrap()..];
    let end = &end[..end.find("\n}\n").unwrap()];
    assert!(end.contains("libc::_exit(1)") && !end.contains("process::exit"), "{end}");
}

/// Setup builds the lifecycle over the managed `DbState` (after it is managed), manages it, and
/// starts the tick thread; both commands are registered.
#[test]
fn setup_wires_the_lifecycle_and_the_tick_and_registers_the_commands() {
    let db = LIB.find("app.manage(DbState(Mutex::new(conn)));").expect("DbState is managed");
    let build = LIB.find("mcp::server::wiring::app_lifecycle(app.handle(), &data_dir)").expect("the lifecycle is built");
    let manage = LIB.find("app.manage(mcp_lifecycle.clone());").expect("the lifecycle is managed");
    let tick = LIB.find("mcp::server::wiring::start_ticks(mcp_lifecycle)?;").expect("the tick thread starts");
    assert!(db < build && build < manage && manage < tick);
    for command in ["commands::mcp::mcp_status,", "commands::mcp::mcp_set_enabled,"] {
        assert_eq!(LIB.matches(command).count(), 1, "{command} registered once");
    }
}

/// `main.rs` decides the relay before `run()` and prints nothing itself. (The built binary is run
/// for real by `tests/mcp_relay_bin.rs`.)
#[test]
fn main_branches_to_the_relay_before_run_and_prints_nothing() {
    let branch = MAIN.find("orcastudio_lib::is_mcp_relay(").expect("the relay branch");
    let run = MAIN.find("orcastudio_lib::run()").expect("run()");
    assert!(branch < run, "the --mcp branch must come before run()");
    for print in ["print!", "println!", "io::stdout", "write!", "writeln!"] {
        assert!(!MAIN.contains(print), "main.rs writes to stdout: {print}");
    }
}

/// The text of `WIRING` from `signature` up to the first `end` after it (the item's closing brace).
fn wiring_fn(signature: &str, end: &str) -> &'static str {
    let start = WIRING.find(signature).unwrap_or_else(|| panic!("{signature} exists"));
    &WIRING[start..start + WIRING[start..].find(end).expect("the fn ends")]
}

/// `create_job`'s app seams (M3a Part B) need an `AppHandle`, so — like the exit steps — their bodies
/// are pinned in the source: the app's lifecycle gets them; step B is the real one over the app's
/// world, whose closing check reads the lifecycle's exit flag and its gate, whose remote parts are
/// `submit_to_server`'s (the real runner, the app's sink, `JobRunner`'s data dir), and whose local
/// tail is `local_backend::submit`'s; the emit is the app's one emitter; the SMILES call reads the
/// sidecar's status at the moment of the call. NEGATIVE CONTROL: the closing check not reading the
/// exit flag (`APP-SEAMS`) → red.
#[test]
fn the_app_seams_of_create_job_are_the_real_ones() {
    let app = wiring_fn("pub fn app_lifecycle(", "\n}\n");
    assert!(app.contains("app_create_job(app)"), "{app}");
    let seams = wiring_fn("pub fn app_create_job(", "\n}\n");
    for part in ["Arc::new(AppSmiles(app.clone()))", "Arc::new(RealStepB(AppSubmitWorld(app.clone())))", "Arc::new(AppEmit(app.clone()))"] {
        assert!(seams.contains(part), "{part}: {seams}");
    }
    let closing = wiring_fn("    fn closing(&self)", "\n    }\n");
    assert!(
        closing.contains("closing_reason(lifecycle.is_exiting(), lifecycle.gate().is_open())"),
        "APP-SEAMS: the app's step B does not read the exit flag and the gate: {closing}"
    );
    let remote = wiring_fn("    fn with_remote(", "\n    }\n");
    for part in ["runner: &SystemRunner", "AppSink(app.clone())", "data_dir: runner.data_dir()", "app.try_state::<InFlight>()"] {
        assert!(remote.contains(part), "{part}: {remote}");
    }
    let tail = wiring_fn("    fn after_local_queue(", "\n    }\n");
    let emit = tail.find("local_backend::emit_status(&self.0, job_id, JobStatus::Queued)").expect("the emit");
    let start = tail.find("local_backend::try_start_next(&self.0)").expect("the queue start");
    assert!(emit < start, "{tail}");
    let emitter = wiring_fn("impl StatusEmit for AppEmit", "\n}\n");
    assert!(emitter.contains("local_backend::emit_status(&self.0, job_id, status)"), "{emitter}");
    let smiles = wiring_fn("impl SmilesTo3d for AppSmiles", "\n}\n");
    let status = smiles.find("sidecar.get_status()").expect("the status, read at the call");
    let call = smiles.find("SidecarSmiles::from_status(&status).smiles_to_3d(smiles)").expect("the call");
    assert!(status < call, "{smiles}");
}
