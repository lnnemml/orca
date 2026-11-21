//! The MCP service one connection runs (`rmcp`), and that connection's state.
//!
//! - **The request cap** (M0 amendment, item 5). Each connection holds a semaphore of
//!   [`REQUESTS_PER_CONNECTION`] permits. A tool call takes one with `try_acquire`: when none is
//!   free it gets the tool error [`BUSY`] at once and never queues. The permit returns when the
//!   call's work ends. `create_job` is the one exception in `call_tool` ([`OWN_PERMIT_TOOLS`]): its
//!   handler takes its own permit and moves it into its tracked task (`super::create_job_task`), so
//!   `call_tool` takes none for it and a call holds exactly one.
//! - **`closed`** is set the moment the connection's transport reaches read EOF (the listener's
//!   `Transport` wrapper; `rmcp`'s own end comes up to 5 s later, after its response drain), and by
//!   switch-off. Waiting and reading handlers (M3's `wait_for_jobs`) check it; `rmcp` does not
//!   cancel a handler when the transport closes (M0 amendment, items 6 and 10).
//! - **Unknown methods** such as `server/discover` get a JSON-RPC error and the connection stays
//!   open: `rmcp` does this itself (re-measured here: `server_tests::an_unknown_method_*`).
//! - **The recovery sentence** ([`RECOVERY_SENTENCE`]) is in the server's instructions and in every
//!   tool description, from one constant.
//! - **The tools** are `get_grant` (here), the ten read tools ([`super::read_tools`]) and
//!   `create_job` ([`super::create_job_tool`]), one router.
//!   A tool error's text is bounded on its way out ([`bound_errors`]): `rmcp`'s own argument error
//!   quotes the caller's input, so it is cut like an echo.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::tool::ToolCallContext;
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, Implementation, ServerCapabilities,
    ServerConfig,
};
use rmcp::service::RequestContext;
use rmcp::{tool, tool_handler, tool_router, ErrorData, RoleServer, ServerHandler};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use super::create_job_task::CreateJobWiring;
use super::{DbSource, REQUESTS_PER_CONNECTION};
use crate::commands::mcp::current_grant_core;
use crate::mcp::tools::{clip, ECHO_MAX_BYTES, REFUSAL_MAX_BYTES};

/// The recovery after an app restart (M0 amendment, item 3), measured: the client does not
/// dependably reconnect on its own. A macro so `concat!` can put the same literal into the
/// instructions and every tool description; [`RECOVERY_SENTENCE`] is its one value.
macro_rules! recovery_sentence {
    () => {
        "After an app restart, start a new client session."
    };
}

pub(super) use recovery_sentence;

/// The recovery sentence; every tool description and the server instructions carry it.
pub const RECOVERY_SENTENCE: &str = recovery_sentence!();

/// The server's instructions (the `initialize` result).
pub const INSTRUCTIONS: &str = concat!(
    "OrcaStudio's MCP server: read access to the app's state, and create_job, which spends compute ",
    "only under a grant Anton made. Jobs, grants and the audit log live in OrcaStudio's database and ",
    "survive a restart of the app or of this session. ",
    recovery_sentence!()
);

/// The tools whose handler takes its own request permit (to move it into a tracked task):
/// `call_tool` takes none for them, so each call holds exactly one.
pub const OWN_PERMIT_TOOLS: [&str; 1] = ["create_job"];

/// Whether `call_tool` leaves the permit to the tool's own handler ([`OWN_PERMIT_TOOLS`]).
pub fn takes_own_permit(tool: &str) -> bool {
    OWN_PERMIT_TOOLS.contains(&tool)
}

/// The tool error for a call over the connection's request cap.
pub const BUSY: &str = "busy, retry: every request slot of this connection is in use";

/// How `rmcp` begins the tool error for arguments that do not deserialise
/// (`rmcp::handler::server::tool`, 3.11.0). The rest is `serde_json`'s message, which quotes the
/// offending input (an unknown field's name, a string given for a number).
pub const ARGUMENT_ERROR_PREFIX: &str = "failed to deserialize parameters:";

const GET_GRANT_DESCRIPTION: &str = concat!(
    "The current compute grant, as Settings → MCP shows it: its state (active, expired or revoked), ",
    "targets, granted method levels, the job and in-flight limits, and the jobs charged so far. ",
    "Returns {\"grant\": null} before any grant has been made. ",
    recovery_sentence!()
);

/// One connection's state, shared by its service, its serve task and the lifecycle.
#[derive(Debug)]
pub struct ConnState {
    /// The transport has ended, or MCP was switched off.
    pub closed: AtomicBool,
    permits: Arc<Semaphore>,
}

impl ConnState {
    pub fn new() -> Self {
        ConnState { closed: AtomicBool::new(false), permits: Arc::new(Semaphore::new(REQUESTS_PER_CONNECTION)) }
    }

    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }

    /// A request permit, or `None` when all are in use (the call is refused [`BUSY`]) or the
    /// connection has drained.
    pub fn try_permit(&self) -> Option<OwnedSemaphorePermit> {
        self.permits.clone().try_acquire_owned().ok()
    }

    /// Wait until every permit has returned, then refuse any later request. The connection's slot
    /// is released only after this (M0 amendment, item 5: "a connection's slot frees only when its
    /// last permit has returned").
    pub async fn drain(&self) {
        // `acquire_many` fails only on a closed semaphore, which is already drained.
        let all = self.permits.acquire_many(REQUESTS_PER_CONNECTION as u32).await;
        // Closed while every permit is held here, so no later request can slip in; the permits
        // then return, and `in_flight` reads 0.
        self.permits.close();
        drop(all);
    }

    /// Permits not available: held by running calls, or, once the connection drains, gathered by the
    /// drain while it waits for the rest. Reads 0 after the drain.
    pub fn in_flight(&self) -> usize {
        REQUESTS_PER_CONNECTION - self.permits.available_permits()
    }
}

