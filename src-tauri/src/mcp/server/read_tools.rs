//! The ten read tools registered in the `rmcp` service (ADR-027 decision 7; M2 amendment, items 1,
//! 7 and 8; unit M2b Part B). Each tool is a thin shell over its core in [`crate::mcp::tools`]: typed
//! arguments, the core's call with the right lock, and [`respond`].
//!
//! **Arguments.** Each tool takes one struct with `deny_unknown_fields`, so a misspelt or unknown
//! argument is a tool error, never silently ignored; `list_run_targets` takes the empty
//! [`NoArgs`]. The schema bounds every string and integer the core bounds (the core still clamps or
//! refuses on its own; the schema is advice to the client).
//!
//! **Threads and the lock** (decision 1; M2 amendment, item 5; `tauri-core.md`'s threading rule).
//! Every core runs on Tauri's blocking pool ([`on_blocking_pool`]), never on an async worker:
//! - a database read locks `DbState` **once**, for that read, inside the blocking task
//!   ([`McpService::with_db`]); no guard lives across an `.await`;
//! - `memory_estimate` takes no lock here: its core locks `DbState` itself (twice, briefly);
//! - `read_output` locks only to read the job's `output.out` path ([`output_path`]); the guard is
//!   dropped before the file is read ([`read_output_at`]);
//! - `get_geometry` and `measure` on a `scan:`, `neb:` or `goat:` source hold the lock across the
//!   resolver's bounded, size-capped file reads of that job's directory, as the app's own commands
//!   that read them do. That is allowed: only an `.await` under the lock is forbidden.
//!
//! **What the client receives** ([`respond`]). A result is the core's value as compact JSON, every
//! field kept (`withheld_fields`, the truncation flags), and checked once more against
//! [`RESULT_CAP_BYTES`] as it leaves (rule #9 at the process boundary): over it, the call is a
//! tool error, never served. A [`ToolError`] is a tool error whose text is the error's own message
//! (the cores already echo input cut to 200 bytes and cut a refusal to 600), cut to
//! [`REFUSAL_MAX_BYTES`] again; it never adds the arguments.

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ContentBlock};
use rmcp::schemars::JsonSchema;
use rmcp::{tool, tool_router, ErrorData};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use super::service::{recovery_sentence, McpService};
use crate::commands::settings::DbState;
use crate::error::AppError;
use crate::mcp::tools::geometry::{get_geometry, measure};
use crate::mcp::tools::groups::list_groups;
use crate::mcp::tools::jobs::{get_job, list_jobs, JobFilter};
use crate::mcp::tools::manual::search_manual;
use crate::mcp::tools::memory::memory_estimate;
use crate::mcp::tools::output::{output_path, read_output_at, OutputRequest};
use crate::mcp::tools::results::get_results;
use crate::mcp::tools::targets::list_run_targets;
use crate::mcp::tools::{clip, result_text, size_of_text, ToolError, REFUSAL_MAX_BYTES, RESULT_CAP_BYTES};

// ----- descriptions -----

const LIST_JOBS: &str = concat!(
    "OrcaStudio's jobs, newest first, one page at a time: id, title, status, origin (user or mcp), ",
    "group, target profile, times (UTC), final energy (Eh) and an error excerpt. Filter by status, ",
    "origin or group_id; page with offset and limit (default 20, at most 40); next_offset is the next ",
    "page's offset. A field holding coordinate-like text is withheld and named in withheld_fields. ",
    recovery_sentence!()
);

const GET_JOB: &str = concat!(
    "One job: status, origin, times (UTC), energy (Eh), wall time (s), its input header, atom count, ",
    "charge and multiplicity with their source, notices, error, MCP reasons, grant_id and mcp_state. ",
    "The header is rebuilt from the parsed input, never copied from it: values outside the display ",
    "allowlist are not shown, geometry appears only as markers, and the coordinate block is never ",
    "returned. A field holding coordinate-like text is withheld and named in withheld_fields. ",
    recovery_sentence!()
);

const GET_RESULTS: &str = concat!(
    "A bounded summary of a job's parsed results; every number names its unit in its field name: ",
    "final energy (Eh), converged, imaginary modes (cm-1, IR km/mol), thermochemistry (Eh, K), the ",
    "relaxed-scan profile (Å or degrees, Eh) and the NEB summary (Å, Eh). Each list is capped and ",
    "states its full count; parsed: false says why there are no results. ",
    recovery_sentence!()
);

const GET_GEOMETRY: &str = concat!(
    "A stored geometry as xyz text (Å), with charge, multiplicity and their source. source is ",
    "job:<id>, job:<id>/scan:<k>, job:<id>/neb:<k|ts>, job:<id>/goat:<k> or molecule:<id>; a job ",
    "source needs a job that finished successfully. Atom indices here and in measure are 0-based ",
    "(the first atom is 0). Refused above 600 atoms: use measure instead. ",
    recovery_sentence!()
);

