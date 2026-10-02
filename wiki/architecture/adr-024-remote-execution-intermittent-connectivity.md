# ADR-024: Remote execution under intermittent connectivity

**Status:** Accepted · 2026-10-02 (Proposed → Accepted after the review + acceptance amendments below)

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

**Amended 2026-10-02 (review) — core pinning (rule #8).** The wrapper pins ORCA to the
profile's measured core mask and disables OpenMPI's own binding so the two don't fight:
`OMPI_MCA_hwloc_base_binding_policy=none taskset -c <mask> /opt/orca/orca …` (domain rule #8,
`../orca/performance.md`). For a **slot count > 1** the slot↔mask assignment must be
deterministic, so run **one `tsp` queue per slot** — each its own `TS_SOCKET`, each with **1
slot** and **its own mask** (the natural split is one slot per NUMA node, e.g. the uni server's
node0 `0–11,24–35` / node1 `12–23,36–47`, once measured — rule #10, still UNDETERMINED). **Not**
one `tsp` daemon with `-S N`: `tsp` does not pass the task its slot index, so a single N-slot
queue cannot map a job to a fixed mask, and two jobs could land on the same cores while another
node sits idle. One-queue-per-slot makes the mapping explicit and keeps `%pal`/mask aligned
(`align_pal_nprocs`, gotchas).

**Amended 2026-10-02 (review) — isolation & queue survival** (Decision a/b). The account `yats`
is **shared** (uni-server.md), so nothing may rely on the user's default `tsp` state:
- Each queue uses a **dedicated `TS_SOCKET`** under OrcaStudio's own server directory (not the
  shared default socket), so OrcaStudio's queue is isolated from anything else `yats` runs.
- The wrapper's and ORCA's **stdout/stderr are redirected into the job directory**, not left to
  `tsp`'s default sink in `/tmp` (which the OS clears on reboot — the logs would vanish exactly
  when a restart makes them most needed).
- The **`tsp` queue lives in the daemon's memory and is lost on a server restart**: a job that
  was *enqueued but not yet started* simply disappears from `tsp` after a reboot. This is not a
  loss of results (nothing ran) but it **is** a reconciliation case — see Decision d's
  **never-started** sub-case.

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

**Amended 2026-10-02 (review) — boot-id-anchored liveness.** A bare "PID alive?" check is unsafe
across a server restart: the kernel reuses PIDs, so after a reboot some unrelated process may
hold the old PID and a dead job would read as `running`. So the wrapper, **at start**, writes a
`.started` marker into the job directory: its **PID**, the host **boot id**
(`/proc/sys/kernel/random/boot_id` — a fresh UUID every boot), and the start time. The classifier
then uses:
- **running** = `.started` present **and** its `boot_id` equals the host's *current* `boot_id`
  **and** that PID is alive **and** `/proc/<pid>/cmdline` is **our** wrapper/ORCA (all three —
  boot-id guards against a stale marker surviving a reboot; the cmdline check guards against PID
  reuse *within* the same boot).
- **lost** = `.started` present, its `boot_id` ≠ current `boot_id`, and no `.exit_code` (something
  ran, then the machine rebooted under it).
- **never-started** (**new sub-case**) = the job directory exists but there is **no `.started`**
  and `tsp` does not know the job. Nothing ever executed (the enqueued-but-unstarted job the
  restart dropped from `tsp`'s in-memory queue, per Decision b).

**`never-started` is NOT folded into `lost`, and is NOT a new terminal state.** Argument: `lost`
is *terminal* and means "a run began and was interrupted" → its recovery is
**restart-from-last-geometry** (Decision e), which must select and validate a seed from partial
output. `never-started` means **nothing computed** → there is no seed, no partial data, no
trust question; the safe and correct action is to simply **re-enqueue the original input** and
return the job to **`queued`**. Conflating the two would force a clean re-submission through the
seed-selection path (an open question, e) for no reason, and would mislabel a job as "interrupted
mid-run" when it never ran. So `never-started` is a **non-terminal reconciliation outcome** that
transitions the job back to `queued`; only a genuinely-interrupted run becomes `lost`.

**Amended 2026-10-02 (acceptance) — bounded re-enqueue.** Two tightenings make the
`never-started` path safe to automate:
- The wrapper writes **`.started` as its very first action**, before any other logic, so the
  window in which a started job looks like `never-started` is as small as possible.
- Automatic re-enqueue of a `never-started` job is capped at **once per job** (a counter in the
  local DB); a **second** `never-started` on the same job becomes **`failed`** with reason
  *"wrapper never started"* and requires a user decision. Rationale: `never-started` also covers
  a **wrapper that crashed before writing `.started`** (not only a restart-dropped queue entry) —
  without a bound, such a job would re-enqueue, crash, and re-enqueue forever. One retry absorbs
  the benign restart-drop case; a repeat is a real fault the user must see.

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

**h) Resource invariants + preflight (Amended 2026-10-02, review).** Before a job is enqueued to
a profile, a **preflight** checks, in the profile's measured terms (rule #10):
- **`nprocs ≤ physical cores of the slot`** — oversubscribing a slot's mask runs *slower*, not
  faster (gotchas / `align_pal_nprocs`).
