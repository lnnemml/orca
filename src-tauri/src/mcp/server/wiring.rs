//! The transport wired into the app (ADR-027 M2a Part B; M2 amendment, items 2, 3 and 6). `lib.rs`
//! calls three things here: [`app_lifecycle`] and [`start_ticks`] in setup, and [`exit_requested`]
//! on `RunEvent::ExitRequested`. `main.rs` calls [`relay_main`] for `orcastudio --mcp`.
//!
//! **The write tools' seams over the app (M3a Part B)** are built here too ([`app_write_seams`]): the
//! real step B over [`AppSubmitWorld`] (`mcp::step_b`), the `job:status` emit ([`AppEmit`],
//! `local_backend::emit_status`) and the sidecar's SMILES call ([`AppSmiles`]). Each only looks up
//! the app's managed state at the moment of the call, with `try_state`: the tick thread may serve
//! before the rest of setup has run, and a state not managed yet is a refusal, never a panic.
//! `cancel_job`'s world (M3b Part B, [`AppCancelWorld`]) is built here as well: the app's poller
//! (`poller::app::with_poller`, after `poller_is_managed`) and `commands::jobs::dispatch_cancel`, the
//! same pair Anton's Cancel runs.
//!
//! Every piece that does not need an `AppHandle` is a plain function over injected parts, so the
//! tests drive the production environment ([`wired_lifecycle`]), the exit decision
//! ([`prevents_exit`]), the exit steps ([`guarded_steps`]) and the backstop ([`run_backstop`])
//! without a window.
//!
//! **The exit path.** The main thread only asks [`McpLifecycle::on_exit_requested`], which takes no
//! mutex and hands the flow to Tauri's blocking pool; `prevent_exit` is called exactly when the
//! decision is not `PassThrough`. The flow pauses the local queue, then runs the MCP wait,
//! `terminate_on_exit`, the sidecar stop, the lock release and `app.exit(0)`, in that order, off the
//! main thread. Two guards keep a broken step from leaving the app un-exitable:
//! - each step runs under `catch_unwind`, so a panic in one does not stop the ones after it;
//! - a backstop thread waits for the `exit` step. If the flow has not reached it within
//!   [`EXIT_FLOW_DEADLINE`], or the flow ended without reaching it, the backstop runs the queue
//!   pause, then the kill, then the sidecar stop itself (bounded by [`BACKSTOP_STEPS_DEADLINE`]) and ends the process. If
//!   `exit` was reached but the process is still alive [`EXIT_GRACE`] later, it ends it too. The end
//!   is `libc::_exit(1)` ([`hard_end`]), injected so the tests never call it.

use std::io;
use std::path::Path;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use tauri::{AppHandle, Manager};

use super::create_job_task::WriteSeams;
use super::m3b_tools::{CancelCall, CancelWorld};
use super::lifecycle::{BlockingPool, ExitDecision, ExitSteps, Executor, LifecycleEnv, McpLifecycle};
use super::paths::{resolve_for_this_user, McpPaths, PathRefusal};
use super::relay::{relay, RelayEnd};
use super::service::HostPaths;
use super::tick::{run_ticks, Every, TICK};
use super::DbSource;
use crate::commands::settings::DbState;
use crate::in_flight::InFlight;
use crate::local_backend::{self, JobRunner};
use crate::mcp::create_job::{StatusEmit, StepBOutcome};
use crate::mcp::resolve::{SmilesStructure, SmilesTo3d};
use crate::mcp::sidecar_smiles::SidecarSmiles;
use crate::mcp::step_b::{closing_reason, RealStepB, RemoteParts, SubmitWorld};
use crate::models::job::JobStatus;
use crate::poller::app::AppSink;
use crate::poller::plan::PollerMemory;
use crate::poller::remote_state::RemoteStates;
use crate::remote::ssh::SystemRunner;
use crate::sidecar::SidecarManager;

/// How long the exit flow may take before the backstop ends the process. In M2a the MCP wait is
/// immediate (no tracked tasks), `terminate_on_exit` takes at most about 14.5 s (2 s pgid wait, 10 s
/// drain, 2 s grace, the sweeps) and the sidecar stop about 0.3 s, so a flow that takes longer has hung.
pub const EXIT_FLOW_DEADLINE: Duration = Duration::from_secs(60);
/// How long the process may outlive the flow's `app.exit(0)` before the backstop ends it (decided by
/// Anton). The event loop normally ends the process at once.
pub const EXIT_GRACE: Duration = Duration::from_secs(10);
/// How long the backstop waits for its own queue pause, kill and sidecar stop before it ends the
/// process anyway. The steps' worst case is about 14.8 s (`terminate_on_exit`'s ≈ 14.5 s: 2 s pgid
/// wait, 10 s drain, 2 s grace, the sweeps; then the 0.3 s sidecar stop), so 20 s leaves a margin of
/// about 5 s.
pub const BACKSTOP_STEPS_DEADLINE: Duration = Duration::from_secs(20);

