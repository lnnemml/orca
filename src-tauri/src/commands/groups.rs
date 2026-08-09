//! Group commands: the CRUD surface over the `groups` table (an adjacency-list
//! tree) and the nullable `jobs.group_id` (schema v16, Phase 4.7.2, ADR-019).
//!
//! Same shape as `commands::reactions`: each Tauri command is a thin wrapper that
//! locks the shared connection and delegates to a `*_conn` helper taking a
//! `&Connection`, so the logic is unit-testable without a running Tauri app.
//!
//! **Groups are pure metadata (ADR-019 Decision 0), NEVER a filesystem hierarchy.**
//! A job keeps its isolated `job_dir` (domain rule #3) wherever it sits in the tree;
//! moving a job is a one-row `UPDATE jobs.group_id` with ZERO filesystem operations.
//! This module makes **no filesystem calls at all** — grep it for `std::fs` / `job_dir`
//! and you will find none. That is the invariant, not an accident.
//!
//! Two load-bearing correctness rules (FK enforcement is ON —
//! `SQLITE_DEFAULT_FOREIGN_KEYS=1`, measured; see the v14 note in `db.rs`):
//!
//! 1. **Cycle guard on `move_group`.** Moving a group under its own descendant would
//!    make a node its own ancestor — any parent-walk then loops forever and the tree
//!    is corrupt. `move_group_conn` refuses a self-parent or a descendant-parent via a
//!    **bounded** walk up the new parent's chain (bounded by the group count, so even a
//!    pre-existing corrupt cycle can't hang the check).
//!
//! 2. **Promotion-to-parent on `delete_group`.** `jobs.group_id` is `ON DELETE SET NULL`,
//!    so a naive `DELETE FROM groups` would drop that group's jobs to ROOT — WRONG for a
//!    non-root group. ADR-019 Decision 3 mandates **promotion**: child groups AND jobs are
//!    re-parented to the deleted group's PARENT, explicitly, in one transaction, BEFORE the
//!    row is removed. `SET NULL` is only the fallback for a delete that bypasses this command.
//!    (Child groups' `parent_id` has no cascade → the row delete RESTRICT-fails unless the
//!    children are re-parented first — the same load-bearing cleanup shape as job deletion.)

use rusqlite::{params, Connection, OptionalExtension};
use tauri::State;
use uuid::Uuid;

use crate::commands::settings::DbState;
use crate::error::AppError;
use crate::models::group::Group;

// --- Existence probes (app-level referential integrity) ---------------------

fn group_exists(conn: &Connection, id: &str) -> Result<bool, AppError> {
    Ok(conn
        .query_row("SELECT 1 FROM groups WHERE id = ?1", params![id], |_| Ok(()))
        .optional()?
        .is_some())
}

fn job_exists(conn: &Connection, id: &str) -> Result<bool, AppError> {
    Ok(conn
        .query_row("SELECT 1 FROM jobs WHERE id = ?1", params![id], |_| Ok(()))
        .optional()?
        .is_some())
}

// --- Connection-level helpers (testable) ------------------------------------

/// A single group by id, or [`AppError::NotFound`].
fn get_group_conn(conn: &Connection, id: &str) -> Result<Group, AppError> {
    let sql = format!("SELECT {} FROM groups WHERE id = ?1", Group::COLUMNS);
    conn.query_row(&sql, params![id], Group::from_row)
        .optional()?
        .ok_or_else(|| AppError::NotFound(format!("group {id}")))
}

/// Create a group. If `parent_id` is `Some`, the parent must exist (else
/// [`AppError::NotFound`] — no orphan row is written). `None` = a root-level group.
fn create_group_conn(
    conn: &Connection,
    name: &str,
    parent_id: Option<&str>,
) -> Result<Group, AppError> {
    if let Some(p) = parent_id {
        if !group_exists(conn, p)? {
            return Err(AppError::NotFound(format!("group {p}")));
        }
    }
    let id = Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO groups (id, name, parent_id) VALUES (?1, ?2, ?3)",
        params![id, name, parent_id],
    )?;
    get_group_conn(conn, &id)
}

