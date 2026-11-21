//! `create_job`'s real step B (ADR-027 decision 5; M3 amendment, item 1; unit M3a Part B): a charged
//! draft submitted through the app's **existing** submit paths, with no database lock held.
//!
//! - **This machine** ([`local_step_b`]): `local_backend::submit`'s steps — the draft becomes
//!   `queued` ([`queue_draft_conn`]), then the `job:status` emit and `try_start_next` (the
//!   `SubmitWorld`'s `after_local_queue`), as the `submit_job` command's local arm does.
//! - **A server profile** ([`remote_step_b`]): `submit_to_server`'s guarded path — the job's
//!   in-flight guard, then [`announcing_change`] around [`attempt_with_contact`] around
//!   [`submit_remote`] (`persist_submit`, the ssh calls and the rsync). `submit_to_server` itself is
//!   async and spawns its own blocking task; step B already runs on the blocking pool inside the
//!   tracked task, so it runs the same pieces in the same order on its own thread.
//!
//! Neither path checks `mcp_state`: the `submit_job` guard sits in the commands, and step B submits
//! this very job (decision 5, "the window").
//!
//! **Exit and switch-off (decided by Anton, M3a).** Once the exit flow has begun **or** the admission
//! gate has closed (switch-off or exit), step B refuses with [`APP_CLOSING`] or [`MCP_SWITCHING_OFF`]
//! **before** the draft is queued, and on the remote path before `persist_submit`: the row stays a
//! charged `draft`, step C refunds it to PROPOSED, and no ssh starts for it. After `persist_submit`
//! has committed `queued` (before any ssh, `ssh_backend::submit_remote`) the job is never refunded.
//!
//! **The residual window.** The check is a read, not a lock shared with the exit flow. An exit (or a
//! switch-off) that begins after the read is not seen by this step B: on the remote path the window
//! is the wait for the `DbState` mutex plus `persist_submit`'s own work — its transaction, the local
//! job dir's file writes and the upload listing (`ssh_backend::persist_submit`) — and an exit that
//! begins inside it **does** get its ssh calls started during the exit. Its job is then `queued` and
//! kept, as for an exit that begins during the ssh calls. On the local path the window is the lock
//! wait plus the queueing transaction; a job queued in it claims no slot once the exit flag is set.
//!
//! **What step B reports.** [`StepBOutcome::Handed`] once the path took the job (local `queued`; on a
//! server, every outcome after the persist step: enqueued, pending, "not on the server", unknown),
//! with the outcome in words; [`StepBOutcome::Refused`] for an error. Step C does not read the type:
//! it judges the row (an error after `persist_submit` leaves a `queued` row, which keeps its charge).
//!
//! Everything here is `AppHandle`-free and tested over a scratch database and the `ssh_backend`
//! fake runners; the app's [`SubmitWorld`] (`server::wiring`) only looks up its managed state.

use std::cell::Cell;
use std::path::Path;

use super::classify::Target;
use super::create_job::{StepB, StepBOutcome};
use crate::commands::remote_jobs::{announcing_change, attempt_with_contact};
use crate::commands::settings::DbState;
use crate::error::AppError;
use crate::in_flight::InFlight;
use crate::local_backend::queue_draft_conn;
use crate::poller::plan::PollerMemory;
use crate::poller::remote_state::RemoteStates;
use crate::poller::PollerSink;
use crate::remote::ssh::CommandRunner;
use crate::ssh_backend::{submit_remote, SubmitAttempt};

/// Step B's refusal once the exit flow has begun.
pub const APP_CLOSING: &str = "the app is closing; not submitted";
/// Step B's refusal once the admission gate has closed without an exit (MCP switched off).
pub const MCP_SWITCHING_OFF: &str = "MCP is switching off; not submitted";

/// Why step B must not start a submit now: the exit flow has begun, or the gate has closed. `None`:
/// it may.
pub fn closing_reason(exiting: bool, gate_open: bool) -> Option<&'static str> {
    if exiting {
        Some(APP_CLOSING)
    } else if !gate_open {
        Some(MCP_SWITCHING_OFF)
    } else {
        None
    }
}

