# ADR-019: Job organization — groups as a tree of metadata

**Status:** accepted · 2026-08-08
**Relates:** [ADR-017](adr-017-operation-log.md) (the sibling precedent: a **logical model decoupled
from disk** — the op log materializes snapshots, never a recompute-from-backend recipe),
[ADR-004](adr-004-sqlite-storage.md) (the SQLite-owns-metadata storage this extends),
[ADR-003](adr-003-execution-backend.md) / [ADR-009](adr-009-process-orchestration.md) (the isolated
`job_dir` + crash-reconciliation + killpg-by-cwd this invariant protects), CLAUDE.md **domain rule
#3** (one isolated job directory per calculation).
**Stages:** ROADMAP **Phase 4.7**. This ADR is the **design** — the model + the group lifecycle. NO
schema is applied here; the migration and CRUD are Phase 4.7 units (§Roadmap). Ingested as a design
decision settled in a Claude web/desktop session (per CLAUDE.md "Division of labor").

## Context

The job list is a **flat list**. It works for a handful of jobs, but stops scaling the moment a
researcher runs several reaction studies in parallel: a study like *"reduction of ibuprofen by
LiAlH₄"* spawns dozens of jobs (GOAT ensembles, DFT re-opts, scans, references) that belong together,
and studies themselves nest (per-substrate, per-face, per-method sub-folders). The app needs
**unlimited nested grouping** to keep the list navigable, plus the ability to **delete a job** (today
there is none — every job created lives forever in the list).

This ADR fixes the **organization model**. Two failure modes it must avoid: (1) letting the logical
grouping leak into the filesystem and break domain rule #3; (2) a group operation that destroys job
records or job directories.

## Decision 0 — LOAD-BEARING INVARIANT: groups are a TREE OF METADATA IN SQLITE, never a filesystem hierarchy

State this first and loudly, because every other decision depends on it and a future contributor will
be tempted to "just make folders on disk":

