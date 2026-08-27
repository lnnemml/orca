# Module: Execution backends

**Status:** `LocalBackend` runs ORCA end-to-end — isolated job dir, CPU core pinning, a sequential
in-SQLite queue, cancellation with an MPI-rank sweep, and startup reconciliation. The
`ExecutionBackend` trait (ADR-003) exists (`src-tauri/src/execution_backend.rs`); `LocalBackend`
implements it, and **the Tauri command layer dispatches through the trait** — `submit_job` /
`cancel_job` construct a `LocalBackend` and call `submit` / `cancel` on it. The running machinery
still lives in `src-tauri/src/local_backend.rs` (queue, process tree, cancellation) — each trait
method **delegates** there. `SshBackend` is a later Phase 5 unit.

## The `ExecutionBackend` trait (`execution_backend.rs`, ADR-003)

The trait uses ADR-003's five signatures verbatim; all methods return `Result<_, AppError>` and are
**Tauri-type-free** (no `AppHandle` in a signature) so `SshBackend` — which has no `AppHandle` to
reach app state — can implement them:

```rust
fn submit(&self, job: &Job) -> Result<JobHandle>;
fn poll_log(&self, h: &JobHandle, offset: u64) -> Result<LogChunk>;
fn status(&self, h: &JobHandle) -> Result<JobStatus>;
fn fetch_results(&self, h: &JobHandle, policy: FetchPolicy) -> Result<()>;
fn cancel(&self, h: &JobHandle) -> Result<()>;
```

- **`JobHandle(pub String)`** — a backend-opaque reference wrapping the job id (the stable key both
  backends state-key on: the DB row locally, the remote scratch dir over SSH).
- **`LogChunk { offset: u64, data: String }`** — one incremental log slice; `offset` is the **new**
  byte offset *after* `data`, fed back on the next poll so reads resume where they stopped (and
  survive an app restart once persisted — ADR-003).
- **`FetchPolicy { include_gbw: bool }`** — output/xyz/hess always; the large `.gbw` is opt-in.
  **Degenerate for local** (everything is already on disk); it exists now because it shapes
  `SshBackend` (ADR-023).

**`poll_log` is the offset-pull name.** The ROADMAP's `stream_log` wording folds into `poll_log`:
there is one log method and it is pull-based (offset-in, chunk-out), per ADR-003's "pull, not push"
so the same interface serves a local file and a remote `tail -c +<offset>`. Note the live UI today
still uses the **push** `job:log` event (the tailing thread) — Part B does **not** flip push→pull;
`poll_log` is the additive pull path, wired to callers in a later unit.

**Command dispatch (Part B).** `commands::jobs::submit_job` and `cancel_job` route through the trait:
each constructs a `LocalBackend::new(app)` from the `AppHandle` it already receives and calls
`backend.submit(&job)` / `backend.cancel(&JobHandle(id))`. No new managed state and no command-
signature change — the backend is a zero-cost `AppHandle` wrapper, so construct-at-call-site is the
least-churn seat. The Tauri command names, signatures, return types, and the `job:log` / `job:status`
/ `job:convergence` events are **byte-identical** to the pre-Part-B direct `local_backend::` calls:
the trait method delegates to the same free function. The queue-control and log-read free functions
that have **no** trait method (`set_paused` / `is_paused` / `remove_job_dir` / `read_tail_lines` /
`read_convergence` / `read_scan_surface`) stay direct `local_backend::` calls — they are not part of
the five-method `ExecutionBackend` surface.

**No crate-level `dead_code` allow.** Part A's `#![allow(dead_code)]` is removed. `submit` / `cancel`
/ `JobHandle` / `LocalBackend` are now reached by live callers. The still-unrouted trait surface —
`poll_log`, `status`, `fetch_results`, and `FetchPolicy` — carries a **targeted** `#[allow(dead_code)]`
per item, each with a comment naming where it gets routed (the push→pull flip for `poll_log`, the
`SshBackend` unit for `status` / `fetch_results` / `FetchPolicy`). Targeted over blanket so a
*genuinely* unrouted item stays visible while any *accidentally* dead code elsewhere still warns.

**`LocalBackend { app: AppHandle }` — delegation map** (trait method → existing free function):