/// The backstop's three bounds; [`BackstopTimings::PRODUCTION`] in the app, short ones in tests.
#[derive(Debug, Clone, Copy)]
pub struct BackstopTimings {
    /// For the flow to reach `exit`.
    pub flow: Duration,
    /// For the process to end after `exit`.
    pub grace: Duration,
    /// For the backstop's own queue pause, kill and sidecar stop.
    pub steps: Duration,
}

impl BackstopTimings {
    pub const PRODUCTION: BackstopTimings =
        BackstopTimings { flow: EXIT_FLOW_DEADLINE, grace: EXIT_GRACE, steps: BACKSTOP_STEPS_DEADLINE };
}

// ----- setup -----

/// The managed `DbState`, reached through the app handle.
pub struct ManagedDb(pub AppHandle);

impl DbSource for ManagedDb {
    fn db(&self) -> &DbState {
        // `DbState` is managed before the lifecycle is built (`lib.rs` setup), and never removed.
        self.0.state::<DbState>().inner()
    }
}

/// The production lifecycle at `paths` and `host`: [`LifecycleEnv::production_at`], nothing else.
/// The app builds it through [`app_lifecycle`]; the wiring tests build it here under a scratch
/// directory.
pub fn wired_lifecycle(
    db: Arc<dyn DbSource>,
    paths: Result<McpPaths, PathRefusal>,
    host: HostPaths,
    write_seams: WriteSeams,
) -> Arc<McpLifecycle> {
    McpLifecycle::new(LifecycleEnv::production_at(db, paths, host, write_seams))
}

/// The app's lifecycle, over the managed `DbState`, this user's paths, the real `/proc/meminfo` and
/// `data_dir` (the one `JobRunner` is built over), and the write tools' seams over the app. A path
/// refusal or an unknown base dir does not fail setup: it is kept in the environment and becomes the
/// status reason once the switch is on.
pub fn app_lifecycle(app: &AppHandle, data_dir: &Path) -> Arc<McpLifecycle> {
    wired_lifecycle(
        Arc::new(ManagedDb(app.clone())),
        resolve_for_this_user(),
        HostPaths::production(data_dir),
        app_write_seams(app),
    )
}

// ----- the write tools' seams -----

/// Step B's refusal while a piece of the app's state is not managed yet (the tick thread can serve
/// before setup has managed everything).
pub const APP_NOT_READY: &str = "the app is still starting; not submitted";

/// The write tools' seams over the app: the real step B, the `job:status` emit, the SMILES call (`create_job`),
/// and [`AppCancelWorld`] (`cancel_job`).
pub fn app_write_seams(app: &AppHandle) -> WriteSeams {
    WriteSeams {
        smiles: Arc::new(AppSmiles(app.clone())),
        step_b: Arc::new(RealStepB(AppSubmitWorld(app.clone()))),
        emit: Arc::new(AppEmit(app.clone())),
        cancel: Arc::new(AppCancelWorld(app.clone())),
    }
}

/// `cancel_job`'s refusal while the poller's state is not managed yet.
pub const CANCEL_NOT_READY: &str = "the app is still starting; nothing was cancelled";

/// `cancel_job`'s world in the app: the poller `commands::jobs::cancel_job` uses
/// (`poller::app::with_poller`: the real runner, the app's `AppSink`, `JobRunner`'s data dir) and its
/// dispatch (`commands::jobs::dispatch_cancel`: `Backend::for_job(..).cancel`). `with_poller` panics
/// on a missing state, so the call first checks [`poller_is_managed`](crate::poller::app::poller_is_managed):
/// the MCP tick starts before setup manages the poller's state.
pub struct AppCancelWorld(pub AppHandle);

