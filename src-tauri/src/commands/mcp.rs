//! The MCP grant commands (ADR-027 decision 5, unit M1b Part B): Settings → MCP creates, revokes and
//! reads the grant through these, and they are **the only way a grant is made**. No MCP tool calls
//! them; the server only reads the grant (`mcp::grant::grant_snapshot_conn` in step A).
//!
//! Each command is a thin shell over a `*_conn` core here, which is a thin wrapper over the grant
//! core (`mcp::grant`): it reads `now` from SQLite (`datetime('now')`, UTC on the laptop's clock),
//! spells the targets, and returns a [`GrantView`] the UI can show. Every write is the grant core's
//! own `BEGIN IMMEDIATE` transaction, short and local, so the commands are synchronous like the
//! other short database commands (the threading rule in `wiki/modules/tauri-core.md`).
//!
//! The switch's two commands (ADR-027 M2a), `mcp_status` and `mcp_set_enabled`, are the exception:
//! they reach the MCP lifecycle, whose transition can reconcile, bind and `stat`, so they are `async`
//! with their work in `spawn_blocking`, and the main thread never takes the lifecycle mutex.
//!
//! Settings → MCP's other commands (ADR-027 M4 amendment, items 3 and 4; unit M4a):
//! - `mcp_claude_jobs_summary`, `mcp_list_audit` and `mcp_audit_row` are short reads, synchronous
//!   like the grant commands (`mcp::claude_jobs`, `mcp::audit_view`);
//! - `mcp_cancel_claude_jobs` is `async` with its work on the blocking pool: each job goes through
//!   Anton's per-job Cancel body (`commands::jobs::cancel_one_job`), and a remote one makes ssh calls.

use std::sync::Arc;

use rusqlite::Connection;
use serde::Serialize;
use tauri::{AppHandle, Manager, State};

use crate::commands::jobs::{cancel_one_job, get_job_conn, CancelResult};
use crate::commands::settings::DbState;
use crate::error::AppError;
use crate::mcp::audit_view::{audit_row_conn, list_audit_conn, AuditPage, AuditRowView};
use crate::mcp::classify::GrantStatus;
use crate::mcp::claude_jobs::{cancel_claude_jobs, claude_jobs_summary_conn, ClaudeCancelReport, ClaudeJobsSummary};
use crate::mcp::grant::{
    create_grant_conn, get_grant_conn, grant_snapshot_conn, in_flight_conn, now_utc_conn, revoke_grant_conn, target_of,
    target_text, Grant, NewGrant,
};
use crate::mcp::resolve::header_of_input;
use crate::mcp::server::lifecycle::{McpLifecycle, McpStatus};
use crate::mcp::server::wiring::ManagedDb;
use crate::mcp::server::DbSource;
use crate::mcp::signature::MethodSignature;

/// The state of a grant as the UI shows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum GrantState {
    Active,
    Expired,
    Revoked,
}

impl From<GrantStatus> for GrantState {
    fn from(s: GrantStatus) -> Self {
        match s {
            GrantStatus::Active => GrantState::Active,
            GrantStatus::Expired => GrantState::Expired,
            GrantStatus::Revoked => GrantState::Revoked,
        }
    }
}

/// A grant and its counters, for Settings → MCP. Times are UTC on the laptop's clock,
/// `YYYY-MM-DD HH:MM:SS`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GrantView {
    pub id: String,
    pub state: GrantState,
    pub created_at: String,
    pub expires_at: String,
    pub revoked_at: Option<String>,
    /// `"local"` or a server profile id.
    pub targets: Vec<String>,
    /// Each granted level's signature, as one line (`MethodSignature::display`).
    pub levels: Vec<String>,
    pub max_jobs: u32,
    pub max_in_flight: u32,
    /// Stored: step A adds one, only a refund takes one back.
    pub charged_jobs: u32,
    /// Derived: every non-terminal job charged to any grant (decision 5).
    pub in_flight: u32,
}

/// The view of `grant` at `now` (its state from `Grant::status_at`, the rule step A's snapshot
/// uses), with the live `in_flight` count.
fn view_conn(conn: &Connection, grant: Grant, now: &str) -> Result<GrantView, AppError> {
    Ok(GrantView {
        state: grant.status_at(now).into(),
        targets: grant.targets.iter().map(|t| target_text(t).to_string()).collect(),
        levels: grant.levels.iter().map(MethodSignature::display).collect(),
        max_jobs: grant.max_jobs,
        max_in_flight: grant.max_in_flight,
        charged_jobs: grant.charged_jobs,
        in_flight: in_flight_conn(conn)?,
        id: grant.id,
        created_at: grant.created_at,
        expires_at: grant.expires_at,
        revoked_at: grant.revoked_at,
    })
}

