# ADR-024: Remote execution under intermittent connectivity

**Status:** Proposed · 2026-10-02

Refines [ADR-003](adr-003-execution-backend.md) (the `ExecutionBackend` trait + job state
machine) and extends [ADR-023](adr-023-server-agnostic-remote-execution.md) (one `SshBackend`
per `ServerProfile`). Rides on [ADR-005](adr-005-system-ssh.md) (system `ssh`/`rsync`). The
first profiled host is the university server — [infrastructure/uni-server.md](../infrastructure/uni-server.md).

## Context

The first remote target (the university server) is **not continuously reachable**:

- The university **cuts internet outside 08:00–22:00 Europe/Kyiv**. The laptop loses the link
  every night, but the server keeps computing.
- The server is on a **UPS good for only a few minutes**; whether it signals a clean shutdown is
  unknown. A server restart mid-study is a realistic event.
- A mechanism study is 300–800 jobs that run far longer than any one connected session.

So a job's lifetime routinely **exceeds** the laptop↔server connection's lifetime. The laptop
cannot be the thing that keeps a job alive or the authority on its state — both must survive the
laptop being closed, asleep, or off-network for hours. ADR-003 already anticipated this ("jobs
survive laptop sleep, app restarts, and SSH drops" via marker-file reconciliation); this ADR
makes the mechanism concrete for a link that is *expected* to be down, not merely *might* drop.

## Decision

**a) The queue lives on the server.** For the MVP, use **task-spooler** (`tsp`) on the server as
the job queue. The laptop enqueues a job and disconnects; the server runs it whether or not the
laptop is connected. The number of slots is a **per-profile setting** (see f), passed to `tsp`.

**b) Detached execution.** The per-job wrapper is launched **through `tsp`** and does **not**
depend on the ssh session that submitted it — closing the laptop does not stop the job. The
wrapper:
- writes `.exit_code` into the job directory on completion (**rule #6**: completion =
  `.exit_code` **and** `ORCA TERMINATED NORMALLY`);
- invokes ORCA by absolute path `/opt/orca/orca` (**rule #1**);
- runs in **one directory per job** (**rule #3**).

**c) The server filesystem is the source of truth.** The local SQLite is a **cache** that is
**reconciled** against the server's job directories after the link is restored. Where the two
disagree, the server's on-disk state wins. (Consistent with ADR-003's reconciliation-on-startup;
this ADR names the authority explicitly for the remote case.)

**d) Reconnect reconciliation protocol.** On reconnect, for every **non-terminal** remote job,
derive its state from the **contents of its job directory** (plus whether its `tsp`/process is
still live):
- **queued** — enqueued in `tsp`, not yet started (no output / no `.pid` live).
- **running** — process alive (`.pid` / `tsp` running), no `.exit_code` yet.
- **completed** — `.exit_code == 0` **and** `ORCA TERMINATED NORMALLY` (rule #6).
- **failed** — `.exit_code` present and non-zero, or normal-termination marker absent.
- **lost** — a **new terminal state**: the process is gone **and** no `.exit_code` was ever
  written (e.g. the server restarted mid-run, or the UPS gave out). Neither running nor cleanly
  finished — the run was interrupted and cannot be trusted.

**e) Restart of a `lost` job is a separate path.** Re-running a `lost` job is
**restart-from-last-geometry**, a distinct flow that seeds from the last geometry written to the
job directory. It does **NOT** go through `resolveCarryForwardGeometry`
(`src/scene/carryForward.ts`), which **deliberately refuses** on non-converged / scan / NEB
results — a `lost` job is interrupted and almost never converged, so carry-forward would
(correctly, for its own purpose) refuse. The exact seed-selection and safety rules for a `lost`
restart are an **open question** (see Open questions), not settled here.

**f) Concurrency is a backend/profile setting, not a global constant.** `LocalBackend` stays at
**1** (domain rule #4). The university-server profile is **also 1 slot** until parallel slots
(e.g. 2×12 cores) are **measured** (rule #10 — the box's parallel behaviour is unmeasured today,
see uni-server.md Open items). Each profile carries its own slot count.

**g) Profiles carry an optional availability window.** A `ServerProfile` may declare an expected
availability window (the 08:00–22:00 cutoff). Outside it, the UI shows **"unreachable — outside
expected window"**, an informational state — **not** an error. A failed connection *inside* the
window is still a real error.

## Alternatives rejected

- **Queue on the laptop.** The laptop is offline 22:00–08:00, so a laptop-side queue would idle
  the server all night — exactly the resource the design exists to keep busy. Rejected.
- **A custom server-side daemon.** Writing and operating our own long-lived server process is
  extra attack/ops surface for an MVP; `tsp` already gives detached queued execution. Rejected
  for now.
- **SLURM on a single node.** Heavyweight administration for one box; deferred to Phase 6 if a
  real multi-node allocation ever appears. Rejected for the MVP.
- **`tmux`/`screen` per job.** Gives detachment but **no queue** and no clean, machine-readable
  state to reconcile — you would be scraping terminal scrollback. Rejected.
- **`systemd-run --user`.** Needs lingering enabled for the user and has **no queue semantics**
  (slots, ordering). Rejected.

## Consequences

- **ExecutionBackend (ADR-003).** `SshBackend`'s `submit` enqueues via `tsp` rather than spawning
  directly; `status` is derived from job-directory contents + `tsp`/process liveness (the
  reconnect protocol in d). `poll_log`'s offset-pull (already in ADR-003) is what makes
  tail-after-reconnect work. **Amends ADR-003**'s reconciliation sketch by naming the server FS
  as authoritative and defining the five-way classification.
- **Job state machine.** Adds a new terminal state **`lost`**. Today `JobStatus`
  (`src-tauri/src/models/job.rs`) is `{ Draft, Queued, Running, Completed, Parsed, Failed,
  Cancelled }` — there is **no `Lost`**. **Amends ADR-003** (and the Phase 5 ROADMAP item that
  listed only `uploading → running → syncing`): `lost` is required to represent "interrupted, no
  `.exit_code`". (The `uploading`/`syncing` transient states from the ROADMAP are orthogonal and
  still apply to the rsync phases.)
- **Domain rule #4.** Rule #4 today reads as a **global** "default concurrency = 1". This ADR
  makes concurrency a **per-backend/profile** setting (Local = 1; each server profile carries its
  own slot count, defaulting to 1 until measured). **Amends rule #4** — proposed re-wording is in
  the session report, not applied to `CLAUDE.md` here.
- **ADR-023.** **Extends** it: the `ServerProfile` gains a **slot count** and an **optional
  availability window**. No contradiction with the server-agnostic model — these are more
  per-profile measured/configured data, exactly "specs as data, not code."
- **Rules #1, #3, #6 — reaffirmed, not changed.** The `tsp` wrapper preserves absolute-path
  invocation, one-dir-per-job, and `.exit_code` + normal-termination completion.

## Open questions

- **(e) `lost`-restart seed selection.** How restart-from-last-geometry picks and validates its
  seed (last `_trj.xyz` frame? last `.xyz`? a post-condition that the geometry is sane?), given
  it bypasses `resolveCarryForwardGeometry`. Unresolved.
- **Exact nightly cutoff window in UTC** — stated as 08:00–22:00 Europe/Kyiv, to confirm
  (uni-server.md).
- **UPS → clean shutdown** — does the UPS signal the server (`lsusb` / `nut`)? Affects how often
  `lost` actually occurs (uni-server.md).
- **Parallel slots on the university server** — whether 2×12-core slots are safe is unmeasured;
  the profile stays at 1 slot until a rule-#10 measurement (f, uni-server.md).