impl CancelWorld for AppCancelWorld {
    fn run(&self, call: CancelCall<'_>) -> Result<crate::mcp::cancel::CancelJobResult, crate::mcp::tools::ToolError> {
        let app = &self.0;
        if !crate::poller::app::poller_is_managed(app) {
            return Err(crate::mcp::tools::ToolError::refused(CANCEL_NOT_READY));
        }
        crate::poller::app::with_poller(app, |poller| {
            call(poller, &|job| crate::commands::jobs::dispatch_cancel(app, job))
        })
    }
}

/// The sidecar's `/smiles-to-3d`, over the sidecar's status at the moment of the call
/// ([`SidecarSmiles::from_status`]): a sidecar that is starting, restarted or down is seen as it is
/// now. `create_job` calls it inside its task, on the blocking pool, with no lock held.
pub struct AppSmiles(pub AppHandle);

impl SmilesTo3d for AppSmiles {
    fn smiles_to_3d(&self, smiles: &str) -> Result<SmilesStructure, String> {
        let sidecar = self.0.try_state::<Arc<SidecarManager>>().ok_or("the SMILES sidecar is not started yet")?;
        let status = sidecar.get_status().map_err(|e| format!("the SMILES sidecar's status could not be read: {e}"))?;
        SidecarSmiles::from_status(&status).smiles_to_3d(smiles)
    }
}

/// `job:status`, through the app's one emitter (`local_backend::emit_status`).
pub struct AppEmit(pub AppHandle);

impl StatusEmit for AppEmit {
    fn job_status(&self, job_id: &str, status: JobStatus) {
        local_backend::emit_status(&self.0, job_id, status);
    }
}

/// What the real step B reaches in the app: the lifecycle's exit flag and gate, the managed
/// `DbState`, the local queue, and the remote path's managed state with the real ssh/rsync runner.
pub struct AppSubmitWorld(pub AppHandle);

