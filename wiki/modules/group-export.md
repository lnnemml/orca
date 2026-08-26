# Module: group export

**What it is.** A first-class "Export…" action on a job group that **projects** the group (and all its
sub-groups) onto a self-contained, human-readable, UUID-traceable directory tree plus a `manifest.json`.
The canonical `{data_dir}/jobs/<UUID>/` dirs and the SQLite rows are the source of truth and are **never
touched** — export reads them and writes a fresh copy elsewhere. Design: [ADR-021](../architecture/adr-021-group-export-projection.md).
Relates to [groups](groups.md) (the tree it exports) and [groups-ui](groups-ui.md) (the sidebar it hangs off).

**Status:** landed (Phase 4.7 unit 4.7.5, 2026-08-14).

## Shape

- **Pure core** — `src-tauri/src/commands/export_group.rs`. No fs, no DB, no clock. Owns the naming rules,
  the manifest structs, the curated allowlist, and `build_manifest` (pure over in-memory rows). Exhaustively
  unit-tested and deterministic (`exported_at` is injected, not read from a clock).
- **Impure wiring** — `src-tauri/src/commands/export.rs`, the `export_group` Tauri command + its testable
  `export_group_conn` core: DB reads, the dir listing, the copy, and the post-condition.
- **Frontend** — `src/export/save.ts` `exportGroup(groupId, mode)` (folder picker → `invoke`), wired to a
  per-group inline Curated/Full chooser in `src/groups/GroupSidebar.tsx`; a native `message()` dialog reports
  the export path.

## The projection

1. Resolve the group **plus all descendant sub-groups** (a bounded downward `parent_id` walk, mirroring the
   cycle-safe walk in `commands/groups.rs`). A missing group is `NotFound`.
2. Collect every job whose `group_id` is in that set (a job lives in exactly one group — ADR-019). Each job's
   canonical dir is listed for its present files; a **draft/never-run** job (`job_dir` NULL) contributes a
   manifest entry with empty files and **no directory**.
3. `build_manifest` orders jobs by `created_at` (tie-broken by id), assigns `exported_dir =
   "{numbered_prefix}_{slug}"`, fills structure from persisted FKs only, and partitions each job's files into
   `included`/`omitted` for the copy mode.
4. Create a **fresh** export root under the chosen destination (`{group-slug}-export`, timestamp-and-counter
   disambiguated if the name exists — never clobber a prior export).
5. Copy each non-draft job's `included` files into its `exported_dir`; write `manifest.json`.
6. **Rule #9 post-condition:** re-read the written `manifest.json` and assert every collected job appears
   exactly once, every `exported_dir` is unique, every copied dir exists on disk, and every uuid resolves to a
   real `jobs` row. Any mismatch fails loudly.

## Naming rules (pure)

- **`slugify(title)`** — only `[a-z0-9]` survive (lowercased); every other byte (`/`, `:`, unicode arrows,
  control, `.`, whitespace) becomes a separator, runs collapse to a single `-`, ends trimmed, empty → `"job"`.
  Guarantees no path separator, no `..`, no non-ascii in a leaf. `a/b→c` → `a-b-c`; `../../etc/passwd` →
  `etc-passwd`. Directory uniqueness is carried by the prefix, so identical titles never collide.
- **`numbered_prefix(index, group_size)`** — 1-based, zero-padded to `len(str(group_size))`, so a lexical
  sort of the dirs equals creation order (`01_…`, `02_…`, … `10_…`, not `1_`, `10_`, `2_`).

## The curated allowlist (pinned from a probe — rule #10)

`ls -a` was run against real COMPLETED, SCAN, and NEB job dirs before pinning, so the globs match actual
filenames (not memory). `curated_match(filename)` selects:

