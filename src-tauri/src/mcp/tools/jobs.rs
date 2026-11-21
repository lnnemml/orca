//! `list_jobs` and `get_job` (ADR-027 decision 7; M2 amendment, item 8).
//!
//! **Never a coordinate block (ADR-014 invariant 1).** The `Job` row carries `input_content` (with
//! its `* xyz` block) and `scene_json`. Neither tool serialises a `Job`: each builds a projection
//! field by field. `get_job`'s header is **built from M1a's parsed structure, never filtered from raw
//! text** ([`super::header_view`]; decided by Anton after two verifier FAILs on filtering). The atom
//! count and the charge and multiplicity come from the strict readers; every error the view builds
//! (the header's, the charge and multiplicity's, a coordinate row's) is a kind and a line, never
//! source text. The job's `error` field is not built here: it is ORCA's stored text, served under
//! the tripwire.
//!
//! **The rule-#9 tripwire is per field** (Anton, round 4): each text field (the title, the header, the
//! error, the reasons, the notices, …) is checked whole, before any cut, and replaced by [`WITHHELD`]
//! when it holds a coordinate-like row; `withheld_fields` names it and the rest is served. A failed
//! job's stored error (its stderr tail, or else the last 20 lines of `output.out`:
//! `local_backend::detect_completion`) is the common case. `list_jobs` guards its title and error
//! excerpt the same way, and has no backstop (its ids are app-minted v4 UUIDs). Behind `get_job`'s
//! guards, [`tripwire`] walks the serialised view and refuses the whole view if a row escaped them,
//! an ID field included.
//!
//! **Paging.** `list_jobs` reads one page of [`Job::COLUMNS`] rows, newest first (`created_at DESC,
//! id DESC`), with an `offset`; `next_offset` is the next page's offset, or `None` on the last page.
//! An offset page can repeat a row when a job is created between two calls, and skip one when a job
//! is deleted; `total_count` is the count at the time of the call.

use rusqlite::{params, Connection};
use serde::Serialize;

use super::{clip, echo, page_limit, result_text, within_cap, ToolError, RESULT_CAP_BYTES};
use crate::commands::jobs::get_job_conn;
use crate::mcp::assemble::read_coordinate_block;
use crate::mcp::charge_mult::locate_charge_mult;
use super::header_view::{coordinate_row, reconstruct, Field};
use crate::mcp::steps::McpReason;
use crate::models::job::{Job, JobOrigin, JobStatus};

/// `list_jobs`: the page size when the caller names none, and the largest allowed.
pub const LIST_JOBS_DEFAULT: u32 = 20;
pub const LIST_JOBS_MAX: u32 = 40;
/// A title in a list or a job view is cut to this many serialised bytes (as every `_BYTES` budget).
pub const TITLE_MAX_BYTES: usize = 120;
/// A list row's error is an excerpt of this many bytes.
pub const ERROR_EXCERPT_BYTES: usize = 240;
/// `get_job`: the built header, the error and each MCP reason are cut to these lengths, and at most
/// [`REASONS_MAX`] reasons are shown. A reason for a missing header or charge is cut to
/// [`WHY_MAX_BYTES`].
pub const HEADER_MAX_BYTES: usize = 6_000;
pub const ERROR_MAX_BYTES: usize = 4_000;
pub const REASON_MAX_BYTES: usize = 400;
pub const REASONS_MAX: usize = 20;
pub const WHY_MAX_BYTES: usize = 200;

/// `list_jobs`' filters; each is optional, and all given must match.
#[derive(Debug, Clone, Default)]
pub struct JobFilter {
    /// A job status (`draft`, `queued`, `running`, `cancelling`, `completed`, `parsed`, `failed`,
    /// `cancelled`, `lost`).
    pub status: Option<String>,
    /// `user` or `mcp`.
    pub origin: Option<String>,
    /// A group id.
    pub group_id: Option<String>,
}

/// One row of `list_jobs`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct JobSummary {
    pub id: String,
    pub title: String,
    pub title_truncated: bool,
    pub status: JobStatus,
    pub origin: JobOrigin,
    pub group_id: Option<String>,
    /// The server profile the job runs on; `None` is this machine.
    pub target_profile_id: Option<String>,
    pub created_at_utc: String,
    pub started_at_utc: Option<String>,
    pub completed_at_utc: Option<String>,
    /// The final energy (Eh): the parsed `results.final_energy_eh`, or the output's last
    /// `FINAL SINGLE POINT ENERGY` while unparsed (`set_job_results_conn`, `set_job_energy_conn`).
    pub energy_eh: Option<f64>,
    /// The start of the error message, if any.
    pub error_excerpt: Option<String>,
    pub error_truncated: bool,
    /// Fields replaced by [`WITHHELD`] because they held a coordinate-like row (rule #9).
    pub withheld_fields: Vec<String>,
}

