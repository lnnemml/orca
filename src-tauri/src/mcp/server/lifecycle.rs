//! The lifecycle owner (ADR-027 M2 amendment, items 2 and 6): switch-on, switch-off, the tick step,
//! `set_enabled`, the status and the exit flow.
//!
//! **One lifecycle mutex** (`transitions`) serialises every transition: a tick step,
//! `set_enabled` (its row write included) and the exit flow's switch-off. It is held for the
//! decision and the short synchronous steps (row read, lock, reconcile, bind, `stat`s, gate open or
//! close), **never across the wait for tasks**: a stop's wait and unlink run in a task on Tauri's
//! runtime, and the exit flow waits for that task with the mutex released.
//!
//! **Lock order:** lifecycle → gate, lifecycle → `DbState`, lifecycle → the status facts. Nothing
//! takes the lifecycle mutex while holding `DbState`, the gate or the facts. Handlers never take
//! it. [`McpLifecycle::status`] does not take it. The read ledger (`super::reads`) is a leaf held
//! alone: the accept loop enters a connection, its serve task folds it, the status snapshots it. **The main thread never takes it**: the only
//! entry the main thread uses is [`McpLifecycle::on_exit_requested`], which is one atomic
//! `compare_exchange` and a hand-off to the executor.
//!
//! **A transition** reads the row, asks the pure [`decide`] what to do, and does it, all under one
//! hold of the mutex:
//!
//! | observed | action |
//! |---|---|
//! | `exiting` | nothing |
//! | the row read failed | nothing (retried on the next tick) |
//! | "on", serving | nothing |
//! | "on", a stop still running | nothing (the next tick after it completes starts serving) |
//! | "on", holder | start: reconcile if this acquisition has not, then bind and serve |
//! | "on", not holder | try the lock; if taken, start as above |
//! | "off", serving | stop |
//! | "off", not serving | nothing (the `flock` is kept) |
//!
//! **Acquisition.** Taking the `flock` begins one. `reconcile_submitting_conn` runs once per
//! acquisition, before the first bind; the acquisition counts as done only when it commits. A
//! failure binds nothing, shows the reason, and the next tick retries it.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use rusqlite::Connection;
use serde::Serialize;
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;

use super::create_job_task::{WriteSeams, WriteWiring};
use super::gate::{self, AdmissionGate};
use super::listener::{accept_loop, kernel_peer_uid, own_uid, AcceptContext, PeerUid, Registry};
use super::lock::{self, Acquire, McpLock};
use super::paths::{resolve_for_this_user, McpPaths, PathRefusal};
use super::reads::{McpReads, ReadLedger};
use super::service::HostPaths;
use super::socket::{self, SocketIdentity, Unlinked};
use super::{DbSource, CONNECTIONS};
use crate::error::AppError;
use crate::mcp::create_job::CreateJobEnv;
use crate::mcp::steps::{reconcile_submitting_conn, Reconciled};
use crate::mcp::switch::{read_enabled_conn, write_enabled_conn};

/// The status reason while another instance holds the lock.
pub const ANOTHER_INSTANCE: &str = "another OrcaStudio instance holds the MCP lock";
/// The status reason once exit has begun.
pub const EXITING: &str = "exiting";
/// The status reason while the switch is off.
pub const OFF: &str = "MCP is off";
/// The status reason while the switch is on and a stop is still running.
pub const STOPPING: &str = "stopping; serving resumes on the next tick after the stop completes";
/// The status reason before the first transition.
pub const NOT_CHECKED: &str = "MCP has not been checked yet";

/// The crash reconciliation, injected so a test can count it or fail it.
pub type Reconcile = Box<dyn Fn(&Connection) -> Result<Reconciled, AppError> + Send + Sync>;
type Hook = Arc<dyn Fn() + Send + Sync>;

