//! The M3b tools registered in the `rmcp` service (ADR-027 decisions 5, 7 and 8; M3 amendment,
//! item 1; unit M3b Part B): `cancel_job` and `create_group` (writes, each with its audit rows) and
//! `wait_for_jobs` (the one bounded long-poll, a read). Each is a thin shell over its core, like the
//! read tools: typed arguments with `deny_unknown_fields`, the core on the right thread, and
//! [`respond`].
//!
//! - **`cancel_job`** runs [`cancel_in`] on the blocking pool (the cancel core may make ssh calls, and
//!   it locks `DbState` itself, for one read or write at a time). The app's [`CancelWorld`] is
//!   `super::wiring::AppCancelWorld`: `poller::app::with_poller` and `commands::jobs::dispatch_cancel`,
//!   the same poller and dispatch as Anton's Cancel, so every `job:status` the cancel owes is emitted
//!   by the same code (the poller's `AppSink` for a server job; `local_backend::cancel` for a local
//!   one).
//! - **`create_group`** runs its core under `DbState`, locked once on the blocking pool
//!   (`McpService::with_db`). Once the core has returned `Ok`, so the transaction is committed and the
//!   guard dropped, it emits the payload-free `groups:changed` ([`announce_groups_changed`], through
//!   the service's emit seam, `StatusEmit::groups_changed`) so an open Jobs screen reloads its groups
//!   (M4 amendment, item 7). A refused or failed insert emits nothing. The app's own `create_group`
//!   command is unchanged: its caller already reloads.
//! - **`wait_for_jobs`** awaits its core on the handler's task with the connection's `closed` flag,
//!   the app's reader ([`DbStatusReader`]: one query per tick on the blocking pool) and clock. The
//!   call holds one of the connection's permits (`call_tool`) until the handler returns, so a wait
//!   that ends on `closed` (within one 2 s tick of the client hanging up) returns its permit, and the
//!   connection's slot can free. The result of a wait that ended on `closed` goes nowhere; it is
//!   still sent through [`respond`], as an error.

use std::collections::BTreeMap;
use std::panic::{catch_unwind, AssertUnwindSafe};

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::schemars::JsonSchema;
use rmcp::{tool, tool_router, ErrorData};
use serde::Deserialize;

use super::read_tools::{on_blocking_pool, respond};
use super::service::{recovery_sentence, McpService};
use crate::error::AppError;
use crate::mcp::cancel::{run_cancel_job, CancelJobArgs, CancelJobResult};
use crate::mcp::create_group::{run_create_group, CreateGroupArgs};
use crate::mcp::create_job::StatusEmit;
use crate::mcp::tools::wait::{wait_for_jobs, DbStatusReader, TokioClock, WaitArgs, WaitEnd, WaitResult, WAIT_TICK};
use crate::mcp::tools::ToolError;
use crate::models::job::Job;
use crate::poller::Poller;

/// `cancel_job`'s description.
pub const CANCEL_JOB_DESCRIPTION: &str = concat!(
    "Cancel one of Claude's own jobs (origin mcp), local or on a server. A job Anton made is refused. ",
    "A draft is refused too: a PROPOSED draft is Anton's to delete, and a draft being submitted can be ",
    "cancelled once it is queued. Every cancel and every refusal is written to the audit log. result ",
    "says what happened: dispatched (a local job), delivered or pending (a server job; the poller ",
    "delivers a pending cancel), already_on_server, or moved_on (the job ended first). ",
    recovery_sentence!()
);

/// `create_group`'s description.
pub const CREATE_GROUP_DESCRIPTION: &str = concat!(
    "Create a job group to file jobs under (metadata only: it spends nothing). name: at most 200 ",
    "characters, stored trimmed. parent_id: an id from list_groups, to nest it; omit it for a top-level ",
    "group. Returns the new group's id; written to the audit log. ",
    recovery_sentence!()
);

/// `wait_for_jobs`' description.
pub const WAIT_FOR_JOBS_DESCRIPTION: &str = concat!(
    "Wait until any named job changes status. known maps each job id (1 to 50 jobs) to the status you ",
    "last saw. It answers at once if a status already differs; otherwise it re-reads every 2 s until ",
    "one does or timeout_s passes (default 600; above 600 it is capped at 600; 0 checks once). A job id ",
    "that does not exist, a mistyped one included, is answered at once as \"gone\": check the id ",
    "before reading that as a deletion. The result lists the changed jobs, or timed_out: true. ",
    recovery_sentence!()
);

/// The error a wait that ended on `closed` returns. Nobody receives it: the client has gone.
pub const CONNECTION_CLOSED: &str = "the connection closed; the wait ended";

// ----- arguments -----

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(crate = "rmcp::schemars")]
pub struct CancelJobToolArgs {
    /// The job's id (from list_jobs). Only a job with origin mcp can be cancelled.
    #[schemars(length(min = 1, max = 64))]
    pub job_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(crate = "rmcp::schemars")]