| Trait method | Delegates to |
|---|---|
| `submit` | `local_backend::submit(&app, &job.id)` → returns `JobHandle(job.id)` |
| `poll_log` | `local_backend::read_log_chunk(output.out, offset, cap)` (new bounded offset read) |
| `status` | `get_job_conn(conn, id)?.status` |
| `fetch_results` | no-op (`Ok(())`) — artifacts already on disk; policy only bites over SSH |
| `cancel` | `local_backend::cancel(&app, &id)` |

`read_log_chunk(path, offset, max_bytes) -> (String, u64)` is the additive pull read: seeks to
`offset`, reads at most `max_bytes` forward, returns the slice + the new offset; at/past EOF returns
empty data with the offset clamped to the file length. It is the mirror of `read_tail`'s seek-from-
end and never loads the whole log (domain rule #5). Unit-tested — sequential chunks reassemble the
original byte-exact, EOF holds the offset, the cap is respected — with a **negative control** (a
wrong-offset reader) proven to make the reassembly gate go red.

**Dispatch is `enum`-deferred.** There is one concrete backend and no `enum` / `dyn` dispatch layer.
Commands dispatch through the trait on a **concrete** `LocalBackend` — no runtime backend selection
yet. The `enum Backend { Local(LocalBackend), Ssh(SshBackend) }` static-dispatch selector and the
`jobs.backend_id` column land in the `SshBackend` unit (ADR-023), where a second implementation
forces the still-maturing `poll_log(offset)` / `fetch_results(policy)` shapes and a `match` on
`backend_id` becomes the "Run on:" selector. See `wiki/log.md` (unit 5.0 Part A / Part B).

## Where the code lives

Implemented across two modules, **not** a `backends/` trait dir: the trait + `LocalBackend` in
`src-tauri/src/execution_backend.rs`, the running machinery below in `src-tauri/src/local_backend.rs`.

## How a local job runs (`local_backend.rs`)

Entry point: `submit(app, job_id)` (via the `submit_job` Tauri command). A job flows:

1. **Validate** — job exists and is `draft` (else `AppError::Backend`).
2. **Resolve ORCA path** — read `settings.orca_path`; must be non-empty. Invoked as a **full
   absolute path** (domain rule #1) so ORCA's MPI self-re-invocation for `%pal` works.
3. **Enqueue, don't reserve-or-error.** `submit` moves the draft to `queued` and calls
   `try_start_next` — it **never** returns "another job is already running" (see Sequential queue).
   The single running slot (`JobRunner.running`) is reserved by `try_start_next` when it actually
   starts a job; the mutex reservation is what makes starting race-safe, including React
   StrictMode's dev double-submit. *(Was: submit ran the draft directly and errored on a busy slot;
   changed in `[2026-07-27] LocalBackend: CPU pinning, job queue, cancel`.)*
4. **Isolated job dir** (domain rule #3) — `prepare_job_dir` creates `<data>/jobs/<job_id>/` and
   writes `input.inp`; the absolute path is stored in `jobs.job_dir`.
5. **Spawn** — `run_orca(orca_path, job_dir, cpu_mask)`: `Command` with `stdin` null, `stdout`
   piped (for tailing), `stderr` → `stderr.log`, `current_dir` = job dir. With no mask it runs
   `<orca> input.inp` directly; with a mask, the pinned form (see CPU pinning). Mark job `running`.
6. **Tailing thread** (one per run) — `BufReader` over child stdout, line by line: append each line
   to `output.out` **and** batch to the UI via `job:log` (flush every 50 lines or 100 ms). Never
   loads the whole output into memory (domain rule #5).
7. **Completion** — on `child.wait()`: write the exit code to `.exit_code`, then `detect_completion`
   reads only a ~5 KB **tail** of `output.out`: `completed` iff it contains
   `ORCA TERMINATED NORMALLY` **and** exit code == 0 (domain rule #6); else `failed` with an
   `error_message` from `stderr.log` (or the output tail). Persist via `finalize_job_conn`, release
   the slot, emit terminal `job:status`.

Artifacts in each job dir: `input.inp`, `output.out`, `stderr.log`, `.exit_code` (plus ORCA's own
scratch files — the cleanup policy in `orca-basics.md` is not yet applied).

**No runner script (local).** Locally we pipe stdout in Rust and write `.exit_code` ourselves —
simpler, and it gives the live stream directly. The `SshBackend` will still use a remote runner
script (a pipe can't be held across SSH), so the `.exit_code` marker convention is shared; only the
local path skips the wrapper.

Unit-tested: `prepare_job_dir`, `read_tail`, `last_lines`, `detect_completion`; plus an `#[ignore]`d
end-to-end `real_orca_water_single_point_completes` that runs a real water single point through
`run_orca` + `detect_completion` against `/opt/orca/orca`.

## CPU pinning (domain rule #8)

- **`cpu_presets.rs`** — measured presets: `interactive` = mask `8-15` / 8 ranks (the default),
  `max_throughput` = `0,2,4,6,8-15` / 12 ranks. **The masks are specific to the dev machine's
  i5-12500H** — documented loudly in the module doc-comment; a different machine uses the `custom`
  preset. No topology auto-detection (out of scope). `get_cpu_presets` exposes them to the Settings
  UI. Preset rationale and the benchmark are in `wiki/orca/performance.md`.
- `resolve_cpu_config(&Connection) -> (Option<String>, u32)` reads `cpu_preset` / `cpu_mask` /
  `cpu_nprocs` from `settings`; falls back to the interactive preset on missing/malformed values.
  A `None` mask means no pinning (direct invocation).
- With a mask, `run_orca` spawns `taskset -c <mask> <orca> input.inp` **with
  `OMPI_MCA_hwloc_base_binding_policy=none`** so taskset and OpenMPI don't fight over placement.
  Missing `taskset` → a clear Backend error ("install util-linux, or set cpu_preset to disable
  pinning"), never a silent failure.
- `align_pal_nprocs(input, nprocs) -> (String, bool)` rewrites/inserts `%pal nprocs N end` to match
  the pinned core count (oversubscribing the mask is ~3× *slower*, not faster — 12 ranks on 4
  cores). Handles single-line and block `%pal` forms; inserts after the `!` line when absent. When
  it rewrites, an info line is emitted to the job log
  (`[OrcaStudio] %pal nprocs aligned to N (cpu preset: …)`) — not silent magic.
- Verified against real ORCA (headless): benzene B3LYP/def2-SVP `%pal nprocs 4` via the exact
  rule-8 command line — all 5 ORCA processes pinned to cores 8–15, sharing one PGID.

## Sequential queue — in SQLite, not in memory

- **No worker thread / channel.** The queue *is* the set of jobs with `status='queued'`.
  `try_start_next(app)` picks the oldest queued job (`ORDER BY created_at ASC`) and starts it if the
  slot is free and the queue isn't paused. Called after enqueue, after each job finishes
  (`drive_job`), and on resume. This survives an app restart for free.
- `JobRunner` = `data_dir` + `Mutex<Option<RunningJob>>` (the single slot) + an `AtomicBool` pause
  flag. `RunningJob { job_id, pgid, cancelled }`. Concurrency = 1 (domain rule #4).
- **Pause is queue-only.** `pause_queue` stops the *next* job from starting; the running job runs to
  completion. We deliberately do **not** SIGSTOP the running ORCA: it holds all its RAM
  (nprocs × maxcore) frozen, and MPI ranks stopped mid-communication may not resume cleanly.
- **Lock order** is always `running` → `db` (only `try_start_next` nests them); `cancel` and
  `start_run` take each lock alone. No inversion, no deadlock.

## Cancellation — killpg the group **and** sweep the escaped MPI ranks by cwd

**The trap:** `process_group(0)` does **not** put ORCA and all its MPI ranks in one group. On a
`%pal nprocs 4` run (verified with real `ps`), only the `orca` parent, its `sh`, and `mpirun` share
the leader's group — **each MPI rank (`orca_*_mp`) has its own process group** (`PGID == its own
PID`), because `mpirun` `setpgid`s every rank so terminal signals can't reach them. So
`killpg(pgid, …)` reaches only orca + sh + mpirun, **not the ranks**. See
`debugging/004-mpi-ranks-escape-process-group.md` and `orca/gotchas.md`.

A plain `killpg` *appears* to work only because a SIGTERM'd `mpirun` reaps its own ranks on the way
out (pure OpenMPI cooperation). It breaks on the SIGKILL path: after the grace period
`killpg(SIGKILL)` kills `mpirun` instantly, so it can't forward anything → **N orphaned ranks burn
N cores forever** — exactly the heavy-job-won't-exit-on-SIGTERM case where the user hits Cancel.

**Fix — cwd is the reliable membership signal.** Every process of a job runs with `cwd` = the job
directory. *(Changed in `[2026-07-28] fix: MPI ranks escape process group on cancel`.)*

- `sweep_job_processes(job_dir, sig)` walks `/proc/<pid>/cwd` and signals every live process whose
  cwd matches the canonicalized job dir, skipping our own pid. Best-effort; unreadable `/proc`
  entries are skipped. This is the safety net behind `killpg`.
- `terminate_job(pgid, job_dir)`: (1) `killpg(SIGTERM)`; (2) wait **up to 10 s** for the group to
  drain (heavy jobs may still be flushing `.gbw`); (3) `sweep(SIGTERM)` — catches ranks mpirun
  never reached — **before** any SIGKILL, so ranks still get a clean exit; (4) 2 s grace;
  (5) `killpg(SIGKILL)` **and** `sweep(SIGKILL)`. Logs (`eprintln!`) if the final sweep had to
  hard-kill anything — that means the graceful path failed. (`terminate_job` and
  `sweep_job_processes` are `pub(crate)` and reused by the xtb pre-optimizer — ADR-009.)
- **Non-blocking cancel.** `terminate_job` can take ~12 s, so `cancel` spawns it on a thread and
  returns immediately; the `cancelled` flag already guarantees `drive_job` finalizes as
  `cancelled`, so the UI needn't wait. The frontend Cancel button shows a disabled "Cancelling…"
  until the terminal `job:status` arrives.
- **App exit** (`terminate_on_exit`, from the `lib.rs` `ExitRequested` handler) runs the same
  `terminate_job` **synchronously** — a spawned thread would die with the process before the ranks
  do, stranding them.
- `cancel(app, job_id)`: **queued** → finalize as `cancelled` (nothing to kill); **running** → set
  the `cancelled` flag, then spawn `terminate_job`; other status → `Backend(...)`. `drive_job`
  checks the flag after `child.wait()` and records `cancelled` with a clean message.

## Startup reconciliation

`reconcile_on_startup(&Connection)` (called in `lib.rs` setup before the connection is managed):
every job still `running` in the DB is re-checked — if its dir shows a finished ORCA run
(`.exit_code` + banner) it is finalized (with results); otherwise it is marked `failed` with "app
was closed while this job was running". `queued` jobs are left for the startup `try_start_next` to
resume. This closes the Phase 1 gap where a crashed `running` job stayed `running`.

## Graceful stop — investigated, not implemented

ORCA reportedly supports stopping a geometry optimization cleanly after the current cycle via a
marker file (preserving a valid `.gbw` + last geometry). This could **not** be confirmed: the ORCA
6.1 manual isn't indexed locally yet (Phase 4; `resources/manual/` holds only a README). So only
hard kill (killpg + sweep) is implemented. See `wiki/orca/gotchas.md` — revisit "Stop after current
cycle" once the manual is indexed.

## SshBackend (Phase 5)

- Upload: `rsync -az <job_dir>/ <host>:<scratch>/<job_id>/`
- Launch: `ssh <host> 'cd <dir> && nohup bash run.sh > /dev/null 2>&1 & echo $! > .pid'`, where
  `run.sh` = the ORCA invocation + a `.exit_code` writer.
  - **Cancel gotcha (same as local):** ORCA's MPI ranks escape the parent's process group
    (`orca_*_mp`, each its own PGID — `debugging/004`). Killing the `.pid` parent (or its group)
    remotely leaves the ranks burning the remote node's cores. The remote cancel must sweep by cwd
    too, e.g. `ssh <host> "fuser -k <dir>"` or `pkill -f <dir>`; a `.pid` marker for the parent
    alone is **not** enough. Design the remote runner so every rank inherits `cwd = <dir>`.
- Poll: `ssh <host> "tail -c +<offset> <dir>/output.out"` every 5–10 s; offset persisted in SQLite
  so polling resumes across app restarts.
- Fetch: `rsync` back per `FetchPolicy` (output/xyz/hess always, gbw opt-in, cubes on demand).
- Remote cube generation: `ssh <host> 'cd <dir> && <orca_bin_dir>/orca_plot ...'`.

## Invariants (both backends)

- Completion = `.exit_code` present **and** `ORCA TERMINATED NORMALLY` in the output tail.
- Reconciliation on startup for any non-terminal job state.
- Concurrency 1 per backend by default.