/// Test seams. Every field is `None` in production.
#[derive(Default, Clone)]
pub struct Hooks {
    /// After the `chmod`s, before the mode post-condition.
    pub after_bind: Option<Arc<dyn Fn(&McpPaths) + Send + Sync>>,
    /// In a stop, after the wait for tasks and before the unlink.
    pub before_unlink: Option<Hook>,
    /// Inside a transition, between the decision and the action (the mutex is held there).
    pub after_decide: Option<Arc<dyn Fn(Action) + Send + Sync>>,
    /// Just before every acquisition of the lifecycle mutex.
    pub on_lifecycle_lock: Option<Hook>,
    /// The gate's hook between its check and its spawn.
    pub gate: Option<gate::Hook>,
}

/// What the lifecycle runs against.
pub struct LifecycleEnv {
    pub paths: Result<McpPaths, PathRefusal>,
    pub db: Arc<dyn DbSource>,
    /// What `memory_estimate` reads besides the database.
    pub host: Arc<HostPaths>,
    pub reconcile: Reconcile,
    pub peer_uid: PeerUid,
    pub own_uid: u32,
    pub hooks: Hooks,
    /// The write tools' seams: `create_job`'s SMILES call, step B and emit, and `cancel_job`'s cancel world (the
    /// database and host facts are the fields above).
    pub write_seams: WriteSeams,
}

impl LifecycleEnv {
    /// The app's environment: this user's paths, the real reconciliation, `SO_PEERCRED`, the real
    /// `/proc/meminfo` and the app's data dir, and the write tools' seams.
    pub fn production(db: Arc<dyn DbSource>, host: HostPaths, write_seams: WriteSeams) -> Self {
        Self::production_at(db, resolve_for_this_user(), host, write_seams)
    }

    /// [`LifecycleEnv::production`] at given paths and host facts: what the app runs, minus the path
    /// resolution, so a test can wire the production environment under a scratch directory.
    pub fn production_at(
        db: Arc<dyn DbSource>,
        paths: Result<McpPaths, PathRefusal>,
        host: HostPaths,
        write_seams: WriteSeams,
    ) -> Self {
        LifecycleEnv {
            paths,
            db,
            host: Arc::new(host),
            reconcile: Box::new(reconcile_submitting_conn),
            peer_uid: kernel_peer_uid(),
            own_uid: own_uid(),
            hooks: Hooks::default(),
            write_seams,
        }
    }
}

/// What a transition observed, under the lifecycle mutex.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Observed {
    pub exiting: bool,
    /// The `mcp_enabled` row; `None` when the read failed.
    pub row_on: Option<bool>,
    pub holds_lock: bool,
    pub serving: bool,
    /// A stop's tasks or its unlink are still pending.
    pub stop_running: bool,
}

/// What a transition does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Holder: reconcile if this acquisition has not, then bind and serve.
    Start,
    /// Not the holder: try the lock, and on success [`Action::Start`].
    Acquire,
    Stop,
    Nothing(Why),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Why {
    Exiting,
    RowUnreadable,
    Serving,
    StopRunning,
    Off,
}

/// The tick table (M2 amendment, item 2), as a pure function.
pub fn decide(o: &Observed) -> Action {
    if o.exiting {
        return Action::Nothing(Why::Exiting);
    }
    let Some(on) = o.row_on else {
        return Action::Nothing(Why::RowUnreadable);
    };
    match (on, o.serving) {
        (true, true) => Action::Nothing(Why::Serving),
        (true, false) if o.stop_running => Action::Nothing(Why::StopRunning),
        (true, false) if o.holds_lock => Action::Start,
        (true, false) => Action::Acquire,
        (false, true) => Action::Stop,
        (false, false) => Action::Nothing(Why::Off),
    }
}

/// The exit pass-through rule (M2 amendment, item 6): an `ExitRequested` passes through iff the
/// exit flow has finished. Not "the first time", not "not exiting".
pub fn pass_through(exit_ready: bool) -> bool {
    exit_ready
}

/// What [`McpLifecycle::on_exit_requested`] did; the caller calls `prevent_exit` unless
/// [`ExitDecision::PassThrough`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitDecision {
    /// The exit flow has finished: let the exit happen.
    PassThrough,
    /// This request won the `compare_exchange` and started the exit flow.
    Started,
    /// The flow is already running: this request is absorbed.
    Absorbed,
}

