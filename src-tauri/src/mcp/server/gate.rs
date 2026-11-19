//! The admission gate (ADR-027 M2 amendment, item 4). `TaskTracker::close` "does not prevent you
//! from spawning new tasks" (sourced, `tokio-util` `task_tracker.rs:331`), so the refusal is ours:
//!
//! - one mutex holds `open`, and is held across **both** the check and the spawn; `close` takes the
//!   same mutex, so no task is admitted once `close` has returned;
//! - `TaskTracker` is used only for `wait()`;
//! - opening after a close starts a fresh tracker **generation**. Every generation is kept until its
//!   tasks have ended, and [`AdmissionGate::wait_all`] waits on all of them.
//!
//! M2a spawns no task: M3's `create_job` is the first. The gate's mutex is a leaf below the
//! lifecycle mutex (lifecycle → gate); a handler takes the gate, never the lifecycle.

use std::future::Future;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use tokio_util::task::TaskTracker;

/// The refusal a handler returns once the gate is closed (M0 amendment, item 6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("MCP is shutting down")]
pub struct GateClosed;

/// A test hook run between the gate's check and its spawn, with the gate's mutex held.
pub type Hook = Arc<dyn Fn() + Send + Sync>;

pub struct AdmissionGate {
    state: Mutex<GateState>,
    between_check_and_spawn: Option<Hook>,
}

struct GateState {
    open: bool,
    /// The generation new tasks join while the gate is open.
    current: TaskTracker,
    /// Closed generations whose tasks may still run.
    older: Vec<TaskTracker>,
}

impl Default for AdmissionGate {
    fn default() -> Self {
        Self::new(None)
    }
}

impl AdmissionGate {
    /// A closed gate. `between_check_and_spawn` is `None` outside tests.
    pub fn new(between_check_and_spawn: Option<Hook>) -> Self {
        let current = TaskTracker::new();
        current.close();
        AdmissionGate { state: Mutex::new(GateState { open: false, current, older: Vec::new() }), between_check_and_spawn }
    }

    fn state(&self) -> MutexGuard<'_, GateState> {
        // A panic under this mutex leaves `open` and the trackers each valid on their own.
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Open the gate. After a close, the closed generation is kept and a fresh one begins.
    pub fn open(&self) {
        let mut s = self.state();
        if s.open {
            return;
        }
        let closed = std::mem::replace(&mut s.current, TaskTracker::new());
        s.older.retain(|t| !t.is_empty());
        if !closed.is_empty() {
            s.older.push(closed);
        }
        s.open = true;
    }

    /// Close the gate. Once this returns, [`AdmissionGate::spawn`] refuses.
    pub fn close(&self) {
        let mut s = self.state();
        s.open = false;
        s.current.close();
    }

    pub fn is_open(&self) -> bool {
        self.state().open
    }

    /// Spawn `task` on Tauri's runtime as a tracked task, or refuse if the gate is closed. The check
    /// and the spawn happen under one hold of the gate's mutex.
    pub fn spawn<F>(&self, task: F) -> Result<tauri::async_runtime::JoinHandle<F::Output>, GateClosed>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        let s = self.state();
        if !s.open {
            return Err(GateClosed);
        }
        if let Some(hook) = &self.between_check_and_spawn {
            hook();
        }
        Ok(tauri::async_runtime::spawn(s.current.track_future(task)))
    }

    /// Tracked tasks still running, across every generation.
    pub fn task_count(&self) -> usize {
        let s = self.state();
        s.current.len() + s.older.iter().map(TaskTracker::len).sum::<usize>()
    }

    /// Wait until every task of every generation has ended. Call it after [`AdmissionGate::close`]:
    /// a tracker's `wait` completes only once it is closed and empty.
    pub async fn wait_all(&self) {
        let trackers: Vec<TaskTracker> = {
            let s = self.state();
            s.older.iter().cloned().chain(std::iter::once(s.current.clone())).collect()
        };
        for tracker in trackers {
            tracker.wait().await;
        }
    }
}
