# Module: Rust core (src-tauri/)

**Status:** Phase 1 step 3 done — LocalBackend runs ORCA end-to-end (spawn, live log
tailing, completion detection), on top of the job model (step 1) and Phase 0 scaffold.

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
  as `draft`), `list_jobs() -> Vec<Job>` (newest first, `created_at DESC`),
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