impl Default for ConnState {
    fn default() -> Self {
        Self::new()
    }
}

/// The host facts `memory_estimate` reads besides the database: `/proc/meminfo` and the app's data
/// dir (the disk the local job directories live on). Injected, so a test can point them at a
/// scratch directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostPaths {
    pub meminfo: PathBuf,
    pub data_dir: PathBuf,
}

impl HostPaths {
    /// The app's: the real `/proc/meminfo`, and the data dir `JobRunner` is built over.
    pub fn production(data_dir: &Path) -> HostPaths {
        HostPaths { meminfo: PathBuf::from(crate::commands::memory::PROC_MEMINFO), data_dir: data_dir.to_path_buf() }
    }
}

/// The service one connection runs.
#[derive(Clone)]
pub struct McpService {
    tool_router: ToolRouter<Self>,
    pub(super) conn: Arc<ConnState>,
    pub(super) db: Arc<dyn DbSource>,
    pub(super) host: Arc<HostPaths>,
    pub(super) create_job_wiring: CreateJobWiring,
}

#[tool_router]
impl McpService {
    pub fn new(conn: Arc<ConnState>, db: Arc<dyn DbSource>, host: Arc<HostPaths>, create_job_wiring: CreateJobWiring) -> Self {
        McpService {
            tool_router: Self::tool_router() + Self::read_tools_router() + Self::create_job_router(),
            conn,
            db,
            host,
            create_job_wiring,
        }
    }

    /// Every tool this server lists, for the description check.
    pub fn tools(&self) -> Vec<rmcp::model::Tool> {
        self.tool_router.list_all()
    }

    #[tool(description = GET_GRANT_DESCRIPTION)]
    async fn get_grant(&self) -> Result<CallToolResult, ErrorData> {
        let db = self.db.clone();
        // DB work off the async workers; `DbState` is locked for this one read and never across an
        // `.await` (decision 1; M2 amendment, item 5).
        let read = tauri::async_runtime::spawn_blocking(move || {
            let conn = db.db().lock()?;
            current_grant_core(&conn)
        })
        .await;
        Ok(match read {
            Ok(Ok(grant)) => {
                let json = serde_json::json!({ "grant": grant });
                CallToolResult::success(vec![ContentBlock::text(json.to_string())])
            }
            Ok(Err(e)) => CallToolResult::error(vec![ContentBlock::text(format!("get_grant failed: {e}"))]),
            Err(e) => CallToolResult::error(vec![ContentBlock::text(format!("get_grant task failed: {e}"))]),
        })
    }
}

// The combined router (`get_grant`, the read tools and `create_job`), not the macro's default
// `Self::tool_router()`, which lists `get_grant` alone.
#[tool_handler(router = self.tool_router)]
impl ServerHandler for McpService {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("orcastudio", env!("CARGO_PKG_VERSION")))
            .with_instructions(INSTRUCTIONS)
    }

    /// Every tool call takes one of the connection's permits first, and holds it until the call's
    /// work ends. None free: the immediate tool error [`BUSY`], never a queue. A tool in
    /// [`OWN_PERMIT_TOOLS`] (`create_job`) takes its own in its handler, so none is taken here.
    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let _permit = if takes_own_permit(&request.name) {
            None
        } else {
            let Some(permit) = self.conn.try_permit() else {
                return Ok(CallToolResult::error(vec![ContentBlock::text(BUSY)]).into());
            };
            Some(permit)
        };
        let call = ToolCallContext::new(self, request, context);
        Ok(bound_errors(self.tool_router.call(call).await?))
    }
}

/// Every tool error's text, bounded: `rmcp`'s argument error keeps its prefix and at most
/// [`ECHO_MAX_BYTES`] of `serde_json`'s message (which quotes the input), and any error text is cut
/// to [`REFUSAL_MAX_BYTES`]. A successful result passes unchanged (the read tools cap their own).
pub fn bound_errors(response: CallToolResponse) -> CallToolResponse {
    let CallToolResponse::Complete(mut result) = response else { return response };
    if result.is_error == Some(true) {
        for block in &mut result.content {
            if let ContentBlock::Text(t) = block {
                t.text = bounded_error_text(&t.text);
            }
        }
    }
    CallToolResponse::Complete(result)
}

/// See [`bound_errors`].
pub fn bounded_error_text(text: &str) -> String {
    let text = match text.strip_prefix(ARGUMENT_ERROR_PREFIX) {
        Some(rest) => format!("{ARGUMENT_ERROR_PREFIX} {}", clip(rest.trim_start(), ECHO_MAX_BYTES).0),
        None => text.to_string(),
    };
    clip(&text, REFUSAL_MAX_BYTES).0
}