const MEASURE: &str = concat!(
    "A distance (Å), angle (degrees, 0 to 180, the middle index is the vertex) or dihedral (degrees, ",
    "0 to 360) on a stored geometry, by 2, 3 or 4 atom indices, 0-based (the first atom is 0), in ",
    "ASE's convention. Quote this number rather than computing it. Degenerate input (an index out of ",
    "range, a repeated index, coincident atoms, collinear atoms in a dihedral) is refused. source as ",
    "in get_geometry. ",
    recovery_sentence!()
);

const READ_OUTPUT: &str = concat!(
    "A bounded tail or search of a job's output.out, never the whole file. Without query: the last ",
    "lines (default 100, at most 400, from the last 256 KiB). With query: a search, a literal ",
    "substring or a regular expression (regex: true), 20 matches by default and at most 50, up to 3 ",
    "context lines, paged with offset. Long lines are cut, and the result says when it cut or ",
    "stopped. ",
    recovery_sentence!()
);

const SEARCH_MANUAL: &str = concat!(
    "Full-text search of the indexed ORCA manual: each hit's section_id, file, anchor, title, ",
    "breadcrumb and a snippet with the match between « and ». 10 hits by default, at most 20; ",
    "bm25_score is dimensionless, lower is a better match. ",
    recovery_sentence!()
);

const LIST_RUN_TARGETS: &str = concat!(
    "Where a job can run: this machine (\"local\") and each server profile, with whether it is a ",
    "run target and why not, its availability window (HH:MM-HH:MM, the laptop's local time) and ",
    "whether now is inside it. Reachability is \"unknown\" for every server. ",
    recovery_sentence!()
);

const MEMORY_ESTIMATE: &str = concat!(
    "The memory and disk check a job with this input header would get on a target (\"local\" or a ",
    "server profile id from list_run_targets), with the %maxcore the app would set (MiB). Leave ",
    "%maxcore out of the header: the app sets it. verdict is Fits only when every fact is known and ",
    "nothing warns. ",
    recovery_sentence!()
);

const LIST_GROUPS: &str = concat!(
    "Job groups in creation order, one page at a time (default 50, at most 100): id, name, ",
    "parent_id (null at the root) and creation time (UTC). ",
    recovery_sentence!()
);

// ----- arguments -----

/// No arguments; an unknown one is refused.
#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(crate = "rmcp::schemars")]
pub struct NoArgs {}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(crate = "rmcp::schemars")]
pub struct ListJobsArgs {
    /// Only jobs in this status: draft, queued, running, cancelling, completed, parsed, failed,
    /// cancelled or lost.
    #[schemars(length(max = 16))]
    pub status: Option<String>,
    /// Only jobs of this origin: user or mcp.
    #[schemars(length(max = 8))]
    pub origin: Option<String>,
    /// Only jobs in this group (an id from list_groups).
    #[schemars(length(max = 200))]
    pub group_id: Option<String>,
    /// Rows to skip (default 0): the previous page's next_offset.
    pub offset: Option<u64>,
    /// Rows in this page (default 20, at most 40).
    #[schemars(range(min = 1, max = 40))]
    pub limit: Option<u32>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(crate = "rmcp::schemars")]
pub struct JobIdArgs {
    /// The job's id (from list_jobs).
    #[schemars(length(max = 200))]
    pub job_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(crate = "rmcp::schemars")]
pub struct GeometryArgs {
    /// job:<id>, job:<id>/scan:<k>, job:<id>/neb:<k|ts>, job:<id>/goat:<k> or molecule:<id>.
    #[schemars(length(max = 200))]
    pub source: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(crate = "rmcp::schemars")]
pub struct MeasureArgs {
    /// As in get_geometry.
    #[schemars(length(max = 200))]
    pub source: String,
    /// 2, 3 or 4 atom indices, 0-based: a distance, an angle (the middle one the vertex) or a
    /// dihedral.
    #[schemars(length(min = 2, max = 4))]
    pub atom_indices: Vec<usize>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(crate = "rmcp::schemars")]