/// The body of [`mcp_create_grant`]: the grant core's `create_grant_conn` at SQLite's `now`. A
/// refused grant (a level outside the grammar or with no method token, a count below 1, an expiry
/// not after now, a server target with no profile) is the core's `AppError`, and nothing is written.
pub(crate) fn create_grant_core(
    conn: &Connection,
    targets: Vec<String>,
    levels: Vec<String>,
    max_jobs: u32,
    max_in_flight: u32,
    expires_at: String,
) -> Result<GrantView, AppError> {
    let now = now_utc_conn(conn)?;
    let new = NewGrant {
        targets: targets.into_iter().map(target_of).collect(),
        levels,
        max_jobs,
        max_in_flight,
        expires_at,
    };
    let grant = create_grant_conn(conn, &new, &now)?;
    view_conn(conn, grant, &now)
}

/// The body of [`mcp_revoke_grant`]: idempotent, the first `revoked_at` is kept.
pub(crate) fn revoke_grant_core(conn: &Connection, id: &str) -> Result<GrantView, AppError> {
    let now = now_utc_conn(conn)?;
    let grant = revoke_grant_conn(conn, id, &now)?;
    view_conn(conn, grant, &now)
}

/// The body of [`mcp_get_grant`]: the newest grant (the unrevoked one, else the most recently
/// revoked) with its state and counters; `None` when no grant was ever made. It is the grant step
/// A's snapshot reads (`grant_snapshot_conn`).
pub(crate) fn current_grant_core(conn: &Connection) -> Result<Option<GrantView>, AppError> {
    let now = now_utc_conn(conn)?;
    let Some(current) = grant_snapshot_conn(conn, &now)? else { return Ok(None) };
    view_conn(conn, get_grant_conn(conn, &current.id)?, &now).map(Some)
}

/// The body of [`mcp_level_from_job`]: the method level of job `job_id`'s input, as level text the
/// grant editor can paste ("this job's level"). The header (the input outside its coordinate block,
/// the app's `%maxcore` line set aside) is read strictly (`MethodSignature::of_level_text`).
/// Post-condition (rule #9): the text read back gives the same signature, else an internal error.
pub(crate) fn level_from_job_conn(conn: &Connection, job_id: &str) -> Result<String, AppError> {
    let job = get_job_conn(conn, job_id)?;
    let header =
        header_of_input(&job.input_content).map_err(|e| AppError::Invalid(format!("job {job_id}'s input: {e}")))?;
    let signature = MethodSignature::of_level_text(&header)
        .map_err(|e| AppError::Invalid(format!("job {job_id}'s header gives no level: {e}")))?;
    let text = signature.level_text();
    match MethodSignature::of_level_text(&text) {
        Ok(back) if back == signature => Ok(text),
        other => Err(AppError::Internal(format!(
            "job {job_id}: the level text `{text}` does not read back as its signature ({other:?})"
        ))),
    }
}

/// Create the grant (Settings → MCP), revoking whatever grant is unrevoked in the same transaction.
/// `targets`: `"local"` or server profile ids. `levels`: each a `!` line and optionally a `%cpcm`
/// block, as pasted. `expires_at`: UTC, `YYYY-MM-DD HH:MM:SS`, after now.
#[tauri::command]
pub fn mcp_create_grant(
    db: State<'_, DbState>,
    targets: Vec<String>,
    levels: Vec<String>,
    max_jobs: u32,
    max_in_flight: u32,
    expires_at: String,
) -> Result<GrantView, AppError> {
    let conn = db.lock()?;
    create_grant_core(&conn, targets, levels, max_jobs, max_in_flight, expires_at)
}

/// Revoke grant `id`. Running jobs are not cancelled.
#[tauri::command]
pub fn mcp_revoke_grant(db: State<'_, DbState>, id: String) -> Result<GrantView, AppError> {
    let conn = db.lock()?;
    revoke_grant_core(&conn, &id)
}

/// The newest grant and its counters, or `null` when none was ever made.
#[tauri::command]
pub fn mcp_get_grant(db: State<'_, DbState>) -> Result<Option<GrantView>, AppError> {
    let conn = db.lock()?;
    current_grant_core(&conn)
}

/// "This job's level": job `job_id`'s method level as level text for the grant editor.
#[tauri::command]
pub fn mcp_level_from_job(db: State<'_, DbState>, job_id: String) -> Result<String, AppError> {
    let conn = db.lock()?;
    level_from_job_conn(&conn, &job_id)
}

// ----- the switch (ADR-027 M2 amendment, item 2) -----

