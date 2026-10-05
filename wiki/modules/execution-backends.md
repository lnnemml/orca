# Module: Execution backends

**Status:** `LocalBackend` runs ORCA end-to-end — isolated job dir, CPU core pinning, a sequential
in-SQLite queue, cancellation with an MPI-rank sweep, and startup reconciliation. The
`ExecutionBackend` trait (ADR-003) exists (`src-tauri/src/execution_backend.rs`); `LocalBackend`
implements it, and **the Tauri command layer dispatches through the trait** — `submit_job` /
`cancel_job` construct a `LocalBackend` and call `submit` / `cancel` on it. The running machinery
still lives in `src-tauri/src/local_backend.rs` (queue, process tree, cancellation) — each trait
method **delegates** there. `SshBackend` is Phase 5 unit 5.3. See ADR-024 (accepted): the
remote queue lives on the server (`tsp`), the server FS is the source of truth, and concurrency
becomes a per-backend/profile setting rather than the global rule-#4 constant.

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
- **`LogChunk { offset: u64, bytes: Vec<u8>, reset: bool }`** — one incremental log slice of **raw
  bytes**; `offset` is the **new** byte offset *after* `bytes`, fed back on the next poll. Bytes are
  never decoded at the transport: a chunk boundary can fall inside a multi-byte UTF-8 character
  (measured, `orca/remote-sync-probe.md`). `reset` means the log is now **shorter than the requested
  offset** (replaced or truncated): `offset` is 0, `bytes` empty, and the consumer drops its carry and
  live state and reads again from 0 (ADR-024 o item 7). An absent log is `LogChunk::unchanged(offset)`,
  not an error.
- **`plan_log_read(size, offset, cap) -> LogRead`** — the one rule both backends follow: `size < offset`
  ⇒ `Reset`, else `Range { start: offset, len: min(cap, size − offset) }`. The local read uses it to
  decide what to read; the remote reply parser uses it as its post-condition.
- **`LineAssembler`** — the consumer side: keeps the bytes after the last `\n` as a carry and decodes
  only complete lines (a trailing `\r` dropped, as `BufRead::lines`), so a split character is decoded
  whole and no byte is lost or repeated across polls; a `reset` chunk drops the carry; `take_partial`
  returns an unterminated last line. The carry is bounded (`MAX_LINE_CARRY`, 1 MiB): past it the valid
  UTF-8 prefix is emitted. Its caller is the remote log poller (5.3 Part B); the local live log is
  still the push `job:log` event.
- **`FetchPolicy { include_gbw: bool }`** — over SSH every fetch brings down the shared artifact list
  (below) plus `stderr.log`, the markers and `.tsp-out/`; the large `.gbw` is the opt-in.
  **Degenerate for local** (everything is already on disk).

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

`read_log_chunk(path, offset, max_bytes) -> LogChunk` is the pull read: it plans with `plan_log_read`,
seeks to `offset`, reads at most `max_bytes` (`POLL_LOG_MAX_BYTES`, 256 KiB, shared with the remote
poll) and returns raw bytes + the new offset; at EOF the offset holds, past EOF it is a `reset`. It is
the mirror of `read_tail`'s seek-from-end and never loads the whole log (domain rule #5). Unit-tested —
sequential chunks (with Å/ü/→) reassemble the original byte-exact, EOF holds the offset, past EOF
resets, the cap is respected — with a **negative control** (a wrong-offset reader) proven to make the
reassembly gate go red.

**Dispatch is `enum`-deferred.** There is one concrete backend and no `enum` / `dyn` dispatch layer.
Commands dispatch through the trait on a **concrete** `LocalBackend` — no runtime backend selection
yet. The `enum Backend { Local(LocalBackend), Ssh(SshBackend) }` static-dispatch selector lands in
5.3 Part B. It keys on the job's **coordinates** (`Job::is_remote`: `remote_host` non-NULL, schema
v20), never on `backend_id`, which only names the profile and is nulled when a profile is deleted
(ADR-024 o item 1). See `wiki/log.md` (unit 5.0 Part A / Part B).

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

