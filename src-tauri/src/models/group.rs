//! The `Group` model (schema v16, Phase 4.7.2, ADR-019).
//!
//! A **Group** is one node in the job-organization tree — the UI metaphor is a
//! "folder". Groups are **grouping metadata over jobs, never a filesystem
//! hierarchy** (ADR-019 Decision 0): a job keeps its isolated `job_dir` (domain
//! rule #3) wherever it sits in the tree; moving a job between groups is a one-row
//! `UPDATE jobs.group_id` with ZERO filesystem operations.
//!
//! The tree is stored as an **adjacency list** (ADR-019 Decision 2): each node
//! carries a nullable `parent_id` pointing at its parent (`None` = a root-level
//! group). A subtree is walked via the parent chain (or one recursive CTE);
//! rename / move / reparent touch a single row. `group_id` on a job is a single
//! nullable FK — **one group per job, a tree not tags** (Decision 1).

use rusqlite::Row;
use serde::Serialize;

/// A node in the job-group tree. Mirrors the `groups` table.
#[derive(Debug, Clone, Serialize)]
pub struct Group {
    pub id: String,
    pub name: String,
    /// Parent node id. `None` = a root-level group. Unlimited nesting via the
    /// parent chain (adjacency list). Deliberately NOT `ON DELETE CASCADE` in the
    /// schema — deleting a group PROMOTES its children to this parent (ADR-019
    /// Decision 3), done explicitly in `delete_group_conn`.
    pub parent_id: Option<String>,
    pub created_at: String,
}

impl Group {
    /// Column list used by every `SELECT` that hydrates a [`Group`]. The order
    /// here is the contract [`Group::from_row`] relies on.
    pub const COLUMNS: &'static str = "id, name, parent_id, created_at";

    /// Build a [`Group`] from a row selected in [`Group::COLUMNS`] order.
    pub fn from_row(row: &Row) -> rusqlite::Result<Group> {
        Ok(Group {
            id: row.get(0)?,
            name: row.get(1)?,
            parent_id: row.get(2)?,
            created_at: row.get(3)?,
        })
    }
}
