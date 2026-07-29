# Module: Rust core (src-tauri/)

**Status:** Phase 2 complete. LocalBackend runs ORCA end-to-end (spawn, pinning, sequential
queue, cancel with MPI-rank sweep); molecule library; streaming convergence parse (`parser.md`)
and output search (`output_search.rs`). Built on the job model + Phase 0 scaffold.

## As built (Phase 0)
Files: `lib.rs` (builder + setup + exit handling), `db.rs`, `error.rs`, `sidecar.rs`,
`commands/settings.rs`.
- **Commands implemented:** `get_settings() -> HashMap<String,String>`,
  `set_setting(key, value)`, `get_sidecar_status() -> SidecarStatus { status, port }`.
- **DB (`db.rs`):** `init_db(data_dir)` opens `orcastudio.db` under `dirs::data_dir()/orcastudio`
  and runs migration v1 (`settings` k/v table, seeds `orca_path=/opt/orca/orca`,
  `schema_version=1`). Idempotent (`IF NOT EXISTS` / `INSERT OR IGNORE`) — a user-changed
  `orca_path` survives restart (verified).
- **Managed state:** `DbState(Mutex<Connection>)` (Connection is `Send` not `Sync`) and
  `Arc<SidecarManager>`.
- **`AppError`** variants: `Database`, `Sidecar`, `Io`, `Internal` (poisoned-mutex etc.).
  Phase 0 serializes it to the frontend as a **plain string** (not `{code, message}` as the
  planned surface below envisions — revisit when the UI needs structured error codes).
- **Startup sequence (setup hook):** open+migrate SQLite → spawn sidecar → background
  health-poll thread. `RunEvent::ExitRequested` stops the sidecar; `Drop` on `SidecarManager`
  is the backstop.

## As built (Phase 1 step 1) — job model + state machine
Files added: `models/job.rs`, `commands/jobs.rs`. `db.rs` gained migration v2.

- **Migration v2 (`db.rs`):** `migrate()` is now version-aware. It always ensures the v1
  `settings` table/seeds, reads the stored `schema_version`, and steps forward: `version < 2`
  → `CREATE TABLE jobs (...)`, then persists `schema_version=2`. Backward-compatible — an
  existing v1 DB is upgraded in place, settings untouched (test `migrate_v1_to_v2_preserves_settings`).
- **`jobs` table:** `id` (UUID v4 TEXT PK), `title`, `input_content` (full `.inp` text),
  `status` (`draft|running|completed|failed`, default `draft`), `job_dir`, `energy` (REAL),
  `wall_time` (REAL), `error_message`, `created_at` (`datetime('now')`), `started_at`,
  `completed_at`. `Option` columns stay `NULL` until the relevant lifecycle step fills them.
- **`JobStatus` enum (`models/job.rs`):** `Draft|Running|Completed|Failed`, serialized to/from
  lowercase strings on the wire and in the DB (`as_str`, `from_db`). Remote states
  (uploading/syncing) and `parsed` are deliberately deferred to later phases.
- **`Job` struct:** mirrors the table 1:1, `#[derive(Serialize)]`. `Job::from_row` hydrates
  from a row in `Job::COLUMNS` order; `Job::COLUMNS` is the single source of truth for the
  select list.
- **Commands implemented:** `create_job(title, input_content) -> Job` (generates UUID, inserts
  as `draft`) *(signature gained `scene_json: Option<String>` in schema v4 — see "Schema v4"
  below)*, `list_jobs() -> Vec<Job>` (newest first, `created_at DESC`),
  `get_job(id) -> Job` (`AppError::NotFound` if absent), `update_job_status(id, status)` —
  stamps `started_at` on `running`, `completed_at` on `completed`/`failed`; `NotFound` if the
  id doesn't exist. Each command is a thin lock-and-delegate wrapper over a `*_conn(&Connection)`
  helper so the state-machine logic is unit-testable without a Tauri app.
- **`AppError`** gained `NotFound(String)`.
- **Not yet:** no UI, no `LocalBackend`/process spawn, no filesystem job dirs — those are
  Phase 1 later steps.

## As built (Phase 1 step 3) — LocalBackend
New file: `local_backend.rs`. New command `submit_job`, new managed state `JobRunner`.
Full backend detail lives in `wiki/modules/execution-backends.md`; core-facing summary:

- **`submit_job(app, id)`** (in `commands/jobs.rs`, delegates to `local_backend::submit`):
  validates the job is `draft`, reads `orca_path`, reserves the single execution slot,
  prepares the isolated job dir, spawns ORCA, marks `running`, and returns immediately — a
  background thread streams the log and finalizes. Needs `AppHandle` (declared as a
  `app: tauri::AppHandle` command param) for `emit`.
- **`JobRunner` managed state** (`app.manage`d in `lib.rs` setup): holds the job-dir root
  (`<data>/jobs/`) and `Mutex<Option<String>>` — the running job id, enforcing concurrency = 1.
- **New jobs.rs DB helpers** (all `pub(crate)`): `set_job_dir_conn`, `finalize_job_conn`
  (terminal status + `completed_at` + `error_message`), and `get_job_conn`/`update_job_status_conn`
  promoted from private so the backend can reuse them.
- **`AppError::Backend(String)`** added for spawn failures / bad config / queue-full.
- **Events emitted** (Rust → UI, via `tauri::Emitter`): `job:log { job_id, lines: [String] }`
  (batched every 50 lines / 100 ms) and `job:status { job_id, status }` on running + terminal.
  Frontend `listen`s (allowed by `core:default` capability) and filters by `job_id`.
- **Not yet:** startup reconciliation of jobs left `running` after a crash/close (a job stays
  `running` in the DB) — deferred to Phase 2 (matches ROADMAP).

## As built (Phase 1 step 4) — output backfill, results, open folder
Closes Phase 1 (MVP). New commands + a results write path.