- **Exact:** `input.inp`, `output.out`, `input.xyz`, `.exit_code` (the run, its log, the input geometry, the
  completion marker — rule #6).
- **Suffix:** `*.property.txt` (the authoritative results artifact; also catches fragment outputs like
  `input_atom53.property.txt`), `*.hess`, `*_trj.xyz` (all trajectory artifacts — subsumes `*_MEP_trj.xyz`,
  `input_MEP_ALL_trj.xyz`, `input_initial_path_trj.xyz`), `*.NEB.log`, `*.final.interp`, `*_converged.xyz`.
- **Special:** `*.relaxscan*.dat` (relaxed-scan curve), `input.[0-9]*.xyz` (per-step scan geometries
  `input.001.xyz` … — requires the digit, so `input.xyz` is matched only by the exact rule).

**Rule applied to `*_converged.xyz`, not `*_NEB-TS_converged.xyz`:** curated **never discards a
converged/final geometry**, so a NEB-CI run's `input_NEB-CI_converged.xyz` (the located TS) is exported, not
relegated to `omitted`. Nothing scratch ends in `_converged.xyz`.

Everything a job dir also holds — `.gbw`, `.densities`/`.densitiesinfo`, `orbital.*.cube`, `.tmp` scratch,
`stderr.log`, `input.allxyz`, `input.interp` — is **not** curated and is recorded in `files.omitted` (honest-
or-absent), never silently dropped. **Full** mode copies everything and leaves `omitted` empty.

## Manifest schema (`ManifestV1`)

```json
{ "manifest_version": 1, "exported_at": "…", "copy_mode": "curated" | "full",
  "source": { "group_id": "…", "group_name": "…" },
  "jobs": [ { "exported_dir": "01_hcn-opt", "uuid": "…", "display_name": "HCN opt",
              "job_type": null, "status": "parsed", "created_at": "…",
              "group_path": ["HCN reduction", "si-face"], "pathway_id": null,
              "lineage": { "source_ensemble_job_id": null, "source_conformer_index": null },
              "results": { "energy_eh": -93.42, "imaginary_count": 0 },
              "files": { "included": [ … ], "omitted": [ "input.gbw", … ] },
              "computed_identity": null } ],
  "notes": [ "Order = job creation order; structure = group tree + pathway membership. The fine
              OptTS/NEB/connectivity source lineage is not persisted and is NOT asserted here." ] }
```

Honesty invariants baked into the schema: `job_type` is **always null in v1** (there is no `job_type`
column, and a guessed type is the identity-mislabel error this unit removes — the reader infers type from
the curated `input.inp` and `results.imaginary_count`); `results` is `null` when unparsed (never `0.0`);
`computed_identity` is **always null in v1** (a formula stamp is blind to constitutional isomers — the
connectivity stamp is a follow-up); the `notes` line states the source DAG is not asserted. No field uses
`skip_serializing_if`, so a null field appears as JSON `null` (present, not omitted) — the post-condition
re-reads this file.

## Guards & quirks

- **Inverted path guard** — reuses `local_backend::path_is_within`; export **refuses** a destination inside
  `data_dir/jobs` (the delete path uses the same function to *confirm* a path is inside before removing it).
- **No migration, no new column, no canonical-dir write** — projection only.
- **Timestamps** come from SQLite (`datetime('now')` / `strftime`), matching the app's `created_at` format,
  so no new date/time dependency was added.
- **Copy is best-effort per file** — a source file that vanished between listing and copy is skipped, still
  recorded as intended-`included` in the manifest.

## Single-job export — the sibling (`export_job`)

Exporting ONE open job reuses this exact machinery, minus the group tree. `export_job(job_id, dest_parent,
copy_mode)` (a testable `export_job_conn` core, mirroring `export_group_conn`) reads the one `jobs` row + its
`results` row, then reuses the **same** `reject_if_in_data_dir`/`path_is_within` guard, `fresh_export_dir`,
the shared `copy_manifest_job_dirs` copy loop, and `verify_export_postcondition` (one job). The export dir is
**named after the job** — `{slug(title)}-export/{slug(title)}/…` — with **no numeric prefix** (a lone job needs
no ordinal). The manifest is `build_single_job_manifest` (the pure sibling of `build_manifest`, sharing the
`manifest_job_entry` per-job construction), still a `ManifestV1`.

- **`source.group` is nullable for the single-job case.** `ManifestSource.group_id`/`group_name` are now
  `Option<String>`: a grouped job names its own group, an **ungrouped** job carries `null` — never a fabricated
  group (honest-or-absent). A group export always fills `Some(...)`, which serializes to the same bare string,
  so a group manifest's JSON is **byte-identical** and `verify_export_postcondition` is reused unchanged.
- Same invariants as the group export: curated omissions recorded (not dropped), a draft/never-run job
  (`job_dir` NULL) is a manifest entry with no artifact dir, the canonical `<UUID>/` dir + DB rows are untouched,
  no migration. UI: an **"Export Folder…"** action beside "Open Folder" on the job detail screen, with the same
  Curated/Full chooser and the path toast (`src/export/save.ts` `exportJob`).

## Multi-job selection export — the third projection (`export_selection`)

Exporting an **explicit, possibly cross-group set of hand-picked jobs** — the third export projection beside
the group and single-job forms. Driven from a Jobs-view multi-select (see [groups-ui](groups-ui.md)); the pure
core is `build_selection_manifest`, the impure wiring is `export_selection_conn` + the `export_selection`
Tauri command. A selection is **not a group**, so two identity-critical things differ from the group export
(both live in `build_selection_manifest`, the exact failure ADR-021 exists to remove):

- **`source.group` is null by design.** `ManifestSource { group_id: None, group_name: None }` — never the
  first job's group. Fabricating a source group for a hand-picked set is false provenance; the manifest also
  carries a second note, the **`SELECTION_NOTE`**, stating in the artifact itself that this is a selection, not
  a group, and that `source.group` is null by design. (A selection manifest therefore has **two** notes —
  `HONESTY_NOTE` + `SELECTION_NOTE` — where a group/single-job manifest has only the `HONESTY_NOTE`.)
- **Each job's `group_path` resolves against ALL groups**, not a subtree. `build_selection_manifest` builds
  its `by_id` map from `all_group_nodes(conn)` (`SELECT id, name, parent_id FROM groups`), so a job in ANY
  group keeps its full `group_path`. A subtree-scoped lookup would silently leave a cross-group job's
  `group_path` empty — losing its real provenance (the negative control in
  `selection_resolves_each_group_path_against_all_groups`).

Ordering/numbering are the **same** as the group export: both `build_manifest` and `build_selection_manifest`
go through the single extracted helper **`ordered_manifest_jobs(jobs, results, by_id, copy_mode)`** (the
`created_at`-asc / id-tiebreak sort + `numbered_prefix` + `manifest_job_entry` loop), so a selection's dirs are
`1_…`/`2_…`/… in creation order exactly like a group.

Wiring (`export_selection_conn`, mirroring `export_group_conn`) reuses the group machinery unchanged — the
inverted rule-#3 guard, `fresh_export_dir`, `copy_manifest_job_dirs`, `verify_export_postcondition`:

- **Root is `selected-jobs-export/`** — `fresh_export_dir(dest_parent, "selected jobs", stamp)` (never-clobber
  as usual).
- **Empty selection → `Err`** (`AppError::Internal`) **before any write** — a selection is a caller error, not
  an empty export; nothing is created under the destination.
- **Unknown id → `AppError::NotFound` naming the missing id(s).** `fetch_jobs_by_ids` **dedups the ids on
  ingest** (a repeated id exports exactly once — the post-condition's "appears exactly once" is the second
  line, not the first), runs `WHERE id IN (…)` over `JOB_EXPORT_COLUMNS`, fills `present_files`, and fails
  loudly if any distinct requested id resolved to no row (no partial export).
- Same invariants otherwise: curated omissions recorded, draft/never-run job is a manifest entry with no dir,
  canonical dirs + rows untouched, no migration/struct/schema change.