/// Where the exit flow runs: never on the caller's thread.
pub trait Executor {
    fn execute(&self, job: Box<dyn FnOnce() + Send>);
}

/// The production executor: Tauri's blocking pool, which queues rather than fails.
pub struct BlockingPool;

impl Executor for BlockingPool {
    fn execute(&self, job: Box<dyn FnOnce() + Send>) {
        // The handle is not needed: the flow ends in the injected `exit`.
        drop(tauri::async_runtime::spawn_blocking(job));
    }
}

/// The app's exit steps. `pause_queue` runs first, before the MCP wait; the rest run in this order
/// after the MCP stop has completed.
pub struct ExitSteps {
    /// Pause the local queue for good, so no queued ORCA job starts once the running one is killed.
    pub pause_queue: Box<dyn FnOnce() + Send>,
    /// `local_backend::terminate_on_exit`.
    pub terminate: Box<dyn FnOnce() + Send>,
    /// The sidecar's stop.
    pub sidecar_stop: Box<dyn FnOnce() + Send>,
    /// `app.exit(0)`; its `ExitRequested` passes through, because `exit_ready` is set first.
    pub exit: Box<dyn FnOnce() + Send>,
}

/// The status Settings → MCP shows (M2 amendment, item 2). **Advisory**: a snapshot whose fields
/// may momentarily disagree; no decision reads it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct McpStatus {
    /// The DB-wide `mcp_enabled` row; `None` when it could not be read.
    pub enabled: Option<bool>,
    pub holds_lock: bool,
    /// This instance serves.
    pub serving: bool,
    /// While a stop runs: its tracked tasks still running. `None` when no stop runs.
    pub stopping_tasks: Option<usize>,
    /// Clients connected to this instance.
    pub connections: usize,
    pub socket_path: Option<String>,
    /// Why this instance does not serve.
    pub reason: Option<String>,
    /// The read count of this instance (M4 amendment, item 2): the total since app start and one
    /// entry per live or closing connection.
    pub reads: McpReads,
}

/// A stop in progress or complete: the wait for every tracked task, then the unlink.
#[derive(Clone)]
pub struct StopHandle(Arc<StopState>);

struct StopState {
    /// `Some` once complete: the problem the unlink met, if any.
    done: Mutex<Option<Option<String>>>,
    cv: Condvar,
}

impl StopHandle {
    fn new() -> Self {
        StopHandle(Arc::new(StopState { done: Mutex::new(None), cv: Condvar::new() }))
    }

    fn complete(&self, problem: Option<String>) {
        *self.0.done.lock().unwrap_or_else(PoisonError::into_inner) = Some(problem);
        self.0.cv.notify_all();
    }

    pub fn is_complete(&self) -> bool {
        self.0.done.lock().unwrap_or_else(PoisonError::into_inner).is_some()
    }

    fn problem(&self) -> Option<String> {
        self.0.done.lock().unwrap_or_else(PoisonError::into_inner).clone().flatten()
    }

    /// Block until the stop is complete. Never call it on the main thread.
    pub fn wait(&self) {
        let mut done = self.0.done.lock().unwrap_or_else(PoisonError::into_inner);
        while done.is_none() {
            done = self.0.cv.wait(done).unwrap_or_else(PoisonError::into_inner);
        }
    }
}

/// What the lifecycle mutex guards.
#[derive(Default)]
struct Transitions {
    lock: Option<McpLock>,
    /// This acquisition's reconciliation has committed.
    reconciled: bool,
    serving: Option<Serving>,
    /// The latest stop.
    stop: Option<StopHandle>,
}

/// One start: the socket it bound and what switch-off cancels.
struct Serving {
    socket: SocketIdentity,
    cancel: CancellationToken,
    registry: Arc<Registry>,
    accept: tauri::async_runtime::JoinHandle<()>,
}

/// The status facts, published at the end of every transition. A leaf mutex.
#[derive(Clone)]
struct Facts {
    holds_lock: bool,
    serving: bool,
    reason: Option<String>,
    stop: Option<StopHandle>,
}

