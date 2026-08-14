# Module: job groups UI (the Jobs-view sidebar)

**Status:** built (Phase 4.7.3–4.7.4, closing Phase 4.7). The tree sidebar + deep filter +
Move-to picker + assign-on-create, plus a search/status filter, over the Jobs view. Unit 2a adds a
reusable `<GroupSelect>` + an **explicit destination-group picker on New Job**; unit 2b mounts that
picker at the **four result-derived spawn sites** (Scan / NEB / Connectivity / re-opt), seeded from
the **source** job's group — the explicit group picker is now **COMPLETE**.
**Files:** `src/groups/tree.ts` (pure group logic), `src/groups/tree.test.ts`,
`src/groups/search.ts` (pure search/status logic), `src/groups/search.test.ts`,
`src/groups/GroupSelect.tsx` (+`.test.tsx`, the reusable picker + pure helpers),
`src/groups/useGroupPicker.ts` (the shared derived-spawn hook + `useJobGroupId` seed fetch),
`src/groups/GroupSidebar.tsx`, `src/screens/JobsScreen.tsx` (two-pane host),
`src/App.tsx` (lifted selection), `src/screens/NewJobScreen.tsx` (the New Job picker),
the four spawn sites (`scan/ScanProfilePanel.tsx`, `reactions/NebBandPanel.tsx`,
`spectrum/ConnectivityPanel.tsx`, `screens/JobDetailScreen.tsx`),
group + filter styles in `src/styles/app.css`.
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
2. **The New Job destination picker's default** — `App` derives `activeGroupId = selection.kind ===
   "group" ? selection.id : null` and passes it to `NewJobScreen`, where it is the **default** of the
   explicit `<GroupSelect>` (overridable — see "Explicit destination-group picker" below).
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
sidebar beside the table. Each row keeps its existing click (open detail) + Cancel / Run / Delete,
and gains a **"Move…"** action → a native `<select>` of all groups **plus "(ungrouped)"** →
`move_job(job.id, group | null)` (a job has no cycle concern, so every group is a valid target). A
guard effect resets the selection to "All jobs" if the selected group vanished (deleted elsewhere),
so the filter never points at a non-existent group.

## Search / status filter, composed on the group filter (`search.ts`, Phase 4.7.4)

The rendered rows are **`filterJobsBySearch(filterJobsByGroup(jobs, selection, groups), query,
statuses)`** — the group filter runs **first**, the search/status filter **second**, so search
narrows WITHIN the selected group's subtree and never re-widens to all jobs. Both are pure functions
over `Job[]`; the composition is the single load-bearing invariant here (a regression would show jobs
outside the selected group when a search is active — a `search.test.ts` composition test pins it).

`filterJobsBySearch(jobs, query, statuses)` (`src/groups/search.ts`, unit-tested, no backend):
- **query** — case-insensitive **substring** (not FTS5, not fuzzy) over `title` OR the job's
  **method**, where method is `parseMethodLine(input_content)` = the first `!` keyword line stripped
  of its `!` (client-side, NOT a column / SQL — e.g. `"! r2SCAN-3c Opt Freq"` → `"r2SCAN-3c Opt
  Freq"`, `""` if there is no `!` line);
- **status** — the job's status ∈ `statuses`, or all when `statuses` is empty;
- ANDed; **empty query + empty statuses ⇒ identity** (drops nothing).

The search box + the seven **status chips** (draft…cancelled toggles) + a Clear affordance are
**local** state on `JobsScreen` (not lifted to `App`, not persisted). The empty-state is three-way:
"No jobs yet" (`jobs` empty) vs "No jobs in this group" (`groupJobs` empty) vs **"No jobs match this
filter"** (the search over-narrowed) — so an over-narrow combination reads as a filter, not an empty
app.

## The reusable picker (`GroupSelect.tsx`)

`<GroupSelect groups value onChange>` is a **purely presentational** controlled `<select>` of the
existing groups plus an "(ungrouped)" choice — no `invoke` / store / `move_job`; the consumer decides
what to do with the emitted value. Two invariants, both pure-tested:

- **The sentinel never escapes.** A `<select>` value can't be `null`, so the ungrouped option needs a
  sentinel (`ROOT_OPTION = "__root__"`) — but `onChange` emits a **clean `string | null`** via
  `groupIdFromOptionValue` (`"__root__"` → `null`, else the id); `value` is shown via
  `optionValueFromGroupId` (`null` → `"__root__"`). `ROOT_OPTION` lives **here** and is imported by
  `JobsScreen` and `GroupSidebar` (the two duplicate local copies were consolidated onto this export).
- **The New-Job assign decision is a pure function** — `resolveGroupAssignment(groupTouched,
  pickedGroupId)` (see below).