- **`nprocs × %maxcore ≤ the profile's RAM budget`** — `%maxcore` is per-process, so the whole
  job's peak is `nprocs × %maxcore`; swapping is a failure mode (uni-server.md: swap-is-failure).
- An input **without `%maxcore`** (a hand-written Monaco input — the builders always emit it)
  inherits ORCA's **default 4000 MB/proc since 6.1.0** (`../orca/gotchas.md`); on a 24-proc,
  62 GiB host that is ~96 GB → over RAM. Preflight raises a **warning that proposes a concrete
  value for the user to confirm**; **silent insertion is forbidden** (honest-or-absent — we do
  not mutate the user's input behind their back).
- Preflight also checks **free space in the profile's working directory** (ORCA litters scratch;
  a full disk fails mid-run). The remote disk is a single aging HDD with no redundancy
  (uni-server.md), so this is not hypothetical.

**i) Cancel (Amended 2026-10-02, review).** A remote cancel maps to the **existing `Cancelled`**
state (no new state):
- **queued** → delete the job from its `tsp` queue; nothing ran, mark `cancelled`.
- **running** → kill the **entire process group** (the wrapper, `mpirun`, and every MPI rank it
  forked), then **no `.exit_code` is written** → the reconnect classifier must not read a killed
  job as `lost`; the local record marks it `cancelled` directly.
- The **process-group kill mechanism is a probe, not a fact.** Locally this is already hard —
  `mpirun` `setpgid`s each rank into its own group, so a single `killpg` misses the ranks and a
  cwd-sweep is needed (`../debugging/004-mpi-ranks-escape-process-group.md`, gotchas). The
  equivalent over SSH (e.g. `pkill -g` / `fuser -k <job_dir>` / a recorded rank list) is
  **UNDETERMINED until measured on the server** (Open questions).

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
- **`fetch_results` to the laptop is backup by design, not just convenience** (Amended
  2026-10-02, review). The server's results live on a single aging HDD with no redundancy
  (`sdb` — uni-server.md); it is **not** a backup. So pulling results down after each job
  (ADR-003 `fetch_results` / rsync-down) is the only durable copy, and the reconnect
  reconciliation (Decision c/d) is also what drives that pull. This makes the laptop cache's
  job both "source of UI truth after a disconnect" and "the backup of record."

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
- **(a) Does the `tsp` daemon survive the ssh session that launched it exiting?** (Amended
  2026-10-02, review.) The detach premise (Decision b) requires yes, but it is only **measured
  for `tmux`** so far (uni-server.md, 2026-10-02); `tsp`'s own daemon lifetime vs the launching
  session is a **probe**, not a fact.
- **(b) `tsp` behaviour across a server restart** (Amended 2026-10-02, review). We assert the
  in-memory queue is lost (Decision b) and reconcile via `never-started` (Decision d), but the
  exact post-reboot `tsp` state — does the daemon auto-restart, does `tsp -C`/socket survive,
  are partially-written job dirs left clean — is **unmeasured** (probe).
- **(c) Process-group kill for an MPI job over SSH** (Amended 2026-10-02, review). The remote
  mechanism that reliably kills the wrapper + `mpirun` + all ranks (Decision i) is UNDETERMINED;
  the local analogue needed a cwd-sweep (`debugging/004`). Probe before relying on remote cancel.
