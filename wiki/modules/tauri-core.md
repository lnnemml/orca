# Module: Rust core (src-tauri/)

**Status:** Phase 2.5 complete. LocalBackend runs ORCA end-to-end (spawn, pinning, sequential
queue, cancel with MPI-rank sweep); molecule library; streaming convergence parse (`parser.md`)
and output search (`output_search.rs`); `jobs.scene_json` snapshot (schema v4); and xTB
pre-optimization (`xtb.rs` — off-thread starter, killpg + cwd sweep, post-conditions). Built on
the job model + Phase 0 scaffold.

## Responsibilities & boundaries

Job state machine, SQLite ownership, ExecutionBackend implementations, sidecar lifecycle, log
tailing, settings, and **all external-process spawning** (ORCA and xtb — ADR-009; the sidecar
never runs a binary). The runtime mechanics of running ORCA live in
`wiki/modules/execution-backends.md`; result parsing in `wiki/modules/parser.md`.

## Files

- `lib.rs` — Tauri builder, setup hook, exit handling, invoke-handler registration.
- `db.rs` — SQLite open + versioned migrations (v1–v13).
- `results.rs` — store/read parsed results (all four artifact readers) into the `results` table
  (ADR-012); the completion hook lives in `local_backend`. See `modules/artifact-readers.md`.
- `orca_json.rs` — spawn `orca_2json` (ADR-009), lazy-cached gbw→JSON in the job dir (unit 3.7).
- `error.rs` — `AppError` (thiserror).
- `sidecar.rs` — `SidecarManager`: spawn/health-poll/kill uvicorn; the version handshake.
- `commands/{settings,jobs,molecules,reactions}.rs` — Tauri command surface (thin wrappers over `*_conn`).
- `models/{job,molecule}.rs` — row structs (`COLUMNS` + `from_row`).
- `local_backend.rs` — ORCA execution, queue, cancel (see `execution-backends.md`).
- `cpu_presets.rs` — measured core-pinning presets (see `execution-backends.md`).
- `result_extraction.rs`, `convergence.rs` — result/convergence parsing (see `parser.md`).
- `output_search.rs` — streaming output search + presets.
- `xtb.rs` — xTB pre-optimization.
- `secrets.rs` — Anthropic API key in the OS keyring (ADR-015): `KeySource` four-state resolution
  + store/read/delete; the key never leaves Rust. See below.
- `anthropic.rs` — the **single** outbound network path to Anthropic (ADR-015): the minimal
  `verify` call; model + API version in constants; no logging of key or body.

## Database & migrations (`db.rs`)

`init_db(data_dir)` opens `orcastudio.db` under `dirs::data_dir()/orcastudio`. `migrate()` is
version-aware and **forward-only to `SCHEMA_VERSION`**: it ensures the v1 base, reads the stored
`schema_version`, and steps forward, so a v1 DB upgrades straight through to the current version in
place. All steps are idempotent (`IF NOT EXISTS` / `INSERT OR IGNORE`) — a user-changed `orca_path`
survives a restart.

- **v1** — `settings` k/v table; seeds `orca_path=/opt/orca/orca` and (idempotently, no bump)
  `cpu_preset=interactive`, `cpu_mask=8-15`, `cpu_nprocs=8`, `xtb_path=xtb`.
- **v2** — `jobs` table: `id` (UUID v4 TEXT PK), `title`, `input_content` (full `.inp`), `status`
  (default `draft`), `job_dir`, `energy` (REAL), `wall_time` (REAL), `error_message`, `created_at`
  (`datetime('now')`), `started_at`, `completed_at`. `Option` columns stay `NULL` until their
  lifecycle step fills them.
- **v3** — `molecules` table: `id` (UUID v4 TEXT PK), `name`, `formula` (default `''`), `xyz` (full
  standard xyz), `charge`/`multiplicity` (INTEGER, defaults 0/1), `tags` (comma-separated TEXT,
  default `''`), `created_at`. **Not** linked to `jobs` — no `molecule_id` FK yet (Phase 4.5).
