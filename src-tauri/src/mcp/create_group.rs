//! `create_group`'s core (ADR-027 decision 7; M3 amendment, items 1 and 3; unit M3b; the bounds and
//! the audit decided by Anton, 2026-11-21). A group is metadata (ADR-019): it spends nothing, so no
//! grant is needed.
//!
//! **The order** ([`run_create_group`]):
//! 1. **The argument bounds**, refused with no row: `name` at most [`GROUP_NAME_MAX_CHARS`]
//!    characters (counted on the name as given) and not empty after trimming; `parent_id`, when given,
//!    not empty and at most [`PARENT_ID_MAX_BYTES`].
//! 2. **One `BEGIN IMMEDIATE` transaction** ([`create_group_audited_conn`]): the group insert through
//!    `commands::groups::create_group_conn` (the same core as the Groups panel; the name stored
//!    **trimmed**) and its audit row (NULL disposition, the new group's id in `reasons`). A group
//!    without its row cannot commit. A `parent_id` that does not exist is the core's
//!    `NotFound`: the transaction rolls back, and no row is written (an argument error is not a
//!    disposition: M3 amendment, item 3).

use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use super::audit::{append_audit_conn, AuditEntry};
use super::grant::begin_immediate;
use super::steps::McpReason;
use super::tools::{clip, within_cap, ToolError, RESULT_CAP_BYTES};
use crate::commands::groups::create_group_conn;
use crate::error::AppError;
use crate::models::group::Group;

/// The `tool` of every `create_group` row.
pub const CREATE_GROUP: &str = "create_group";

/// The longest group name, in characters (Unicode scalar values) of the name as given.
pub const GROUP_NAME_MAX_CHARS: usize = 200;

/// The longest `parent_id`, in bytes (a v4 UUID is 36; the same bound as `create_job`'s ids).
pub const PARENT_ID_MAX_BYTES: usize = 64;

/// The name in the audit row's `args`, in serialised bytes: room for 200 characters of any width,
/// escapes included.
const AUDIT_NAME_MAX_BYTES: usize = 6 * GROUP_NAME_MAX_CHARS;

/// The tool's arguments.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateGroupArgs {
    pub name: String,
    pub parent_id: Option<String>,
}

/// What `create_group` answers: the group as stored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CreateGroupResult {
    pub group_id: String,
    /// The name as stored (trimmed).
    pub name: String,
    pub parent_id: Option<String>,
    pub created_at_utc: String,
}

/// The call's argument text for the audit row: a JSON object, each field cut to its bound.
pub fn audit_args(args: &CreateGroupArgs) -> String {
    serde_json::json!({
        "name": clip(&args.name, AUDIT_NAME_MAX_BYTES).0,
        "parent_id": args.parent_id.as_deref().map(|p| clip(p, PARENT_ID_MAX_BYTES).0),
    })
    .to_string()
}

/// The bounds of step 1; on success, the name to store (trimmed). Sizes only, never the text.
pub fn check_args(args: &CreateGroupArgs) -> Result<String, ToolError> {
    let chars = args.name.chars().count();
    if chars > GROUP_NAME_MAX_CHARS {
        return Err(ToolError::refused(format!("`name` is {chars} characters; at most {GROUP_NAME_MAX_CHARS}")));
    }
    let name = args.name.trim();
    if name.is_empty() {
        return Err(ToolError::refused("`name` is empty"));
    }
    if let Some(p) = &args.parent_id {
        if p.is_empty() {
            return Err(ToolError::refused("`parent_id` is empty; omit it for a top-level group"));
        }
        if p.len() > PARENT_ID_MAX_BYTES {
            return Err(ToolError::refused(format!("`parent_id` is {} bytes; at most {PARENT_ID_MAX_BYTES}", p.len())));
        }
    }
    Ok(name.to_string())
}

/// Step 2: the insert and its audit row in **one** `BEGIN IMMEDIATE` transaction. `name` is already
/// checked and trimmed ([`check_args`]).
pub fn create_group_audited_conn(
    conn: &Connection,
    name: &str,
    parent_id: Option<&str>,
    audit_args: &str,
) -> Result<Group, AppError> {
    let tx = begin_immediate(conn)?;
    let group = create_group_conn(&tx, name, parent_id)?;
    let reasons = [McpReason { row: "group".into(), message: format!("created {}", group.id) }];
    append_audit_conn(
        &tx,
        &AuditEntry { tool: CREATE_GROUP, args: audit_args, disposition: None, reasons: &reasons, job_id: None, grant_id: None },
    )?;
    tx.commit()?;
    Ok(group)
}

/// `create_group` (the module docs' order), over a connection the caller has locked.
pub fn run_create_group(conn: &Connection, args: &CreateGroupArgs) -> Result<CreateGroupResult, ToolError> {
    let name = check_args(args)?;
    let group = create_group_audited_conn(conn, &name, args.parent_id.as_deref(), &audit_args(args))
        .map_err(ToolError::from_app)?;
    within_cap(
        CreateGroupResult { group_id: group.id, name: group.name, parent_id: group.parent_id, created_at_utc: group.created_at },
        RESULT_CAP_BYTES,
    )
}