/// One page of `list_jobs`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct JobsPage {
    pub jobs: Vec<JobSummary>,
    /// Jobs matching the filters, all pages.
    pub total_count: u64,
    pub offset: u64,
    pub limit: u32,
    /// The offset of the next page; `None` on the last page.
    pub next_offset: Option<u64>,
}

/// A status filter's value, or a refusal naming the values.
fn status_filter(s: &str) -> Result<JobStatus, ToolError> {
    JobStatus::from_db(s).map_err(|_| {
        let all: Vec<&str> = JobStatus::ALL.iter().map(JobStatus::as_str).collect();
        ToolError::refused(format!("status `{}` is not one of {}", echo(s), all.join(", ")))
    })
}

fn origin_filter(s: &str) -> Result<JobOrigin, ToolError> {
    JobOrigin::from_db(s).map_err(|_| ToolError::refused(format!("origin `{}` is not `user` or `mcp`", echo(s))))
}

fn summary_of(job: Job) -> JobSummary {
    let mut guard = Guard::default();
    let title = guard.text("title", job.title, Field::Text);
    let (title, title_truncated) = clip(&title, TITLE_MAX_BYTES);
    let (error_excerpt, error_truncated) = match guard.opt("error_excerpt", job.error_message, Field::Text) {
        Some(e) => {
            let (text, cut) = clip(&e, ERROR_EXCERPT_BYTES);
            (Some(text), cut)
        }
        None => (None, false),
    };
    JobSummary {
        id: job.id,
        title,
        title_truncated,
        status: job.status,
        origin: job.origin,
        group_id: job.group_id,
        target_profile_id: job.backend_id,
        created_at_utc: job.created_at,
        started_at_utc: job.started_at,
        completed_at_utc: job.completed_at,
        energy_eh: job.energy,
        error_excerpt,
        error_truncated,
        withheld_fields: guard.withheld,
    }
}

/// `list_jobs`: one page of the jobs matching `filter`, newest first.
pub fn list_jobs(conn: &Connection, filter: &JobFilter, offset: Option<u64>, limit: Option<u32>) -> Result<JobsPage, ToolError> {
    let status = filter.status.as_deref().map(status_filter).transpose()?.map(|s| s.as_str());
    let origin = filter.origin.as_deref().map(origin_filter).transpose()?.map(|o| o.as_str());
    let group = filter.group_id.as_deref();
    let offset = offset.unwrap_or(0);
    let limit = page_limit(limit, LIST_JOBS_DEFAULT, LIST_JOBS_MAX);
    let offset_sql = i64::try_from(offset).map_err(|_| ToolError::refused(format!("offset {offset} is too large")))?;

    const WHERE: &str = "(?1 IS NULL OR status = ?1) AND (?2 IS NULL OR origin = ?2) AND (?3 IS NULL OR group_id = ?3)";
    let total: i64 = conn
        .query_row(&format!("SELECT COUNT(*) FROM jobs WHERE {WHERE}"), params![status, origin, group], |r| r.get(0))
        .map_err(|e| ToolError::App(e.into()))?;
    let sql = format!("SELECT {} FROM jobs WHERE {WHERE} ORDER BY created_at DESC, id DESC LIMIT ?4 OFFSET ?5", Job::COLUMNS);
    let mut stmt = conn.prepare(&sql).map_err(|e| ToolError::App(e.into()))?;
    let jobs = stmt
        .query_map(params![status, origin, group, i64::from(limit), offset_sql], Job::from_row)
        .and_then(|rows| rows.collect::<rusqlite::Result<Vec<_>>>())
        .map_err(|e| ToolError::App(e.into()))?;

    let total_count = u64::try_from(total).unwrap_or(0);
    let end = offset + jobs.len() as u64;
    let page = JobsPage {
        jobs: jobs.into_iter().map(summary_of).collect(),
        total_count,
        offset,
        limit,
        next_offset: (end < total_count).then_some(end),
    };
    within_cap(page, RESULT_CAP_BYTES)
}

/// A charge and multiplicity as a tool shows them: the values, or why there are none, and always
/// where they come from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ChargeMultView {
    /// Elementary charges.
    pub charge: Option<i64>,
    pub multiplicity: Option<i64>,
    /// Where the values are read from.
    pub source: String,
    /// Why they could not be read; `None` when they were.
    pub error: Option<String>,
}

impl ChargeMultView {
    /// The view of a locator result. The error is its kind and line (`ChargeMultError::kind_and_line`),
    /// never its `Display`, which quotes the input's line.
    pub fn of(read: &Result<crate::mcp::charge_mult::ChargeMult, crate::mcp::charge_mult::ChargeMultError>, source: String) -> Self {
        match read {
            Ok(cm) => ChargeMultView { charge: Some(cm.charge), multiplicity: Some(cm.multiplicity), source, error: None },
            Err(e) => ChargeMultView {
                charge: None,
                multiplicity: None,
                source,
                error: Some(clip(&e.kind_and_line(), WHY_MAX_BYTES).0),
            },
        }
    }
}