- **`read_job_output(id, tail_lines: Option<usize>) -> Vec<String>`** — last `tail_lines`
  (default/cap 10 000) lines of `<job_dir>/output.out`, backed by
  `local_backend::read_tail_lines` which reads at most 8 MB from the end (drops a partial head
  line) so it never loads whole multi-MB outputs (domain rule #5). Empty vec (not an error) when
  the job has no dir or no output yet. Used by the detail screen to backfill the log console.
- **`open_job_folder(id)`** — looks up `job_dir` from the DB and spawns the platform file
  manager (`xdg-open` on Linux, also macOS `open` / Windows `explorer`). App-defined command, so
  no capability entry needed.
- **Result extraction on completion** — `drive_job`, after `detect_completion`, on `Completed`
  reads a 64 KB tail and runs `result_extraction::{extract_final_energy, extract_wall_time}`,
  storing them via the new `set_job_results_conn` **before** emitting the terminal `job:status`
  (so the UI's reload sees energy/time). See `wiki/modules/parser.md`.
- **Dependency:** added `regex = "1"`.

## As built (Phase 2.3) — molecule library
New files: `models/molecule.rs`, `commands/molecules.rs`. `db.rs` gained migration v3.

- **Migration v3 (`db.rs`):** `SCHEMA_VERSION` bumped to 3; a `version < 3` arm creates the
  `molecules` table. `migrate()` is forward-only to `SCHEMA_VERSION`, so a v1 or v2 database is
  upgraded straight to v3 in place (the existing `migrate_v1_to_v2_preserves_settings` test now
  asserts the final version is `SCHEMA_VERSION`, not literally 2). New test
  `migrate_v2_to_v3_preserves_jobs`: seed a v2 DB with one job → migrate → job survives and the
  `molecules` table exists.
- **`molecules` table:** `id` (UUID v4 TEXT PK), `name`, `formula` (default `''`), `xyz` (full
  standard xyz string), `charge`/`multiplicity` (INTEGER, defaults 0/1), `tags` (comma-separated
  TEXT, default `''`), `created_at` (`datetime('now')`). Deliberately **not** linked to `jobs` —
  no `molecule_id` FK yet; molecule↔job association is Phase 4.5 (reaction modeling).
- **`Molecule` struct (`models/molecule.rs`):** mirrors the table 1:1, `#[derive(Serialize)]`,
  same `COLUMNS` + `from_row` pattern as `Job` (no enum fields, so `from_row` is a plain hydrate).
- **Commands (`commands/molecules.rs`):** `create_molecule(name, formula, xyz, charge,
  multiplicity, tags) -> Molecule`, `list_molecules() -> Vec<Molecule>` (newest first),
  `get_molecule(id) -> Molecule` (`NotFound`), `update_molecule(id, …) -> Molecule` (full update,
  `NotFound` if absent), `delete_molecule(id)` (`NotFound` if absent). Same thin-wrapper-over-
  `*_conn` shape as jobs; four unit tests cover create/list, get-missing, update-fields,
  delete-removes. All registered in `lib.rs` invoke_handler.

## Schema v4 — `jobs.scene_json` (Phase 2.5, 2.5.0d-3)
- **Migration v4 (`db.rs`):** `SCHEMA_VERSION` → 4; a `version < 4` arm runs
  `ALTER TABLE jobs ADD COLUMN scene_json TEXT` (nullable). Purely additive, so old jobs carry
  `NULL`. Test `migrate_v3_to_v4_preserves_jobs` (seed a v3 DB with a job → migrate → job survives,
  `scene_json` column exists and is NULL). The v2→v3 test's version assertion switched to
  `SCHEMA_VERSION` too (it hardcoded `3`). **Verified against a copy of the real DB:** 13 existing
  jobs all preserved, schema_version 3→4, every job's `scene_json` NULL.
- **`scene_json` semantics (ADR-008 #5 + its amendment):** a versioned `SceneFragment` snapshot
  written **once at create time** — the job's input is immutable, so its snapshot is too (no update
  path). It **annotates** `input_content`; the text stays authoritative for geometry (the frontend's
  `restoreScene` reconciles them). `Job` gained the field, `Job::COLUMNS` + `from_row` extended (11th
  column), `create_job(title, input_content, scene_json: Option<String>)`. Test
  `create_persists_and_reloads_scene_json`.

## As built (Phase 2) — CPU pinning, queue, cancel
New file: `cpu_presets.rs`. `local_backend.rs` gained the queue/cancel/pinning logic (detailed
in `wiki/modules/execution-backends.md`); the core-facing surface:

- **`JobStatus` extended:** `Draft | Queued | Running | Completed | Failed | Cancelled`
  (was four states). `as_str`/`from_db` and the TS `JobStatus` union updated in lockstep. State
  machine: `draft → queued → running → completed | failed | cancelled`. `update_job_status_conn`
  stamps `completed_at` on `cancelled` too; `queued` is a status-only transition.
- **No migration:** `status` is TEXT, so the two new values need no schema change. `settings`
  gained three seeded keys (idempotent `INSERT OR IGNORE`, no version bump): `cpu_preset`
  (`interactive`), `cpu_mask` (`8-15`), `cpu_nprocs` (`8`).
- **New commands** (all in `commands/jobs.rs` unless noted):
  - `cancel_job(id)` → `local_backend::cancel` (queued: drop; running: killpg the tree).
  - `pause_queue()` / `resume_queue()` / `is_queue_paused() -> bool` — queue-only pause.
  - `get_cpu_presets() -> Vec<CpuPresetInfo>` (in `cpu_presets.rs`) for the Settings UI.
  - `submit_job` unchanged in signature but now **enqueues** (never errors on a busy slot).
- **Startup sequence** now runs `local_backend::reconcile_on_startup(&conn)` before managing the
  DB, then spawns a thread that calls `try_start_next` to resume any `queued` jobs.
- **Dependency:** `libc = "0.2"` (Unix-only target dep) for `killpg`.

## As built (Phase 2.7) — streaming output search + viewer content
New file: `output_search.rs`; plus `read_job_output_for_viewer` in `commands/jobs.rs`. All
registered in `lib.rs`.

- **`search_job_output(id, opts: SearchOptions) -> SearchResult`** — search a job's `output.out`
  for `opts.query` (`regex` / `case_sensitive` flags). Empty result (not an error) when the job
  has no dir or output yet.
- **`get_search_presets() -> Vec<SearchPresetInfo>`** — the curated ORCA search chips.
- **`read_job_output_for_viewer(id) -> OutputContent { content, first_line_no, total_lines,
  truncated }`** — the file for the Monaco viewer. **Capped to the last `MAX_VIEWER_LINES`
  (300 000 ≈ 30 MB)**: streams line by line into a `VecDeque` that evicts the oldest past the cap,
  so a hundreds-of-MB file is never held whole (domain rule #5). We keep the **tail** (where a run's
  interesting end is) and report `first_line_no` (`> 1` iff truncated) so the viewer shows absolute
  file line numbers and search hits still map. Empty (not an error) when there's no dir/output.
- **`read_job_ensemble(id) -> String`** (2.5.1b) — reads a GOAT job's `input.finalensemble.xyz`
  (the fixed `input.inp` name gives a fixed ensemble name — see `wiki/orca/goat.md`) whole. Unlike
  `output.out` this file is tiny (a multi-frame xyz of one small fragment), so reading it fully is
  fine; capped at `MAX_ENSEMBLE_BYTES` (8 MB) defensively. Empty string (not an error) when there's
  no dir/file or it isn't a GOAT run; `JobDetailScreen` parses it lazily on a completed job.
- **Streaming search (domain rule #5):** `search_output` reads line by line through a `BufReader`,
  holding only an optional context ring buffer, the matches still awaiting trailing context, and
  the capped result list. Each `OutputMatch` carries `line_no`, the matched `line`, and the hit's
  **1-indexed char column range `col_start`/`col_end`** (exclusive end — Monaco range semantics) for
  precise editor decoration. Context (`context_before`/`context_after`) is **opt-in** via
  `SearchOptions.context_lines` (the viewer passes `0`, saving ~2500 lines of payload at 500 hits;
  the old excerpt UI used 2). Single pass, matches finalized in line order, leftovers flushed at
  EOF. Measured: **431 KB / ~8600 lines searched in ~3 ms**.
- **`MAX_MATCHES = 500`** caps returned matches, but `total` counts every hit (so the UI can say
  "500 of 637") and `truncated = total > matches.len()`.
- **Matcher** now returns the match's char column range (not just a bool): regex via
  `RegexBuilder.case_insensitive(!case_sensitive)` (invalid pattern →
  `AppError::Backend("invalid regular expression: …")`) → first `Match` byte range → char columns;
  literal `find` otherwise, with the needle lowercased **once** up front for the case-insensitive
  path (positions taken in the lowercased line — 1:1 for ASCII, which all ORCA output is). Empty
  query → empty result, not an error.
- **Presets (`SEARCH_PRESETS`)** — `id/label/query/regex/case_sensitive/description`. Wording
  **verified against real ORCA 6.1 output** (see `orca/output-files.md`). Two correctness points
  worth remembering:
  - **`errors` is case-SENSITIVE** (`ERROR|error termination|aborting|ABORTING`): a
    case-insensitive `error` matches the benign `DIIS Error` / `Startup error` printed on every SCF
    (12+ hits in a *successful* run). Verified: the case-sensitive query fires **0 times** across
    12 real successful outputs. This is why `SearchPreset` carries a per-preset `case_sensitive`
    flag (a deviation from the original task struct — justified by the false-positive check).
  - **`imaginary` = literal `imaginary mode`**, NOT bare `imaginary` (which hits
    `imaginary perturbations`, a CPHF count present in every Freq run). Confirmed it matches
    ORCA's real `***imaginary mode***` marker on a saddle-point output.
- **11 unit tests** — literal+context (`context_lines: 2`), file-boundary context, case
  sensitivity, regex, invalid regex → error, cap-but-count (600 hits → 500/600/truncated), empty
  query, the real `opt_output_excerpt.txt` fixture (`Geometry convergence` → 2), and **column
  reporting** (`reports_match_columns`, `regex_match_columns_point_at_first_hit`).

## As built (2.5.2d-1) — stale-sidecar detection
`sidecar.rs` gained a version handshake so the app can tell it's talking to an out-of-date sidecar
(the `npm run tauri dev` HMR trap — `wiki/debugging/005`).
- **`Health` gained `Stale`** (serialized `"stale"`) — responding but older than
  `EXPECTED_MIN_SIDECAR_VERSION` (`"0.2.0"`). Distinct from `Down`: the process is alive, it just
  needs a restart. `SidecarStatus` gained `version` (the reported one) and `expected_version`.
- **`health_check`** now reads the `/health` body's `version` and sets `Healthy` vs `Stale` via
  `version_at_least(actual, expected)` — a pure, unit-tested **component-wise numeric** compare
  (string compare lies: `"0.10.0" < "0.9.0"`). Unparseable version → treated as stale, never healthy.
- **`--reload` in dev + process-group kill.** Debug builds launch uvicorn with `--reload`; `start`
  sets the child's **own process group** (`CommandExt::process_group(0)`) and `stop`/`Drop` call
  `kill_process_tree` → `killpg(SIGTERM)` → grace → `killpg(SIGKILL)`, so the `--reload` worker child
  isn't orphaned (the `debugging/004` process-group discipline, reused here). Verified live: no
  orphaned uvicorn, port released.
- **4 new unit tests** (`sidecar::tests`): `version_at_least` component-wise (incl. `0.10.0` vs
  `0.9.0` where string order is wrong), ordering basics, unparseable → stale, and
  `parse_health_version`.

## As built (Phase 2.5.5) — xtb pre-optimization (`xtb.rs`)
Standalone GFN2-xTB relaxation of a scene while holding the user's constraints, so the geometry
handed to ORCA is already sensible. **In Rust, not the sidecar** (logged decision): Rust owns
process spawning (isolation rule #3, kill-the-group `debugging/004`), and the binary path is a
setting (SQLite, under Rust). Details of the tool itself: `wiki/orca/xtb.md`.

- **`xtb_path` setting** — seeded `'xtb'` in migration v1's idempotent seeds (no schema bump; it's
  a `settings` row like `orca_path`). Never bundled (#7). `xtb_version` command runs
  `<path> --version` and parses the banner for the Settings "Check" button; `resolve_binary`
  turns a bare name into an absolute path via `$PATH`.
- **`xtb_optimize(xyz, charge, multiplicity, constraints, timeout_secs?) -> XtbResult`** — the core
  command. `constraints` deserialize from the TS `Constraint` (0-based atoms; `valueText` ignored).
  Flow: parse the input xyz once (for target resolution AND the element-order post-condition) →
  resolve each constraint's target (explicit value, or the geometry's CURRENT value for a
  freeze-as-is) → **`build_xcontrol`** writes the `$constrain`/`$fix` blocks with every index **`+1`
  (xtb is 1-based — `wiki/orca/xtb.md`)** → run `<xtb> input.xyz --input xcontrol --opt --gfn 2
  --chrg <c> --uhf <mult−1>` by full path, in an **isolated dir** (`<data>/xtb/<uuid>`), in its own
  process group → read `xtbopt.xyz`.
- **Post-conditions INSIDE the command** (not only tests — the price of a missed error is the wrong
  geometry into a multi-hour ORCA run): atom count unchanged; element sequence unchanged
  positionally; **each constraint held within tolerance** (`check_held`: 0.1 Å distance / 5° angle /
  0.01 Å `$fix`; the distance tolerance is measured — realistic hold 0.011 Å at force constant 1.0,
  `wiki/orca/xtb.md`). Any breach → `AppError::Backend` with a diagnostic, never a silently-returned
  geometry. The held-check also catches an index-base mistake: a wrong `+1` constrains a different
  pair and the intended one drifts past tolerance.
- **Isolation + cleanup + kill (rule #3, `debugging/004`).** The scratch dir is removed after
  reading, on every path. Single-slot `XtbRunner` managed state holds the running pgid + dir;
  `xtb_cancel` flags it and calls `terminate_job` (killpg SIGTERM→grace→SIGKILL + **cwd sweep**),
  the SAME primitives as the ORCA backend (made `pub(crate)` — one copy). A timeout does the same.
  xtb is synchronous (seconds), so it's a blocking command polling `try_wait`, not a queued job.
- **Registration:** `xtb::{xtb_version, xtb_optimize, xtb_cancel}` in the invoke handler;
  `app.manage(xtb::XtbRunner::default())` in setup. 10 unit tests (`xtb::tests`): 1-based xcontrol
  per op, cartesian→`$fix`, freeze-as-is resolves the current value, out-of-range rejected before
  spawn, `check_held` flags a drift / passes within tolerance, xyz parse.

## Deviation note
`dirs` crate used for the data dir (per task spec) rather than Tauri's `app.path()` API —
harmless; consolidate later if desired.

## Responsibilities
Job state machine, SQLite ownership, ExecutionBackend implementations, sidecar lifecycle,
log tailing/polling, settings.

## Key commands (planned API surface)
`create_job`, `submit_job`, `cancel_job`, `list_jobs`, `get_job`,
`read_log_chunk(job_id, offset)`, `save_molecule`, `list_molecules`,
`get_settings`, `set_settings`, `test_server_profile`.

## Events emitted
`job:status(job_id, status)`, `job:log(job_id, lines)`, `job:convergence(job_id, point)`,
`sidecar:status(healthy|down)`.

## Conventions
- Every command returns `Result<T, AppError>` (thiserror), serialized as
  `{ code, message }` for the frontend.
- Startup sequence: open+migrate SQLite → reconcile job states → spawn sidecar → emit ready.
- No `.unwrap()` outside tests.
