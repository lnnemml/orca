# Module: Rust core (src-tauri/)

**Status:** Phase 0 scaffold done — SQLite init, settings commands, sidecar lifecycle.

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