pub struct McpLifecycle {
    env: LifecycleEnv,
    /// **The** lifecycle mutex.
    transitions: Mutex<Transitions>,
    /// Terminal; set once, by the winning `compare_exchange` in `on_exit_requested`.
    exiting: AtomicBool,
    /// Set by the exit flow after the lock is released; the pass-through key.
    exit_ready: AtomicBool,
    gate: Arc<AdmissionGate>,
    /// The write tools' gate, `create_job` environment and `cancel_job` world,
    /// handed to every connection's service.
    write_wiring: WriteWiring,
    /// The connection slots, shared by every start (a draining connection still holds one).
    slots: Arc<Semaphore>,
    connected: Arc<AtomicUsize>,
    /// The read count since app start: a leaf mutex, outlives every start (M4 amendment, item 2).
    reads: Arc<ReadLedger>,
    facts: Mutex<Facts>,
}

impl McpLifecycle {
    pub fn new(env: LifecycleEnv) -> Arc<Self> {
        let gate = Arc::new(AdmissionGate::new(env.hooks.gate.clone()));
        let seams = env.write_seams.clone();
        let write_wiring = WriteWiring {
            gate: gate.clone(),
            env: Arc::new(CreateJobEnv {
                db: env.db.clone(),
                smiles: seams.smiles,
                host: env.host.clone(),
                step_b: seams.step_b,
                emit: seams.emit,
            }),
            cancel: seams.cancel,
        };
        Arc::new(McpLifecycle {
            env,
            transitions: Mutex::new(Transitions::default()),
            exiting: AtomicBool::new(false),
            exit_ready: AtomicBool::new(false),
            gate,
            write_wiring,
            slots: Arc::new(Semaphore::new(CONNECTIONS)),
            connected: Arc::new(AtomicUsize::new(0)),
            reads: Arc::new(ReadLedger::default()),
            facts: Mutex::new(Facts { holds_lock: false, serving: false, reason: Some(NOT_CHECKED.into()), stop: None }),
        })
    }

    /// The admission gate M3's `create_job` spawns through.
    pub fn gate(&self) -> &Arc<AdmissionGate> {
        &self.gate
    }

    /// The write tools' gate, `create_job` environment and `cancel_job` world, as every connection's service gets them. Test-only.
    #[cfg(test)]
    pub fn write_wiring(&self) -> WriteWiring {
        self.write_wiring.clone()
    }

    pub fn is_exiting(&self) -> bool {
        self.exiting.load(Ordering::SeqCst)
    }

