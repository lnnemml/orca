# ADR-021 — Group export is a PROJECTION, never a rename of the canonical dirs

**Status:** accepted (Phase 4.7, unit 4.7.5, 2026-08-14).
**Context builds on:** [ADR-019](adr-019-job-organization.md) (groups = a tree of metadata in SQLite,
never a filesystem hierarchy; `job_dir` never follows the group), [ADR-020](adr-020-optts-refinement-source-agnostic.md)
(the fine OptTS/NEB source lineage is deliberately NOT persisted), [ADR-009](adr-009-process-orchestration.md)
(Rust owns file I/O), CLAUDE.md **domain rule #3** (one isolated job dir; never littered) and **rule #9**
(a post-condition that checks the result in OUR terms).

## Context

The canonical on-disk layout is `{data_dir}/jobs/<UUID>/` — one opaque UUID directory per calculation
(rule #3). That is correct for the app (stable, collision-free, crash-reconcilable) but **unreadable for
a human** who wants to share, archive, or inspect a study: `02586c8f-…/` says nothing about what it holds.
The historical workaround was a **manual rename** — copy the dir out and name it `HCN-opt/` by hand — and
that step is exactly where identity was lost: a slip swapped `HCN-opt` ↔ `HNC-opt` (constitutional isomers,
same formula CHN), and nothing caught it.

The need: **one action** that turns a job group into a self-contained, human-readable, UUID-traceable
directory tree plus a `manifest.json`, with **zero manual renaming**.

## Decision 0 — LOAD-BEARING: export is a PROJECTION; the canonical dirs and SQLite are NEVER touched

> **Export READS the canonical `<UUID>/` dirs and the SQLite rows and WRITES a fresh copy elsewhere.
> It never renames, moves, or deletes a canonical dir, and never mutates a row.** The source of truth
> stays the UUID dirs + the database; the export is a derived, throwaway artifact.

This is the sibling of ADR-019 Decision 0 (the group tree is metadata, never disk): there, *organization*
is decoupled from disk; here, the *human-readable view* is decoupled from the canonical store. Consequences:
no migration, no new column, no write into `data_dir/jobs`. The **inverted path guard** enforces the last
point — reusing `local_backend::path_is_within`, which the delete path uses to *confirm* a path is inside
the jobs root before removing it, export *refuses* a destination inside that root.

## Decision 1 — The manifest encodes ONLY persisted structure; honest-or-absent everywhere

The manifest (`ManifestV1`) is the identity-integrity artifact. It asserts **only** what the database can
prove:

- **Order = creation order.** Jobs are ordered by `created_at` (tie-broken by id) and each gets a
  zero-padded numbered prefix (`01_…`, width = digits of the group size) so a lexical sort of the exported
  dirs equals creation order. This is labelled **creation order, NOT logical/mechanistic order** — the app
  does not persist a reaction-step ordering.
- **Structure = the group tree + pathway membership.** `group_path` is the persisted `parent_id` chain
  (root → leaf); `pathway_id` is the persisted FK; the conformer-reopt FKs
  (`source_ensemble_job_id` / `source_conformer_index`) are carried where present.
- **The fine source DAG is NOT asserted.** The OptTS/NEB/connectivity "this TS came from that scan image"
  lineage is deliberately not persisted (ADR-020). The manifest's `notes` field says so **in the artifact
  itself**, so a reader is never misled into thinking an edge was proven.
- **`computed_identity` is ALWAYS null in v1.** A bare formula stamp is blind to constitutional isomers
  (HCN/HNC = CHN) — it would give *false confidence*, the very failure export exists to remove. The real
  connectivity-based identity stamp is a deferred follow-up; until then the field is present-but-null.
- **Results are honest-or-absent.** `results` (`energy_eh`, `imaginary_count`) come from the `results`
  row, or are `null` — never a fabricated `0.0`.

## Decision 2 — A title can never escape its directory leaf

`slugify` maps a free-form job title to an fs-safe ascii leaf: only `[a-z0-9]` survive (lowercased), every
other byte — `/`, `:`, a unicode arrow, control chars, `.` — becomes a separator, runs collapse to a single
`-`, ends trimmed, empty → `"job"`. This guarantees **no path separator, no `..` traversal, no non-ascii**
in a directory name, so a title like `a/b→c` becomes the flat leaf `a-b-c` and a `"/"` never creates a
subdirectory. Directory *uniqueness* is carried by the numbered prefix, not the slug, so two identically
titled jobs (`Opt`, `Opt`) get distinct dirs (`1_opt`, `2_opt`) and neither overwrites the other.

## Decision 3 — Curated vs full, and curated omissions are RECORDED, never silent

Two copy modes:

- **Curated** (default) — copies only a **pinned allowlist** of scientific artifacts (probed from real
  COMPLETED/SCAN/NEB dirs per rule #10; see `modules/group-export.md`). The heavy/derivable files a job's
  dir also contains (`.gbw`, `.densities`, cubes, `.tmp` scratch) are **listed in the manifest's
  `files.omitted`** for that job — honest-or-absent, never dropped without a record.
- **Full** — copies the whole dir verbatim; `omitted` is empty.

A **draft/never-run** job (`job_dir` NULL) is recorded in the manifest with empty files and gets **no
directory** (no empty-dir clobber).

## Decision 4 — Rule #9 post-condition: re-read the written manifest and check it in OUR terms

After writing `manifest.json`, export re-reads it from disk and asserts: every collected job appears
**exactly once**, every `exported_dir` is **unique**, every copied job's directory **exists on disk**, and
every uuid **resolves to a real `jobs` row**. Any mismatch fails loudly (`AppError::Internal`) rather than
leaving a plausible-but-wrong export. "Wrong" here = a dir whose name does not match its contents, or a
manifest asserting a structure the DB cannot prove — the exact failure class this unit removes.

## Boundary — what this ADR is NOT

- **No `.zip`/archive packaging** — a later unit; v1 emits a plain directory tree.
- **No `computed_identity` value** — deferred (Decision 1).
- **No source-DAG reconstruction** — the OptTS/NEB/connectivity parent→child edges are not persisted
  (ADR-020) and are not fabricated.
- **No "assign job to group at spawn time" picker** — that is a separate unit; assign-on-create already
  exists (4.7.3).
- **No migration, no new column, no canonical-dir write** — projection only.

## Consequences

- A researcher shares/archives a study with one action; the exported tree reads as folders and the manifest
  round-trips uuid ↔ name ↔ structure.
- The manual-rename identity slip is designed out: names are derived deterministically from titles, and the
  post-condition proves name↔content↔row consistency.
- The canonical store is inert during export — rule #3, crash-reconciliation, and killpg-by-cwd untouched.
- Full/curated is a mode flag; the curated allowlist is revisitable (it is pinned from a probe, not memory).