/// An MCP reason as `get_job` shows it, its message cut to [`REASON_MAX_BYTES`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReasonView {
    pub row: String,
    pub message: String,
}

/// `get_job`'s projection of one job (decision 7). It has no field for the input's coordinate
/// block or the scene.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct JobView {
    pub id: String,
    pub title: String,
    pub title_truncated: bool,
    pub status: JobStatus,
    pub origin: JobOrigin,
    pub group_id: Option<String>,
    /// The server profile the job runs on; `None` is this machine.
    pub target_profile_id: Option<String>,
    /// The server host alias of a remote job (its stored coordinates, ADR-024); `None` for a local one.
    pub remote_host: Option<String>,
    pub created_at_utc: String,
    pub started_at_utc: Option<String>,
    pub completed_at_utc: Option<String>,
    /// See [`JobSummary::energy_eh`].
    pub energy_eh: Option<f64>,
    /// ORCA's `TOTAL RUN TIME`, seconds.
    pub wall_time_s: Option<f64>,
    /// The header built from the parsed structure ([`super::header_view`]): keywords, allowlisted
    /// values, other keys by name, markers for geometry and refused lines. `None` when the header
    /// cannot be separated from the coordinate block ([`JobView::header_withheld`] says why, as a
    /// kind and a line).
    pub header: Option<String>,
    pub header_truncated: bool,
    pub header_withheld: Option<String>,
    /// The rows of the input's `* xyz` block; `None` when the block is unreadable.
    pub atom_count: Option<usize>,
    pub charge_mult: ChargeMultView,
    /// The job's pending states, in words: a cancel requested or sent, a submit waiting for the
    /// server's slot, Claude submitting it.
    pub notices: Vec<String>,
    pub error: Option<String>,
    pub error_truncated: bool,
    /// Why an MCP job was proposed, or why its charge was refunded.
    pub mcp_reasons: Vec<ReasonView>,
    /// Reasons not shown past [`REASONS_MAX`], or why the stored reasons could not be read.
    pub mcp_reasons_note: Option<String>,
    pub grant_id: Option<String>,
    pub mcp_state: Option<String>,
    /// Fields replaced by [`WITHHELD`] because they held a coordinate-like row (rule #9).
    pub withheld_fields: Vec<String>,
}

/// What a field holding a coordinate-like row is replaced by.
pub const WITHHELD: &str = "withheld: coordinate-like content (rule #9)";

/// **The field-level tripwire** (rule #9; Anton, M2b round 4): a text field with a coordinate-like
/// row ([`coordinate_row`]) is replaced by [`WITHHELD`] and its name recorded; the rest of the view is
/// served. Checked on the whole text, before any cut. A failed job's error (the last lines of
/// `output.out`, with ORCA's `***` banners) is the common case.
#[derive(Debug, Default)]
struct Guard {
    withheld: Vec<String>,
}

impl Guard {
    fn text(&mut self, name: &str, text: String, field: Field) -> String {
        if text.lines().any(|l| coordinate_row(l, field)) {
            self.withheld.push(name.to_string());
            WITHHELD.to_string()
        } else {
            text
        }
    }

    fn opt(&mut self, name: &str, text: Option<String>, field: Field) -> Option<String> {
        text.map(|t| self.text(name, t, field))
    }
}

/// The backstop behind the field-level guard: every string of the serialised view, line by line,
/// through [`coordinate_row`] (the built header in [`Field::Header`] mode, everything else in
/// [`Field::Text`]). A hit means a field escaped its guard: the view is refused, never served.
pub fn tripwire(view: JobView) -> Result<JobView, ToolError> {
    let value: serde_json::Value = serde_json::from_str(&result_text(&view)?)
        .map_err(|e| ToolError::App(crate::error::AppError::Internal(format!("re-read get_job's view: {e}"))))?;
    fn hit(v: &serde_json::Value, path: &str) -> Option<String> {
        match v {
            serde_json::Value::String(s) => {
                let field = if path == ".header" { Field::Header } else { Field::Text };
                s.lines().any(|l| coordinate_row(l, field)).then(|| path.to_string())
            }
            serde_json::Value::Array(items) => items.iter().enumerate().find_map(|(i, x)| hit(x, &format!("{path}[{i}]"))),
            serde_json::Value::Object(map) => map.iter().find_map(|(k, x)| hit(x, &format!("{path}.{k}"))),
            _ => None,
        }
    }
    match hit(&value, "") {
        Some(field) => Err(ToolError::refused(format!(
            "get_job's view of job {} holds a coordinate-like row in `{field}`; not served (rule #9)",
            echo(&view.id)
        ))),
        None => Ok(view),
    }
}