    fn lock_transitions(&self) -> MutexGuard<'_, Transitions> {
        if let Some(hook) = &self.env.hooks.on_lifecycle_lock {
            hook();
        }
        // A panic mid-transition leaves each field valid on its own; the next tick re-decides.
        self.transitions.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn facts(&self) -> MutexGuard<'_, Facts> {
        self.facts.lock().unwrap_or_else(PoisonError::into_inner)
    }

    // ----- transitions -----

    /// One tick step (the 30 s tick, and once at app start). Returns what it did.
    pub fn tick_step(&self) -> Action {
        self.transition(None).unwrap_or(Action::Nothing(Why::RowUnreadable))
    }

    /// `mcp_set_enabled`: write the row (its only writer), then run one tick step at once, all under
    /// the lifecycle mutex. Once exit has begun it writes nothing and the status says "exiting".
    pub fn set_enabled(&self, on: bool) -> Result<McpStatus, AppError> {
        self.transition(Some(on))?;
        Ok(self.status())
    }

    /// The one transition body: the optional row write, the decision and the action, under one
    /// hold of the lifecycle mutex.
    fn transition(&self, write: Option<bool>) -> Result<Action, AppError> {
        let mut t = self.lock_transitions();
        if let Some(on) = write {
            if self.is_exiting() {
                // The row is shared: writing it while exiting would silently change other instances.
                return Ok(Action::Nothing(Why::Exiting));
            }
            write_enabled_conn(&*self.env.db.db().lock()?, on)?;
        }
        let action = decide(&self.observe(&t));
        if let Some(hook) = &self.env.hooks.after_decide {
            hook(action);
        }
        self.apply(&mut t, action);
        Ok(action)
    }

    fn observe(&self, t: &Transitions) -> Observed {
        Observed {
            exiting: self.is_exiting(),
            row_on: self.read_row().ok(),
            holds_lock: t.lock.is_some(),
            serving: t.serving.is_some(),
            stop_running: t.stop.as_ref().is_some_and(|s| !s.is_complete()),
        }
    }

    fn read_row(&self) -> Result<bool, AppError> {
        read_enabled_conn(&*self.env.db.db().lock()?)
    }

    fn apply(&self, t: &mut Transitions, action: Action) {
        let reason = match action {
            Action::Nothing(Why::RowUnreadable) => return, // changes nothing, not even the status
            Action::Nothing(Why::Exiting) => Some(EXITING.to_string()),
            Action::Nothing(Why::Serving) => None,
            Action::Nothing(Why::StopRunning) => Some(STOPPING.to_string()),
            Action::Nothing(Why::Off) => Some(OFF.to_string()),
            Action::Stop => {
                self.switch_off(t);
                Some(OFF.to_string())
            }
            Action::Acquire => self.acquire(t).and_then(|()| self.start(t)).err(),
            Action::Start => self.start(t).err(),
        };
        self.publish(t, reason);
    }

    /// Take the lock (a non-holder that reads "on"). A new acquisition must reconcile again.
    fn acquire(&self, t: &mut Transitions) -> Result<(), String> {
        let paths = self.env.paths.as_ref().map_err(ToString::to_string)?;
        socket::prepare_dir(&paths.dir, self.env.own_uid).map_err(|e| e.to_string())?;
        match lock::try_acquire(&paths.lock).map_err(|e| e.to_string())? {
            Acquire::Acquired(held) => {
                t.lock = Some(held);
                t.reconciled = false;
                Ok(())
            }
            Acquire::HeldElsewhere => Err(ANOTHER_INSTANCE.to_string()),
        }
    }

    /// Switch-on, for the holder: reconcile once per acquisition, then bind, `stat`, open the gate,
    /// accept (M2 amendment, item 6). It is reached only through [`decide`]'s `Start` (which it
    /// gives the holder alone) or a successful [`McpLifecycle::acquire`]: the lock is the one guard
    /// on reconciling and binding, and `lifecycle_tests` shows it bites.
    fn start(&self, t: &mut Transitions) -> Result<(), String> {
        let paths = self.env.paths.as_ref().map_err(ToString::to_string)?;
        if !t.reconciled {
            let db = self.env.db.db();
            let conn = db.lock().map_err(|e| e.to_string())?;
            with_busy_timeout(&conn, RECONCILE_BUSY_TIMEOUT, |c| (self.env.reconcile)(c)).map_err(|e| {
                format!("the reconciliation failed ({e}); nothing is served, and the next tick retries it")
            })?;
            t.reconciled = true;
        }

        socket::prepare_dir(&paths.dir, self.env.own_uid).map_err(|e| e.to_string())?;
        let bound = socket::bind(paths, self.env.own_uid, self.env.hooks.after_bind.as_deref())
            .map_err(|e| e.to_string())?;
        let identity = bound.identity.clone();
        let listener = into_tokio(bound.listener).map_err(|e| {
            let _ = socket::unlink_if_same(&identity);
            format!("cannot listen on {}: {e}", identity.path.display())
        })?;

        self.gate.open();
        let cancel = CancellationToken::new();
        let registry = Arc::new(Registry::default());
        let ctx = AcceptContext {
            cancel: cancel.clone(),
            registry: registry.clone(),
            slots: self.slots.clone(),
            connected: self.connected.clone(),
            reads: self.reads.clone(),
            peer_uid: self.env.peer_uid.clone(),
            own_uid: self.env.own_uid,
            db: self.env.db.clone(),
            host: self.env.host.clone(),
            write_wiring: self.write_wiring.clone(),
        };
        let accept = tauri::async_runtime::spawn(accept_loop(listener, ctx));
        t.serving = Some(Serving { socket: identity, cancel, registry, accept });
        Ok(())
    }

    /// Switch-off. Under the mutex: close the gate, set every `closed` flag, cancel the services.
    /// Then a task on Tauri's runtime, outside the mutex: wait for the accept loop and every
    /// tracked task, unlink the socket **this start bound** (by inode), and mark the stop complete.
    /// The `flock` is kept.
    fn switch_off(&self, t: &mut Transitions) -> StopHandle {
        self.gate.close();
        let serving = t.serving.take();
        if let Some(s) = &serving {
            s.registry.shut();
            s.cancel.cancel();
        }
        let stop = StopHandle::new();
        let (gate, done, before_unlink) = (self.gate.clone(), stop.clone(), self.env.hooks.before_unlink.clone());
        tauri::async_runtime::spawn(async move {
            let socket = match serving {
                Some(s) => {
                    let _ = s.accept.await;
                    Some(s.socket)
                }
                None => None,
            };
            gate.wait_all().await;
            if let Some(hook) = before_unlink {
                hook();
            }
            let problem = socket.and_then(|id| match socket::unlink_if_same(&id) {
                Ok(Unlinked::Removed | Unlinked::AlreadyGone) => None,
                Ok(Unlinked::Replaced) => Some(format!("{} was replaced by a newer socket; left in place", id.path.display())),
                Err(e) => Some(format!("cannot remove {}: {e}", id.path.display())),
            });
            done.complete(problem);
        });
        t.stop = Some(stop.clone());
        stop
    }

    fn publish(&self, t: &Transitions, reason: Option<String>) {
        let mut f = self.facts();
        f.holds_lock = t.lock.is_some();
        f.serving = t.serving.is_some();
        f.reason = reason;
        f.stop = t.stop.clone();
    }

    // ----- status -----

    /// The advisory status. It takes the facts' leaf mutex, then `DbState`, then the read ledger's
    /// leaf mutex, each alone and released before the next; never the lifecycle mutex, and the
    /// gate's only while a stop runs (when no handler can hold it for long: a closed gate refuses at
    /// once).
    pub fn status(&self) -> McpStatus {
        let facts = self.facts().clone();
        let enabled = self.read_row().ok();
        let reads = self.reads.snapshot();
        let stop_running = facts.stop.as_ref().filter(|s| !s.is_complete());
        let last_problem = facts.stop.as_ref().and_then(StopHandle::problem);
        let reason = if self.is_exiting() {
            Some(EXITING.to_string())
        } else {
            match (facts.reason, last_problem) {
                (Some(r), Some(p)) => Some(format!("{r}; the last stop: {p}")),
                (r, _) => r,
            }
        };
        McpStatus {
            enabled,
            holds_lock: facts.holds_lock,
            serving: facts.serving,
            stopping_tasks: stop_running.map(|_| self.gate.task_count()),
            connections: self.connected.load(Ordering::SeqCst),
            socket_path: self.socket_path().map(|p| p.display().to_string()),
            reason,
            reads,
        }
    }

    pub fn socket_path(&self) -> Option<PathBuf> {
        self.env.paths.as_ref().ok().map(|p| p.socket.clone())
    }

    /// The status's `connections`, without the row read. Test-only.
    #[cfg(test)]
    pub fn open_transports(&self) -> usize {
        self.connected.load(Ordering::SeqCst)
    }

    /// The connections of the current start. Test-only: it takes the lifecycle mutex, which no
    /// handler may.
    #[cfg(test)]
    pub fn connections(&self) -> Vec<Arc<super::service::ConnState>> {
        let t = self.lock_transitions();
        t.serving.as_ref().map(|s| s.registry.connections()).unwrap_or_default()
    }

    // ----- exit -----

    /// `RunEvent::ExitRequested`, on the caller's (main) thread. Takes **no** mutex: the
    /// pass-through check, one `compare_exchange` on `exiting`, and for the winner a hand-off of the
    /// exit flow to `executor`. The caller calls `prevent_exit` unless this returns
    /// [`ExitDecision::PassThrough`].
    pub fn on_exit_requested(self: &Arc<Self>, executor: &dyn Executor, steps: ExitSteps) -> ExitDecision {
        if pass_through(self.exit_ready.load(Ordering::SeqCst)) {
            return ExitDecision::PassThrough;
        }
        if self.exiting.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst).is_err() {
            return ExitDecision::Absorbed;
        }
        let me = Arc::clone(self);
        executor.execute(Box::new(move || me.exit_flow(steps)));
        ExitDecision::Started
    }

    /// The exit flow, off the caller's thread (M2 amendment, item 6):
    /// 0. `pause_queue`: no queued local job starts from here on (the killed job's slot frees);
    /// 1. take the lifecycle mutex and switch off (or adopt a stop that began before exit), then
    ///    release the mutex;
    /// 2. wait for **stop complete**: every tracker generation ended and the unlink done;
    /// 3. `terminate`, then `sidecar_stop`;
    /// 4. release the lock, set `exit_ready`, call `exit`. `exit_ready` is set **before** `exit`,
    ///    so the `ExitRequested` that `exit` raises passes through (pinned by a test).
    fn exit_flow(&self, steps: ExitSteps) {
        (steps.pause_queue)();
        let stop = {
            let mut t = self.lock_transitions();
            let running = t.stop.clone().filter(|s| !s.is_complete());
            let stop = match running {
                Some(stop) if t.serving.is_none() => stop,
                _ => self.switch_off(&mut t),
            };
            self.publish(&t, Some(EXITING.to_string()));
            stop
        };
        stop.wait();
        (steps.terminate)();
        (steps.sidecar_stop)();
        {
            let mut t = self.lock_transitions();
            t.lock = None;
            t.reconciled = false;
            self.publish(&t, Some(EXITING.to_string()));
        }
        self.exit_ready.store(true, Ordering::SeqCst);
        (steps.exit)();
    }
}