/// Settings → MCP's status: the switch, whether this instance serves, and why not. Advisory.
/// `async`, with the work in `spawn_blocking`, so the GTK main thread never runs it.
#[tauri::command]
pub async fn mcp_status(app: AppHandle) -> Result<McpStatus, AppError> {
    mcp_status_core(managed_lifecycle(&app)?).await
}

/// Settings → MCP's switch: write `mcp_enabled` (its only writer) and run one tick step at once.
/// Returns the status after it. `async`, with the work in `spawn_blocking`: the step can take the
/// lifecycle mutex, reconcile, bind and `stat`, and the main thread never takes that mutex.
#[tauri::command]
pub async fn mcp_set_enabled(app: AppHandle, on: bool) -> Result<McpStatus, AppError> {
    mcp_set_enabled_core(managed_lifecycle(&app)?, on).await
}

fn managed_lifecycle(app: &AppHandle) -> Result<Arc<McpLifecycle>, AppError> {
    app.try_state::<Arc<McpLifecycle>>()
        .map(|state| state.inner().clone())
        .ok_or_else(|| AppError::Internal("the MCP lifecycle is not set up".into()))
}

/// [`mcp_status`]'s body, off the caller's thread.
pub(crate) async fn mcp_status_core(lifecycle: Arc<McpLifecycle>) -> Result<McpStatus, AppError> {
    off_the_callers_thread("mcp_status", move || Ok(lifecycle.status())).await
}

/// [`mcp_set_enabled`]'s body, off the caller's thread.
pub(crate) async fn mcp_set_enabled_core(lifecycle: Arc<McpLifecycle>, on: bool) -> Result<McpStatus, AppError> {
    off_the_callers_thread("mcp_set_enabled", move || lifecycle.set_enabled(on)).await
}

// ----- Settings → MCP: "Cancel Claude's jobs" and the audit view (ADR-027 M4 amendment, items 3, 4) -----

/// The confirm's numbers for "Cancel Claude's jobs": the jobs it would cancel now and the drafts in
/// step B it would skip. The same statement as the cancel (`mcp::claude_jobs`).
#[tauri::command]
pub fn mcp_claude_jobs_summary(db: State<'_, DbState>) -> Result<ClaudeJobsSummary, AppError> {
    let conn = db.lock()?;
    claude_jobs_summary_conn(&conn)
}

/// "Cancel Claude's jobs": every `origin = 'mcp'` job in `queued`, `running` or `cancelling`,
/// queued first, each through Anton's per-job Cancel body; one result per job, and the count of
/// skipped drafts in step B. Writes no audit row (Anton's action).
#[tauri::command]
pub async fn mcp_cancel_claude_jobs(app: AppHandle) -> Result<ClaudeCancelReport, AppError> {
    let db: Arc<dyn DbSource> = Arc::new(ManagedDb(app.clone()));
    cancel_claude_jobs_off_thread(db, move |id| cancel_one_job(&app, id)).await
}

/// [`mcp_cancel_claude_jobs`]'s body: the selection and every per-job cancel on Tauri's blocking
/// pool, never on the IPC thread or an async worker.
pub(crate) async fn cancel_claude_jobs_off_thread(
    db: Arc<dyn DbSource>,
    cancel_one: impl FnMut(&str) -> Result<CancelResult, AppError> + Send + 'static,
) -> Result<ClaudeCancelReport, AppError> {
    off_the_callers_thread("mcp_cancel_claude_jobs", move || cancel_claude_jobs(db.db(), cancel_one)).await
}

/// One page of the audit log, newest first: the rows below `before_id` (or the newest), `limit`
/// of them (default 50, at least 1, at most 200).
#[tauri::command]
pub fn mcp_list_audit(db: State<'_, DbState>, before_id: Option<i64>, limit: Option<u32>) -> Result<AuditPage, AppError> {
    let conn = db.lock()?;
    list_audit_conn(&conn, before_id, limit)
}

/// Audit row `id` whole (its `args` uncut); `NotFound` for an unknown id.
#[tauri::command]
pub fn mcp_audit_row(db: State<'_, DbState>, id: i64) -> Result<AuditRowView, AppError> {
    let conn = db.lock()?;
    audit_row_conn(&conn, id)
}

/// Run `work` on Tauri's blocking pool and wait for it (as `test_server_profile` does).
async fn off_the_callers_thread<T: Send + 'static>(
    what: &'static str,
    work: impl FnOnce() -> Result<T, AppError> + Send + 'static,
) -> Result<T, AppError> {
    tauri::async_runtime::spawn_blocking(work)
        .await
        .map_err(|e| AppError::Backend(format!("{what} task failed: {e}")))?
}

#[cfg(test)]
#[path = "mcp_tests.rs"]
mod tests;
