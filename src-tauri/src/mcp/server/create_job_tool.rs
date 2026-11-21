//! `create_job` registered in the `rmcp` service (ADR-027 decisions 3–5 and 7; M3 amendment, item 1;
//! unit M3a Part B): a thin shell over [`create_job_call`], like the read tools over their cores.
//!
//! - **Arguments**: [`CreateJobToolArgs`], with `deny_unknown_fields`, so a misspelt argument (or a
//!   `coordinates`, `charge` or `multiplicity` the model tries to pass) is a tool error, never
//!   ignored. The schema carries the core's bounds; the core refuses over them on its own
//!   (`create_job::check_args`).
//! - **The permit**: `call_tool` takes none for `create_job` (`service::OWN_PERMIT_TOOLS`);
//!   [`create_job_call`] takes one and moves it into the tracked task.
//! - **What the client receives**: the result through [`respond`] (compact JSON, the 40,000-byte cap
//!   checked as it leaves), an error through the same bounded mapping as every tool.

use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::schemars::JsonSchema;
use rmcp::{tool, tool_router, ErrorData};
use serde::Deserialize;

use super::create_job_task::create_job_call;
use super::read_tools::respond;
use super::service::{recovery_sentence, McpService};
use crate::mcp::create_job::CreateJobArgs;

/// `create_job`'s description: what the app does with the call, the three results, and the grant.
pub const CREATE_JOB_DESCRIPTION: &str = concat!(
    "Create an ORCA job. The app assembles the input from the source: your header (the ! lines and % ",
    "blocks, without %maxcore) plus the source's geometry, charge and multiplicity. No coordinates are ",
    "accepted. source is job:<id> (or job:<id>/scan:<k>, /neb:<k|ts>, /goat:<k>), molecule:<id> or ",
    "smiles:<s>; target is \"local\" or a server profile id from list_run_targets. The result is ",
    "SUBMITTED (handed to the target), PROPOSED (a draft Anton runs) or REFUSED, each with its ",
    "reasons. A grant Anton made governs spending: without an active grant covering the target and ",
    "the method, nothing is submitted. ",
    recovery_sentence!()
);

/// `create_job`'s arguments (decision 4: no coordinates, charge or multiplicity).
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(crate = "rmcp::schemars")]
pub struct CreateJobToolArgs {
    /// The ! lines and % blocks of the input, without %maxcore (the app sets it) and without
    /// coordinates. At most 20,000 bytes.
    #[schemars(length(min = 1, max = 20000))]
    pub header: String,
    /// The geometry source: job:<id>, job:<id>/scan:<k>, job:<id>/neb:<k|ts>, job:<id>/goat:<k>,
    /// molecule:<id> or smiles:<s>. Its charge and multiplicity come with it.
    #[schemars(length(min = 1, max = 1024))]
    pub source: String,
    /// "local" or a server profile id (from list_run_targets).
    #[schemars(length(min = 1, max = 64))]
    pub target: String,
    /// The group to file the job under (an id from list_groups).
    #[schemars(length(min = 1, max = 64))]
    pub group_id: Option<String>,
    /// The job's title; by default "Claude: <source>".
    #[schemars(length(min = 1, max = 200))]
    pub title: Option<String>,
}

impl From<CreateJobToolArgs> for CreateJobArgs {
    fn from(a: CreateJobToolArgs) -> Self {
        CreateJobArgs { header: a.header, source: a.source, target: a.target, group_id: a.group_id, title: a.title }
    }
}

#[tool_router(router = create_job_router, vis = "pub(super)")]
impl McpService {
    #[tool(description = CREATE_JOB_DESCRIPTION)]
    async fn create_job(&self, Parameters(a): Parameters<CreateJobToolArgs>) -> Result<CallToolResult, ErrorData> {
        let wiring = &self.write_wiring;
        Ok(respond("create_job", create_job_call(&self.conn, &wiring.gate, wiring.env.clone(), a.into()).await))
    }
}
