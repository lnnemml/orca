//! `list_groups` (ADR-027 decision 7): the job groups (ADR-019), paged. The tree is built from
//! `parent_id`; `None` is a root-level group.

use rusqlite::Connection;
use serde::Serialize;

use super::{clip, within_cap, ToolError, RESULT_CAP_BYTES};
use crate::commands::groups::list_groups_conn;

pub const GROUPS_DEFAULT: usize = 50;
pub const GROUPS_MAX: usize = 100;
pub const GROUP_NAME_MAX_BYTES: usize = 120;

/// One group.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GroupView {
    pub id: String,
    pub name: String,
    pub name_truncated: bool,
    pub parent_id: Option<String>,
    pub created_at_utc: String,
}

/// One page of groups, in creation order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GroupsPage {
    pub groups: Vec<GroupView>,
    pub total_count: usize,
    pub offset: usize,
    pub limit: usize,
    /// The next page's offset; `None` on the last page.
    pub next_offset: Option<usize>,
}

/// `list_groups`: one page, in `list_groups_conn`'s order (`created_at, id`).
pub fn list_groups(conn: &Connection, offset: Option<usize>, limit: Option<usize>) -> Result<GroupsPage, ToolError> {
    let all = list_groups_conn(conn).map_err(ToolError::App)?;
    let offset = offset.unwrap_or(0);
    let limit = limit.unwrap_or(GROUPS_DEFAULT).clamp(1, GROUPS_MAX);
    let total_count = all.len();
    let groups: Vec<GroupView> = all
        .into_iter()
        .skip(offset)
        .take(limit)
        .map(|g| {
            let (name, name_truncated) = clip(&g.name, GROUP_NAME_MAX_BYTES);
            GroupView { id: g.id, name, name_truncated, parent_id: g.parent_id, created_at_utc: g.created_at }
        })
        .collect();
    let end = offset.saturating_add(groups.len());
    let page = GroupsPage { groups, total_count, offset, limit, next_offset: (end < total_count).then_some(end) };
    within_cap(page, RESULT_CAP_BYTES)
}