- **No worker thread / channel.** The local queue *is* the set of **local** jobs with
  `status='queued'`. `try_start_next(app)` picks the oldest one (`next_local_queued_job`:
  `status = 'queued' AND remote_host IS NULL ORDER BY created_at ASC`) and starts it if the
  slot is free and the queue isn't paused. A remote job is also `queued` while it waits in its
  server's `tsp` queue, so the `remote_host IS NULL` filter is what keeps the laptop from running it
  (ADR-024 o item 1; a test with the filter removed goes red). Called after enqueue, after each job finishes
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
every **local** job still `running` in the DB (`remote_host IS NULL`; a remote job keeps computing on
its server while the app is closed and is decided only by the server's facts) is re-checked — if its dir shows a finished ORCA run
(`.exit_code` + banner) it is finalized (with results); otherwise it is marked `failed` with "app
was closed while this job was running". `queued` jobs are left for the startup `try_start_next` to
resume. This closes the Phase 1 gap where a crashed `running` job stayed `running`.

## Graceful stop — investigated, not implemented

ORCA reportedly supports stopping a geometry optimization cleanly after the current cycle via a
marker file (preserving a valid `.gbw` + last geometry). This could **not** be confirmed: the ORCA
6.1 manual isn't indexed locally yet (Phase 4; `resources/manual/` holds only a README). So only
hard kill (killpg + sweep) is implemented. See `wiki/orca/gotchas.md` — revisit "Stop after current
cycle" once the manual is indexed.

## SshBackend (Phase 5 — not built yet)

The `SshBackend` itself is not wired yet (unit 5.3). The design is in ADR-024 (Decisions a–l). Its
server-side parts exist and are tested on this machine: the three scripts and the pure classifier,
in `src-tauri/src/remote/` ([remote-jobs.md](remote-jobs.md)). Unit order is ROADMAP Phase 5: 5.2
scripts + classifier (done), 5.3 wiring, 5.4 cancel/reconnect, 5.5 preflight.
- **Queue and launch:** a static wrapper script (`include_str!`, to be uploaded content-addressed by
  rename in 5.3) runs through a per-slot `tsp` queue. It publishes `.started` with a no-clobber
  `ln -T` (an existing `.started` in any form refuses the start), and `.exit_code` by temp file +
  `rename`. `.cancelled` (cancel script) and `.enqueued` (5.3) are also published by temp file +
  `rename`.
- **Transport:** per-job arguments cross ssh only as a NUL-separated list on stdin, never as ssh argv
  (measured as injection-capable, ADR-024 l / P1). Job-dir paths match `[A-Za-z0-9._/-]+`.
- **Cancel:** one cancel script — `.cancelled` first, a `tsp -r` only after the id is verified, TERM
  to the verified wrapper's group, and a **cwd-filtered SID sweep** (ADR-024 i, l). It never kills by
  `tsp` id or by name.
- **Status:** a pure classifier over a raw-fact snapshot from the server (ADR-024 l, precedence
  table). The server filesystem is the source of truth (ADR-024 c). The classifier itself exists
  (pure, unwired): [remote-jobs.md](remote-jobs.md).
- **Submit:** upload, then one atomic server call under a per-account `flock` (values checked,
  `KillUserProcesses` (its own outcome `refused-kup`), realpath, the wrapper named by its sha, no marker/row, the upload's names and sha256, the slot
  scan, the `.submitting` claim, the enqueue with `9>&-`), and a read-only label call for every remote
  `Queued` job (ADR-024 o item 3). The scripts and their parsers exist and are tested on this machine
  ([remote-jobs.md](remote-jobs.md)); the ssh calls are wired in 5.3 Part B.
- **Poll / fetch:** byte-offset `poll_log` and selective rsync down per `FetchPolicy`, with a server
  listing for the download post-condition. The scripts and the pure parts exist (`remote/poll.rs`,
  `remote/sync.rs`, [remote-jobs.md](remote-jobs.md)); the ssh calls are wired in 5.3 Part B.

## The shared artifact list (`src-tauri/src/artifacts.rs`, ADR-024 o item 6)

`ARTIFACT_PATTERNS` is the **one** list of a job's result artifacts, as leaf-name globs (`input.inp`,
`output.out`, `input.xyz`, `.exit_code`, `*.property.txt`, `*.hess`, `*_trj.xyz`, `*.NEB.log`,
`*.final.interp`, `*_converged.xyz`, `*.relaxscan*.dat`, `input.[0-9]*.xyz`, `*.finalensemble.xyz`). `is_artifact(name)` matches
a name against it with rsync's wildcard meaning (`glob_match`: `*` and `?` stop at `/`, `[a-z]`
classes). Two consumers derive from it and nothing else:
- the curated group export, `export_group::curated_match` ([group-export.md](group-export.md));
- the remote download filter, `remote::sync::download_filter_args(policy)`: one `--include=` per
  pattern, then `stderr.log` and the markers (`.exit_code`, `.started`, `.enqueued`, `.cancelled`,
  `.submitting`), `--include=.tsp-out/` **and** `--include=.tsp-out/**`, `--include=*.gbw` only when
  `FetchPolicy.include_gbw`, and `--exclude=*` last.

Gates, run against the **real local rsync** (3.2.7) dir to dir: a fixture with example names of every
pattern plus negatives (`input.gbw`, `input.tmp`, `input.densities`, `.tmp/x`, `sub/deep.xyz`, an rsync
temp name …) must come down exactly — nothing missing, nothing extra — under both policies; dropping
**any one** filter rule, or `*.gbw` without the opt-in, turns it red (permanent negative-control
tests). Parity: for every leaf fixture name, `curated_match` equals "came down", minus the download-only
extras; a rule added only to the curated side turns it red. `download_selects(path, policy)` — the
Rust mirror of the filter, for the local side of the download post-condition — is checked against the
same rsync runs. A reader inventory test asserts every file a reader opens comes down, with no
allowed gap (the GOAT `input.finalensemble.xyz` included). The download argv is `rsync -a --checksum
<filter> -e '…'`, so a retried fetch re-sends a locally corrupted file whose size and mtime match. The class syntax `[0-9]` in an
rsync filter is confirmed by these runs (not by the 5.3a probe, which used only `*`).

## Invariants (both backends)

- Completion = `.exit_code` present **and** `ORCA TERMINATED NORMALLY` in the output tail.
- Reconciliation on startup for any non-terminal job state.
- Concurrency 1 per backend by default.