pub struct ReadOutputArgs {
    /// The job's id (from list_jobs).
    #[schemars(length(max = 200))]
    pub job_id: String,
    /// Tail only: how many last lines (default 100, at most 400).
    #[schemars(range(min = 1, max = 400))]
    pub lines: Option<usize>,
    /// Search for this text; omit it for the tail. At most 200 characters.
    #[schemars(length(min = 1, max = 200))]
    pub query: Option<String>,
    /// Search only: treat query as a regular expression (default false: a literal substring).
    pub regex: Option<bool>,
    /// Search only: match case (default false).
    pub case_sensitive: Option<bool>,
    /// Search only: context lines on each side of a match (default 0, at most 3).
    #[schemars(range(max = 3))]
    pub context_lines: Option<usize>,
    /// Search only: matches to skip (default 0): the previous page's next_offset.
    pub offset: Option<usize>,
    /// Search only: matches in this page (default 20, at most 50).
    #[schemars(range(min = 1, max = 50))]
    pub limit: Option<usize>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(crate = "rmcp::schemars")]
pub struct SearchManualArgs {
    /// Words to search for (SQLite FTS5); at most 500 characters.
    #[schemars(length(max = 500))]
    pub query: String,
    /// Hits to return (default 10, at most 20).
    #[schemars(range(min = 1, max = 20))]
    pub limit: Option<usize>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(crate = "rmcp::schemars")]
pub struct MemoryEstimateArgs {
    /// The input header: the ! lines and % blocks, without coordinates and without %maxcore. At
    /// most 20,000 characters.
    #[schemars(length(max = 20000))]
    pub header: String,
    /// "local" or a server profile id (from list_run_targets).
    #[schemars(length(max = 200))]
    pub target: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(crate = "rmcp::schemars")]
pub struct ListGroupsArgs {
    /// Groups to skip (default 0): the previous page's next_offset.
    pub offset: Option<usize>,
    /// Groups in this page (default 50, at most 100).
    #[schemars(range(min = 1, max = 100))]
    pub limit: Option<usize>,
}

/// `read_output`'s request: the tail without `query`, a search with it. A search-only argument with
/// no `query`, or `lines` with one, is refused rather than ignored.
pub fn output_request(a: &ReadOutputArgs) -> Result<OutputRequest, ToolError> {
    match &a.query {
        None => {
            let search_only = [
                ("regex", a.regex.is_some()),
                ("case_sensitive", a.case_sensitive.is_some()),
                ("context_lines", a.context_lines.is_some()),
                ("offset", a.offset.is_some()),
                ("limit", a.limit.is_some()),
            ];
            let given: Vec<&str> = search_only.iter().filter(|(_, set)| *set).map(|(name, _)| *name).collect();
            if !given.is_empty() {
                return Err(ToolError::refused(format!("{} apply to a search only; give query, or leave them out for the tail", given.join(", "))));
            }
            Ok(OutputRequest::Tail { lines: a.lines })
        }
        Some(query) => {
            if a.lines.is_some() {
                return Err(ToolError::refused("lines applies to the tail only; leave it out of a search, or leave out query"));
            }
            Ok(OutputRequest::Search {
                query: query.clone(),
                regex: a.regex.unwrap_or(false),
                case_sensitive: a.case_sensitive.unwrap_or(false),
                context_lines: a.context_lines,
                offset: a.offset,
                limit: a.limit,
            })
        }
    }
}

// ----- the mapping -----

/// A core's answer as the client receives it: the value as compact JSON, or a tool error. See the
/// module docs.
pub fn respond<T: Serialize>(tool: &str, answer: Result<T, ToolError>) -> CallToolResult {
    match answer.and_then(|value| served_text(&value)) {
        Ok(text) => CallToolResult::success(vec![ContentBlock::text(text)]),
        Err(e) => CallToolResult::error(vec![ContentBlock::text(error_text(tool, &e))]),
    }
}

/// The value's compact JSON, checked against [`RESULT_CAP_BYTES`] as it is served: the cores check
/// it too, and this is the post-condition at the boundary the client sees.
pub fn served_text<T: Serialize>(value: &T) -> Result<String, ToolError> {
    let text = result_text(value)?;
    let bytes = size_of_text(&text);
    if bytes > RESULT_CAP_BYTES {
        return Err(ToolError::TooLarge { bytes, cap: RESULT_CAP_BYTES });
    }
    Ok(text)
}

/// A tool error's text: the error's own message (an app fault named as one), cut to
/// [`REFUSAL_MAX_BYTES`].
pub fn error_text(tool: &str, e: &ToolError) -> String {
    let text = match e {
        ToolError::App(app) => format!("{tool} failed: {app}"),
        other => other.to_string(),
    };
    clip(&text, REFUSAL_MAX_BYTES).0
}

// ----- threads and the lock -----

/// Run `work` on Tauri's blocking pool, never on an async worker, and wait for it.
pub async fn on_blocking_pool<T: Send + 'static>(work: impl FnOnce() -> Result<T, ToolError> + Send + 'static) -> Result<T, ToolError> {
    tauri::async_runtime::spawn_blocking(work)
        .await
        .map_err(|e| ToolError::App(AppError::Internal(format!("the tool's task failed: {e}"))))?
}