/// What the remote path reaches: the app's managed state in production (`server::wiring`), scratch
/// state and a fake runner in tests.
pub struct RemoteParts<'a> {
    pub db: &'a DbState,
    pub in_flight: &'a InFlight,
    pub remote: &'a RemoteStates,
    pub memory: &'a PollerMemory,
    /// The `job:status` and pending-flag events (`AppSink` in the app).
    pub sink: &'a dyn PollerSink,
    /// The local job dirs' root (`JobRunner::data_dir`).
    pub data_dir: &'a Path,
    /// The ssh/rsync runner (`SystemRunner` in the app).
    pub runner: &'a dyn CommandRunner,
}

/// Step B on this machine: refuse if closing, else queue the draft and run `after_queued` (the emit
/// and `try_start_next`).
pub fn local_step_b(
    db: &DbState,
    job_id: &str,
    closing: &dyn Fn() -> Option<&'static str>,
    after_queued: impl FnOnce(),
) -> StepBOutcome {
    if let Some(why) = closing() {
        return StepBOutcome::Refused(why.into());
    }
    match db.lock().and_then(|conn| queue_draft_conn(&conn, job_id)) {
        Ok(()) => {
            after_queued();
            StepBOutcome::Handed("queued on this machine".into())
        }
        Err(e) => StepBOutcome::Refused(e.to_string()),
    }
}

/// Step B on server profile `profile_id`: `submit_to_server`'s guarded path (module docs), with the
/// closing check right before `persist_submit`.
pub fn remote_step_b(
    parts: &RemoteParts<'_>,
    job_id: &str,
    profile_id: &str,
    closing: &dyn Fn() -> Option<&'static str>,
) -> StepBOutcome {
    let refused_closing: Cell<Option<&'static str>> = Cell::new(None);
    let attempt = parts.in_flight.acquire(job_id).and_then(|_guard| {
        announcing_change(parts.db, parts.sink, job_id, |job| {
            attempt_with_contact(parts.remote, parts.memory, parts.sink, &job.id, parts.runner, |runner| {
                if let Some(why) = closing() {
                    refused_closing.set(Some(why));
                    return Err(AppError::Conflict(why.into()));
                }
                submit_remote(parts.db, runner, parts.sink, parts.data_dir, &job.id, profile_id)
            })
        })
    });
    match (attempt, refused_closing.get()) {
        (_, Some(why)) => StepBOutcome::Refused(why.into()),
        (Ok(a), None) => StepBOutcome::Handed(describe(&a, profile_id)),
        (Err(e), None) => StepBOutcome::Refused(e.to_string()),
    }
}

/// A remote attempt in words: its outcome first, then the memory notice and the `%pal` notice.
fn describe(attempt: &SubmitAttempt, profile_id: &str) -> String {
    let mut parts = vec![attempt.outcome.failure().unwrap_or_else(|| format!("enqueued on server profile {profile_id}"))];
    parts.extend(attempt.memory_notice.clone());
    parts.extend(attempt.pal.notice());
    parts.join("; ")
}

/// What the real step B reaches outside its arguments. The app's is `server::wiring`'s, over the
/// `AppHandle`; a test's is scratch state.
pub trait SubmitWorld: Send + Sync {
    /// [`closing_reason`] now, over the app's lifecycle and gate.
    fn closing(&self) -> Option<&'static str>;
    /// The database, or why it is not available yet.
    fn db(&self) -> Result<&DbState, &'static str>;
    /// After a local draft was queued: the `job:status` emit and `try_start_next`.
    fn after_local_queue(&self, job_id: &str);
    /// `work` over the remote parts, or why they are not available yet.
    fn with_remote(&self, work: &mut dyn FnMut(&RemoteParts<'_>) -> StepBOutcome) -> Result<StepBOutcome, &'static str>;
}

/// The real step B over a [`SubmitWorld`]: the target picks the path.
pub struct RealStepB<W>(pub W);

impl<W: SubmitWorld> StepB for RealStepB<W> {
    fn submit(&self, job_id: &str, target: &Target) -> StepBOutcome {
        let world = &self.0;
        let closing = || world.closing();
        let reached = match target {
            Target::Local => world.db().map(|db| local_step_b(db, job_id, &closing, || world.after_local_queue(job_id))),
            Target::Server { profile_id } => world.with_remote(&mut |parts| remote_step_b(parts, job_id, profile_id, &closing)),
        };
        reached.unwrap_or_else(|why| StepBOutcome::Refused(why.into()))
    }
}
