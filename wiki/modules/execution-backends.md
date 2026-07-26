# Module: Execution backends (src-tauri/src/backends/)

**Status:** LocalBackend **built and verified against real ORCA** (Phase 1.3). SshBackend
still Phase 5. Note: implemented as a single module `src-tauri/src/local_backend.rs`, not a
`backends/` trait dir yet — the `ExecutionBackend` trait (ADR-003) will be extracted when the
second backend arrives (ROADMAP Phase 2 makes the seam explicit).

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

**Not yet:** startup reconciliation (a job left `running` after a crash stays `running`);
cancel/kill of a running job; result parsing (energy, wall time).

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