fn lock(db: &DbState) -> Result<std::sync::MutexGuard<'_, Connection>, ToolError> {
    db.lock().map_err(ToolError::App)
}

impl McpService {
    /// `read` with `DbState` locked once, on the blocking pool. The guard drops when `read` returns,
    /// inside the blocking task: never held across an `.await`.
    async fn with_db<T: Send + 'static>(
        &self,
        read: impl FnOnce(&Connection) -> Result<T, ToolError> + Send + 'static,
    ) -> Result<T, ToolError> {
        let db = self.db.clone();
        on_blocking_pool(move || read(&*lock(db.db())?)).await
    }

    /// `read_output`: the output path under the lock, then the bounded file read after it is
    /// released.
    async fn output(&self, a: ReadOutputArgs) -> Result<crate::mcp::tools::output::OutputView, ToolError> {
        let request = output_request(&a)?;
        let db = self.db.clone();
        on_blocking_pool(move || {
            let path = {
                let conn = lock(db.db())?;
                output_path(&conn, &a.job_id)?
            };
            read_output_at(&a.job_id, path.as_deref().map_err(String::as_str), &request)
        })
        .await
    }

    /// `memory_estimate`: no lock here; the core locks `DbState` itself.
    async fn estimate(&self, a: MemoryEstimateArgs) -> Result<crate::mcp::tools::memory::MemoryEstimateView, ToolError> {
        let (db, host) = (self.db.clone(), self.host.clone());
        on_blocking_pool(move || memory_estimate(db.db(), &host.meminfo, &host.data_dir, &a.header, &a.target)).await
    }
}

// ----- the tools -----

#[tool_router(router = read_tools_router, vis = "pub(super)")]
impl McpService {
    #[tool(description = LIST_JOBS)]
    async fn list_jobs(&self, Parameters(a): Parameters<ListJobsArgs>) -> Result<CallToolResult, ErrorData> {
        let filter = JobFilter { status: a.status, origin: a.origin, group_id: a.group_id };
        let (offset, limit) = (a.offset, a.limit);
        Ok(respond("list_jobs", self.with_db(move |c| list_jobs(c, &filter, offset, limit)).await))
    }

    #[tool(description = GET_JOB)]
    async fn get_job(&self, Parameters(a): Parameters<JobIdArgs>) -> Result<CallToolResult, ErrorData> {
        Ok(respond("get_job", self.with_db(move |c| get_job(c, &a.job_id)).await))
    }

    #[tool(description = GET_RESULTS)]
    async fn get_results(&self, Parameters(a): Parameters<JobIdArgs>) -> Result<CallToolResult, ErrorData> {
        Ok(respond("get_results", self.with_db(move |c| get_results(c, &a.job_id)).await))
    }

    #[tool(description = GET_GEOMETRY)]
    async fn get_geometry(&self, Parameters(a): Parameters<GeometryArgs>) -> Result<CallToolResult, ErrorData> {
        Ok(respond("get_geometry", self.with_db(move |c| get_geometry(c, &a.source)).await))
    }

    #[tool(description = MEASURE)]
    async fn measure(&self, Parameters(a): Parameters<MeasureArgs>) -> Result<CallToolResult, ErrorData> {
        Ok(respond("measure", self.with_db(move |c| measure(c, &a.source, &a.atom_indices)).await))
    }

    #[tool(description = READ_OUTPUT)]
    async fn read_output(&self, Parameters(a): Parameters<ReadOutputArgs>) -> Result<CallToolResult, ErrorData> {
        Ok(respond("read_output", self.output(a).await))
    }

    #[tool(description = SEARCH_MANUAL)]
    async fn search_manual(&self, Parameters(a): Parameters<SearchManualArgs>) -> Result<CallToolResult, ErrorData> {
        Ok(respond("search_manual", self.with_db(move |c| search_manual(c, &a.query, a.limit)).await))
    }

    #[tool(description = LIST_RUN_TARGETS)]
    async fn list_run_targets(&self, Parameters(_): Parameters<NoArgs>) -> Result<CallToolResult, ErrorData> {
        Ok(respond("list_run_targets", self.with_db(list_run_targets).await))
    }

    #[tool(description = MEMORY_ESTIMATE)]
    async fn memory_estimate(&self, Parameters(a): Parameters<MemoryEstimateArgs>) -> Result<CallToolResult, ErrorData> {
        Ok(respond("memory_estimate", self.estimate(a).await))
    }

    #[tool(description = LIST_GROUPS)]
    async fn list_groups(&self, Parameters(a): Parameters<ListGroupsArgs>) -> Result<CallToolResult, ErrorData> {
        Ok(respond("list_groups", self.with_db(move |c| list_groups(c, a.offset, a.limit)).await))
    }
}