impl SubmitWorld for AppSubmitWorld {
    fn closing(&self) -> Option<&'static str> {
        match self.0.try_state::<Arc<McpLifecycle>>() {
            Some(lifecycle) => closing_reason(lifecycle.is_exiting(), lifecycle.gate().is_open()),
            None => Some(APP_NOT_READY),
        }
    }

    fn db(&self) -> Result<&DbState, &'static str> {
        self.0.try_state::<DbState>().map(|db| db.inner()).ok_or(APP_NOT_READY)
    }

    /// `local_backend::submit`'s tail: the emit, then `try_start_next` (once `JobRunner` is
    /// managed; until then setup's own `try_start_next` picks the queued job up).
    fn after_local_queue(&self, job_id: &str) {
        local_backend::emit_status(&self.0, job_id, JobStatus::Queued);
        if self.0.try_state::<JobRunner>().is_some() {
            local_backend::try_start_next(&self.0);
        }
    }

    /// The pieces `submit_to_server` reaches through `attempt_in_app` and `SshBackend::submit_attempt`.
    fn with_remote(&self, work: &mut dyn FnMut(&RemoteParts<'_>) -> StepBOutcome) -> Result<StepBOutcome, &'static str> {
        let app = &self.0;
        let (Some(db), Some(in_flight), Some(remote), Some(memory), Some(runner)) = (
            app.try_state::<DbState>(),
            app.try_state::<InFlight>(),
            app.try_state::<RemoteStates>(),
            app.try_state::<PollerMemory>(),
            app.try_state::<JobRunner>(),
        ) else {
            return Err(APP_NOT_READY);
        };
        let sink = AppSink(app.clone());
        let parts = RemoteParts {
            db: db.inner(),
            in_flight: in_flight.inner(),
            remote: remote.inner(),
            memory: memory.inner(),
            sink: &sink,
            data_dir: runner.data_dir(),
            runner: &SystemRunner,
        };
        Ok(work(&parts))
    }
}

/// Start the tick thread: one step at once (so a switch left on serves from app start), then one
/// every [`TICK`], until exit begins.
pub fn start_ticks(lifecycle: Arc<McpLifecycle>) -> io::Result<()> {
    thread::Builder::new().name("mcp-tick".into()).spawn(move || run_ticks(&lifecycle, &mut Every(TICK)))?;
    Ok(())
}

// ----- exit -----

/// `ExitRequested` calls `prevent_exit` exactly when this is true: whenever the decision is not
/// `PassThrough` (the flow has just started, or is running and absorbs the request).
pub fn prevents_exit(decision: ExitDecision) -> bool {
    decision != ExitDecision::PassThrough
}

/// Run `step`, and survive its panic: the message goes to stderr, and the flow goes on.
fn guarded(name: &str, step: impl FnOnce()) {
    if catch_unwind(AssertUnwindSafe(step)).is_err() {
        eprintln!("[OrcaStudio] exit: {name} panicked; the exit goes on");
    }
}

/// The exit steps, each guarded against a panic, and the receiver that hears when `exit` is called.
/// The receiver disconnects if the steps are dropped unrun (the flow died before `exit`).
pub fn guarded_steps(
    pause_queue: impl FnOnce() + Send + 'static,
    terminate: impl FnOnce() + Send + 'static,
    sidecar_stop: impl FnOnce() + Send + 'static,
    exit: impl FnOnce() + Send + 'static,
) -> (ExitSteps, Receiver<()>) {
    let (called, heard) = mpsc::channel();
    let steps = ExitSteps {
        pause_queue: Box::new(move || guarded("the queue pause", pause_queue)),
        terminate: Box::new(move || guarded("terminate_on_exit", terminate)),
        sidecar_stop: Box::new(move || guarded("the sidecar stop", sidecar_stop)),
        exit: Box::new(move || {
            let _ = called.send(());
            guarded("app.exit", exit);
        }),
    };
    (steps, heard)
}

/// Pause the local queue for good, so the killed job's freed slot starts nothing.
fn pause_queue(app: &AppHandle) {
    if let Some(runner) = app.try_state::<JobRunner>() {
        runner.pause_for_exit();
    }
}

/// Kill a running ORCA tree. Off the main thread now (the exit flow, or the backstop): it touches
/// only managed state, a `Mutex`, signals and `/proc` (see `local_backend::terminate_on_exit`).
fn terminate(app: &AppHandle) {
    local_backend::terminate_on_exit(app);
}

/// Stop the sidecar so uvicorn does not outlive the app.
fn stop_sidecar(app: &AppHandle) {
    if let Some(sidecar) = app.try_state::<Arc<SidecarManager>>() {
        let _ = sidecar.stop();
    }
}

/// The app's four exit steps.
fn app_exit_steps(app: &AppHandle) -> (ExitSteps, Receiver<()>) {
    let (p, a, b, c) = (app.clone(), app.clone(), app.clone(), app.clone());
    guarded_steps(move || pause_queue(&p), move || terminate(&a), move || stop_sidecar(&b), move || c.exit(0))
}

/// One `ExitRequested`, given the lifecycle: ask it, start the backstop when the flow has just
/// started, and say whether to prevent the exit.
pub fn handle_exit_request(
    lifecycle: &Arc<McpLifecycle>,
    executor: &dyn Executor,
    steps: ExitSteps,
    exit_heard: Receiver<()>,
    backstop: impl FnOnce(Receiver<()>),
) -> bool {
    let decision = lifecycle.on_exit_requested(executor, steps);
    if decision == ExitDecision::Started {
        backstop(exit_heard);
    }
    prevents_exit(decision)
}

/// `RunEvent::ExitRequested`, on the main thread. Returns whether the caller must call
/// `prevent_exit`. Takes no mutex and runs no step here.
pub fn exit_requested(app: &AppHandle) -> bool {
    let (steps, exit_heard) = app_exit_steps(app);
    let Some(lifecycle) = app.try_state::<Arc<McpLifecycle>>() else {
        // Unreachable once setup has run (setup manages the lifecycle, and a failed setup never
        // reaches the event loop). Kept honest anyway: the previous synchronous path, no MCP.
        (steps.pause_queue)();
        (steps.terminate)();
        (steps.sidecar_stop)();
        return false;
    };
    let backstop_app = app.clone();
    handle_exit_request(&lifecycle, &BlockingPool, steps, exit_heard, move |heard| {
        start_backstop(backstop_app, heard)
    })
}

/// Why the backstop ends the process. Every outcome ends it: a backstop thread that is still running
/// at all means the process outlived the point where it should have gone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backstop {
    /// The flow did not reach `exit` within its deadline.
    Hung,
    /// The flow ended without reaching `exit` (its steps were dropped unrun).
    Died,
    /// The flow reached `exit`, and the process was still alive a grace later.
    StillAlive,
}

impl Backstop {
    pub fn why(self) -> &'static str {
        match self {
            Backstop::Hung => "the exit flow did not reach app.exit in time",
            Backstop::Died => "the exit flow ended without reaching app.exit",
            Backstop::StillAlive => "the process outlived app.exit by the grace",
        }
    }
}