/// The job's pending states, in words (ADR-024's cancel and submit columns; ADR-027's `mcp_state`).
fn notices_of(job: &Job) -> Vec<String> {
    let mut notices = Vec::new();
    match (&job.cancel_requested_at, &job.cancel_sent_at) {
        (Some(asked), None) => notices.push(format!("a cancel was requested at {asked} UTC and is not yet sent")),
        (Some(asked), Some(sent)) => notices.push(format!(
            "a cancel was requested at {asked} UTC and sent at {sent} UTC; the server has not yet confirmed the stop"
        )),
        _ => {}
    }
    if let Some(since) = &job.submit_pending_since {
        notices.push(format!("the submit has waited for the server's slot since {since} UTC"));
    }
    if job.mcp_state.as_deref() == Some(crate::mcp::steps::SUBMITTING) {
        notices.push("Claude is submitting this job; it cannot be submitted by hand until that ends".into());
    }
    notices
}

fn reasons_of(stored: Option<&str>, guard: &mut Guard) -> (Vec<ReasonView>, Option<String>) {
    let Some(json) = stored else { return (Vec::new(), None) };
    match serde_json::from_str::<Vec<McpReason>>(json) {
        Ok(all) => {
            let note = (all.len() > REASONS_MAX).then(|| format!("{} more reasons not shown", all.len() - REASONS_MAX));
            let shown = all
                .into_iter()
                .take(REASONS_MAX)
                .map(|r| ReasonView {
                    row: clip(&guard.text("mcp_reasons.row", r.row, Field::Text), 16).0,
                    message: clip(&guard.text("mcp_reasons.message", r.message, Field::Text), REASON_MAX_BYTES).0,
                })
                .collect();
            (shown, note)
        }
        Err(e) => (Vec::new(), Some(format!("the stored reasons are not readable: {e}"))),
    }
}

/// `get_job`: the projection of job `id`.
pub fn get_job(conn: &Connection, id: &str) -> Result<JobView, ToolError> {
    let job = get_job_conn(conn, id).map_err(ToolError::from_app)?;
    let mut guard = Guard::default();
    let (header, header_truncated, header_withheld) = match reconstruct(&job.input_content) {
        Ok(h) => {
            let h = guard.text("header", h, Field::Header);
            let (text, cut) = clip(&h, HEADER_MAX_BYTES);
            (Some(text), cut, None)
        }
        Err(why) => {
            let why = guard.text("header_withheld", why.to_string(), Field::Text);
            (None, false, Some(clip(&why, WHY_MAX_BYTES).0))
        }
    };
    let atom_count = read_coordinate_block(&job.input_content).ok().map(|b| b.rows.len());
    let mut charge_mult = ChargeMultView::of(&locate_charge_mult(&job.input_content), format!("the `* xyz` line of job {id}'s input"));
    charge_mult.source = guard.text("charge_mult.source", charge_mult.source, Field::Text);
    charge_mult.error = guard.opt("charge_mult.error", charge_mult.error, Field::Text);
    let notices = notices_of(&job).into_iter().map(|n| guard.text("notices", n, Field::Text)).collect();
    let (mcp_reasons, mcp_reasons_note) = reasons_of(job.mcp_reasons.as_deref(), &mut guard);
    let mcp_reasons_note = guard.opt("mcp_reasons_note", mcp_reasons_note, Field::Text);
    let title = guard.text("title", job.title, Field::Text);
    let (title, title_truncated) = clip(&title, TITLE_MAX_BYTES);
    let (error, error_truncated) = match guard.opt("error", job.error_message, Field::Text) {
        Some(e) => {
            let (text, cut) = clip(&e, ERROR_MAX_BYTES);
            (Some(text), cut)
        }
        None => (None, false),
    };
    let remote_host = guard.opt("remote_host", job.remote_host, Field::Text);
    let view = JobView {
        id: job.id,
        title,
        title_truncated,
        status: job.status,
        origin: job.origin,
        group_id: job.group_id,
        target_profile_id: job.backend_id,
        remote_host,
        created_at_utc: job.created_at,
        started_at_utc: job.started_at,
        completed_at_utc: job.completed_at,
        energy_eh: job.energy,
        wall_time_s: job.wall_time,
        header,
        header_truncated,
        header_withheld,
        atom_count,
        charge_mult,
        notices,
        error,
        error_truncated,
        mcp_reasons,
        mcp_reasons_note,
        grant_id: job.grant_id,
        mcp_state: job.mcp_state,
        withheld_fields: guard.withheld,
    };
    within_cap(tripwire(view)?, RESULT_CAP_BYTES)
}
