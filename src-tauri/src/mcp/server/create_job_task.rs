//! The `create_job` handler's side (ADR-027 M0 amendment, items 5, 6 and 10; M2 amendment, item 4;
//! unit M3a Part A): the **whole** call runs as one task admitted through the gate, and the handler
//! only awaits it.
//!
//! - **The permit goes with the work.** The handler takes one of the connection's request permits
//!   (none free: [`BUSY`], at once) and moves it into the task, where it is held by the blocking
//!   pipeline itself: it returns when step C has ended, not when the handler returns, so a
//!   connection's slot frees only after its last `create_job` has finished.
//! - **Admission.** The task is spawned through [`AdmissionGate::spawn`]: the gate's check and the
//!   spawn happen under its one mutex, and a closed gate refuses with "MCP is shutting down" before
//!   anything is read or written. Switch-off and exit close the gate first and then wait for every
//!   tracked task, so a clean exit releases the lock only after step C.
//! - **Nothing can abandon it.** The spawn happens before any DB write, and the pipeline runs on
//!   the blocking pool inside the tracked task. A hang-up (`closed`), a service cancel (the handler's
//!   future dropped), a switch-off and exit each leave the task running to step C: dropping a tokio
//!   `JoinHandle` detaches the task, it does not abort it, and the pipeline never reads `closed`.
//!
//! The tool is registered in the `rmcp` service (`super::create_job_tool`), whose `call_tool` takes
//! no generic permit for `create_job`: this handler takes its own, to move it into the task.

use std::sync::Arc;

use tokio::sync::OwnedSemaphorePermit;

use super::gate::AdmissionGate;
use super::m3b_tools::CancelWorld;
use super::service::{ConnState, BUSY};
use crate::error::AppError;
use crate::mcp::create_job::{run_create_job, CreateJobArgs, CreateJobEnv, CreateJobResult, StatusEmit, StepB};
use crate::mcp::resolve::SmilesTo3d;
use crate::mcp::tools::ToolError;

/// What the write tools reach besides the database and the host facts (which the lifecycle already
/// holds): for `create_job`, the sidecar's SMILES call, step B and the `job:status` emit; for
/// `cancel_job` (M3b), its [`CancelWorld`] (the app's poller and the backend's cancel). The app's are
/// built over the `AppHandle` (`super::wiring::app_create_job`); a test passes its own.
#[derive(Clone)]
pub struct CreateJobSeams {
    pub smiles: Arc<dyn SmilesTo3d + Send + Sync>,
    pub step_b: Arc<dyn StepB>,
    pub emit: Arc<dyn StatusEmit>,
    pub cancel: Arc<dyn CancelWorld>,
}

/// What a connection's service needs for the write tools: the lifecycle's admission gate and the one
/// `create_job` environment every call runs in, and `cancel_job`'s world.
#[derive(Clone)]
pub struct CreateJobWiring {
    pub gate: Arc<AdmissionGate>,
    pub env: Arc<CreateJobEnv>,
    pub cancel: Arc<dyn CancelWorld>,
}

/// The `create_job` handler: take a permit, admit the task through the gate, and await its result.
/// Every step of the call — the memory verdict, step A, step B, step C — runs in the task.
pub async fn create_job_call(
    conn: &ConnState,
    gate: &AdmissionGate,
    env: Arc<CreateJobEnv>,
    args: CreateJobArgs,
) -> Result<CreateJobResult, ToolError> {
    let Some(permit) = conn.try_permit() else {
        return Err(ToolError::Refused(BUSY.into()));
    };
    let task = gate.spawn(create_job_task(permit, env, args)).map_err(|closed| ToolError::refused(closed.to_string()))?;
    match task.await {
        Ok(result) => result,
        Err(e) => Err(ToolError::App(AppError::Internal(format!("the create_job task failed: {e}")))),
    }
}

/// The tracked task: the pipeline on the blocking pool (it locks the database, may call the sidecar
/// and runs step B's ssh), holding the permit until it returns.
async fn create_job_task(
    permit: OwnedSemaphorePermit,
    env: Arc<CreateJobEnv>,
    args: CreateJobArgs,
) -> Result<CreateJobResult, ToolError> {
    let work = tauri::async_runtime::spawn_blocking(move || {
        let _permit = permit;
        run_create_job(&env, args)
    });
    match work.await {
        Ok(result) => result,
        Err(e) => Err(ToolError::App(AppError::Internal(format!("the create_job work failed: {e}")))),
    }
}
