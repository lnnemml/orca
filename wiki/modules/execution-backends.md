# Module: Execution backends

**Status:** `LocalBackend` runs ORCA end-to-end — isolated job dir, CPU core pinning, a sequential
in-SQLite queue, cancellation with an MPI-rank sweep, and startup reconciliation. The
`ExecutionBackend` trait (ADR-003) exists (`src-tauri/src/execution_backend.rs`); `LocalBackend`
implements it, and **the Tauri command layer dispatches through the trait** — `submit_job` /
`cancel_job` construct a `LocalBackend` and call `submit` / `cancel` on it. The running machinery
still lives in `src-tauri/src/local_backend.rs` (queue, process tree, cancellation) — each trait
method **delegates** there. The remote backend's core — submit, retry, label and withdraw of a
remote job — is `src-tauri/src/ssh_backend.rs` (unit 5.3 B1 Part A, below); the commands do not
dispatch to it yet (B1 Part B). See ADR-024 (accepted): the remote queue lives on the server
(`tsp`), the server FS is the source of truth, and concurrency becomes a per-backend/profile setting
rather than the global rule-#4 constant.

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

**Dispatch.** `enum Backend { Local(LocalBackend), Ssh(SshBackend) }` is the static-dispatch selector
(ADR-023: an `enum`, not `dyn`); it implements the trait by delegating to its variant.
- An existing job's backend is `backend_kind(&job)`: **keyed on the job's coordinates, never on
  `backend_id`** (ADR-024 o item 1) — no coordinates → `Local`; all three → `Ssh(RemoteCoordinates)`;
  a partial set is an error, never "local". `backend_id` only names the profile; a finished remote
  job whose profile was deleted keeps its coordinates and stays remote. `Backend::for_job` builds the
  enum from it.
