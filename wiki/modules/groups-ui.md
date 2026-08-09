# Module: job groups UI (the Jobs-view sidebar)

**Status:** built (Phase 4.7.3). The tree sidebar + deep filter + Move-to picker +
assign-on-create over the Jobs view.
**Files:** `src/groups/tree.ts` (pure logic), `src/groups/tree.test.ts`,
`src/groups/GroupSidebar.tsx`, `src/screens/JobsScreen.tsx` (two-pane host),
`src/App.tsx` (lifted selection), `src/screens/NewJobScreen.tsx` (assign-on-create),
group styles in `src/styles/app.css`.
**Data layer:** [groups.md](groups.md) (schema v16 + the CRUD commands this UI composes).
**Decision record:** [ADR-019](../architecture/adr-019-job-organization.md) (Decision 3 +
the "New-job creation should become group-aware" consequence).

## The single source of truth

The active selection is **one `GroupSelection` state lifted to `App`** —
`{ kind: "all" } | { kind: "ungrouped" } | { kind: "group"; id }`. It is React-only
(**not persisted** across restarts) and lives in `App` because it must survive a tab
switch to "New Job" (assign-on-create reads it there). That one selection drives **three**
things, so they can never disagree:

1. **The deep jobs filter** — `JobsScreen` renders `filterJobsByGroup(jobs, selection, groups)`.
2. **Assign-on-create** — `App` derives `activeGroupId = selection.kind === "group" ? selection.id
   : null` and passes it to `NewJobScreen`; a newly created job inherits it.
3. **The Move-to exclusion** — a group's move picker offers `moveTargetsFor(groups, id)`.

Filter (1) and exclusion (3) both derive from the **same** pure `descendantGroupIds` (one subtree
walk, reused — not two hand-rolled walks).

## Pure logic (`src/groups/tree.ts`, unit-tested — no React, no `invoke`)

- `buildGroupTree(groups)` — flat adjacency list (`parent_id`) → nested `GroupNode[]`.
  Deterministic order (name, then created_at, then id). **Orphan defense:** a group whose
  `parent_id` points at a missing group is treated as a **root** — nothing vanishes.
- `descendantGroupIds(groups, rootId)` — `rootId` + all descendants. **Cycle-safe** (a `visited`
  set; the backend prevents cycles, but the UI must never hang on bad data).
- `filterJobsByGroup(jobs, sel, groups)` — `"all"` → all; `"ungrouped"` → `group_id == null`;
  `"group"` → **DEEP**: `group_id ∈ descendantGroupIds(sel.id)` (the group OR any descendant, a
  ratified decision — NOT a shallow `group_id === sel.id`).
- `moveTargetsFor(groups, movingGroupId)` — all groups EXCEPT `movingGroupId` and its descendants,
  so the picker can never offer a target that would create a cycle. (Moving to root is offered
  separately by the UI as "(root)".)

## The sidebar (`GroupSidebar.tsx`)

Renders the two special roots — **"All jobs"** (no filter) and **"Ungrouped"** (`group_id IS
NULL`) — then `buildGroupTree(groups)` with per-node **expand/collapse**. Clicking a node sets the
lifted `GroupSelection`. Per-group inline actions (revealed on row hover; **no drag-and-drop** —
an explicit picker is the ratified choice, DnD over WebKitGTK deferred):

- **New subgroup** (`＋`) / top-level **"＋ New group"** — an inline name input →
  `create_group(name, parentId | null)`.
- **Rename** (`✎`) — an inline input (Enter/blur commits, Escape cancels) → `rename_group(id, name)`.
- **Move to…** (`⇄`) — a native `<select>` of `moveTargetsFor(groups, id)` **plus "(root)"** →
  `move_group(id, target | null)`. The `<select>` is auto-themed by the element-level WebKitGTK fix
  (`debugging/003`) — no per-control styling needed.
- **Delete** (`🗑`) — a `confirm` (`@tauri-apps/plugin-dialog`, warning; copy: "sub-groups and jobs
  move up to the parent — no job is deleted") → `delete_group(id)`. If the deleted group was
  selected, the selection falls back to "All jobs".

Every mutation calls `onChanged()` (reload groups + jobs). The sidebar re-implements **none** of the
cycle guard beyond hiding self+descendants from the Move-to targets — the backend `move_group` is the
source of truth (it re-validates and would reject a cycle); the UI just avoids offering an invalid
target.

## Two-pane Jobs view (`JobsScreen.tsx`)

`JobsScreen` owns the `groups` list (`list_groups`) and the jobs list (`list_jobs`), and lays the
sidebar beside the table. It renders `filterJobsByGroup(jobs, selection, groups)` (not the raw list),
with a header showing the active selection's name and an empty-state that distinguishes "no jobs at
all" from "no jobs in this group". Each row keeps its existing click (open detail) + Cancel / Run /
Delete, and gains a **"Move…"** action → a native `<select>` of all groups **plus "(ungrouped)"** →
`move_job(job.id, group | null)` (a job has no cycle concern, so every group is a valid target). A
guard effect resets the selection to "All jobs" if the selected group vanished (deleted elsewhere),
so the filter never points at a non-existent group.

## Assign-on-create (`NewJobScreen.tsx`)

`App` passes `activeGroupId`. After `create_job` returns the new `Job`, `assignActiveGroup(id)` calls
`move_job(id, activeGroupId)` when it is non-null — for **both** the normal create path and the
"Find conformers" GOAT quick-action. Pure composition of the existing command, **no Rust change**.
When "All jobs"/"Ungrouped" is active, `activeGroupId` is null and the new job stays ungrouped.

## The one Rust touch

`Job.group_id` was surfaced on the model (`models/job.rs`: appended to `COLUMNS` at row index 14,
`from_row`, and the struct field) so `list_jobs`/`get_job` carry it. **No new command, no migration,
no schema change** — the v16 column and all group commands already exist (4.7.2). `types.ts` gained
`Job.group_id: string | null` and the `Group` interface.

## Out of scope (deliberately)

Filter/search box (4.7.4); drag-and-drop; molecules/reactions grouping; persisting the active group
across restarts.
