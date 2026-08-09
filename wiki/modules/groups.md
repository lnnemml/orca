# Module: job groups (data layer)

**Status:** data layer built (Phase 4.7.2). Schema v16 + Rust CRUD. **No UI yet** — the
tree sidebar (folder metaphor) is Phase 4.7.3.
**Files:** `src-tauri/src/models/group.rs`, `src-tauri/src/commands/groups.rs`, the v16 arm in
`src-tauri/src/db.rs`.
**Decision record:** [ADR-019](../architecture/adr-019-job-organization.md) (Decisions 0–5).

## What a group is

A **group** is one node in the job-organization tree; the UI metaphor is a **"folder"**. Groups
are **grouping metadata over jobs, NEVER a filesystem hierarchy** (ADR-019 Decision 0). A job keeps
its isolated `job_dir` (domain rule #3) wherever it sits in the tree; **moving a job between groups
is a one-row `UPDATE jobs.group_id` with ZERO filesystem operations**. The physical directory path
never follows the logical group — so rule #3, crash-reconciliation, and killpg-by-cwd are all
preserved. `commands/groups.rs` makes **no filesystem calls at all** (production code): grep it for
`std::fs` / `job_dir` and the only hits are the `#[cfg(test)]` temp-dir cleanup and doc-comments.

## Schema (v16)

```sql
CREATE TABLE groups (
    id         TEXT PRIMARY KEY,
    name       TEXT NOT NULL,
    parent_id  TEXT REFERENCES groups(id),   -- adjacency list; NOT ON DELETE CASCADE
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);
ALTER TABLE jobs ADD COLUMN group_id TEXT REFERENCES groups(id) ON DELETE SET NULL;
```

- **Adjacency list** (`parent_id`), not closure-table / materialized-path (ADR-019 Decision 2) —
  cheap rename/move/reparent (touch one row) at this scale (hundreds, not millions).
- **One group per job** — `jobs.group_id` is a single nullable FK, a **tree not tags** (Decision 1).
  `NULL` = ungrouped (root / "All jobs"). The `group_id` axis is **orthogonal** to the pipeline FKs
  `pathway_id` / `source_ensemble_job_id` / `reaction_reference_jobs` (Decision 5) — a job can sit in
  a group AND be a re-opt child AND be a reaction reference simultaneously.
- **`parent_id` is deliberately NOT `ON DELETE CASCADE`** — a cascade would destroy a subtree, the
  opposite of the promotion below.

FK enforcement is **ON** in this build (`SQLITE_DEFAULT_FOREIGN_KEYS=1`, measured — see the v14 note
in `db.rs`), which makes both rules below load-bearing rather than advisory.

## The two load-bearing rules

### 1. Cycle guard on `move_group`

Moving a group under its own descendant makes a node its own ancestor — any parent-walk then loops
forever and the tree is corrupt. `move_group_conn(id, new_parent_id)` **refuses** a self-parent
(`new_parent_id == id`) or a descendant-parent, detected by walking UP the new parent's chain: if
`id` is reached, the new parent is a descendant of `id`. The walk is **bounded by the group count**,
so even a pre-existing corrupt cycle can't hang the check (it returns a "hierarchy is corrupt" error
past the bound instead of spinning). `new_parent_id = None` moves to root (always allowed).

### 2. Promotion-to-parent on `delete_group`

`jobs.group_id` is `ON DELETE SET NULL`, so a naive `DELETE FROM groups` would drop that group's jobs
to **root** — WRONG for a non-root group. ADR-019 Decision 3 mandates **promotion**: a deleted
folder's contents rise **one level**, to the deleted group's own parent. `delete_group_conn(id)`, in
**one transaction, in order**:

1. resolve `parent := groups.parent_id` of the deleted group (may be `NULL` = root);
2. `UPDATE groups SET parent_id = <parent> WHERE parent_id = ?id` — promote child **groups**;
3. `UPDATE jobs SET group_id = <parent> WHERE group_id = ?id` — promote **jobs EXPLICITLY** to the
   parent (do **not** rely on `ON DELETE SET NULL`, which would send them to root, not the parent);
4. `DELETE FROM groups WHERE id = ?id`.

`ON DELETE SET NULL` is only the **fallback** for a group row removed outside this command. Because
child groups' `parent_id` has no cascade, the row delete **RESTRICT-fails** unless the children are
re-parented first — the same load-bearing cleanup shape as job deletion (Phase 4.7.1).

**Post-conditions (rule #9, asserted in tests):** after a delete, (a) no job's `group_id` points at a
non-existent group; (b) the count of (jobs + child-groups) that were under `id` is conserved under
`parent` — promoted, none lost; (c) no `job_dir` / filesystem is touched anywhere.

## CRUD surface (`commands/groups.rs`)

Internal `*_conn(&Connection, …)` helpers (unit-testable, not-found-first everywhere;
`NotFound("group {id}")` for an absent group), each with a thin `#[tauri::command]` wrapper that locks
`DbState`:

| Command | Behavior |
|---|---|
| `create_group(name, parent_id?)` | new uuid; `parent_id` (if `Some`) must exist or `NotFound` (no orphan) |
| `list_groups()` | all groups (`ORDER BY created_at, id`); the UI builds the tree from `parent_id` |
| `rename_group(id, name)` | exists-check, `UPDATE name` |
| `move_group(id, new_parent_id?)` | cycle-guarded reparent (rule 1); `None` = to root |
| `move_job(job_id, group_id?)` | job + (if `Some`) group must exist; `UPDATE jobs.group_id`; `None` = ungroup |
| `delete_group(id)` | delete with promotion (rule 2) |

## Tests (all in `commands::groups::tests`, in-memory DB)

Happy paths (create/rename/list, move-to-root, move-job set/clear); `create_group` under a missing
parent and `move_job` to a missing group → `NotFound`; `delete_group_absent_is_not_found`. Two
**bite-verified negative controls**: `move_group_refuses_cycle` (self- and descendant-parent both
error; parents unchanged) and `raw_delete_without_reparent_hits_restrict_fk` (a bare
`DELETE FROM groups` with a child trips the RESTRICT FK — proves enforcement is ON and the promotion
re-parent is required). The headline `delete_group_promotes_children_to_parent_not_root` proves a job
lands at the **parent, not root** (the `ON DELETE SET NULL` trap), with
`delete_group_conserves_count_under_parent` for the conservation + no-dangling-FK post-condition, and
`delete_root_group_promotes_children_to_root` for the parent==root case. Plus the migration test
`migrate_v15_to_v16_adds_groups_table_and_group_id_and_preserves_jobs` in `db::tests`.

## Deliberately out of scope

Molecules/reactions `group_id` (ADR-019 Decision 4 — jobs-only FK now, additive later with zero
churn); tags / multi-membership (one group per job); closure-table / materialized-path (Decision 2);
the tree-sidebar UI + assign-on-create (Phase 4.7.3); filter/search (4.7.4).