- A draft's backend is the run target chosen next to Submit (o item 5): `Backend::for_submit(app,
  None | Some(profile id))`.
- `dispatch_keys_on_the_coordinates_never_on_backend_id` pins the rule (negative control: key on
  `backend_id` and it goes red).

The commands still construct a `LocalBackend` directly; routing them through `Backend` is unit 5.3
B1 Part B. See `wiki/log.md` (unit 5.0 Part A / Part B, 5.3 B1 Part A).

## Where the code lives

Implemented across three modules, **not** a `backends/` trait dir: the trait, `LocalBackend`,
`SshBackend`, `enum Backend` and `backend_kind` in `src-tauri/src/execution_backend.rs`; the local
running machinery in `src-tauri/src/local_backend.rs`; the remote backend's Tauri-free core in
`src-tauri/src/ssh_backend.rs` (over the scripts and parsers of `src-tauri/src/remote/`).

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
  the `cancelled` flag, then spawn `terminate_job`; other status → `Backend(...)`. A **remote** job
  that is queued or running is refused first (`ssh_backend::refuse_if_remote_live`, "remote cancel
  arrives in unit 5.4"): it is queued on its server, and a local `cancelled` would be a state the
  server contradicts (ADR-024 i, o item 2). `drive_job`
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

## SshBackend (`ssh_backend.rs`)

The design is ADR-024 (Decisions a–o). The server-side scripts, the pure classifier and the wire
parsers are in `src-tauri/src/remote/` ([remote-jobs.md](remote-jobs.md)). The backend's **core** is
`src-tauri/src/ssh_backend.rs`: Tauri-free functions over the database (`&DbState`, locked only
around their own reads and writes — never across an ssh call, which a test checks with `try_lock`)
and a `CommandRunner` (the real `SystemRunner`, ADR-005: system `ssh`/`rsync`). `SshBackend` in
`execution_backend.rs` is the thin `AppHandle` wrapper over it, like `LocalBackend` over
`local_backend`; no command calls either yet (unit 5.3 B1 Part B). Unit order is ROADMAP Phase 5.

**Coordinates.** A job is remote iff `remote_host`, `remote_job_dir`, `remote_socket` are set
(`coordinates(&job)`; schema v20). They are written once at submit and every later call uses them,
never the profile's current values (n 6b); `recorded_root` reads the root back from the job dir
`<root>/jobs/<id>`.

**Submit** — `submit_remote(db, runner, data_dir, job_id, profile_id) -> Result<SubmitAttempt>`
(`SubmitAttempt { outcome: SubmitOutcome, pal: PalAlignment }`):

| step | what | the row after a failure here |
|---|---|---|
| 1 | no ssh: a draft without coordinates; the profile a run target (`is_run_target`, one slot, a valid host); the local job dir `<data_dir>/jobs/<id>` prepared by `local_backend::prepare_job_dir` with the input from `jobs.input_content`, its `%pal` **aligned downward** to the mask (`align_remote_pal`, below), and the aux files (`read_aux_files`, shared with the local run), then listed by `upload_expected` (an unreadable `%pal`, over 1000 files or a name outside the path rule refuses) | the row unchanged (a draft, no coordinates) — `Err`; the local dir may be written and is rewritten on the next attempt (o 14.4) |
| 2 | **persist** (o 3.1), in the same lock as step 1: one transaction sets `queued`, `backend_id`, the coordinates (`<root>/jobs/<id>`, `<root>/tsp/slot0.sock`) and the local `job_dir`, guarded on `status = 'draft' AND remote_host IS NULL` | rolled back: a draft |
| 3 | **prepare** call (read-only, o 3.2/13.1/14.1): every existing component of the root a directory at its own realpath, `<root>/bin` and `<root>/tsp` included; does each uploaded script (wrapper, cancel, collect) hash right | `queued` + coordinates; label "not on the server" |
| 4 | when `bin/`, `tsp/` or any script is missing: **install** call, then **prepare again** — every script must now hash right (post-condition) | same |
| 5 | **upload**: `rsync -a --checksum --mkpath`, never `--delete`, timeout 60 s + 1 s per 256 KiB (a floor, not measured) | same |
| 6 | **the submit call** (o 3.3, 60 s): `Enqueued`, `Refused`, `RefusedKup`, `FailedAfterClaim`, or no readable reply | `queued` + coordinates; the label call decides |

Steps 3–6 never change the status: whatever happens, the row stays `queued` with its coordinates
and the label call (o 3.4) resolves it from the server (Decision c). Their one write
(`record_attempt`): `error_message` = the attempt's failure for people (`NULL` after `Enqueued`),
and — **only** for `SubmitReply::RefusedKup` — the profile's stamp and the facts it certified are
cleared (`clear_verified`; n item 7, o 13.3). A refusal is never matched by its text. The outcome:

| `SubmitOutcome` | from | offered next |
|---|---|---|
| `Enqueued { tsp_id }` | `enqueued <id>` | (the poller, B2) |
| `NotClaimed { step, reason }` | a failure in steps 3–5, or `refused` | retry or withdraw ("not on the server") |
| `KillUserProcesses { evidence }` | `refused-kup` | the profile is no run target until a connection test passes; withdraw |
| `FailedAfterClaim { reason }` | `failed-after-claim` | withdraw only ("submit interrupted") |
| `Unknown { reason }` | a timeout, ssh exit 255, an unreadable reply or an echo that differs | the label call decides |

Every stdin-fed call goes through one helper: `ssh -o BatchMode=yes -o ConnectTimeout=10 -- <host>
bash -s` with the script and its NUL list (n item 11); the reply is parsed first, ssh exit 255 is
ssh's own failure, and a complete reply with any exit status but 0 is not trusted.

**`%pal`, aligned downward only** (o item 14.2, domain rule #8) — `align_remote_pal(input, mask)`, on
every attempt, from `jobs.input_content` (the database keeps the user's original) with the
**attempt's** mask: `nprocs = min(the input's %pal nprocs, the distinct CPUs of the mask)`
(`models::server_profile::distinct_cpus`: `0-3,2-5` is 6); no `%pal` → the distinct-CPU count, as a
local run. A small `%pal` is never raised (a per-rank `%maxcore` multiplies with it). The input's
`nprocs` is read by `local_backend::read_pal_nprocs` (the first `%pal`, single-line or block form; a
`%pal` without a readable positive `nprocs` refuses, never guessed); the text is rewritten by the local
run's own `align_pal_nprocs` (local behaviour unchanged). Post-condition: exactly one `%pal` directive,
stating that `nprocs`. The aligned input is written into the local job dir before `upload_expected`,
so the local dir = the uploaded bytes = the hashed list. `PalAlignment { input_nprocs, nprocs,
mask_cpus, rewritten }` comes back with every attempt; `notice()` is the visible line `[OrcaStudio]
%pal nprocs aligned to N (the server profile's core mask has K CPUs; the input had …)`, `None` when
nothing changed — **Part B shows it with the submit/retry result** (it is not an error, so never
`error_message`). Open (not measured; the local path shares it): which of a `%pal` block and a
`PALn` keyword ORCA honours when both are present.

**Label** — `label_remote(runner, &job)`: the read-only label call by the recorded coordinates →
`LabelReport { facts, label }`. Writes nothing.

**Retry** — `resubmit_remote(db, runner, data_dir, job_id) -> SubmitAttempt`: for a remote `queued`
job, only when the label call says **"not on the server"** ("submit interrupted" is never retried:
the interrupted call may still be running on the server, o item 8). The profile (by `backend_id`)
supplies the mask and the ORCA path and must still be a run target with the recorded host and root;
only then (the label call comes first, so a refused retry writes nothing) is the local dir (the
recorded `job_dir`) written again from `input_content` with the **current** mask — a retry after a
mask change uploads the new `nprocs` — and steps 3–6 run.

**Withdraw** — `withdraw_remote(db, runner, job_id)` (o items 2, 14.1), with no `verified_at` gate
(n 6a): only for a remote `queued` job the label call finds "not on the server" or "submit
interrupted" (otherwise it is in the server's hands: remote cancel, unit 5.4). Then: prepare →
install if any script is missing → prepare again → **mkjob** (`mkdir -p <job dir>` and the o-1 shapes
re-asserted after it, one call) → `cancel.sh cancel <job> <root>` **through the trampoline** (rc 0
required; it publishes `.cancelled` first) → `collect.sh <job> <recorded socket>` through the
trampoline, its snapshot unwrapped from the `stdout` record → `classify` (with the one retake it may
ask for; `reenqueue_count` 0) decides: `Cancelled` → the row becomes `cancelled`; anything else
(`Completed { late_cancel }`, `Cancelling`, `Failed`, …) leaves it `queued` for the poller or 5.4,
with the verdict in `error_message`. Never a hard-coded `Cancelled` (Decision c). A failure at any
step leaves the row as it was.

**The trampoline** (o item 14.1; format and controls in [remote-jobs.md](remote-jobs.md)): every call
of an uploaded `cancel.sh`/`collect.sh` is one `bash -s` call whose stdin is the trampoline and the
NUL list `<root> <name> <sha> <args…>`, so no per-job value passes through the remote login shell. It
runs only a name of its closed allow-list (`cancel`, `collect`), by its sha, as a regular file at its
own realpath, under a per-script time budget, with stdin at EOF.

**Refusals** (o item 2, n 6b): `refuse_if_remote_live(&job)` refuses a local cancel or delete of a
remote job that is `queued`/`running` ("remote cancel arrives in unit 5.4") — `delete_job_conn` and
`local_backend::cancel` call it — the latter **at function entry, before any branch** (o 14.5), so
no caller reaches a local kill or a local `Cancelled` for a remote job; a profile with such jobs keeps its host and root and cannot be
deleted ([server-profiles.md](server-profiles.md)).

**The `SshBackend` trait methods**: `submit` runs `submit_remote` with the real runner and blocks
for the whole sequence (call it off the main thread); only `Enqueued` is `Ok`, any other outcome an
`Err` carrying its failure (the row is `queued` either way). The trait has no room for the `%pal`
notice, so a command that must show it calls `submit_remote` directly. `status` reads the row; `cancel` refuses
(above); `poll_log` and `fetch_results` refuse until the poller (B2).

**Tests.** Over a fake runner (`ssh_backend/tests.rs`): the call order (prepare → upload → submit;
prepare → install → prepare → … on a fresh server), the row at every call (persisted before the
first), every failure point after the persist (each leaves `queued` + coordinates + the failure,
nothing after it runs, the stamp kept), only `RefusedKup` clearing the stamp, refusals before any
ssh writing nothing, retry and withdraw gating, the withdraw's sequence (prepare → install → prepare →
mkjob → cancel → collect, no stamp gate), each withdraw failure leaving the row as it was, the
withdraw's classifier mapping (with a retake), and `%pal` (48 on 4 CPUs → 4, 2 on 12 → 2, `0-3,2-5` →
6, the block form, an unreadable or doubled `%pal` refused before anything is written, a retry
aligning to the current mask).
End to end against the real scripts: [remote-jobs.md](remote-jobs.md) (`backend_e2e_tests.rs`).
Negative controls: [log.md](../log.md) (5.3 B1 Part A, 2026-10-05).

**How the job runs there:** a static wrapper (`include_str!`, uploaded content-addressed as
`<root>/bin/wrapper-<sha>.sh` by the install call, with `cancel.sh` and `collect.sh` beside it) runs through a per-slot `tsp` queue. It
publishes `.started` with a no-clobber `ln -T` and `.exit_code` by temp file + `rename`; the cancel
script is `.cancelled` first, a verified `tsp -r`, TERM to the verified wrapper's group and a
cwd-filtered SID sweep (ADR-024 i, l); status is the pure classifier over a raw-fact snapshot. Per-job
arguments cross ssh only as a NUL list on stdin, never as ssh argv (measured injection-capable, ADR-024
l / P1).

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