## Explicit destination-group picker on New Job (`NewJobScreen.tsx`, unit 2a)

The New Job form mounts a `<GroupSelect>` (a "Group" field beside the title) so the user **explicitly
chooses** where the job lands. It supersedes the 4.7.3 implicit active-group assign:

- **Default follows the active group until touched.** `pickedGroupId` initializes to `activeGroupId`,
  and an effect keeps it in sync **while `groupTouched` is false** — so switching the sidebar group
  before creating updates the default (a `groupTouched` flag, NOT a naive `useState(activeGroupId)`
  init that would freeze on the first render). The first explicit pick sets `groupTouched` and freezes
  the default.
- **`groups` is loaded locally** (`list_groups` on mount), mirroring how `JobsScreen` loads its own —
  `App` holds only the `GroupSelection`, not the list.
- **The assign is `resolveGroupAssignment(groupTouched, pickedGroupId)`**, then `move_job` when it says
  assign — the **same `null`-for-ungrouped semantics** as `JobsScreen.moveJob`, applied to **all three**
  create paths (`create_job` normal + GOAT "Find conformers" + the builder `create_neb_job`). The rule:
  - **untouched + no active group** (`null`) → **NO-OP**: exactly today's behavior, and — load-bearing
    — it does **not** clobber a create-path default, e.g. a NEB-TS's **reactant-inherited group**
    (Unit 1, stamped in Rust);
  - **untouched + an active group** → assign it (the 4.7.3 assign-on-create);
  - **an explicit pick** → the picker **WINS**, including "(ungrouped)" (`null`), overriding any default.

  Bite-verified in `GroupSelect.test.tsx`: the four `resolveGroupAssignment` cases (the touched-`null`
  override is a distinct bite — it must NOT fall into the untouched no-op), plus the sentinel-leak and
  render bites. Pure composition of `move_job`, **no Rust change**.

## The derived-spawn picker (unit 2b)

The same `<GroupSelect>` is mounted at the **four result-derived spawn sites**, so a refined/re-opt
child's destination is overridable at the moment it is spawned — the mirror of the New Job picker, with
**one crucial difference in the seed**:

> **New Job seeds the picker from the ACTIVE sidebar group; a derived child seeds it from its SOURCE
> job's group.** (This is the same "defaults-backwards" invariant as the Rust Unit-1 inherit — a refined
> child belongs with the job it came from, not with whatever folder is selected in the sidebar.)

The wiring is factored into a thin hook `useGroupPicker(seedGroupId)` (`src/groups/useGroupPicker.ts`) —
impure glue (like `useContainerWidth`) composing the *proven* pure `resolveGroupAssignment` + `move_job`;
`groups` via `list_groups`, `pickedGroupId` + `groupTouched` + the follow-until-touched sync, and an
`assignPicked(jobId)` the site calls after each child create. A companion `useJobGroupId(jobId)` fetches
the source's `group_id` (the seed) for the three sites that hold only the source id. The seed **FOLLOWS**
until touched, so a seed that arrives after the async fetch still lands.

The four sites (each already fetched its source Job at the action — the picker only adds the seed read
and the `assignPicked` call; **no Rust/command/param change**, override = the existing `move_job`):

| Site | Source (seed) | Children moved |
|---|---|---|
| `scan/ScanProfilePanel.tsx` — Refine with OptTS | `useJobGroupId(jobId)` | the OptTS child |
| `reactions/NebBandPanel.tsx` — Refine TS with OptTS | `useJobGroupId(jobId)` | the OptTS child |
| `spectrum/ConnectivityPanel.tsx` — Verify connectivity | `useJobGroupId(tsJobId)` | **both** forward + backward |
| `screens/JobDetailScreen.tsx` — Re-optimize top-k | `job.group_id` (in hand) | **all k** fan-out children |

The `resolveGroupAssignment` no-op rule is what makes the redundant `move_job` on an untouched grouped
source harmless and, crucially, keeps an untouched picker from clobbering the Rust Unit-1 default on an
**ungrouped** source (no-op → the child keeps the source's group); an explicit "(ungrouped)" pick
force-moves it out. No equality-gating of the `move_job` (frontend authoritative, Rust the fallback).

## The one Rust touch

`Job.group_id` was surfaced on the model (`models/job.rs`: appended to `COLUMNS` at row index 14,
`from_row`, and the struct field) so `list_jobs`/`get_job` carry it. **No new command, no migration,
no schema change** — the v16 column and all group commands already exist (4.7.2). `types.ts` gained
`Job.group_id: string | null` and the `Group` interface.

## Out of scope (deliberately)

Drag-and-drop; molecules/reactions grouping; persisting the active group or the search/status filter
across restarts; a backend/FTS5 query (search is frontend substring over the in-memory list).