- **v4** — additive `ALTER TABLE jobs ADD COLUMN scene_json TEXT` (nullable); old jobs carry `NULL`.
- **v5** — `results` table: parsed `.property.txt` per job (Phase 3, ADR-012 + ADR-004). Shape:
  a few **narrow typed columns** (`final_energy_eh`, `dipole_magnitude_au`, the thermochemistry
  fields, `parser_version`, `parsed_at`) for the card and future job-list sorting, plus one
  **`data_json` TEXT** column holding the full parsed structure. `job_id` is the PK (FK →
  `jobs(id)` ON DELETE CASCADE); one row per job, **upserted idempotently** (`ON CONFLICT(job_id)
  DO UPDATE`) so re-parsing updates, never duplicates. `create_results_table` is factored out so
  tests build just this table. Units are in the column names (rule #11); `t_times_s_eh` is **T·S
  in Eh, not entropy S** (measured `entropyS == H − G`).
- **v6** — `results.imaginary_count INTEGER` (nullable), the negative-frequency count from
  `.hess` (unit 3.6): the job list sorts by it and the minimum/TS warning stands on it. Added by a
  **guarded** `ALTER` (`column_exists` via `PRAGMA table_info`) so the additive step is idempotent
  across paths — `create_results_table` already carries the column for a fresh install, and the
  guard covers a DB that stopped at v5.
- **v7** — `results.homo_lumo_gap_eh REAL` (nullable), the HOMO/LUMO gap from `orca_2json` (unit
  3.7); same guarded-`ALTER` pattern. NULL without an ORCA `.gbw` (xTB/GOAT).
  - **What `data_json` gains (units 3.6–3.7):** vibrational data (frequencies/IR/normal-mode
    matrix with the `$atoms` element order), the `_trj.xyz` **trajectory** (frames = opt cycles,
    element order stored once + per-frame Å coords + comment energy), and MO **energies +
    occupancies**. **`MOCoefficients` are never stored** — the n×n block is ~52–62% of the
    orca_2json JSON and useless to us (rule #5); the reader streams past it. Trajectory-size note:
    the longest measured run is 26 frames × 8 atoms; a realistic 30-cycle × 50-atom opt is ~tens of
    KB of JSON — kept inline; if a pathological run ever bloated the row, the switch is frames→file
    path (not done, not needed).
- **v8** — a one-time **data backfill**, not a schema change (`SCHEMA_VERSION = 8`; no column added):
  `UPDATE jobs SET energy = (SELECT r.final_energy_eh FROM results r WHERE r.job_id = jobs.id)` for
  every job whose `energy` is still NULL but whose parsed `results` row already holds the value (unit
  3.9 defect 2 — the `output.out` tail regex missed a final energy measured **164 KB past the 64 KB
  window** on a 33-atom Freq, while the authoritative tier had it). Idempotent: it touches only NULL
  energies and only where a non-NULL `final_energy_eh` exists, so an already-filled job is never
  clobbered and a never-parsed job stays NULL. **Guarded** on `jobs.energy` existing so migration-test
  fixtures that stub `jobs` as `(id)` only skip cleanly. See `debugging/007`.
- **v9** — the **ORCA manual index** (Phase 4.3, ADR-013), the first tables the manual owns:
  `manual_sections` (row per section; synthetic `id` PK, **nullable `anchor`** + `anchor_source`,
  `body_md`, JSON `breadcrumb`/`labels`, index on `(orca_version, file)`), **`manual_fts`** — an
  **external-content** FTS5 over `manual_sections` so the body is not duplicated — and
  `manual_provenance`. Created via `create_manual_tables` (factored out, like `create_results_table`).
  Ingest + search live in `src/manual/index.rs`. Full schema: `modules/manual-index.md`.
- **v10** — additive `ALTER TABLE jobs ADD COLUMN index_map_json TEXT` (nullable, guarded `ALTER` like
  v6/v7). Holds the job's `IndexMap<OrcaIndex>` as one of two shapes (`results::StoredIndexMap`,
  externally tagged), **written once at `create_job`** (unit 1e):
  - `{"minted":[<AtomId u32s in text-row order>]}` — minted from the **submitted `input_content`**
    verified against the scene (`orcastudio_core::mint_index_map`: element sequence + float-tolerant
    coords, the `xyzMatchesScene` standard). Never from the scene alone — a scene/text drift SKIPS.
  - `{"skipped":"<reason>"}` — a **self-describing skip**: text↔scene mismatch, or an input form we
    cannot map (`* xyzfile`, `%coords`, no inline block). The job is not blocked.
  `results::resolve_job_mapping` reads it at parse: a minted map is used (cross-checked against the
  artifact via a **scene-sourced** anchor, so a corrupted stored map is caught — see
  `modules/artifact-readers.md`); a skip / NULL / legacy row falls back to the derived identity map
  (unit 1d). A clone / "new iteration" mints its OWN map (it runs through `create_job`).
- **v11** — additive `ALTER TABLE jobs ADD COLUMN scene_log_json TEXT` (nullable, guarded `ALTER` like
  v6/v7/v10; ADR-017 unit 2b). Holds the serialized **operation log** (log format v1, scenes v2
  inside), **co-written with `scene_json` in the same INSERT** at `create_job` — so the two are atomic
  and a later restore can cross-check them. The frontend's `restoreSceneLog` honours the log only if
  its current snapshot equals `scene_json`; on a mismatch the **log is rejected (named reason) and the
  snapshot wins** — `scene_json` (the map-minting contract above) stays more authoritative than the
  history. NULL for pre-v11 / no-scene jobs (a legacy job seeds a fresh log on New iteration). The
  `Job` struct + `Job::COLUMNS` carry `scene_log_json` as the 13th column so `get_job`/`list_jobs`
  return it to the UI.
- **v12** — additive `ALTER TABLE molecules ADD COLUMN is_reagent INTEGER NOT NULL DEFAULT 0`
  (guarded `ALTER` like v10/v11 — on the **table** existing, via `column_exists("molecules","id")`, so
  the migration fixtures that stub a DB without `molecules` skip cleanly; Phase 4.2 tail-2). A **role
  flag** so the `molecules` table doubles as the **user reagent catalog**: a user-saved reagent is a
  molecules row with `is_reagent = 1` plus its (mandatory) `charge` — the `charge` column already
  existed, so no new geometry storage. Existing rows and every `create_molecule` save default to 0, so
  the molecule library and its screen are unchanged. Two new commands split the one table by role:
  `create_reagent(name, xyz, charge)` (charge is a plain `i32`, **never** an `Option`/default — the
  ADR-014 no-silent-charge rule) and `list_reagents` (`WHERE is_reagent = 1`); `list_molecules` now
  filters `WHERE is_reagent = 0`. `Molecule::COLUMNS`/`from_row` carry `is_reagent` (bool from the 0/1
  INTEGER) as the 9th column. Frontend converter + curated↔user split: `src/scene/reagent-catalog.ts`.
- **v13** — the **reaction/pathway data model** (Phase 4.5 Stage C1, ADR-007 as amended). Two new
  grouping tables + a nullable `jobs.pathway_id`:
  - `reactions(id TEXT PK, name TEXT NOT NULL, description TEXT, created_at)` — a named transformation
    (ADR-007's central object).
  - `pathways(id TEXT PK, reaction_id TEXT NOT NULL REFERENCES reactions(id), label TEXT NOT NULL,
    created_at)` — one approach geometry under a reaction. **Lean by design:** it does NOT store the
    scan coordinate/method/profile (those live in the attached job's input/results; C2 reads them
    there — one source of truth).
  - `ALTER TABLE jobs ADD COLUMN pathway_id TEXT REFERENCES pathways(id)` — nullable, guarded `ALTER`
    like v10/v11 (on `jobs` existing). ADD-COLUMN-with-REFERENCES is legal because the default is NULL.

  **Normalized deviation from ADR-007's sketch:** a job carries `pathway_id` **only** — no
  `reaction_id` on jobs; the reaction is derived by joining `pathways`. Two columns that can disagree
  is the trap this project refuses; one source of truth wins over the one-join saving. **Jobs-survive
  invariant (load-bearing):** jobs are the work, reactions/pathways are grouping metadata —
  `delete_reaction` nulls the `pathway_id` of every job attached to its pathways and deletes the
  pathway rows; `delete_pathway` nulls its jobs' `pathway_id` and deletes the row; **neither ever
  deletes a job.** A promoted-then-ungrouped scan job is exactly the standalone job it was.
  `pathway_id = NULL` is the normal state for every job today; the model is purely additive. In C1
  the `Job` struct did not carry the column; **C2a adds `pathway_id` as `Job`'s 14th column**
  (`Job::COLUMNS`/`from_row`, `Option<String>`) so the reaction UI can map a pathway to its attached
  job by matching `Job.pathway_id == Pathway.id` (the job carries the FK; the pathway does not — one
  source of truth). **Referential integrity lives in the commands** (for clean `NotFound` errors and
  correct delete ordering): `create_pathway` under a missing reaction and `attach_job_to_pathway` with a
  missing job/pathway return `NotFound` with no orphan/partial write. **Correction (measured 2026-08-08,
  rule #10):** the older claim here — "this DB leaves SQLite FK enforcement off, the `REFERENCES`
  clauses are docs" — is **false**. The `bundled` SQLite is compiled with
  `SQLITE_DEFAULT_FOREIGN_KEYS=1` (`PRAGMA foreign_keys` reads **1**, and
  `pragma_compile_options` lists `DEFAULT_FOREIGN_KEYS`), so `REFERENCES` clauses are **actively
  enforced** on every connection. The command-level checks are therefore belt-and-suspenders (nicer
  errors), and **delete ordering is load-bearing** — a parent row must be deleted after its children or
  SQLite raises error 787 (`SQLITE_CONSTRAINT_FOREIGNKEY`); the commands already order it child-first.
  This corrects the same "FK off" phrasing wherever it recurs in the codebase comments. Commands (`commands/reactions.rs`, thin wrappers over `*_conn`, `Reaction`/`Pathway`
  in `models/reaction.rs`): `create_reaction(name, description?)`, `list_reactions`,
  `rename_reaction(id, name)`, `delete_reaction(id)`, `create_pathway(reaction_id, label)`,
  `list_pathways(reaction_id)`, `delete_pathway(id)`, `attach_job_to_pathway(job_id, pathway_id)` (the
  bottom-up **promote**, permissive about job kind — comparability guards are C2),
  `detach_job_from_pathway(job_id)`. Four cargo controls: `migrate_v12_to_v13_adds_reaction_tables_and_preserves_data`
  (db.rs) + `delete_reaction_keeps_jobs`/`referential_integrity_is_enforced`/
  `standalone_job_unaffected_by_reaction_model` (commands::reactions); the delete-keeps-jobs control is
  **bite-verified** (a naive DELETE-instead-of-NULL cascade turns it red). Frontend types:
  `Reaction`/`Pathway` in `src/types.ts` (no component yet — that is C2).
- **v14** — the **summed reactant reference** for absolute barriers (Phase 4.5 Stage C2b-2a, ADR-018).
  One lean join table:
  - `reaction_reference_jobs(reaction_id TEXT NOT NULL REFERENCES reactions(id), job_id TEXT NOT NULL
    REFERENCES jobs(id), created_at, PRIMARY KEY (reaction_id, job_id))` — a reaction's reactant
    reference is a **list of references to optimized-reactant jobs whose parsed final energies SUM** to
    `E(ref)`, and it is **optional** (0+). One job = pre-reaction complex; 2+ = separated reactants
    (e.g. substrate + BH₄⁻). Additive + idempotent (`CREATE IF NOT EXISTS`); the `(reaction_id, job_id)`
    PK makes `add_reference_job` idempotent.

  **Honest-or-absent (load-bearing, ADR-018):** `E(ref) = Σ` of the reference jobs' `final_energy_eh`,
  read on demand from the authoritative `results` tier (ADR-012) — **never cached** on `reactions` (no
  two-sources-of-truth drift). `reaction_reference_energy(reaction_id) -> { jobs, energy_eh }` returns
  `energy_eh = Some(Σ)` **only if the list is non-empty AND every job is parsed**; if ANY reference job
  is unparsed/running/failed the reference is **incomplete → `energy_eh: None`**, with `jobs` still
  listing all of them (the missing one's `final_energy_eh` is `None`) so the C2b-2b UI can name it. A
  partial sum is never returned — a wrong `E(ref)` would silently poison every absolute barrier.
  Expressed totally via `Option<f64>: Sum<Option<f64>>` (None if any element is None), no `unwrap`.
  **Jobs-survive (same as v13):** `delete_reaction` also `DELETE FROM reaction_reference_jobs WHERE
  reaction_id = ?` (before the parent row); `remove_reference_job(reaction_id, job_id)` drops the
  grouping row only. Neither ever deletes the job — it stays standalone in the Jobs list.
  Commands (`commands/reactions.rs`, `ReferenceJob`/`ReferenceEnergy` in `models/reaction.rs`):
  `add_reference_job(reaction_id, job_id)` (errors `NotFound` if either is absent; idempotent),
  `remove_reference_job(reaction_id, job_id)`, `list_reference_jobs(reaction_id) -> Vec<ReferenceJob {
  job_id, title, final_energy_eh: Option<f64> }>`, `reaction_reference_energy(reaction_id)`. Four cargo
  controls: `migrate_v13_to_v14_adds_reference_jobs_and_preserves_data` (db.rs) +
  `reference_energy_incomplete_is_none_not_partial`/`reference_energy_sums_when_all_parsed`/
  `delete_reaction_keeps_reference_jobs`/`add_reference_job_integrity_and_idempotent`
  (commands::reactions). The incomplete-not-summed and delete-keeps-jobs controls are **bite-verified**
  (a partial `filter_map` sum, and a naive job cascade, each turn them red). Frontend types mirror the
  structs — `ReferenceJob`/`ReferenceEnergy` in `src/types.ts` — but no component yet (absolute
  barriers in the overlay are C2b-2b).
  **Future integrity point:** jobs are **not deletable today** (no `delete_job` command exists). When one
  is added it MUST also `DELETE FROM reaction_reference_jobs WHERE job_id = ?` (and null `jobs.pathway_id`
  cleanup, already the C1 pattern) — otherwise a deleted job leaves a dangling reference row.
- The queue statuses (`queued`, `cancelled`) and `parsed` needed **no migration** — `status` is TEXT.
- Migration tests assert preservation across each step (…`migrate_v6_to_v7_adds_homo_lumo_gap`,
  `migrate_v7_to_v8_backfills_energy_from_results`, `migrate_v8_to_v9_adds_manual_tables_and_preserves_data`,
  `migrate_v9_to_v10_adds_index_map_json_and_preserves_jobs`,
  `migrate_v10_to_v11_adds_scene_log_json_and_preserves_jobs`,
  `migrate_v12_to_v13_adds_reaction_tables_and_preserves_data`,
  `migrate_v13_to_v14_adds_reference_jobs_and_preserves_data`; version assertions use `SCHEMA_VERSION`,
  not a literal). The reagent role + persistence are covered in `commands::molecules`
  (`reagent_role_separates_from_the_molecule_library`, `reagent_persists_across_reopen`). A separate `fts5_is_available_with_ranking_and_snippet`
  test gates the bundled SQLite's FTS5 support (Phase 4 / ADR-013 stands on it) — not a migration.
  Verified against a copy of the real DB: 13 existing jobs preserved across 3→4.

**Per-atom data rule (the load-bearing storage invariant).** Per-atom arrays go into `data_json`
**with the element sequence they were verified against** — charges next to their own
`elements`/`atomic_numbers`, the gradient next to the `order_elements` of its `$Geometry`. There is
**no** position-keyed "result atom" table: a DB row outlives the code that knew the order, so a bare
positional array would strand the atoms it belongs to (exactly ADR-010's concern, at the longest
horizon). A storage-boundary post-condition (rule #9) reads the row back after writing and asserts
the per-atom counts and element order survived serialization. Details: `results.rs`,
`modules/artifact-readers.md`.

**`scene_json` semantics** (ADR-008 #5 + amendment): a versioned `SceneFragment` snapshot written
**once at create time** — the job's input is immutable, so its snapshot is too (no update path). It
**annotates** `input_content`; the text stays authoritative for geometry (the frontend's
`restoreScene` reconciles them). **`scene_log_json`** (v11, ADR-017 unit 2b) rides alongside it —
the operation log, co-written in the same INSERT and cross-checked against `scene_json` on New
iteration (a diverged log is rejected, the snapshot wins).

## Models (`models/`)

`Job` and `Molecule` mirror their tables 1:1 (`#[derive(Serialize)]`). `from_row` hydrates from a
row in `COLUMNS` order; `COLUMNS` is the single source of truth for the select list (`Job` gained
`scene_json` as its 11th column in v4). `JobStatus` = `Draft | Queued | Running | Completed |
Parsed | Failed | Cancelled`, serialized to/from lowercase strings on the wire and in the DB
(`as_str`/`from_db`; the TS `JobStatus` union tracks it in lockstep). State machine: `draft →
queued → running → completed → parsed | failed | cancelled`. **`parsed` is post-`completed`**
(Phase 3): the calculation already succeeded; `parsed` only adds that our `.property.txt` parse of
it succeeded too. The two failure modes are kept distinct — a `failed` job is a calculation that
failed; a `completed` job with an `error_message` is a calculation that ran fine but whose results
would not parse (OUR problem, not the run's); and a completed job with no `.property.txt` is
"nothing to parse", not a failure. Remote states (uploading/syncing) are still deferred. An unknown
status string from the DB → `AppError::Internal` via `from_db`.

**Completion → parse hook** (`local_backend::parse_results_after_completion`, shared by the live
finish path and startup reconciliation): once a job is `completed`, read `input.property.txt`,
`verify` it against the reference geometry extracted from the job's own `input_content` (the reader
never reads `input.inp`), store + read back, and advance to `parsed`. A parse failure records the
reason and leaves the job `completed`; a missing artifact leaves it `completed` silently. The
`read_job_results` command returns the stored structure to the frontend's `ResultsCard`. The
`.hess` reader runs in the same hook when `input.hess` is present (its reference is the
`.property.txt` final geometry — the Freq geometry — via a distance-invariant check); absent for
SP/GOAT, which is normal.

**Forward note (Phase 5).** `ParseOutcome::NoArtifact` is currently silent and conflates two cases
that will diverge under remote execution: "this calculation type produces no such file" (fine) and
"rsync has not pulled it back yet" (a transient the UI should reflect). Not reworked now — flagged
so it is not lost when `SshBackend` lands.

## Errors (`error.rs`)

`AppError` variants: `Database`, `Sidecar`, `Io`, `Internal` (poisoned mutex etc.),
`NotFound(String)`, `Backend(String)` (spawn failure / bad config / queue issues). **Serialized to
the frontend as a plain string** today; the `{code, message}` structured surface is aspirational —
revisit when the UI needs error codes.

## Startup sequence (setup hook, `lib.rs`)

open + migrate SQLite → `local_backend::reconcile_on_startup(&conn)` (advance any job left
`running` by a crash — see `execution-backends.md`) → manage `DbState(Mutex<Connection>)` (the
`Connection` is `Send`, not `Sync`) + `Arc<SidecarManager>` + `JobRunner` + `XtbRunner` → spawn the
sidecar + a background health-poll thread → a thread that runs `try_start_next` to resume `queued`
jobs → `prune_diagnostic_dirs` off-thread (xtb). `RunEvent::ExitRequested` stops the sidecar and
runs `terminate_on_exit` synchronously; `Drop` on `SidecarManager` is the backstop.

## Commands (thin wrappers over `*_conn(&Connection)` helpers)

- **Settings:** `get_settings() -> HashMap<String,String>`, `set_setting(key, value)`,
  `get_sidecar_status() -> SidecarStatus { status, port, version, expected_version }`.
- **Jobs:** `create_job(title, input_content, scene_json: Option<String>, scene_log_json: Option<String>) -> Job` (UUID, inserts
  `draft`, snapshot written once); `list_jobs() -> Vec<Job>` (`created_at DESC`); `get_job(id)`
  (`NotFound`); `update_job_status(id, status)` (stamps `started_at` on `running`, `completed_at`
  on `completed`/`failed`/`cancelled`); `submit_job(app, id)` (enqueues, returns at once — needs
  `app: tauri::AppHandle` for `emit`); `cancel_job(id)`; `pause_queue()` / `resume_queue()` /
  `is_queue_paused() -> bool`; `read_job_output(id, tail_lines: Option<usize>) -> Vec<String>`;
  `read_job_output_for_viewer(id) -> OutputContent`; `read_job_convergence(id)`;
  `read_job_ensemble(id) -> String`; `open_job_folder(id)`; `search_job_output(id, opts) ->
  SearchResult`; `get_search_presets()`.
- **Molecules:** `create_molecule(name, formula, xyz, charge, multiplicity, tags)`,
  `list_molecules()` (newest first), `get_molecule(id)`, `update_molecule(id, …)` (full update),
  `delete_molecule(id)` — each `NotFound` on a missing id.
- **CPU / xtb:** `get_cpu_presets() -> Vec<CpuPresetInfo>`; `xtb_version`, `xtb_optimize`,
  `xtb_cancel` (see below).
- **API key (ADR-015, `secrets.rs` + `anthropic.rs`):** `api_key_status() -> KeySource`,
  `set_api_key(key)`, `delete_api_key()`, `verify_api_key() -> String`. **No command returns the
  key** — the frontend learns only the source *state*. `KeySource` is a four-variant enum
  (`stored-in-keyring` / `absent` / `from-environment` / `unavailable`, internally-tagged kebab);
  the state carries only `last4` for recognition, never the key. The fallback trigger is a
  *distinct third state* (`NoDefaultStore` = "no keyring backend", ≠ `NoEntry` = "keyring empty");
  the env var (`ANTHROPIC_API_KEY`) is read **only** when the keyring is unusable, and that source
  is shown in the UI — **no silent plaintext write to the DB**. `set_api_key` fails loudly if the
  keyring is unavailable. `verify_api_key` is **`async` + `spawn_blocking`** (the threading rule:
  a 15 s offline timeout must not freeze the window) and hits `GET /v1/models` — authenticates
  without spending generation tokens. Wiring tests pin the return types (none is the key) and that
  `KeySource` serializes without the secret body (negative control: a `last4` that forgets to
  truncate turns the gate red). Measured keyring gate: `wiki/architecture/keyring-availability.md`.
- **Explain / models (ADR-014 T1, ADR-015; `anthropic.rs`, all `async` + `spawn_blocking`):**
  - `list_anthropic_models() -> Vec<ModelInfo>` — the **live** `/v1/models` list of what THIS key may
    use; the Settings model picker's options (rule #10: options measured, not hardcoded — so the menu
    can only offer a model the key can actually reach, and no wrong belief about the model lineup can
    reach it, because its source is the run, not our memory). `ModelInfo { id, display_name }`; **no
    price** (the endpoint doesn't return it, and hardcoding one is the recalled-constant anti-pattern
    ADR-014 (1a)).
  - `explain_selection(word, line, section) -> String` — the T1 explain. **Exactly three data fields**;
    the command has **no** parameter for the input file or coordinates (the bound is the type;
    `build_explain_prompt`'s `fn(&str,&str,&str)->String` signature is pinned by a wiring test).
    Reads the model from `settings` (`anthropic_model`, seeded `claude-sonnet-4-6`), not from the
    caller. Returns advice text — **writes nothing to the editor** (tier-zero: no insert path exists).
  - **Error causes are distinct (`status_error`, unit-tested):** `401` = key rejected, `404` = model not
    available to this key, other = generic API error, transport = offline (a normal mode, not a failure).
    A single "failed" would hide the cause. No logging of key or request/response body, even in debug.
- **DB helpers** (`pub(crate)`, reused by the backend): `set_job_dir_conn`, `finalize_job_conn`
  (terminal status + `completed_at` + `error_message`), `set_job_results_conn`, `get_job_conn`,
  `update_job_status_conn`.

`open_job_folder` spawns the platform file manager (`xdg-open` / macOS `open` / Windows
`explorer`) — an app-defined command, so no capability entry is needed. The frontend `listen`s to
events (allowed by the `core:default` capability) and filters by `job_id`.

**Pollable-path rule for new commands ([ADR-014](../architecture/adr-014-ai-integration-boundary.md)
(4)).** The threading rule below makes every long operation *event-driven*, but a future MCP client
(T3) does **not** listen to Tauri events — so **every new compute-spending command must also expose a
*pollable* path to its status/result, not only an event**. `submit_job` already satisfies this via
`get_job`; `xtb_optimize` does **not** (it emits only `xtb:done` / `xtb:error`, no status query). That
gap is a **named debt**, not fixed here — retrofitting `xtb_optimize` is out of scope for the ADR-014
documentation unit and touches no code.

## The threading rule — a long operation NEVER lives inside a synchronous command

A `#[tauri::command] fn` executes on the **main thread**, which on Linux is the GTK/WebKitGTK UI
thread. Anything slow inside it freezes the window for the whole duration AND starves every other
command (a separate cancel command can't be delivered while the main thread is busy). **The pattern
is: a starter command that validates + reserves state + RETURNS immediately, the actual work in
`std::thread::spawn`, and results/errors reported to the frontend as events** (`app.emit`,
`<domain>:<kind>` payloads). Two places apply it:

- **`drive_job`** (`local_backend.rs`) — `submit_job` returns at once; a spawned thread tails ORCA's
  stdout and emits `job:log` / `job:status` / `job:convergence`.
- **`xtb_optimize`** (`xtb.rs`) — a starter; the run is off-thread and emits `xtb:done` /
  `xtb:error` (this was a synchronous command at first, froze the window, and made `xtb_cancel`
  undeliverable — the defect is invisible to `cargo test` and shows on the first click, so the
  acceptance step is a manual run in the real window; changed in `[2026-07-29] 2.5.5-fix`).

## Result extraction & convergence

On completion, `drive_job` runs `result_extraction::{extract_final_energy, extract_wall_time}` over
a 64 KB output tail and stores them via `set_job_results_conn` **before** the terminal `job:status`.
The incremental `convergence.rs` parser feeds off the same stdout stream. Full detail in
`wiki/modules/parser.md`.

## Output search & viewer content (`output_search.rs`)

- **`search_job_output(id, opts: SearchOptions) -> SearchResult`** — streaming search of a job's
  `output.out` (`regex` / `case_sensitive` flags). `search_output` reads line by line through a
  `BufReader`, holding only an optional context ring buffer, the matches awaiting trailing context,
  and the capped result list — **never the whole file** (domain rule #5). Single pass, matches
  finalized in line order, leftovers flushed at EOF. Measured: **431 KB / ~8600 lines in ~3 ms**.
  Empty result (not an error) when the job has no dir/output; empty query → empty result.
- Each `OutputMatch` carries `line_no`, the matched `line`, and the hit's **1-indexed char column
  range `col_start`/`col_end`** (exclusive end — Monaco range semantics). Context
  (`context_before`/`context_after`) is **opt-in** via `SearchOptions.context_lines` (the viewer
  passes `0`, saving ~2500 lines of payload at 500 hits). **`MAX_MATCHES = 500`** caps returned
  matches while `total` counts every hit (so the UI says "500 of 637"; `truncated = total >
  matches.len()`).
- **Matcher:** regex via `RegexBuilder.case_insensitive(!case_sensitive)` (invalid pattern →
  `AppError::Backend("invalid regular expression: …")`) → first `Match` byte range → char columns;
  else literal `find` with the needle lowercased **once** up front for the case-insensitive path
  (positions taken in the lowercased line — 1:1 for ASCII, which all ORCA output is).
- **`read_job_output_for_viewer(id) -> OutputContent { content, first_line_no, total_lines,
  truncated }`** — the file for the Monaco viewer, **capped to the last `MAX_VIEWER_LINES` (300 000
  ≈ 30 MB)**: streams line by line into a `VecDeque` that evicts the oldest past the cap, so a
  hundreds-of-MB file is never held whole. Keeps the **tail** and reports `first_line_no` (`> 1` iff
  truncated) so the viewer shows absolute line numbers and hits still map.
- **`read_job_ensemble(id) -> String`** — reads a GOAT job's `input.finalensemble.xyz` (the fixed
  `input.inp` name gives a fixed ensemble name — `wiki/orca/goat.md`) whole; unlike `output.out` it
  is tiny (a multi-frame xyz of one small fragment), capped at `MAX_ENSEMBLE_BYTES` (8 MB)
  defensively. Empty string (not an error) when there's no dir/file or it isn't a GOAT run.
- **Presets (`SEARCH_PRESETS`, `id/label/query/regex/case_sensitive/description`)** — wording
  verified against real ORCA 6.1 output (`orca/output-files.md`). Two correctness points:
  - **`errors` is case-SENSITIVE** (`ERROR|error termination|aborting|ABORTING`) — a
    case-insensitive `error` matches the benign `DIIS Error` / `Startup error` printed on every SCF
    (12+ hits in a *successful* run; the case-sensitive query fires **0×** across 12 real
    successful outputs). This is why `SearchPreset` carries a per-preset `case_sensitive` flag.
  - **`imaginary` = literal `imaginary mode`**, NOT bare `imaginary` (which hits `imaginary
    perturbations`, a CPHF count in every Freq run); it matches ORCA's real `***imaginary mode***`
    marker on a saddle point.

## Sidecar lifecycle & the stale-sidecar handshake (`sidecar.rs`)

`SidecarManager` picks a free port, spawns uvicorn, health-polls on a background thread, and kills
it on `ExitRequested` + `Drop`. `Health` has `Healthy` / `Stale` / `Down`: `health_check` reads the
`/health` body's `version` and sets `Healthy` vs `Stale` via `version_at_least(actual, expected)` —
a pure, unit-tested **component-wise numeric** compare against `EXPECTED_MIN_SIDECAR_VERSION`
(`"0.2.0"`); a string compare would lie (`"0.10.0" < "0.9.0"`). An unparseable version is treated
as stale, never healthy. `SidecarStatus` carries `version` + `expected_version`. Debug builds launch
uvicorn with `--reload`; `start` sets the child's **own process group** (`CommandExt::process_group(0)`)
and `stop`/`Drop` `kill_process_tree` → `killpg(SIGTERM)` → grace → `killpg(SIGKILL)`, so the
`--reload` worker child isn't orphaned. The rule and its rationale are in `wiki/modules/sidecar.md`
+ `wiki/debugging/005`.

## xTB pre-optimization (`xtb.rs`)

Standalone GFN2-xTB relaxation of a scene while holding the user's constraints, so the geometry
handed to ORCA is already sensible. **In Rust, not the sidecar** (ADR-009): Rust owns process
spawning (isolation rule #3, kill-the-group `debugging/004`), and the binary path is a setting.
Tool details: `wiki/orca/xtb.md`.

- **`xtb_path`** setting (seeded `'xtb'`, a `settings` row like `orca_path`; never bundled — #7).
  `xtb_version` runs `<path> --version` and parses the banner for the Settings "Check" button;
  `resolve_binary` turns a bare name into an absolute path via `$PATH`.
- **`xtb_optimize(xyz, charge, multiplicity, constraints, timeout_secs?) -> ()`** — a **starter**:
  it validates synchronously (multiplicity, parse xyz, resolve targets → an out-of-range index
  rejects here for immediate feedback), **reserves the single slot** (rejecting a concurrent run)
  with a `pgid: 0` placeholder, spawns the worker thread, and returns. `constraints` deserialize
  from the TS `Constraint` (0-based atoms; `valueText` ignored). The thread's `run_in_dir` resolves
  each target (explicit value, or the geometry's CURRENT value for a freeze-as-is).
  **`build_xcontrol` returns `Option<String>`** (`None` = no constraints); that one value decides
  both whether the `xcontrol` file is written AND whether `--input` is passed (`xtb_args`, a pure
  argv builder — an empty `--input` file **hangs** xtb, `wiki/debugging/006`), with every index
  **`+1`** (xtb is 1-based — `wiki/orca/xtb.md`). It runs
  `<xtb> input.xyz [--input xcontrol] --opt --gfn 2 --chrg <c> --uhf <mult−1>` by full path in an
  **isolated dir** (`<data>/xtb/<uuid>`) in its own process group, polls `try_wait` + the
  `cancelled` flag every 50 ms, and reads `xtbopt.xyz`. The result rides `xtb:done`, an error
  `xtb:error`.
- **Completion anchored on RESULTS, not the binary's last words (2b-hotfix, `debugging/012`).**
  `classify_completion` (pure, tested on real fixtures): success = an optimized geometry present +
  parseable **and NO `FAILED TO CONVERGE GEOMETRY OPTIMIZATION`** line (scanned over the whole
  size-capped log — the marker can sit hundreds of lines from the end). Measured on xtb 6.6.1: both
  `normal termination` (stderr, buried ~41 lines deep on a clean run) and the exit code (0 even on
  non-convergence) **lie in both directions**, so they are **named diagnostics, never gates**.
  Non-convergence → a named error quoting the FAILED line + iteration count; artifacts **kept** so the
  user can inspect the (non-optimized) geometry — it is not applied. GOAT (`! XTB GOAT` via ORCA) does
  not share this gate.
- **Post-conditions INSIDE the command** (the price of a missed error is the wrong geometry into a
  multi-hour ORCA run): atom count unchanged; element sequence unchanged positionally; **each
  constraint held within tolerance** (`check_held`: 0.1 Å distance / 5° angle / 0.01 Å `$fix`; the
  distance tolerance is measured — realistic hold 0.011 Å at force constant 1.0). Any breach →
  `AppError::Backend` with a diagnostic. The held-check also catches an index-base mistake: a wrong
  `+1` constrains a different pair and the intended one drifts past tolerance.
- **Isolation + cleanup + kill (rule #3, `debugging/004`).** The slot is freed in the thread
  unconditionally right after `run_in_dir`. The scratch **dir cleanup is split**:
  `keep_dir_for_diagnostics(succeeded, cancelled)` → **remove on success and on user-cancel, KEEP on
  any other failure** (timeout / non-zero exit / post-condition breach / parse error). Rule #3 is
  about clearing ORCA-style scratch *litter* on success — it is NOT a licence to delete the
  *evidence* when a run fails, which is exactly when `xtb.out` (the only record of where xtb spent
  its time) is needed. The kept dir's path rides the `xtb:error` payload (`dir`) and the UI shows it
  as copyable text; the error message also carries the **last ~20 lines of `xtb.out`** via the
  shared `read_tail_lines` (bounded tail, rule #5 — one tailer). **Accumulation** is bounded: kept
  dirs are pruned to the **`KEEP_DIAGNOSTIC_DIRS` (5) newest at startup** — `dirs_to_prune` (pure +
  tested) sorts by mtime and returns all but the newest N; `prune_diagnostic_dirs` runs off-thread
  in setup. Newest-kept means a **just-failed run's dir is never pruned** by the next launch. No
  setting.
- **Live progress.** The poll loop also reads the `xtb.out` tail ~once a second (same
  `read_tail_lines`) and emits `xtb:progress { cycle }` on each new optimization cycle — so a stall,
  even a pre-first-cycle startup hang, is visible at once instead of after minutes of silence.
- **Cancel is non-blocking.** The single-slot `XtbRunner` holds only the `cancelled` flag (`Some` =
  busy); **`xtb_cancel` just sets the flag and returns** — it runs on the main thread and must not
  block, and `terminate_job` sleeps up to ~12 s. The **worker thread's poll loop** (holding the
  pgid + dir locally) sees the flag within 50 ms and does the actual `terminate_job` (killpg
  SIGTERM→grace→SIGKILL + **cwd sweep**, the ORCA primitives made `pub(crate)` — one copy) on its
  own thread. It's a helper, not a queued job — just not blocking the UI thread on the run OR the
  cancel.

## Events emitted

`job:status(job_id, status)`, `job:log(job_id, lines)` (batched every 50 lines / 100 ms),
`job:convergence(job_id, event)`; `xtb:done` / `xtb:error` / `xtb:progress { cycle }`;
sidecar status via `SidecarStatus` polling.

## Conventions & quirks

- Every command returns `Result<T, AppError>` (thiserror); no `.unwrap()` outside tests.
- `dirs` crate is used for the data dir (per the task spec) rather than Tauri's `app.path()` API —
  harmless; consolidate later if desired.
- Dependencies added along the way: `uuid` (v4), `regex`, `libc` (Unix-only, for `killpg`).