/// Wait up to `timings.flow` for the flow's `exit` step.
/// - If it comes, the flow has already run its queue pause, kill and sidecar stop: wait `timings.grace` for the
///   event loop to end the process, and report that it did not.
/// - If it does not come, run `steps` (the queue pause, then the kill, then the sidecar stop) on their own thread for at most
///   `timings.steps`, and report why.
///
/// It returns only when the process should be ended; the caller ends it.
pub fn run_backstop(exit_heard: &Receiver<()>, timings: BackstopTimings, steps: impl FnOnce() + Send + 'static) -> Backstop {
    let found = match exit_heard.recv_timeout(timings.flow) {
        Ok(()) => {
            thread::sleep(timings.grace);
            return Backstop::StillAlive;
        }
        Err(RecvTimeoutError::Timeout) => Backstop::Hung,
        Err(RecvTimeoutError::Disconnected) => Backstop::Died,
    };
    let (done, finished) = mpsc::channel();
    let spawned = thread::Builder::new().name("exit-backstop-steps".into()).spawn(move || {
        steps();
        let _ = done.send(());
    });
    if spawned.is_ok() {
        let _ = finished.recv_timeout(timings.steps);
    }
    found
}

/// The backstop's own steps, each guarded, in order: the queue pause (idempotent; in the `Died` arm
/// the flow's pause may never have run), the kill, the sidecar stop.
pub fn backstop_steps(
    pause_queue: impl FnOnce() + Send + 'static,
    terminate: impl FnOnce() + Send + 'static,
    sidecar_stop: impl FnOnce() + Send + 'static,
) -> impl FnOnce() + Send + 'static {
    move || {
        guarded("the queue pause", pause_queue);
        guarded("terminate_on_exit", terminate);
        guarded("the sidecar stop", sidecar_stop);
    }
}

/// The backstop: [`run_backstop`], then `end`. In the app `end` is [`hard_end`], which does not
/// return; the tests inject one that records.
pub fn backstop(
    exit_heard: Receiver<()>,
    timings: BackstopTimings,
    steps: impl FnOnce() + Send + 'static,
    end: impl FnOnce(Backstop),
) {
    let found = run_backstop(&exit_heard, timings, steps);
    end(found);
}

/// End the process at once: one stderr line, then `_exit(1)`.
pub fn hard_end(found: Backstop) {
    eprintln!("[OrcaStudio] exit: {}; ending the process", found.why());
    // SAFETY: `_exit` takes a plain status and never returns. It is chosen over the standard
    // library's exit on purpose: it runs no atexit handlers and no destructors, which could deadlock
    // against the GTK/WebKit loop or a lock held by the very thread that hung. The flow's or the
    // backstop's own queue pause, kill and sidecar stop have already run (bounded), and the OS reclaims the rest.
    unsafe { libc::_exit(1) }
}

/// The production backstop: a thread that ends the process if the flow does not reach `exit`, or if
/// the process outlives `exit` by [`EXIT_GRACE`].
fn start_backstop(app: AppHandle, exit_heard: Receiver<()>) {
    let spawned = thread::Builder::new().name("exit-backstop".into()).spawn(move || {
        let (p, a, b) = (app.clone(), app.clone(), app);
        let steps = backstop_steps(move || pause_queue(&p), move || terminate(&a), move || stop_sidecar(&b));
        backstop(exit_heard, BackstopTimings::PRODUCTION, steps, hard_end);
    });
    if let Err(e) = spawned {
        eprintln!("[OrcaStudio] exit: cannot start the exit backstop ({e}); the exit flow runs without it");
    }
}

// ----- the relay (`orcastudio --mcp`) -----

/// Whether the process was started as `orcastudio --mcp`: the first argument, compared as an
/// `OsStr` (a non-UTF-8 argument is simply not `--mcp`).
pub fn is_relay_invocation(first_arg: Option<&std::ffi::OsStr>) -> bool {
    first_arg == Some(std::ffi::OsStr::new("--mcp"))
}

/// `orcastudio --mcp`: relay the process's stdio to this user's socket and return the exit code.
/// Writes nothing to stdout but relayed bytes; a path refusal is one stderr line and code 2.
pub fn relay_main() -> i32 {
    match resolve_for_this_user() {
        Ok(paths) => i32::from(relay(io::stdin(), io::stdout(), io::stderr(), &paths.socket).code()),
        Err(refusal) => {
            eprintln!("orcastudio --mcp: {refusal}");
            i32::from(RelayEnd::NotRunning.code())
        }
    }
}
