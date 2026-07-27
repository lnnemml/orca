# Module: Execution backends (src-tauri/src/backends/)

**Status:** LocalBackend **built and verified against real ORCA** (Phase 1.3), extended in
Phase 2 with CPU core pinning, a sequential queue, and cancellation. SshBackend still Phase 5.
Note: implemented as a single module `src-tauri/src/local_backend.rs`, not a `backends/` trait
dir yet — the `ExecutionBackend` trait (ADR-003) will be extracted when the second backend
arrives (ROADMAP Phase 2 makes the seam explicit).

## LocalBackend — as built (Phase 1.3)
File: `src-tauri/src/local_backend.rs`. Entry point: `submit(app, job_id)` (via the
`submit_job` Tauri command). Flow:

1. **Validate** — job exists and is `draft` (else `AppError::Backend`).
2. **Resolve ORCA path** — read `settings.orca_path`; must be non-empty. Invoked as a **full
   absolute path** (domain rule #1) so ORCA's MPI self-re-invocation for `%pal` works.
3. **Reserve slot** — `JobRunner.running: Mutex<Option<String>>` holds the one running job id;
   a second submit returns `"another job is already running"` (concurrency = 1, domain rule #4).
   The mutex reservation is what makes this race-safe (incl. React StrictMode double-submit).
4. **Isolated job dir** (domain rule #3) — `prepare_job_dir` creates `<data>/jobs/<job_id>/`
   and writes `input.inp`; the absolute path is stored in `jobs.job_dir`.
5. **Spawn** — `run_orca`: `Command::new(orca_path).arg("input.inp").current_dir(job_dir)`,
   `stdin` null, `stdout` piped (for tailing), `stderr` → `stderr.log`. Mark job `running`.
6. **Tailing thread** (one per run) — `BufReader` over child stdout, line by line:
   append each line to `output.out` **and** batch to the UI via `job:log` (flush every 50
   lines or 100 ms). Never loads the whole output into memory (domain rule #5).
7. **Completion** — on `child.wait()`: write exit code to `.exit_code`, then `detect_completion`
   reads only a ~5 KB **tail** of `output.out`: `completed` iff it contains
   `ORCA TERMINATED NORMALLY` AND exit code == 0 (domain rule #6); else `failed` with an
   `error_message` from `stderr.log` (or the output tail). Persist via `finalize_job_conn`,
   release the slot, emit terminal `job:status`.

Artifacts left in each job dir: `input.inp`, `output.out`, `stderr.log`, `.exit_code`
(plus ORCA's own scratch files — cleanup policy per `orca-basics.md` not yet applied).

**Design note — no runner script.** The earlier plan wrapped ORCA in a `run.sh` that does
`echo $? > .exit_code`. Locally we instead pipe stdout in Rust and write `.exit_code`
ourselves — simpler and gives us the live stream directly. The SshBackend will still use a
remote runner script (can't hold a pipe across SSH), so the `.exit_code` marker convention is
shared; only the local path skips the wrapper.

**Verified:** unit tests for `prepare_job_dir`, `read_tail`, `last_lines`, `detect_completion`;
plus an `#[ignore]`d end-to-end test (`real_orca_water_single_point_completes`) that runs a real
water single point through `run_orca` + `detect_completion` against `/opt/orca/orca` — passes
(`cargo test -- --ignored`), output contains `ORCA TERMINATED NORMALLY`, `.exit_code` written.

## As built (Phase 2) — CPU pinning, sequential queue, cancellation

The LocalBackend moved from "run one draft job, error if busy" to a real sequential queue with
pinning and cancellation. All still concurrency = 1 (domain rule #4).

### CPU pinning (domain rule #8)
- `src-tauri/src/cpu_presets.rs`: measured presets (`interactive` = `8-15`/8 ranks, the default;
  `max_throughput` = `0,2,4,6,8-15`/12 ranks). **Masks are specific to the dev machine's
  i5-12500H** — documented loudly in the module doc-comment; a different machine uses the
  `custom` preset. No topology auto-detection (out of scope). `get_cpu_presets` exposes them to
  the Settings UI.
- `resolve_cpu_config(&Connection) -> (Option<String>, u32)` reads `cpu_preset` / `cpu_mask` /
  `cpu_nprocs` from `settings`; falls back to the interactive preset on missing/malformed values.
  `None` mask = no pinning (direct invocation).
- `run_orca(orca_path, job_dir, cpu_mask: Option<&str>)`: with a mask, spawns
  `taskset -c <mask> <orca> input.inp` **with `OMPI_MCA_hwloc_base_binding_policy=none`** so
  taskset and OpenMPI don't fight over placement. Missing `taskset` → a clear Backend error
  ("install util-linux, or set cpu_preset to disable pinning"), never a silent failure.
- `align_pal_nprocs(input, nprocs) -> (String, bool)`: rewrites/inserts `%pal nprocs N end` to
  match the pinned core count (oversubscribing the mask is 3× *slower*, not faster — 12 ranks on
  4 cores). Handles single-line and block `%pal` forms; inserts after the `!` line when absent.
  When it rewrites, an info line is emitted to the job log
  (`[OrcaStudio] %pal nprocs aligned to N (cpu preset: …)`) — not silent magic.
- **Verified against real ORCA (headless):** benzene B3LYP/def2-SVP `%pal nprocs 4` launched via
  the exact rule-8 command line — all 5 ORCA processes pinned to cores 8–15, sharing one PGID.

### Sequential queue — in SQLite, not in memory
- **No worker thread / channel.** The queue *is* the set of jobs with `status='queued'`.
  `try_start_next(app)` picks the oldest queued job (`ORDER BY created_at ASC`) and starts it if
  the slot is free and the queue isn't paused. Called after enqueue, after each job finishes
  (`drive_job`), and on resume. This survives an app restart for free.
- `submit` now moves a draft job to `queued` and calls `try_start_next` — it **never** returns
  "another job is running". Users can stack up jobs; they run one at a time.
- `JobRunner` = `data_dir` + `Mutex<Option<RunningJob>>` (the single slot) + `AtomicBool` pause
  flag. `RunningJob { job_id, pgid, cancelled }`.
- **Pause is queue-only.** `pause_queue` stops the *next* job from starting; the running job runs
  to completion. We deliberately do **not** SIGSTOP the running ORCA: it holds all its RAM
  (nprocs × maxcore) frozen, and MPI ranks stopped mid-communication may not resume cleanly.
- **Lock order** is always `running` → `db` (only `try_start_next` nests them); `cancel` and
  `start_run` take each lock alone. No inversion, no deadlock.

### Cancellation — killpg the process group
- Spawn uses `CommandExt::process_group(0)` so ORCA and every MPI rank it forks share one process
  group (pgid == child pid). Killing only the parent would orphan the ranks — they'd keep burning
  CPU. Confirmed: `kill -TERM -<pgid>` reaps the whole tree.
- `cancel(app, job_id)`:
  - **queued** → just finalize as `cancelled` (nothing to kill).
  - **running** → set the `cancelled` flag (so `drive_job` records `cancelled`, not `failed`),
    then `terminate_pgid`: `killpg(SIGTERM)`, poll up to 5 s, `killpg(SIGKILL)`. Uses `libc`
    (Unix-only target dep). `.exit_code` gets a `cancelled` marker.
  - any other status → `Backend("job is not running or queued")`.
- `drive_job` checks the `cancelled` flag after `child.wait()` and records `cancelled` with a
  clean message instead of a stderr sheet.

### Startup reconciliation
- `reconcile_on_startup(&Connection)` (called in `lib.rs` setup before the connection is managed):
  every job still `running` in the DB is re-checked — if its dir shows a finished ORCA run
  (`.exit_code` + banner) it's finalized (with results); otherwise it's marked `failed` with
  "app was closed while this job was running". `queued` jobs are left for the startup
  `try_start_next` to resume. Closes the Phase 1 gap (a crashed `running` job stayed `running`).

### Graceful stop — investigated, not implemented
ORCA reportedly supports stopping a geometry optimization cleanly after the current cycle via a
marker file. This could NOT be confirmed: the ORCA 6.1 manual isn't indexed locally yet (Phase 4;
`resources/manual/` holds only a README). So only hard kill (killpg) is implemented. See
`wiki/orca/gotchas.md` — revisit "Stop after current cycle" once the manual is indexed.

## SshBackend (Phase 5)
- Upload: `rsync -az <job_dir>/ <host>:<scratch>/<job_id>/`
- Launch: `ssh <host> 'cd <dir> && nohup bash run.sh > /dev/null 2>&1 & echo $! > .pid'`
  where run.sh = orca invocation + `.exit_code` writer
- Poll: `ssh <host> "tail -c +<offset> <dir>/output.out"` every 5–10s; offset persisted
  in SQLite so polling resumes across app restarts
- Fetch: `rsync` back per FetchPolicy (output/xyz/hess always, gbw opt-in, cubes on demand)
- Cube generation remotely: `ssh <host> 'cd <dir> && <orca_bin_dir>/orca_plot ...'`

## Invariants (both backends)
- Completion = `.exit_code` present AND "ORCA TERMINATED NORMALLY" in output tail.
- Reconciliation on startup for any non-terminal job state.
- Concurrency 1 per backend by default.
