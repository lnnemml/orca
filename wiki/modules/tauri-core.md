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
  `running` in the DB) — deferred to Phase 2 (matches ROADMAP). No result parsing (energy/
  wall_time) yet — separate ROADMAP item.

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