/// All groups (the UI builds the tree from `parent_id`). Ordered by `created_at, id`
/// for a stable listing.
fn list_groups_conn(conn: &Connection) -> Result<Vec<Group>, AppError> {
    let sql = format!("SELECT {} FROM groups ORDER BY created_at, id", Group::COLUMNS);
    let mut stmt = conn.prepare(&sql)?;
    let groups = stmt
        .query_map([], Group::from_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(groups)
}

/// Rename a group; [`AppError::NotFound`] if absent. Returns the updated group.
fn rename_group_conn(conn: &Connection, id: &str, name: &str) -> Result<Group, AppError> {
    if !group_exists(conn, id)? {
        return Err(AppError::NotFound(format!("group {id}")));
    }
    conn.execute("UPDATE groups SET name = ?1 WHERE id = ?2", params![name, id])?;
    get_group_conn(conn, id)
}

/// Move a group under a new parent (`None` = to root). **Cycle guard (RISK 1):**
/// refuses a self-parent or a descendant-parent so a node can never become its own
/// ancestor. The check walks UP the new parent's chain, bounded by the group count
/// so a pre-existing corrupt cycle can't hang it either.
fn move_group_conn(
    conn: &Connection,
    id: &str,
    new_parent_id: Option<&str>,
) -> Result<(), AppError> {
    if !group_exists(conn, id)? {
        return Err(AppError::NotFound(format!("group {id}")));
    }
    if let Some(np) = new_parent_id {
        if !group_exists(conn, np)? {
            return Err(AppError::NotFound(format!("group {np}")));
        }
        // Self-parent is the trivial cycle.
        if np == id {
            return Err(AppError::Backend(
                "cannot move a group under its own descendant (would create a cycle)".into(),
            ));
        }
        // Walk up from the new parent; if we reach `id`, then `np` is a descendant of
        // `id` and the move would create a cycle. Bound the walk by the group count.
        let bound: i64 = conn.query_row("SELECT COUNT(*) FROM groups", [], |r| r.get(0))?;
        let mut current = Some(np.to_string());
        let mut steps: i64 = 0;
        while let Some(cur) = current {
            if cur == id {
                return Err(AppError::Backend(
                    "cannot move a group under its own descendant (would create a cycle)".into(),
                ));
            }
            steps += 1;
            if steps > bound {
                // More steps than nodes → a pre-existing cycle among the ancestors.
                return Err(AppError::Backend(
                    "group hierarchy is corrupt (cycle detected while validating move)".into(),
                ));
            }
            current = conn
                .query_row(
                    "SELECT parent_id FROM groups WHERE id = ?1",
                    params![cur],
                    |r| r.get::<_, Option<String>>(0),
                )
                .optional()?
                .flatten();
        }
    }
    conn.execute(
        "UPDATE groups SET parent_id = ?1 WHERE id = ?2",
        params![new_parent_id, id],
    )?;
    Ok(())
}

/// Move a job into a group (`None` = ungroup / to root). Job and (if `Some`) the
/// target group must exist ([`AppError::NotFound`] else). One-row `UPDATE` — NO
/// filesystem op (the job keeps its `job_dir`, ADR-019 Decision 0).
fn move_job_conn(
    conn: &Connection,
    job_id: &str,
    group_id: Option<&str>,
) -> Result<(), AppError> {
    if !job_exists(conn, job_id)? {
        return Err(AppError::NotFound(format!("job {job_id}")));
    }
    if let Some(g) = group_id {
        if !group_exists(conn, g)? {
            return Err(AppError::NotFound(format!("group {g}")));
        }
    }
    conn.execute(
        "UPDATE jobs SET group_id = ?1 WHERE id = ?2",
        params![group_id, job_id],
    )?;
    Ok(())
}

/// Delete a group with **PROMOTION (RISK 2 / ADR-019 Decision 3)**: its child groups
/// and its jobs are re-parented to the deleted group's OWN parent (root if it was
/// root-level), explicitly, in one transaction, BEFORE the row is removed. A deleted
/// folder's contents rise one level; nothing under it is destroyed, and no job is ever
/// deleted (jobs-survive). This module touches NO filesystem — `job_dir` is untouched.
///
/// The explicit job re-parent (step 3) is load-bearing: `jobs.group_id` is
/// `ON DELETE SET NULL`, so leaning on the cascade would send the jobs to ROOT, not to
/// the parent. We re-parent them to the parent ourselves.
fn delete_group_conn(conn: &Connection, id: &str) -> Result<(), AppError> {
    // Not-found first — nothing is touched if the id is absent.
    let group = get_group_conn(conn, id)?;
    let parent = group.parent_id; // Option<String>; None = the deleted group was root-level.

    let tx = conn.unchecked_transaction()?;
    // 1./2. Promote child GROUPS to the deleted group's parent.
    tx.execute(
        "UPDATE groups SET parent_id = ?1 WHERE parent_id = ?2",
        params![parent, id],
    )?;
    // 3. Promote JOBS to the parent EXPLICITLY (do NOT rely on ON DELETE SET NULL,
    //    which would send them to root instead of the parent).
    tx.execute(
        "UPDATE jobs SET group_id = ?1 WHERE group_id = ?2",
        params![parent, id],
    )?;
    // 4. Remove the now-empty group row.
    tx.execute("DELETE FROM groups WHERE id = ?1", params![id])?;
    tx.commit()?;
    Ok(())
}

// --- Tauri command wrappers (thin: lock DbState, delegate to the _conn fn) ---

#[tauri::command]
pub fn create_group(
    db: State<'_, DbState>,
    name: String,
    parent_id: Option<String>,
) -> Result<Group, AppError> {
    let conn = db.lock()?;
    create_group_conn(&conn, &name, parent_id.as_deref())
}

#[tauri::command]
pub fn list_groups(db: State<'_, DbState>) -> Result<Vec<Group>, AppError> {
    let conn = db.lock()?;
    list_groups_conn(&conn)
}

#[tauri::command]
pub fn rename_group(
    db: State<'_, DbState>,
    id: String,
    name: String,
) -> Result<Group, AppError> {
    let conn = db.lock()?;
    rename_group_conn(&conn, &id, &name)
}

#[tauri::command]
pub fn move_group(
    db: State<'_, DbState>,
    id: String,
    new_parent_id: Option<String>,
) -> Result<(), AppError> {
    let conn = db.lock()?;
    move_group_conn(&conn, &id, new_parent_id.as_deref())
}

#[tauri::command]
pub fn move_job(
    db: State<'_, DbState>,
    job_id: String,
    group_id: Option<String>,
) -> Result<(), AppError> {
    let conn = db.lock()?;
    move_job_conn(&conn, &job_id, group_id.as_deref())
}

#[tauri::command]
pub fn delete_group(db: State<'_, DbState>, id: String) -> Result<(), AppError> {
    let conn = db.lock()?;
    delete_group_conn(&conn, &id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::init_db;

    /// A migrated database in a throwaway temp dir. A process-wide atomic counter
    /// keeps each test's directory unique even under parallel runs.
    fn test_db() -> (Connection, std::path::PathBuf) {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "orcastudio-groups-test-{}-{}",
            std::process::id(),
            n
        ));
        std::fs::remove_dir_all(&dir).ok();
        let conn = init_db(&dir).expect("init_db should succeed");
        (conn, dir)
    }

    /// Insert a standalone job directly (title + input_content are the only NOT NULL
    /// columns without a default).
    fn insert_job(conn: &Connection, id: &str) {
        conn.execute(
            "INSERT INTO jobs (id, title, input_content) VALUES (?1, ?2, ?3)",
            params![id, format!("job {id}"), "! r2SCAN-3c Opt"],
        )
        .expect("insert job");
    }

    fn group_parent(conn: &Connection, id: &str) -> Option<String> {
        conn.query_row("SELECT parent_id FROM groups WHERE id = ?1", params![id], |r| {
            r.get::<_, Option<String>>(0)
        })
        .expect("group should exist")
    }

    fn job_group(conn: &Connection, id: &str) -> Option<String> {
        conn.query_row("SELECT group_id FROM jobs WHERE id = ?1", params![id], |r| {
            r.get::<_, Option<String>>(0)
        })
        .expect("job should exist")
    }

    #[test]
    fn create_rename_list_happy_paths() {
        let (conn, dir) = test_db();

        let root = create_group_conn(&conn, "ibuprofen reduction", None).unwrap();
        assert_eq!(root.name, "ibuprofen reduction");
        assert_eq!(root.parent_id, None);

        let sub = create_group_conn(&conn, "per-face", Some(&root.id)).unwrap();
        assert_eq!(sub.parent_id.as_deref(), Some(root.id.as_str()));

        let renamed = rename_group_conn(&conn, &sub.id, "si-face").unwrap();
        assert_eq!(renamed.name, "si-face");

        let all = list_groups_conn(&conn).unwrap();
        assert_eq!(all.len(), 2);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn create_group_under_missing_parent_is_not_found() {
        let (conn, dir) = test_db();
        assert!(matches!(
            create_group_conn(&conn, "orphan", Some("no-such-group")),
            Err(AppError::NotFound(_))
        ));
        // Nothing was written.
        assert_eq!(list_groups_conn(&conn).unwrap().len(), 0);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn move_group_to_root_works() {
        let (conn, dir) = test_db();
        let a = create_group_conn(&conn, "a", None).unwrap();
        let b = create_group_conn(&conn, "b", Some(&a.id)).unwrap();
        assert_eq!(group_parent(&conn, &b.id).as_deref(), Some(a.id.as_str()));

        move_group_conn(&conn, &b.id, None).unwrap();
        assert_eq!(group_parent(&conn, &b.id), None);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn move_job_sets_and_clears_group_id() {
        let (conn, dir) = test_db();
        let g = create_group_conn(&conn, "study", None).unwrap();
        insert_job(&conn, "j1");
        assert_eq!(job_group(&conn, "j1"), None);

        move_job_conn(&conn, "j1", Some(&g.id)).unwrap();
        assert_eq!(job_group(&conn, "j1").as_deref(), Some(g.id.as_str()));

        move_job_conn(&conn, "j1", None).unwrap();
        assert_eq!(job_group(&conn, "j1"), None);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn move_job_to_missing_group_is_not_found() {
        let (conn, dir) = test_db();
        insert_job(&conn, "j1");
        assert!(matches!(
            move_job_conn(&conn, "j1", Some("no-such-group")),
            Err(AppError::NotFound(_))
        ));
        // job unchanged
        assert_eq!(job_group(&conn, "j1"), None);
        // and a missing job is NotFound too
        assert!(matches!(
            move_job_conn(&conn, "no-such-job", None),
            Err(AppError::NotFound(_))
        ));
        std::fs::remove_dir_all(&dir).ok();
    }

    /// NEGATIVE CONTROL 1 (cycle): A→B→C (C under B under A). Moving A under C (its own
    /// descendant) must be refused, and the parent_ids must be unchanged. Self-parent too.
    #[test]
    fn move_group_refuses_cycle() {
        let (conn, dir) = test_db();
        let a = create_group_conn(&conn, "a", None).unwrap();
        let b = create_group_conn(&conn, "b", Some(&a.id)).unwrap();
        let c = create_group_conn(&conn, "c", Some(&b.id)).unwrap();

        // Descendant-parent: A under C.
        assert!(matches!(
            move_group_conn(&conn, &a.id, Some(&c.id)),
            Err(AppError::Backend(_))
        ));
        // Self-parent: A under A.
        assert!(matches!(
            move_group_conn(&conn, &a.id, Some(&a.id)),
            Err(AppError::Backend(_))
        ));

        // Nothing changed.
        assert_eq!(group_parent(&conn, &a.id), None);
        assert_eq!(group_parent(&conn, &b.id).as_deref(), Some(a.id.as_str()));
        assert_eq!(group_parent(&conn, &c.id).as_deref(), Some(b.id.as_str()));

        // A legal move within the tree still works (B under C is fine — C is not under B's
        // NEW position; wait: C is under B, so B under C IS a cycle). Use a fresh sibling.
        let d = create_group_conn(&conn, "d", None).unwrap();
        move_group_conn(&conn, &d.id, Some(&c.id)).unwrap(); // d under c — no cycle
        assert_eq!(group_parent(&conn, &d.id).as_deref(), Some(c.id.as_str()));

        std::fs::remove_dir_all(&dir).ok();
    }

    /// NEGATIVE CONTROL 2 (RESTRICT bites): a group S with a child group T. A RAW
    /// `DELETE FROM groups WHERE id = S` WITHOUT re-parenting T trips the RESTRICT FK
    /// (T.parent_id references S, no cascade). Proves the promotion re-parent is required.
    #[test]
    fn raw_delete_without_reparent_hits_restrict_fk() {
        let (conn, dir) = test_db();
        let s = create_group_conn(&conn, "s", None).unwrap();
        let _t = create_group_conn(&conn, "t", Some(&s.id)).unwrap();

        let raw = conn.execute("DELETE FROM groups WHERE id = ?1", params![s.id]);
        assert!(
            raw.is_err(),
            "raw delete of a group with a child must fail the RESTRICT FK (enforcement is ON)"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    /// PROMOTION to PARENT (headline behavior): R (root) → S (under R); a child group T
    /// under S and a job J with group_id = S. Deleting S promotes T and J to R — NOT to
    /// root/NULL. This is the ON-DELETE-SET-NULL trap the explicit re-parent avoids.
    #[test]
    fn delete_group_promotes_children_to_parent_not_root() {
        let (conn, dir) = test_db();
        let r = create_group_conn(&conn, "R", None).unwrap();
        let s = create_group_conn(&conn, "S", Some(&r.id)).unwrap();
        let t = create_group_conn(&conn, "T", Some(&s.id)).unwrap();
        insert_job(&conn, "j1");
        move_job_conn(&conn, "j1", Some(&s.id)).unwrap();

        delete_group_conn(&conn, &s.id).unwrap();

        // T promoted to R (S's parent), J promoted to R — NOT to root/NULL.
        assert_eq!(group_parent(&conn, &t.id).as_deref(), Some(r.id.as_str()));
        assert_eq!(
            job_group(&conn, "j1").as_deref(),
            Some(r.id.as_str()),
            "job promoted to the PARENT, not dropped to root (the SET NULL trap)"
        );
        // S is gone.
        assert!(matches!(
            get_group_conn(&conn, &s.id),
            Err(AppError::NotFound(_))
        ));

        std::fs::remove_dir_all(&dir).ok();
    }

    /// PROMOTION at root == root: S at root (parent NULL) with a child T and a job J.
    /// Deleting S sends T and J to root (NULL) — here promote-to-parent coincides with root.
    #[test]
    fn delete_root_group_promotes_children_to_root() {
        let (conn, dir) = test_db();
        let s = create_group_conn(&conn, "S", None).unwrap();
        let t = create_group_conn(&conn, "T", Some(&s.id)).unwrap();
        insert_job(&conn, "j1");
        move_job_conn(&conn, "j1", Some(&s.id)).unwrap();

        delete_group_conn(&conn, &s.id).unwrap();

        assert_eq!(group_parent(&conn, &t.id), None);
        assert_eq!(job_group(&conn, "j1"), None);
        assert!(matches!(
            get_group_conn(&conn, &s.id),
            Err(AppError::NotFound(_))
        ));

        std::fs::remove_dir_all(&dir).ok();
    }

    /// Count of (jobs + child-groups) under a group is conserved under its parent across
    /// a delete — promoted, none lost (ADR-019 Decision 3 post-condition, rule #9).
    #[test]
    fn delete_group_conserves_count_under_parent() {
        let (conn, dir) = test_db();
        let r = create_group_conn(&conn, "R", None).unwrap();
        let s = create_group_conn(&conn, "S", Some(&r.id)).unwrap();
        // Two child groups + three jobs under S.
        let _t1 = create_group_conn(&conn, "T1", Some(&s.id)).unwrap();
        let _t2 = create_group_conn(&conn, "T2", Some(&s.id)).unwrap();
        for j in ["j1", "j2", "j3"] {
            insert_job(&conn, j);
            move_job_conn(&conn, j, Some(&s.id)).unwrap();
        }

        let child_groups_before: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM groups WHERE parent_id = ?1",
                params![s.id],
                |r| r.get(0),
            )
            .unwrap();
        let jobs_before: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM jobs WHERE group_id = ?1",
                params![s.id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(child_groups_before, 2);
        assert_eq!(jobs_before, 3);

        delete_group_conn(&conn, &s.id).unwrap();

        // Everything that was under S is now under R — count conserved, none lost.
        let child_groups_after: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM groups WHERE parent_id = ?1",
                params![r.id],
                |r| r.get(0),
            )
            .unwrap();
        let jobs_after: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM jobs WHERE group_id = ?1",
                params![r.id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(child_groups_after, child_groups_before);
        assert_eq!(jobs_after, jobs_before);

        // Post-condition: no job's group_id points at a non-existent group.
        let dangling: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM jobs WHERE group_id IS NOT NULL \
                 AND group_id NOT IN (SELECT id FROM groups)",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(dangling, 0, "no dangling group_id after promotion");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn delete_group_absent_is_not_found() {
        let (conn, dir) = test_db();
        assert!(matches!(
            delete_group_conn(&conn, "no-such-group"),
            Err(AppError::NotFound(_))
        ));
        std::fs::remove_dir_all(&dir).ok();
    }
}