> **A job group is a node in a tree stored in SQLite. It is NOT a directory. A job keeps its own
> isolated `job_dir` (domain rule #3) at whatever path it was created at, wherever it sits in the
> group tree. Moving a job between groups is `UPDATE jobs.group_id = ?` — ZERO filesystem operations.
> The physical directory path NEVER follows the logical group.**

This is the exact sibling of [ADR-017](adr-017-operation-log.md)'s decision: there, editor history is
a *materialized logical model* that must not become a function of the installed ASE version; here, job
organization is a *logical model* that must not become a function of the on-disk layout. In both, the
logical model is decoupled from the physical/backend representation on purpose.

**Why the invariant is load-bearing** (what breaks if the dir followed the group):

- **Domain rule #3** (one isolated job dir, littered with ORCA scratch, cleaned post-run) assumes a
  job's directory is stable and owned by that job. A move-on-disk would race the running process,
  scratch files, and post-run cleanup.
- **Crash reconciliation** ([ADR-009](adr-009-process-orchestration.md)) reconciles jobs against their
  recorded `job_dir` on restart. If the path moved because the user dragged a folder in the UI, the
  reconciler would look in the wrong place.
- **killpg-by-cwd** — cancellation identifies a run's process group via its working directory. A moved
  cwd would break cancellation of a running job.

So the group tree is pure metadata; the `job_dir` column is the physical truth and the two never
couple. Grouping is free to be reorganized arbitrarily because it touches no bytes on disk.

## Decision 1 — Schema (proposed; the migration is a Phase 4.7 unit, NOT applied here)

A single generic `groups` table (adjacency list) plus one nullable FK on `jobs`:

```sql
CREATE TABLE groups (
    id         TEXT PRIMARY KEY,
    name       TEXT NOT NULL,
    parent_id  TEXT REFERENCES groups(id) ON DELETE …,   -- see Decision 3; NOT cascade
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);
-- parent_id NULL = a root-level group; unlimited nesting via the parent chain.

ALTER TABLE jobs ADD COLUMN group_id TEXT REFERENCES groups(id) ON DELETE SET NULL;
-- group_id NULL = ungrouped (the root / "All jobs" view). ONE group per job.
```

**One group per job — a tree, NOT tags.** `jobs.group_id` is a single nullable FK, not a
many-to-many join. A job lives in exactly one folder (or none). Tags / multi-membership are a
different feature with a different table; deliberately **not** this. (If multi-membership is ever
wanted, it is additive — a `job_tags` join table — and does not disturb this tree.)

## Decision 2 — Adjacency list (`parent_id`), not closure-table / materialized-path

The tree is stored as a **parent pointer** on each node. Rejected alternatives, recorded so this is
not relitigated:

- **Closure table** (a row per ancestor/descendant pair) — makes "all descendants" an O(1) indexed
  read, but every move/rename rewrites a batch of closure rows, and it is a second table to keep
  consistent. Overkill at this scale.
- **Materialized path** (store `"/root/study/face/"` per node) — cheap descendant queries by `LIKE`,
  but **rename/move rewrites the path of every descendant**, exactly the disk-coupling smell this ADR
  rejects in the logical layer too.

At this scale — **hundreds of jobs and groups, not millions** — `parent_id` gives cheap
rename/move/reparent (touch one row) and simple reads (walk parents, or one recursive CTE for a
subtree). The only thing it is slower at is "all descendants," which the UI does by a small in-memory
tree-walk over a set this size. **If** a fast bulk-descendant query is ever needed (measure first —
the project's rule #10 discipline applied to our own scale assumptions), revisit with a closure table
then; do not pay for it now.

## Decision 3 — Group-delete = PROMOTION, never a destructive subtree cascade (jobs-survive)

Deleting a **group** must never delete **jobs**, and must never delete a **subtree**. Two mechanisms,
kept distinct:

- **Jobs orphan to root by construction.** `jobs.group_id … ON DELETE SET NULL`: when a group row is
  removed, its jobs' `group_id` becomes NULL (ungrouped / root). Job records and `job_dir`s are never
  touched. This is the same **jobs-survive** rule as ADR-007 (delete a reaction) and ADR-018 (delete a
  reference) — grouping is metadata over jobs, and removing the metadata leaves the work intact. SQLite
  FK enforcement is **ON** in this build (`SQLITE_DEFAULT_FOREIGN_KEYS=1`, measured — see the v14 note
  in `db.rs`), so `ON DELETE SET NULL` is **load-bearing**, not decorative.
- **Sub-groups are PROMOTED, not cascaded.** Deleting a non-empty group re-parents its **child
  groups** to the deleted group's own parent (or to root, if it was root-level), and its jobs likewise
  become children of that parent. A deleted folder's contents rise one level; nothing under it is
  destroyed.

**Implementation consequence, named so the 4.7 unit builds it right:** `groups.parent_id` is
therefore **NOT `ON DELETE CASCADE`** (cascade would delete the whole subtree — the opposite of
promotion). The promotion is performed **in the delete command**: re-parent the deleted group's child
groups and jobs to `deleted.parent_id` **first**, then remove the row. The post-condition (rule #9):
after a group delete, (a) no job's `group_id` points at a non-existent group, (b) the count of jobs +
child-groups is conserved (promoted, none lost), (c) no `job_dir` was read or written.

## Decision 4 — Generic tree, jobs-only FK now; other domains deferred with zero churn

The `groups` table is **generic** (it references nothing job-specific). Only **`jobs.group_id`**
exists now. Adding `molecules.group_id` or `reactions.group_id` later — grouping molecules or reactions
in the **same tree** — is a pure additive FK on the same table, no churn to the tree or its CRUD. That
extension is **explicitly deferred, not designed here**: this ADR commits only to jobs-in-groups.

## Decision 5 — Orthogonal to the pipeline FKs

`group_id` is an **independent dimension** from the existing job-to-job links. A single job can
simultaneously:

- sit in a group (`group_id`),
- be a DFT re-opt child of a GOAT job (`source_ensemble_job_id` / `source_conformer_index`, ADR — D2a,
  migration v15),
- be a reaction's reference job (`reaction_reference_jobs`, ADR-018, v14),
- be attached to a pathway (`pathway_id`, ADR-007, v13).

Grouping does not overload or replace any of these; it is a separate axis (organization) laid beside
the scientific-relationship axes (provenance, reference, pathway). Recorded here so no one conflates
"what folder is this job in" with "what is this job derived from."

## Naming

The **table/domain term is `group`**; the **UI metaphor is "folder"** (a tree sidebar the user reads
as folders). One term each, fixed here so the code and the UI copy do not drift into synonyms
("collection", "project", "bucket").

## Boundary — what this ADR is NOT

This ADR is the **organization model + the group lifecycle** (create / rename / move / delete-with-
promotion). It does **not** design **deleting an actual JOB** — DB-only vs DB+files, the guard on a
`running` job (killpg + cwd-sweep), which FK links get NULLed. That is the separate **Phase 4.7.1**
unit; it is independent of grouping and can land first.

## Consequences

- **Unlimited nesting**; cheap rename / move / reparent (one-row `UPDATE`s); **disk is never touched**
  by any grouping operation — rule #3, crash-reconciliation, and killpg-by-cwd all preserved.
- The UI grows a **small tree-walk** over a hundreds-scale set (a sidebar), plus move/rename actions.
- **New-job creation should become group-aware:** a job created while a group is "active" inherits that
  `group_id` (NULL if none). Flagged here as a **Phase 4.7.3 UI concern**, not decided in this ADR.
- **Result-derived children inherit the SOURCE's group by default** (Phase 4.7.5): an OptTS / NEB-TS /
  DFT-re-opt child (and connectivity, which routes through OptTS) defaults its `group_id` to the source
  job's current `group_id`, so a refined child lands in the source's folder. An ungrouped source → an
  ungrouped child (never a fabricated root). A **default**, and **orthogonal** to the pathway inherit
  (Decision 5) — a source with both gives a child with both. See [modules/groups.md](../modules/groups.md).
- No performance concern at this scale; the adjacency-list choice is revisitable (Decision 2) if a real
  measurement ever demands bulk-descendant queries.
