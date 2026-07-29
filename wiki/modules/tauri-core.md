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
- `db.rs` — SQLite open + versioned migrations (v1–v4).
- `error.rs` — `AppError` (thiserror).
- `sidecar.rs` — `SidecarManager`: spawn/health-poll/kill uvicorn; the version handshake.
- `commands/{settings,jobs,molecules}.rs` — Tauri command surface (thin wrappers over `*_conn`).
- `models/{job,molecule}.rs` — row structs (`COLUMNS` + `from_row`).
- `local_backend.rs` — ORCA execution, queue, cancel (see `execution-backends.md`).
- `cpu_presets.rs` — measured core-pinning presets (see `execution-backends.md`).
- `result_extraction.rs`, `convergence.rs` — result/convergence parsing (see `parser.md`).
- `output_search.rs` — streaming output search + presets.
- `xtb.rs` — xTB pre-optimization.

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
- The two new queue statuses (`queued`, `cancelled`) needed **no migration** — `status` is TEXT.
- Migration tests assert preservation across each step (`migrate_v1_to_v2_preserves_settings`,
  `migrate_v2_to_v3_preserves_jobs`, `migrate_v3_to_v4_preserves_jobs`; the version assertions use
  `SCHEMA_VERSION`, not a literal). Verified against a copy of the real DB: 13 existing jobs
  preserved across 3→4, `scene_json` NULL on every one.

**`scene_json` semantics** (ADR-008 #5 + amendment): a versioned `SceneFragment` snapshot written
**once at create time** — the job's input is immutable, so its snapshot is too (no update path). It
**annotates** `input_content`; the text stays authoritative for geometry (the frontend's
`restoreScene` reconciles them).

## Models (`models/`)

`Job` and `Molecule` mirror their tables 1:1 (`#[derive(Serialize)]`). `from_row` hydrates from a
row in `COLUMNS` order; `COLUMNS` is the single source of truth for the select list (`Job` gained
`scene_json` as its 11th column in v4). `JobStatus` = `Draft | Queued | Running | Completed |
Failed | Cancelled`, serialized to/from lowercase strings on the wire and in the DB
(`as_str`/`from_db`; the TS `JobStatus` union tracks it in lockstep). State machine: `draft →
queued → running → completed | failed | cancelled`. Remote states (uploading/syncing) and `parsed`
are deferred. An unknown status string from the DB → `AppError::Internal` via `from_db`.

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
- **Jobs:** `create_job(title, input_content, scene_json: Option<String>) -> Job` (UUID, inserts
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
- **DB helpers** (`pub(crate)`, reused by the backend): `set_job_dir_conn`, `finalize_job_conn`
  (terminal status + `completed_at` + `error_message`), `set_job_results_conn`, `get_job_conn`,
  `update_job_status_conn`.

`open_job_folder` spawns the platform file manager (`xdg-open` / macOS `open` / Windows
`explorer`) — an app-defined command, so no capability entry is needed. The frontend `listen`s to
events (allowed by the `core:default` capability) and filters by `job_id`.

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