/// How long the reconciliation waits on another instance's write transaction before it reports
/// `SQLITE_BUSY` (decided by Anton, M2a Part B). It holds `DbState` meanwhile, so every synchronous
/// UI command that locks it waits too: 250 ms, not `rusqlite`'s 5 s default. The tick retries.
pub const RECONCILE_BUSY_TIMEOUT: Duration = Duration::from_millis(250);

/// Run `work` with `conn`'s busy timeout set to `timeout`, and put the previous one back afterwards
/// on every path: success, error and panic (the restore is a drop guard). The previous value is read
/// with `PRAGMA busy_timeout`.
pub fn with_busy_timeout<T>(
    conn: &Connection,
    timeout: Duration,
    work: impl FnOnce(&Connection) -> Result<T, AppError>,
) -> Result<T, AppError> {
    let previous: u64 = conn.pragma_query_value(None, "busy_timeout", |r| r.get(0))?;
    conn.busy_timeout(timeout)?;
    let restore = RestoreBusyTimeout { conn, previous: Duration::from_millis(previous) };
    let result = work(conn);
    drop(restore);
    result
}

/// Puts a connection's busy timeout back when dropped.
struct RestoreBusyTimeout<'c> {
    conn: &'c Connection,
    previous: Duration,
}

impl Drop for RestoreBusyTimeout<'_> {
    fn drop(&mut self) {
        if let Err(e) = self.conn.busy_timeout(self.previous) {
            // Not expected (it fails only on misuse). Said rather than left silent: every later write
            // of the app would give up after the short timeout.
            eprintln!("[OrcaStudio] mcp: cannot restore the database busy timeout ({e})");
        }
    }
}

/// Hand a bound std listener to tokio, inside Tauri's runtime (the IO driver it registers with).
fn into_tokio(listener: std::os::unix::net::UnixListener) -> std::io::Result<tokio::net::UnixListener> {
    listener.set_nonblocking(true)?;
    let handle = tauri::async_runtime::handle();
    let _entered = handle.inner().enter();
    tokio::net::UnixListener::from_std(listener)
}