pub struct CreateGroupToolArgs {
    /// The group's name: at most 200 characters, not blank; stored trimmed.
    #[schemars(length(min = 1, max = 200))]
    pub name: String,
    /// The parent group's id (from list_groups); omit it for a top-level group.
    #[schemars(length(min = 1, max = 64))]
    pub parent_id: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(crate = "rmcp::schemars")]
pub struct WaitForJobsToolArgs {
    /// Each job id (at most 64 characters) with the status you last saw: draft, queued, running,
    /// cancelling, completed, parsed, failed, cancelled or lost. 1 to 50 jobs.
    #[schemars(extend(
        "minProperties" = 1,
        "maxProperties" = 50,
        "propertyNames" = { "minLength": 1, "maxLength": 64 },
        "additionalProperties" = {
            "type": "string",
            "enum": ["draft", "queued", "running", "cancelling", "completed", "parsed", "failed", "cancelled", "lost"]
        }
    ))]
    pub known: BTreeMap<String, String>,
    /// The longest wait, in seconds (default 600; a larger value is capped at 600; 0 checks once).
    pub timeout_s: Option<u64>,
}

// ----- cancel_job's world -----

/// The dispatch `cancel_job_core` calls on its guarded path: the backend's cancel for the job it read.
pub type Dispatch<'a> = &'a dyn Fn(Job) -> Result<(), AppError>;

/// A cancel call, run inside a [`CancelWorld`] with its poller and dispatch.
pub type CancelCall<'a> = &'a mut dyn FnMut(&Poller<'_>, Dispatch<'_>) -> Result<CancelJobResult, ToolError>;

/// What `cancel_job` runs in: the app's poller and the backend's cancel. The app's is
/// `super::wiring::AppCancelWorld`; a test passes a scratch poller and a recording dispatch.
pub trait CancelWorld: Send + Sync + 'static {
    /// Run `call` with the poller and the dispatch, or refuse (the app's state is not ready yet).
    fn run(&self, call: CancelCall<'_>) -> Result<CancelJobResult, ToolError>;
}

/// `cancel_job` at the tool layer: the core ([`run_cancel_job`]: the origin and `Draft` pre-check
/// with its row, the cancel core, the outcome row) inside `world`. Blocking.
pub fn cancel_in(world: &dyn CancelWorld, args: &CancelJobArgs) -> Result<CancelJobResult, ToolError> {
    world.run(&mut |poller, dispatch| run_cancel_job(poller, args, dispatch))
}

/// `groups:changed`, isolated from a panic: the group is already committed, so a failing emit costs
/// one stderr line and never the tool's answer.
pub fn announce_groups_changed(emit: &dyn StatusEmit) {
    if catch_unwind(AssertUnwindSafe(|| emit.groups_changed())).is_err() {
        eprintln!("[OrcaStudio] mcp: the groups:changed emit panicked; create_group answers anyway");
    }
}

/// A wait's end as the tool's answer: the result, or the (unread) closed error.
pub fn wait_answer(end: Result<WaitEnd, ToolError>) -> Result<WaitResult, ToolError> {
    match end? {
        WaitEnd::Answered(result) => Ok(result),
        WaitEnd::ConnectionClosed => Err(ToolError::refused(CONNECTION_CLOSED)),
    }
}

// ----- the tools -----

#[tool_router(router = m3b_tools_router, vis = "pub(super)")]
impl McpService {
    #[tool(description = CANCEL_JOB_DESCRIPTION)]
    async fn cancel_job(&self, Parameters(a): Parameters<CancelJobToolArgs>) -> Result<CallToolResult, ErrorData> {
        let world = self.write_wiring.cancel.clone();
        let args = CancelJobArgs { job_id: a.job_id };
        Ok(respond("cancel_job", on_blocking_pool(move || cancel_in(&*world, &args)).await))
    }

    #[tool(description = CREATE_GROUP_DESCRIPTION)]
    async fn create_group(&self, Parameters(a): Parameters<CreateGroupToolArgs>) -> Result<CallToolResult, ErrorData> {
        let args = CreateGroupArgs { name: a.name, parent_id: a.parent_id };
        let emit = self.write_wiring.env.emit.clone();
        let created = self.with_db(move |c| run_create_group(c, &args)).await;
        // `with_db` has returned: the transaction is committed and `DbState` released. Only a created
        // group is announced.
        if created.is_ok() {
            announce_groups_changed(&*emit);
        }
        Ok(respond("create_group", created))
    }

    #[tool(description = WAIT_FOR_JOBS_DESCRIPTION)]
    async fn wait_for_jobs(&self, Parameters(a): Parameters<WaitForJobsToolArgs>) -> Result<CallToolResult, ErrorData> {
        let args = WaitArgs { known: a.known, timeout_s: a.timeout_s };
        let reader = DbStatusReader(self.db.clone());
        let end = wait_for_jobs(&args, &self.conn.closed, &reader, &TokioClock::new(), WAIT_TICK).await;
        Ok(respond("wait_for_jobs", wait_answer(end)))
    }
}
