# ADR-024: Remote execution under intermittent connectivity

**Status:** Accepted · 2026-10-02 (Proposed → Accepted after the review + acceptance amendments below) · amended 2026-10-03 (probe; probe review; d′ resolution — ready for implementation; l — unit 5.2 script and classifier shape; m — 5.3 submit rules; n — 5.1 Part B profile and connection test) · amended 2026-10-05 (o — 5.3 transport, sync and monitoring; accepted after DESIGN round 4; o13 — Part A2 amendments, accepted after DESIGN review 2026-10-05; o14 — B1 amendments, accepted after DESIGN review 2026-10-05) · amended 2026-10-06 (o15 — B1 Part B: the in-flight guard's scope, accepted after DESIGN review 2026-10-06; o16 — B2: the remote live log is pushed, accepted after DESIGN review 2026-10-06; o17 — B2 Part A: classifier `Cancelled` terminal, drain while watched, accepted after DESIGN review 2026-10-06; o18 — B2 Part B: exit leaves in-flight ssh/rsync to the OS, accepted after DESIGN review 2026-10-06; o19 — B3: the poller's remote state reaches the view, accepted after DESIGN review 2026-10-06) · amended 2026-10-07 (o20 — B3 Part B: run target remembered app-wide, run target only via Create & Run, Withdraw confirms; accepted after DESIGN review 2026-10-07) · amended 2026-10-17 (o21 — Cancel refuses a live remote job before the guard; accepted after DESIGN review 2026-10-17; o22 — unit 5.4a: remote cancel, the pending cancel and the guard; accepted after DESIGN review 2026-10-17, round 2 PASS WITH FINDINGS applied; o22.11–12 — A2a clarifications, reviewed with A2a's CODE verification 2026-10-17) · amended 2026-10-18 (o22.13 — A2b clarifications; accepted after DESIGN review 2026-10-18, round 2 PASS WITH FINDINGS applied; o22.14 — Part B: commands and view; accepted after DESIGN review 2026-10-18, round 2 PASS WITH FINDINGS applied) · amended 2026-10-19 (o23 — unit 5.4c: the pending submit; accepted after DESIGN review 2026-10-19, round 2 PASS WITH FINDINGS applied as o23.13)

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

**Amended 2026-10-03 (probe) — measured mechanism** ([task-spooler-uni-probe.md](task-spooler-uni-probe.md)).
On the uni host (task-spooler 1.0.1):
- **tsp makes every task a session leader** (wrapper `PID == PGID == SID`); a separate `setsid` in
  the wrapper is unnecessary. MPI ranks get their own PGID but **keep the wrapper's SID** and the job
  dir as cwd.
- **Survival of logout is a logind property, not a tsp one.** Daemon, runner and job stay in the ssh
  session's scope (`session-N.scope`), which logind leaves `active (abandoned)` because
  `KillUserProcesses=false` (`Linger=no`). The `SshBackend` connection test should check this setting —
  if it were `yes`, every job would die at logout.
- **Per-slot masks hold.** Two queues (`0-11`, `12-23`) ran concurrently; every thread of `orca`,
  `mpirun` and the ranks had `Cpus_allowed_list` exactly the slot mask, and OpenMPI with
  `binding_policy=none` did not rebind. Only the wrapper's own idle `bash` (taskset's parent) is
  unpinned — pin it too (`taskset -c <mask> wrapper`) if a clean invariant is wanted.
- **`TMPDIR` must be set by the wrapper** (e.g. to the job dir): the task inherits tsp's `TMPDIR`,
  and OpenMPI leaves `pmix-gds-shmem.*` / `ompi.*` litter there after every killed run.

**Amended 2026-10-03 (probe review) — wrapper-owned `TMPDIR`.** The wrapper exports its **own
`TMPDIR` inside the job directory** (e.g. `<job_dir>/.tmp/`) before launching ORCA, and the cancel
path (Decision i) **removes it** after the kill. Rationale (probe): a tsp task inherits the daemon's
`TMPDIR`, and every killed MPI run left `pmix-gds-shmem.*` files and `ompi.*` session-dir entries in
it. A per-job `TMPDIR` keeps that litter inside the one directory that belongs to the job (rule #3).
A shared directory would collect it without limit.

**Amended 2026-10-03 (d′ resolution) — `HWLOC_COMPONENTS=-gl`.** The wrapper also exports
**`HWLOC_COMPONENTS=-gl`** for ORCA. Measured on the uni host (`../infrastructure/uni-server.md`):
without it, hwloc running under a non-GUI user writes **~310 lines** of `Authorization required, but
no authorization protocol specified` to `stderr.log` per water run. With it, stderr is **empty** and
the energy is **bit-identical** (−76.418938720745 Eh). The variable only switches off hwloc's GL
plugin; it does not touch the numerics.

**Amended 2026-10-03 (d′ resolution) — wrapper start sequence.** The wrapper's order is now:
1. write `.started` — still the first action (atomically, via a temp file + `rename`); *(superseded by Decision l: step 0 is now `cd "$job_dir"`, `.started` is published by a no-clobber `ln -T` (refused if present), and a self-check follows step 1)*
2. **check for `.cancelled`** — if present, **exit at once without launching ORCA**. It writes no
   `.exit_code`; `.cancelled` alone decides the classification (Decision d);
3. export `TMPDIR` and `HWLOC_COMPONENTS`, then run the pinned ORCA (rules #1, #8);
4. write `.exit_code` last.

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
  **and** that PID is alive *("alive" redefined by Decision l: state ≠ `Z` and the same `starttime` as `.started`)* **and** `/proc/<pid>/cmdline` is **our** wrapper/ORCA (all three —
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

**Amended 2026-10-03 (probe review) — `.cancelled` is checked first.** The classifier checks for a
**`.cancelled`** marker in the job directory **before every other rule**, and in particular before
`never-started`. If it is present, the job is **`cancelled`** and is never re-enqueued. Rationale: the
probe (C, queued cancel) showed that a job removed from the queue with `tsp -r` leaves a directory
holding only `input.inp`. That is byte-for-byte the `never-started` shape, so without a server-side
marker a cancelled queued job cannot be told apart from a restart-dropped one. A purely local
"cancelled" flag is not a fix, because it would contradict Decision c (the server filesystem is the
source of truth).

**Amended 2026-10-03 (d′ resolution) — classifying a cancelled job.** `.cancelled` is still checked
first. If it is present:
- **process still alive** — `.started` exists, its `boot_id` is current, its PID is alive and
  `/proc/<pid>/cmdline` is ours (the same three-part test as `running` *("alive" redefined by Decision l: state ≠ `Z` and the same `starttime` as `.started`)*) → transient
  **`cancelling`**. The reconcile **repeats the session sweep** (Decision i) and checks again next
  time.
- **process dead, or no `.started` at all** → **`cancelled`**, **whatever the `boot_id`**. A
  cancelled job is never `lost` and never `never-started`, so it is never re-enqueued or restarted.

This closes the gap where a running job killed by cancel (current `boot_id`, dead PID, no
`.exit_code`) matched none of the rules above. `cancelling` is **non-terminal**: a job sits there only
while a cancelled process tree is still alive. That is a new transient status next to
`Cancelled` in `JobStatus` (`src-tauri/src/models/job.rs`); the implementation unit adds it.

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

**Amended 2026-10-03 (probe) — measured cancel mechanism** ([task-spooler-uni-probe.md](task-spooler-uni-probe.md)): *(Superseded for the running case by Decision l: no `tsp -k`; TERM to the verified wrapper's group + a cwd-filtered sweep.)*
- **queued** → `tsp -r <id>` removes it (its runner process exits too); the job dir keeps only
  `input.inp`. That is the same on-disk shape as `never-started` (d), so the local record must be set
  to `cancelled` **before** the next reconcile, or the reconciler would re-enqueue the job.
- **running** → `tsp -k <id>` (SIGTERM to the wrapper's group), then a **session sweep**: take `sid` +
  `boot_id` from `.started`; if the `boot_id` is current and the `sid` is not the sweeping shell's own,
  TERM+CONT every PID in `ps -s <sid>`, wait, then KILL whatever remains. `tsp -k` alone left **0
  survivors** in every measured case; the sweep is belt-and-braces and does not depend on OpenMPI.
- **Discrepancy with the assumption above:** on the uni host the ranks **do not** survive a group
  kill. They escape the PGID as expected, but the kernel SIGKILLs them within <50 ms of `mpirun`
  dying — even when the ranks were SIGSTOPped (parent-death-signal behaviour; that OpenMPI sets it is
  an inference). So the `debugging/004` orphan scenario did not reproduce here. Why it differs from
  the laptop is **not explained** (not re-measured). The session sweep replaces the cwd sweep as the
  remote safety net; both cover the ranks.
- A killed job has `.started`, **no `.exit_code`**, and `tsp -l` shows it as `finished` with E-Level
  −1 (no distinction from a crash). If only `mpirun` dies, ORCA reports `error termination` but **exits
  0** → `.exit_code = 0` without `TERMINATED NORMALLY`. Rule #6's two-part completion check catches
  this; `.exit_code` alone must never mean success.

**Amended 2026-10-03 (probe review) — queued cancel writes `.cancelled`.** To cancel a **queued**
job, the app first writes **`.cancelled`** into the job's server directory, **then** runs `tsp -r <id>`.
Decision d checks `.cancelled` first, so the job reconciles to `cancelled` from the server's own
state. This replaces the 2026-10-03 (probe) wording above that the local record must be set "before
the next reconcile".

*(Decision l replaces `tsp -k` with TERM to the verified wrapper's group; the sweep itself stays, now cwd-filtered.)* **Amended 2026-10-03 (probe review) — the SID sweep stays.** The session-id sweep after `tsp -k`
remains part of the running-cancel path, **even though** on the uni host the ranks die by themselves
when `mpirun` dies. Rationale: on the laptop (`../debugging/004-mpi-ranks-escape-process-group.md`)
the same group kill **did** leave orphaned ranks, and the cause of the difference is not established.
A cancel that silently depends on an unexplained host behaviour would regress on the next host. The
sweep is cheap (one `ps -s <sid>`), guarded by `boot_id`, and harmless when there is nothing to kill.

**Amended 2026-10-03 (d′ resolution) — one cancel script for every state.** A cancel is **one remote
script**, run on the server in the job directory *(superseded by Decision l: it never `cd`s into the job dir and uses absolute paths only)*, the same for queued and running jobs. It supersedes
the queued-only `.cancelled` amendment above.
1. **write `.cancelled`** (atomically: temp file + `rename`);
2. **`tsp -r <id>`**, **ignoring its error** — the job may already be running, finished or gone; *(steps 2–3 superseded by Decision l: `tsp -r` only for a verified `queued` row on a live daemon; no `tsp -k`)*
3. **if `.started` exists and the process is alive** (current `boot_id` + live PID + our cmdline *("alive" redefined by Decision l: state ≠ `Z` and the same `starttime` as `.started`)*, as
   in Decision d) → **`tsp -k <id>`** + the **SID sweep** (TERM+CONT → wait → KILL);
4. remove the job's own `TMPDIR` (`<job_dir>/.tmp/`, Decision b).

**Why there is no race.** The two sides use mirror-image orders on the **same local filesystem**:
- the wrapper **writes `.started`, then checks `.cancelled`**;
- the cancel script **writes `.cancelled`, then checks `.started`**.

Whichever side's write lands second, that side's own check comes after *both* writes, so it sees the
other side's marker. There are only two outcomes:
- **the wrapper sees `.cancelled`** → it exits before launching ORCA (if the script also saw
  `.started`, step 3 finds a dying or dead process — harmless);
- **the cancel script sees `.started`** → it kills the process tree in step 3.

Either way, ORCA is never left running under a `.cancelled` marker. Both markers are created by
`rename` in the same directory on one local filesystem, so visibility is immediate. *(`.started` is now published by a no-clobber hard link, `ln -T` = one `linkat`, per Decision l Part B item 5; still atomic in the same directory, so the argument holds)* The argument would
**not** hold over NFS-style caching; on the uni host the job root `/home/<user>/.orcastudio/` is on local `ext4` (`/dev/sdb4`, measured with `findmnt` on 2026-10-03; uni-server.md).

**Amended 2026-10-03 (d′ resolution) — cancel outside the connection window.** If the user cancels
while the server is unreachable (e.g. outside the 08:00–22:00 window, Decision g), the request is
**stored locally as a pending cancel** and **executed first on reconnect**, before reconciliation.
*(Mechanics: o22 — two DB columns, delivery before the normal status step, Undo until the send is marked.)*
Until then the job's status **does not change** — it shows its last known server state with a
"cancel pending" indication. Rationale: Decision c — the server's state is the truth, and a job
marked `cancelled` locally while it is in fact still running would be a lie that reconciliation then
has to undo.

**j) Single-host time (Amended 2026-10-03, probe review).** The app **never compares a laptop
timestamp with a server timestamp**. Durations and event order are always computed from times taken
on **one** host — server times (`.started`'s `started_at`, file mtimes) are compared only with other
server times, and laptop times only with laptop times. Rationale (measured): the uni server's clock is
**not NTP-synchronised** and was **~2 min 52 s ahead** of the laptop on 2026-10-03. Any cross-host
difference ("stale for N minutes", "started before submit") would be skewed by an unknown, drifting
offset. The boot-id liveness rule (d) already uses no time at all.

**k) Dedicated server account (Amended 2026-10-03, probe review).** A server profile runs under a
**dedicated user without sudo**. OrcaStudio's server root is **`/home/<user>/.orcastudio/`** *(narrowed by Decision n: the root is the profile's `remote_scratch_dir`; this path is the suggested default)*: the tsp
sockets (`TS_SOCKET`, one per slot) and the job directories live under it. Administration (packages,
`/opt/orca`, logind/NTP settings) is done separately, through an admin account the app never uses.
Rationale: the boundary between our jobs and other people's data is then held by **OS permissions,
not by prompt discipline** (e.g. "never `pkill` by name", "never touch the legacy installs"). This is
what makes automatic agent mode acceptable on a shared host: a mistake can at worst damage the
dedicated user's own tree. Measured for the uni host (user `anton`, 2026-10-03): no `sudo` group;
writing or deleting in the shared account's home fails with `Permission denied`; `/opt/orca` is
readable and executable but not writable; the water benchmark is bit-identical
(`../infrastructure/uni-server.md`). The 2026-10-02 "isolation & queue survival" amendment (b) was
written for the shared `yats` account. Its rules (dedicated `TS_SOCKET`, job-dir stdout/stderr) still
apply, now under the dedicated user's root.

**l) Shape of the server-side scripts and the classifier (Amended 2026-10-03, unit 5.2
decomposition; Anton decided every fork, before and after DESIGN review rounds 1 and 2).**

*Scripts.*
- **Static scripts with arguments.** The wrapper and the cancel script are static `.sh` files kept in
  `src-tauri` and embedded with `include_str!`. Every per-job value (job dir, ORCA path, core mask)
  is a **positional argument**, never substituted into the script text. *Rejected:* rendering a
  script per job with `format!` — every quoting rule becomes a place for a bug, and the tests would
  have to check generated text.
- **Two re-parse points remain, outside the scripts** (review H3). `ssh host cmd args` joins the
  arguments into one string that the **remote login shell parses again** (ADR-005), and `tsp` may or
  may not run its command through a shell. So the claim is only "the scripts never parse a value as
  shell". Measured 2026-10-03 (probe P1, [task-spooler-uni-probe.md](task-spooler-uni-probe.md)):
  - **`tsp` passes argv verbatim** (no `sh -c`). Eight hostile arguments (a space, `$HOME`, `'`,
    `;`, `*`, empty, `x;touch …`, a backtick) arrived byte-for-byte, and nothing was executed.
  - **Plain ssh argv is injection-capable.** `ssh uni bash $W <args>` word-split them, expanded
    `$HOME`, ended the command at `;` and ran `*` as a command.
  - **Transport rule:** per-job arguments cross ssh **only as a NUL-separated list on stdin**, read
    by an uploaded script (`while IFS= read -r -d '' a`) *(for cancel/collect: by the stdin trampoline
    that runs the uploaded script, o14.1)*. That form preserved every argument,
    including an embedded newline and an empty one, and it does not depend on the remote login shell.
    `printf '%q'` into one command string also worked, but needs bash as the remote login shell, so it
    is not adopted.
- **Upload: content-addressed, never overwritten** (review M1). 5.3 uploads each script *(wrapper,
  cancel, collect — o14.1)* as
  `<root>/bin/<name>-<sha256 prefix>.sh` *(full 64-hex, o13.2)* (the root per Decision n) via a temp file + `rename`, then checks
  the remote `sha256sum` against the embedded bytes (rule #9). A running wrapper keeps executing its
  own file, because a new version gets a new name. Measured 2026-10-03 (probe P3, laptop and uni,
  bash 5.2.21): when a running script is overwritten **in place** (`cp`/`cat >`, same inode), bash
  resumes at its old byte offset **in the new content**. It runs new lines, or a mid-line fragment as
  a command. A `mv` (rename) leaves the running instance on the old content. **Never `cp`/`>` over a
  script that may be running.** (`rsync`'s default temp + rename was not run.)
- **`.enqueued` marker** (Anton, H1). At submit, after `tsp` returns, the submit step writes
  `.enqueued` (temp file + `rename`) holding the slot's `TS_SOCKET` path and the `tsp` id. The cancel
  script uses it to find the queue entry. Nothing trusts the bare id: `tsp` ids are per daemon and
  restart at 0 after a daemon restart (P4), which is the tsp version of PID reuse.
- **Every marker is written by temp file + `rename`** *(`.started` is now published by a no-clobber hard link, `ln -T` = one `linkat`, per Decision l Part B item 5; still atomic in the same directory, so the argument holds)* — `.started`, `.cancelled`, `.enqueued`, and
  also **`.exit_code`**, including the 97 path below (review round 2, MED-3). A snapshot must never see
  a half-written `.exit_code`: row 6 would make a job that is completing permanently `Failed`.
- **Cancel script, revised** (Anton, round 2 HIGH-1 and MED-5). This supersedes steps 2–3 of Decision
  i's "one cancel script" amendment. Steps 1 (write `.cancelled`) and 4 (remove `.tmp/`) are unchanged.
  - **Queued:** `tsp -r <id>` runs **only if** `tsp -l` on the `.enqueued` socket has a row with
    that id, in state **`queued`**, whose command carries this job's dir as a whole token (round 3
    LOW-F: what `tsp -r` does to a running row is not measured). Otherwise it is skipped, and
    `.cancelled` alone makes sure the job never runs (the wrapper checks it, Decision b).
  - **Running:** **no `tsp -k`**. The script sends TERM to the wrapper's process group (PGID = the
    wrapper PID) **only after** checking the wrapper is ours and alive: a current `boot_id`, plus
    alive and ours as defined below — state ≠ `Z`, field 22 = `.started`'s `starttime`, and the
    "ours" cmdline (round 5 LOW-1). Then comes the sweep.
  - **Sweep and SID guard only within the same boot** (round 5 LOW-4). Field 22 counts ticks since
    boot. So the cancel script evaluates the SID guard and sweeps (orphans included) **only when
    `.started`'s `boot_id` equals the current one**. With a stale `boot_id` there is nothing of ours
    left to kill.
  - **The sweep is cwd-filtered:** TERM+CONT → wait → KILL, applied only to the members of
    `ps -s <sid>` whose `/proc/<pid>/cwd` is **this job's dir**. MPI ranks keep the job dir as cwd
    (probe, process anatomy). **Measured for every member** (probe 5.2b, round 3 LOW-C), in a wrapper
    that `cd`s into the job dir, on a 4-process MPI HF run and a 4-process NumFreq run. The wrapper,
    `orca`, `sh -c mpirun`, `mpirun`, the `orca_*_mpi` ranks, `orca_numfreq` and the per-displacement
    `orca_leanscf` all had cwd **exactly** the job dir. All of them had the wrapper's SID, and no
    process outside the SID had the job dir as cwd. Not measured: Opt/Freq, other ORCA tools, or a
    process that `chdir`s or `setsid`s itself. **The wrapper's step 0 is `cd "$job_dir"`.** The cwd
    filter alone does **not** make a reused SID harmless (round 3
    LOW-B): our own tools, or a debugging shell, can sit in the job dir. So:
    - **SID-reuse guard, by start time** (Anton, round 4 N-2). Before it signals anything, the sweep
      reads `/proc/<sid>/stat`. The SID has been **reused** only if a process exists at that number
      whose field 22 **differs** from the `starttime` recorded in `.started`; then the sweep signals
      **nothing**. If the process is absent, or has the same start time (alive, or a **zombie** that
      tsp's runner has not reaped yet), the wrapper is ours, and the cwd-filtered sweep proceeds.
      - Measured (probe 5.2c, `task-spooler-uni-probe.md`): a zombie keeps its `/proc/<pid>` and an
        **unchanged field 22**, in state `Z`, with an empty cmdline. That was measured on the laptop;
        on uni, state `Z` and the empty cmdline were measured, but the start-time comparison was not.
      - So the cmdline check used alone would have misread a dying wrapper as "reused" — the hazard
        round 4 found.
      - Forced PID reuse was not measured. That a reused number gets a different start time is
        inference (resolution 10 ms, `CLK_TCK` = 100; two processes 50 ms apart differed by 5
        ticks).
    - **Own-SID exclusion:** the sweep never signals its own session. This is the probe-era guard,
      carried over (Decision i, 2026-10-03 probe amendment).
    - **The collector and the cancel script never `cd` into a job dir.** They use absolute paths
      only, so they never match the filter themselves.
    - That a live member pins its SID number, and that a live wrapper's PID = PGID cannot be
      reallocated while it lives (so a TERM to that PGID after the check reaches only our group), is
      **inference from kernel semantics, not measured** (round 3 LOW-H). The window between the
      check and the signal is not closed by a measurement.
  - **Trigger:** the sweep runs when the wrapper is alive and ours **or** the cwd-filtered session is
    non-empty (round 2 MED-2). So orphans of a dead wrapper are swept too.
  - **No kill is ever addressed by a `tsp` id.**

*The classifier — `classify(snapshot, reenqueue_count) -> Outcome`, a pure Rust function.*
- **Raw facts in, decision in Rust.** The snapshot holds raw facts only. It does not accept a remote
  verdict such as `alive=yes/no` or "tsp knows the job" (rule #9; review H1).
- **Snapshot fields:**
  - the host's current `boot_id`;
  - the raw bytes of `.started`. Its fields are `pid`, `pgid`, `sid`, `boot_id`, `started_at` (the
    probe's set) and **`starttime`**, the wrapper's own kernel start time (`/proc/$$/stat` field 22,
    clock ticks since boot; Anton, round 4 N-2). Decision i needs `sid` + `boot_id` + `starttime` for
    the sweep;
  - the raw `/proc/<pid>/stat` line (or "absent") and the raw `/proc/<pid>/cmdline`;
  - for each member of `ps -s <sid>`, its PID and raw `/proc/<pid>/cwd` (taken only when `boot_id` is
    current; Anton, M7). Rust keeps only the members whose cwd is this job's dir — the **job
    session** (Anton, round 2 MED-5). A member whose cwd cannot be read (ENOENT: it exited, or it is
    a zombie, per probe 5.2c) is **not** in the job session; this is not an `Error` (round 5 LOW-3). **The classifier applies the same SID-reuse guard** as the cancel
    script (Anton, 5.2 Part A): if the SID is reused (a process at that number has a different
    start time), the job session is **empty**. Otherwise a foreign process in the job dir would hold a
    cancelled job in `Cancelling` forever, because the guarded sweep signals nothing, or it would show
    up as phantom `Lost` orphans. For this the snapshot also carries the raw `/proc/<sid>/stat` line
    (or "absent");
  - for **each of the profile's slot sockets, plus the socket recorded in `.enqueued`** (it may have
    left the profile after a slot-count change), a three-way raw fact (round 3 MED-A):
    - `NoDaemon` — nothing listens on the socket path: the path is absent, or the socket file is
      stale. This is read from **`/proc/net/unix`**, which is read-only. **`tsp` is never run to find
      out.**
    - `Rows(..)` — a daemon listens, and these are the raw `tsp -l` rows whose command contains
      **this job's dir**;
    - `Error` — any other failure.

    "Query failed" is never read as "no rows" (round 2 MED-4). `NoDaemon` **counts as no rows**: the
    queue lived in the dead daemon's memory. A queued runner dies with it after `tsp -K` (P4,
    measured) and after a reboot (trivially). After a daemon **SIGKILL or crash**, the fate of a queued
    task's live runner is **not measured** (round 4 N-4). If such a runner went on to start its
    wrapper, `.started` would appear and the bracketing re-read of `.started` would catch it. The
    ReEnqueue window that remains is inference.

    Measured, probe 5.2b on tsp 1.0.1 (`task-spooler-uni-probe.md`):
    - **`tsp -l`, `-s` and `-r` on a missing or stale socket silently start a new daemon**, as long
      as the parent dir exists. `-l` then returns rc 0 with only the header.
    - `test -S` is true for a stale socket too, while `/proc/net/unix` / `ss -xl` list only a live
      one.
    - So a collector that called `tsp -l` to look would create empty daemons as a side effect. That
      would make a restart-dropped queue look empty and let the next job start on a mask that a
      surviving job still holds (rule #8).
    - **Rule:** only an explicit submit may touch a socket that has no listening daemon. The cancel
      script makes the same `/proc/net/unix` check before `tsp -l`/`-r`, and does nothing with tsp
      when no daemon listens. (`tsp -K` on a dead socket is **not measured**, so it is not on the
      allowed list; round 4 N-3.)
    - **Accepted residual window** (round 4 N-1): the daemon can die between the `/proc/net/unix` read
      and a `tsp -l`. That `tsp -l` then spawns a new daemon and returns only the header. The
      classification stays correct (`Rows([])` = `NoDaemon`), and the side effect is made harmless by
      the **unconditional slot check on every submit** (Anton, round 4; see the 5.3 note below).
    - **Socket path length:** the probe read both `ss -xlp` and `/proc/net/unix`. It saw `ss`
      truncate long paths. Whether `/proc/net/unix` truncates was not checked. The limit on a Unix socket path is a
      `sun_path` of 108 bytes including the NUL — **sourced** from the laptop's `man 7 unix`:
      *"char sun_path[108]; /* Pathname */"* (https://man7.org/linux/man-pages/man7/unix.7.html), and
      `/usr/include/linux/un.h`: `#define UNIX_PATH_MAX 108` (read 2026-10-03). It was not measured on
      uni. **Submit asserts that each socket path is ≤ 100 bytes** (round 4 N-3). **Post-condition for
      5.3** (rule #9, round 5 LOW-5): after each submit, the socket path must appear **verbatim** in
      `/proc/net/unix`, so a truncated listing can never fake `NoDaemon`.

    Rows are matched by job dir, never by id. Job dirs are unique per job. Measured (probe P4, tsp
    1.0.1):
    - `tsp -l` did not truncate a 256-char line, with or without a tty or `COLUMNS`. Longer lines
      were not tested.
    - Row order is running → queued → finished, not id order.
    - E-Level is `0`, the exit code, or `-1` after `tsp -k`.
    - The command column is argv **joined by single spaces, unquoted**. So a job dir is matched as a
      whole space-separated token. **Job-dir paths must match `[A-Za-z0-9._/-]+`** *(narrowed by Part B item 4: one path rule that also rejects `//`, `.` and `..`, for job dirs, the root and sockets alike)*, which the submit
      step asserts (round 2 LOW-7). That also keeps every per-job path that 5.3's rsync/poll commands
      carry free of shell metacharacters.
    - After `tsp -K` and a fresh daemon on the same socket, the old rows are gone and ids **restart
      at 0**. A **running** job survives `tsp -K` (its runner and process stay), while a queued task's
      runner dies. Rows 7/8 do not need a `tsp` row, so a running job is still classified correctly
      once its row is gone.
  - the raw `.exit_code` bytes, whether `.cancelled` exists, and the last **5 KiB** of `output.out`
    (the local `TAIL_BYTES`, rule #5). Rust checks the tail for `ORCA TERMINATED NORMALLY`, the same
    test as `detect_completion` (`local_backend.rs`).
- **Collection order is fixed** (review H2): `boot_id` → `.started` → `/proc` and `ps -s` → `tsp -l`
  → `.exit_code`, `.cancelled`, tail → **`.started` again**. The wrapper writes `.exit_code` before it
  exits. So once the process has been read as dead, a missing `.exit_code` read afterwards really
  means the wrapper died without writing it. A job that finishes between two reads cannot look
  `lost`.
  - **The second `.started` read closes the reverse window** (round 2 LOW-2). If it is absent at the
    first read and present at the last, the job started during collection. The snapshot is discarded
    and taken again, so a job that ran is never `NeverStarted`. **At most one retake** (round 3
    LOW-E). The retake terminates because `.started` is never removed once written. If the bracketing
    reads still differ (including present → absent, which should not happen), the outcome is
    `Indeterminate`.
- **"Ours"** (review M2): the PID is ours only if its cmdline runs our wrapper from
  `<root>/bin/` **and** carries **this job's dir** as its positional argument. Without the second
  condition, another job's wrapper reusing the PID would pass. The fixture cmdline strings come from a
  recorded run, never invented. Measured (probe P2), a tsp-launched wrapper's `/proc/<pid>/cmdline`
  with NULs shown as `|`:
  `bash|/home/anton/.orcastudio/probe-5.2/bin/wrapper.sh|/home/anton/.orcastudio/probe-5.2/jobs/j1|0-3|/opt/orca|`.
  So "ours" means argv[0] = `bash`, argv[1] is `<root>/bin/wrapper-<any sha>.sh`, and argv[2] is this
  job's dir.
  - **"Alive"** means the stat line exists, its state (field 3) is **not `Z`**, and its field 22
    equals `.started`'s `starttime`. A zombie is dead for every rule in the table (probe 5.2c).
  - **Parsing the stat line:** take the text after the **last** `) `, because comm may contain
    spaces and parens; field N is then token N−2. Measured with a script named `w q) x.sh`.
  - **The wrapper reads its own stat** with the builtin `read -r l </proc/$$/stat`. `/proc/self`
    under `$(…)` or `cat` reports the child's PID (measured). The sha is **any** sha, because after an upgrade the old wrapper is still running (M1).
  The 5.2 fixtures use this **recorded shape**: the probe's path was `bin/wrapper.sh`, without a sha
  (round 2 LOW-5).
  - The wrapper is `PID = PGID = SID`. Its parent is the per-task tsp runner, and `tsp -p` prints the
    wrapper PID.
  - **`taskset` execs.** Its child shows as `sleep|60|` with no `taskset` in the cmdline, so no sweep
    may anchor on `taskset`.
  - The tsp daemon's own argv is rewritten to the first job's command. No sweep may assume a fixed
    daemon cmdline.
- **Precedence table.** First match wins. It supersedes the earlier classification wording, including
  `.pid` (now `.started`) and "five-way classification" in d and in Consequences (review L1). It also
  supersedes the d′ amendment's "`.cancelled` before every other rule" and "cancelled whatever the
  `boot_id`", in two cases (round 2 LOW-3):
  - **row 1** — a cancelled job with a corrupt `.started` is `Failed`. This is intended: without `sid`
    the job cannot be swept, and the user must see that;
  - **row 2** — the late cancel.

  "Job session" below means the cwd-filtered `ps -s` members.

  | # | Condition | Outcome |
  |---|---|---|
  | 1 | `.started` exists but does not parse (empty or missing fields — a disk-full write can publish an empty file, whether by `rename` or by `ln -T`) | `Failed` ("corrupt `.started`"). The cancel sweep is impossible without `sid`; the reason says so. *(After a delivered cancel: terminal `failed` with evidence, o22.4.5.)* |
  | 2 | `.cancelled` + `.exit_code` = 0 + `ORCA TERMINATED NORMALLY` | `Completed { late_cancel: true }` (Anton, M6). A clean result is never discarded, and the UI shows that the cancel came too late. |
  | 3 | `.cancelled`, `boot_id` current, and the wrapper PID is alive and ours **or** the job session is non-empty | `Cancelling` (transient). The reconcile re-runs the cancel script, which sweeps on the same trigger. 5.4 counts the sweeps. **Progress** = the job session shrinks. After 3 sweeps without progress (e.g. a D-state process), the **automatic sweeps stop**, and the job stays `Cancelling` with a user-visible notice and a manual retry (round 2 MED-2, round 3 LOW-D). *(Realised by o22.5: `Cancelling { session }`, the count in `PollerMemory`, Cancel = the manual retry.)* |
  | 4 | `.cancelled` (anything else) | `Cancelled` |
  | 5 | `.exit_code` parses to 0 + `TERMINATED NORMALLY` | `Completed` |
  | 6 | `.exit_code` present (non-zero, empty or not a number; or 0 without `TERMINATED NORMALLY`) | `Failed` (rule #6) |
  | 7 | `.started`, `boot_id` current, wrapper PID alive and ours | `Running` |
  | 8 | `.started` (any other case: a different `boot_id`; or current `boot_id` with the wrapper dead or not ours — OOM, an outside kill) | `Lost { orphans }` (Anton, H2: at once, with no re-check). `orphans` = the job-session PIDs, which are non-empty only with a current `boot_id`. 5.4 sweeps them, with the same cwd-filtered sweep (Anton, round 2 MED-1): an orphan with no wrapper never writes `.exit_code`, so its result can never pass rule #6, and it holds the slot's cores that tsp is already handing to the next job. |
  | 9 | no `.started`, a `tsp` row in state queued or running | `Queued` (the job may be starting right now; the next reconcile sees `.started`) |
  | 10 | no `.started`, and any socket fact in the snapshot is `Error` | `Indeterminate` — no action this pass (round 2 MED-4). Re-enqueueing a job that may still be queued would put two wrappers in one job dir. 5.4 counts consecutive `Indeterminate` passes and tells the user after 3 (round 3 LOW-D) *(unit 5.4b, o22.1)*. |
  | 11 | no `.started`, no socket fact is `Error` (`NoDaemon` = no rows), and either a finished `tsp` row (a wrapper that crashed before writing `.started`) or no row at all (a restart dropped the queue) | `NeverStarted`: `ReEnqueue` if `reenqueue_count` = 0, otherwise `Failed` ("wrapper never started") |
- **The wrapper's start sequence** is now 0 `cd "$job_dir"` (round 3 LOW-C; if the `cd` fails, the
  wrapper exits at once, before writing `.started`, so ORCA never runs outside its job dir: rule #3,
  round 4 N-7) → 1 `.started` → 1a the
  self-check below → 2 `.cancelled` check → 3 env + pinned ORCA → 4 `.exit_code` by `rename`.
- **The wrapper checks its own marker** (rule #9). This is step 1a in Decision b's start sequence
  (round 2 LOW-4). After the `rename` *(`.started` is now published by a no-clobber hard link, `ln -T` = one `linkat`, per Decision l Part B item 5; still atomic in the same directory, so the argument holds)*, the wrapper reads `.started` back and parses it. If that fails,
  it exits without launching ORCA and writes `.exit_code` = 97 (by `rename`), so row 1 or row 6
  explains the failure. Exit code 97 is our own choice, not an ORCA code.
- **The outcome type is the classifier's own enum:** `Queued`, `Running`, `Completed { late_cancel }`,
  `Failed { reason }`, `Lost { orphans }`, `Cancelling`, `Cancelled`, `Indeterminate`, `ReEnqueue`.
  `Lost` and `Cancelling` join `JobStatus` only in 5.4, so 5.2 does not change `JobStatus`.
- **The re-enqueue count is an input.** The `jobs` column (schema v19 *(renumbered v20 by Decision n: v19 is the 5.1 Part B profile columns; then v21 by Decision o: v20 is the 5.3 job coordinates; then later than v21 by o22.1: v21 is 5.4a's pending cancel)*) lands in **unit 5.4** *(5.4b, o22.1)*. The bound
  holds only if 5.4 **persists the increment before** it issues the re-enqueue (review M5). Otherwise
  a crash between `tsp` submit and the DB write would re-enqueue again, without limit.

*Details fixed while building 5.2 Part B (2026-10-03).* Items 1–4 are the orchestrator's calls
within (l); items 5–6 are Anton's decisions.
1. **`.enqueued` format:** `socket=<abs path>\nid=<decimal>\n`, keys in any order, each exactly
   once. 5.3's submit writes exactly this, by `rename`.
2. **An unparsable `.enqueued`** is a socket `Error` fact. So with no `.started` the job is row 10
   `Indeterminate`; with a `.started` it does not block classification.
3. **Failure classes:**
   - a failed `tsp -l` is a socket `Error` fact (row 10);
   - a failed read of `/proc/net/unix` fails the whole snapshot, like any read error other than
     ENOENT/ESRCH (verifier Part A, LOW-1).
4. **Paths:** Rust and shell use **one** validity rule. It rejects `//` and `.`/`..` components (the
   shell rule, which is stricter). The cwd filter compares the kernel's canonical cwd, so **5.3's
   submit asserts that the job dir equals its `realpath`**; a symlink component would never match.
5. **A `.started` that already exists** (Anton): the wrapper **refuses**. It exits at once, runs no
   ORCA, and touches neither `.started` nor `.exit_code`. The test and the creation of the marker
   are atomic, so two wrappers can never run in one job dir. Re-enqueue happens only when there is
   no `.started` (row 11), so this case is a fault, never a normal path. This is also what makes the
   accepted residual ReEnqueue window harmless: after a daemon SIGKILL the fate of a queued runner is
   not measured, but if that runner later starts its wrapper, the second of the two wrappers refuses
   (verifier Part B, D4). `.started` is published by `ln -T` (one `linkat`; it fails on an existing
   file, dir or dangling symlink — measured on the laptop and re-measured by the verifier; **measured on uni
   too**, probe 5.3: one `linkat`, `File exists` in all three forms; strace EEXIST shown for the
   file case).
6. **Failure to create `<job>/.tmp`** (Anton): the wrapper writes `.exit_code` = **96** by `rename`
   and exits without ORCA. With a `TMPDIR` outside the job dir, OpenMPI litter would escape it
   (rule #3), so the run is not allowed. Row 6 → `Failed`. Like 97, 96 is our own code, not ORCA's.
   **Open (rule #10):** ORCA's own exit-code range is not measured, so 96/97 are not *shown* to be
   disjoint from it. They map to `Failed` either way. **Fork for Anton, later (5.4 UI):** whether
   `FailReason` should name 96/97 (e.g. "TMPDIR not creatable", "self-check failed") or keep them as
   `NonZeroExit` (verifier Part B, D5).
7. **`.tmp` is removed at the end of every cancel that completes** (verifier Part B, F6; a cancel that
   stops on a read error fails closed with exit 3 before this step, per the hardening review N4). This keeps
   Decision i step 4 as stated. It is safe: a wrapper that starts later sees `.cancelled` at step 2,
   before it creates `.tmp` at step 3.
8. **The wrapper's core mask is validated** as `^[0-9]+([,-][0-9]+)*$` before `taskset` (verifier
   Part B, F3). A value starting with `-` would otherwise be parsed as an option.

*Tests (5.2 needs no server).*
- **Classifier:** a table test over synthetic snapshots, one or more per row. That includes the
  restart simulations: a stale `boot_id` gives `Lost`, and an empty `tsp` row list gives
  `NeverStarted`.
- **What is synthetic** (review M4): `tsp` is not installed on the laptop (measured 2026-10-03, `command
  -v tsp ts` is empty). So in 5.2 "`tsp -K`" is only a snapshot with no rows, and the wrapper/cancel
  script tests use a **stub `tsp`** on `PATH`.
  - Probe P4 measured what real `tsp -K` does: the running job survives, and the queued runner dies.
    So `tsp -K` simulates a reboot only for **queued** jobs. A stale `boot_id` simulates it for
    running ones.
  - The remaining third-party facts of Open question b stay open (round 2 LOW-1): daemon auto-restart
    and partial dirs after a real reboot.
  - Settled by probe 5.2b: after the daemon dies, a stale socket file is left behind (with SIGKILL),
    and a `tsp` call on it silently starts a new daemon. This is why the collector reads
    `/proc/net/unix` instead.
  - The `NoDaemon` case is part of the 5.2 table tests.
  - **For 5.3** (round 2 LOW-9): a running job that survives `tsp -K`, plus a fresh daemon on the same
    socket, lets tsp start a second job on the same mask (rule #8). **Every submit to a slot checks the
    slot, whether or not a daemon is alive** (Anton, round 4 N-1). An accidentally spawned daemon is
    then harmless.
    - **What is checked** (wording corrected in round 5, MED-1; the literal round-4 wording would
      have refused to queue behind a running job and so defeated Decision a): **every live job session
      on the slot's mask must be accounted for by a `running` row of that slot's daemon, matched by
      job dir.**
      - **"Live job session"** means row 3's trigger: a wrapper that is alive and ours, **or** a
        non-empty cwd-filtered session. Orphans hold cores too.
      - Socket fact `Rows`: a live session with no matching `running` row **blocks** the submit.
      - Socket fact `NoDaemon`: **any** live session on the mask blocks it.
      - Socket fact `Error`: the submit is blocked.
    - **A blocked submit** stays pending locally and is retried on the next reconcile. 5.4 tells the
      user after 3 refusals *(realised by o23: unit 5.4c; the counter per launch, o23.4)*.
    - **Accepted residual:** the daemon could die between this check and the `tsp` submit, leaving a
      survivor unaccounted for. This window is not closed.
  - **Also for 5.3:** tsp writes a `/tmp/ts-out.*` file per task (P4); not yet prevented. Probe 5.3
    measured a candidate remedy: a `TMPDIR` on every `tsp` enqueue (the client's `TMPDIR` decides).
    Where that directory lives and how it is cleaned is decided with 5.3.
- **Shell and Rust liveness agree:** only the **liveness predicate** ("ours and alive") and the
  **cwd filter** of the job session are compared between shell and Rust. A materialiser builds a real job dir and starts a real local process whose
  cmdline has the recorded shape. Both predicates must give the same answer for every fixture. The
  required materialised fixtures (round 5 LOW-3) are:
  - (a) a live wrapper → alive;
  - (b) a **zombie** wrapper (an unreaped child of a parent that does not wait, as in probe 5.2c)
    → not alive, but the SID guard says "ours";
  - (c) a forged `.started` `starttime` → not alive, and the SID guard says "reused";
  - (d) a zombie session member whose cwd gives ENOENT → excluded from the job session;
  - (e) a live member whose cwd is a different dir → excluded.
- **The d′ race:** a model test that enumerates all 6 interleavings of the wrapper's `.started` → check
  and the cancel script's `.cancelled` → check. In every one, ORCA never runs under `.cancelled`.
  The real scripts are also run in both sequential orders, with a stub ORCA.

**m) Unit 5.3 submit rules (Amended 2026-10-03; Anton accepted the orchestrator's leans; DESIGN
review pending, together with the 5.3 decomposition).**
1. **Where `tsp` writes its output file:** in a per-job subdirectory **`<job>/.tsp-out/`**. The
   submit step creates it and passes it as `TMPDIR` on **every** `tsp` enqueue; probe 5.3 measured
   that the client's `TMPDIR` decides where the file goes.
   - The name differs from `.tmp`, which cancel removes.
   - The file holds only the wrapper's own messages (ORCA writes to `output.out`), so it stays with
     the job: it is **always fetched** with the job's results (round 1 MED-8; the always-fetched set
     becomes output/xyz/hess/`.tsp-out/`; *superseded by o item 6: one shared artifact-pattern list feeds `curated_match` and the rsync filter*) and goes away with the job dir (rule #3).
   - **Open (5.3/5.4):** the policy for removing a job dir on the server after a successful fetch is
     not decided. Until it is, nothing removes remote job dirs.
   - Creation is idempotent (`mkdir -p`, also on a ReEnqueue). If it fails, the submit refuses and
     nothing is enqueued.
   - Open, not measured: the `tsp` daemon is started by the first enqueue and inherits that job's
     `TMPDIR`. What happens once that job dir is gone is unknown. It is likely harmless because
     `TS_SOCKET` is always explicit.
   - *Rejected:* a shared directory under the root. It grows without bound and needs its own
     cleanup.
2. **A blocked submit** (the slot check of l refuses):
   - The submit stays pending locally and is **re-checked on every reconcile**. The check only reads
     state; it signals nothing and changes nothing *(o23.3: the re-check is the submit attempt itself,
     which writes nothing in the job dir while blocked and submits when the slot is free)*.
   - After 3 refusals the user is told, and the notice **names the blocking job(s)**.
   - The user may withdraw the pending submit, cancel the blocking job, or keep waiting. *(Withdrawing a pending submit is local; the remote Withdraw command is gone, o22.7. Superseded by o23.8: there is no local withdraw; Cancel is the exit.)*
   - **No automatic action** ever frees a slot: a blocked slot almost always means something really
     is still computing on those cores (rule #8).
   - **Persistence and sequencing** (round 1 MED-9): the pending submit and its refusal counter live
     in the local DB *(o23.4–5: the flag in the DB, v22; the counter per launch in `PollerMemory`)*. Unit **5.4** adds them with the v21 migration *(renumbered from v20 by Decision o: v20 is the 5.3 job coordinates; moved past v21 with unit 5.4c by o22.1)*, next to the re-enqueue counter,
     and runs the re-check from its reconcile loop. Until 5.4, a refused submit simply fails with
     the blocking job named.
3. **"On the slot's mask" means the cores intersect.** *(Scope widened by o item 9: the scan covers every process and live queue of the account, not only the profile's jobs, and runs once inside the submit call instead of through per-job collects; the accounting rule of l round-5 MED-1 stays.)*
   - A session's cores come from the wrapper's mask argument (argv[3]) or, for an orphan, from
     `Cpus_allowed_list` in `/proc/<pid>/status`.
   - The check covers **every non-terminal job of the profile**, across all sockets, including
     `.enqueued` ones — not just the jobs of this slot.
   - This is what stays safe when the slot count changes: a job still running on an old `0-23`
     mask blocks a new `12-23` slot.
   - **This extends the 5.2 collector** (round 1 MED-10). Per member, the snapshot gains the raw
     `Cpus_allowed_list` from `/proc/<pid>/status`; the wrapper's mask is argv[3] of its cmdline,
     which is already collected. The wire format, its strict parser and the shell/Rust parity tests
     grow accordingly in 5.3. The cross-job enumeration is one collect per non-terminal job of the
     profile.
   - Residual: a live session from a job the local DB no longer knows (e.g. its row was deleted
     while it ran) is outside the enumeration. 5.4 decides whether deleting a non-terminal remote
     job is allowed.

**n) Unit 5.1 Part B: profile columns, connection test, verification lifecycle (Amended
2026-10-03; Anton decided every fork; DESIGN review round 1 → FAIL, rewritten).**

*Profile data (schema v19).*
1. `slot_count INTEGER NOT NULL DEFAULT 1 CHECK (slot_count = 1)` (Anton, round 1 HIGH-1).
   - The profile has **one** `core_mask`, and Decision b requires one disjoint mask per slot.
   - So until parallel slots are measured (Decision f), a profile has **exactly one slot**: the
     UI shows it fixed, and the database rejects any other value.
   - Per-slot masks arrive with a later migration, together with the measurement.
   - Existing rows backfill to 1.
2. **A run target** = `verified_at IS NOT NULL` **and** a valid `core_mask`. A valid mask has the
   `taskset` list syntax that the wrapper validates (`^[0-9]+([,-][0-9]+)*$`), and every CPU in it
   lies within `0..core_count-1` (rule #8). A profile with `core_mask` NULL is not a run target, so
   submit refuses.
3. `availability_window TEXT NULL` (Decision g; Anton): `HH:MM-HH:MM`, in the **laptop's local time
   zone**, evaluated only against the laptop's own clock (Decision j).
   - A window may wrap past midnight, e.g. `22:00-08:00`. Equal endpoints (`08:00-08:00`) are
     rejected on write (round 2 F8).
   - A malformed value is rejected on write; NULL means no window.
   - The result is only an informational label, never an error.
4. **`remote_scratch_dir` is the root** of Decision k (Anton, round 1 MED-4). The job dirs, the
   `tsp/` sockets and `bin/` all live under it.
   - When a profile is **saved**, the root is validated:
     - an absolute path;
     - the one path rule of (l) item 4;
     - the socket paths under it fit the ≤ 100-byte bound of (l).
   - The realpath rule is checked by the connection test.
   - `remote_orca_path` must be absolute (rule #1).

*The verification lifecycle* (round 1 HIGH-2).
5. **Changing the value** of any of `host`, `remote_orca_path`, `remote_scratch_dir`, `core_mask`
   or `slot_count` sets `verified_at` and the verified_* facts back to **NULL**: a changed target is
   not the target that was verified.
   - Renaming a profile or editing its `availability_window` keeps the stamp. A save that rewrites
     a field with its unchanged value is not a change (round 2 F3).
   - The existing test that asserts the stamp survives an update is inverted for target fields, not
     deleted.
6. A re-test that is not a **full pass** also sets `verified_at` and the verified_* facts to NULL. A
   stamp never outlives the facts it certified.
   - **Full pass** = every item-8 check passes.
   - Item-9 values are stored when present and NULL otherwise, so `openmpi_version` becomes optional
     in the stamp (round 2 F2).
6a. **`verified_at` gates new submits only.** Reconcile, cancel and fetch of existing jobs never
   consult it, so a transient failure cannot cut off monitoring or cancel (round 2 F1).
6b. **Jobs keep their own coordinates** (Anton, round 2 F1).
   - **At submit:** each job records its absolute job dir, its socket and the profile host it was
     submitted to. Reconcile, cancel and fetch use these, never the profile's current values (5.3).
   - **While non-terminal jobs exist:** the UI refuses to change a profile's `host` or
     `remote_scratch_dir` as long as the profile still has non-terminal jobs (enforced from 5.3 on,
     when such jobs can first exist).
6c. **The stamp is bound to the target that was tested** (orchestrator, 5.1 Part B Part A; acknowledged by Anton 2026-10-03). The
   stamp is written only if the profile still has the exact target that was tested (host, ORCA
   path, root, mask, slot count); otherwise it is refused as a conflict. An edit made while the ssh
   test was running must never be certified.
6d. **Post-condition of the transport** (refines item 11; acknowledged by Anton 2026-10-03). The script echoes every received
   **value**, not only the count, and Rust compares them with what it sent. A script line after the
   read loop lands *inside* value 0 while the count stays correct, so a count alone would not catch
   it (shown by a negative control).
7. **Every submit re-checks `KillUserProcesses`** (Anton): one `busctl` read in the same ssh call.
   Anything other than `b false` refuses the submit and sets `verified_at` to NULL *(signalled by
   `refused-kup`, o13.3)*. This is the one
   host setting that silently kills jobs (Consequences).

*The connection test.*
8. **Mandatory — each one must pass, and "undetermined" counts as "not passed"** (round 1 LOW-7):
   - **ORCA:** `test -x`, then `<path> --version </dev/null 2>&1`. It passes only with a `Program
     Version x.y.z` line. ORCA exits **2** on `--version` (measured), so the rc is not the signal;
     rc 127 or 126 means not runnable.
   - **cores:** `nproc` parses as an integer.
   - **the core mask** (Anton, round 2 F3): when `core_mask` is set, every CPU in it lies within
     `0..nproc-1`, checked in the same test. One test is then enough to make the profile a run
     target. With no mask, or a mask out of range, the profile is not a run target, and the UI says
     why.
   - **KillUserProcesses:** `busctl` prints exactly `b false` with rc 0. `b true`, any other output or
     any rc ≠ 0 means not passed. So a host without logind cannot be a run target; this is accepted
     for now (measured only on systemd 255).
   - **the root:** created with `mkdir -p` if absent (Anton: the test may write inside the profile
     user's own tree). It must equal its `realpath`. `findmnt -no FSTYPE --target <root>` must print
     a type on the **allow-list = `{ext4}`** — the only type measured; any other type, or empty
     output, means not passed. A type joins the allow-list only after a run measures it (rule #10).
9. **Recorded, not gating** (Anton, round 1 MED-3):
   - **OpenMPI:** the version is **recorded**. Rule #2's *match* is **not** checked here: no
     expected version is sourced. The match is shown in practice by a real run (water on uni was
     bit-identical). The stored version is the one `ompi_info --version` reports.
     `mpirun --version` was measured in the connection test's non-tty `bash -s` context. That this
     equals the wrapper's environment under `tsp` is **inference**. Both report 4.1.6 from
     `openmpi-bin` (probe 5.1c).
   - **`sudo` group membership:** a warning (Decision k).
10. **Not checked, by Anton's choice:** that `tsp` is present, and a self-test of the readers. They
    surface at the first submit or collect, which fails closed.
11. **Transport (measured, probe 5.1c):** one static, embedded script followed, on the **same
    `bash -s` stdin**, by the profile's values as a NUL-separated list. This works on both hosts and
    preserved an empty value, an embedded newline, `'`, `$HOME` and `;`, but only with a strict
    shape:
    - bash reads its script from the pipe one command at a time, so any script line after the
      `read` loop is itself read as "values" — silently, rc 0 (measured);
    - the script's **last line** is therefore `args=(); while IFS= read -r -d '' a; do args+=("$a");
      done; main "${args[@]}"; exit`. Before it there may be only definitions and commands that do
      not read stdin (each with `</dev/null`), e.g. a prepended `head.sh` (round 2 F7). Nothing may
      follow it but the NUL list;
    - **every child command gets `</dev/null`**, because a child that reads stdin swallows the rest of
      the script and the data, silently (measured with `cat`);
    - post-condition (rule #9): the script echoes the received argument count, and the Rust side
      asserts it equals what was sent (values too, see 6d);
    - **the same rule applies to 5.3:** any script fed through ssh stdin follows this shape.

**o) Unit 5.3 — transport, sync and monitoring** (2026-10-05; decided by Anton in four rounds, the
rest derived from probes 5.3a/5.3b/5.3c — [orca/remote-sync-probe.md](../orca/remote-sync-probe.md).
DESIGN rounds 1–3 FAILed, each escalated to Anton. After round 3 Anton chose to **simplify the submit
to one atomic server-side call** (items 3, 9) rather than patch the two-call protocol, and that the
status after a withdraw comes from the classifier. DESIGN round 4 → PASS WITH FINDINGS (MED-A/B, LOW-1…8),
findings applied the same day; Anton decided MED-B (a per-child `timeout` plus a holder notice).
**Accepted** 2026-10-05.)

1. **Schema v20 = the job coordinates of n 6b** (Anton). `jobs` gains `remote_host`, the absolute
   remote job dir and the socket, written once and never rewritten from the profile. 5.4's columns
   (re-enqueue counter, pending cancel, pending submit) move to **v21** *(split by o22.1: v21 = the pending cancel; the others move with 5.4b/5.4c)*.
   - **A job is remote iff its coordinates are non-NULL.** Dispatch (`enum Backend`) keys on the
     coordinates, never on `backend_id`. Remote jobs keep `Queued`/`Running` in `jobs.status`
     (Anton); **every local-only query filters `remote_host IS NULL`** — today exactly
     `next_local_queued_job` (used by `try_start_next`) and `reconcile_on_startup`, both in
     `local_backend.rs`.
   - The remote job dir is `<remote_scratch_dir>/jobs/<job_id>`. Before any ssh it is checked against
     the path rule of (l) detail 4 (the root as the connection test stamped it). On the server,
     the submit call asserts `realpath <job> == <job>` and `realpath <job>/.. == <root>/jobs` (l
     detail 4; shapes measured, probe 5.3c — a path reached through a symlink fails the first).
2. **Refusals and the one exit, until 5.4** (Anton; extends n 6b).
   - For a non-terminal remote job, `cancel_job` and `delete_job` refuse with "remote cancel arrives
     in unit 5.4" — never a local `Cancelled` (Decision i). `delete_server_profile` refuses while the
     profile has non-terminal jobs (today it NULLs `backend_id`, `server_profiles.rs:205`, which would
     turn the job local).
   - **Withdraw** *(superseded by o22.7: one Cancel; this sequence is delivery's not-on-the-server branch, o22.4.3)* (Anton, round 2 fork, closes round 2 HIGH-2): a job in the **"not on the server"**
     or **"submit interrupted"** state (item 3.5) can be withdrawn **while the host answers**:
     `mkdir -p <job dir>` then the 5.2 `cancel.sh cancel` *(sequence and transport: o14.1)* (it needs the dir; it publishes
     `.cancelled` first, so a late enqueue's wrapper refuses at its step 2, and it only `tsp -r`s a
     verified row). On rc 0 **one collect + `classify` decides the status** (Anton, round 3): row 4
     `Cancelled`, row 3 `Cancelling` (shown, 5.4's), or row 2 `Completed{late_cancel}` — a job that
     did run cleanly keeps its result — or row 1 `Failed{CorruptStarted}` (a wrapper that started
     before `.cancelled` and left a corrupt `.started`; stays `Queued`, shown, 5.4's, item 4). Never a hard-coded `Cancelled` (Decision c). Once terminal, the
     profile can be edited or deleted.
   - Negative controls: a remote `queued` row is not started by `try_start_next`; a remote `running`
     row survives `reconcile_on_startup` unchanged; cancel of a remote queued job and delete of its
     profile are refused; withdraw of a "not on the server" job leaves `.cancelled` on the server and
     the row `Cancelled` via the classifier; withdraw of a job that has `.exit_code` 0 and a normal
     termination yields `Completed`.
3. **Submit = upload + one atomic server-side call** (Anton, round 3; closes round 3 HIGH-B/C, MED-4
   and removes the two-call TOCTOU of round 2 MED-1). The DB knows the coordinates before any remote
   side effect; the server is the source of truth (Decision c).
   1. Derive the coordinates and persist them with status `Queued` *(failures after it: o14.4)* in one transaction — before any
      ssh. Submits are serialised **per account** by the server lock (3.3.1), aliases included; the
      app adds no per-host guard. The per-job in-flight guard (item 4) keeps one operation per job.
   2. **Upload** *(input aligned first, o14.2; prepare/install cover three scripts + `tsp/`, o14.1/14.3)*: `rsync -a --checksum --mkpath <local job>/ <host>:<remote job>/` (item 6). rc must
      be 0. `--checksum` so a retry does not trust size+mtime over a wrong remote file (rsync semantics,
      **inference** — not load-bearing: the 3.3.4 hash check is the post-condition). **No
      `--delete`**: it would run before the no-marker assert and could touch a live dir. **Before**
      the rsync, a read-only call asserts the realpath shapes of item 1 for every existing component
      (`<root>`, `<root>/jobs`, `<root>/jobs/<id>` if present), so the upload never writes through a
      symlinked component (round 4 LOW-6). **A stale extra file** (e.g. an rsync temp left by a
      killed upload — whether the remote receiver leaves one is **not measured**) makes every retry
      refuse with that file named; the exit is **withdraw** *(Cancel since o22.7)* and a new job (round 4 LOW-5).
   3. **One submit call** (n-11 shape). Its NUL list carries the job dir, root, slot socket, slot
      mask, ORCA path and the **expected name + sha256 of every uploaded file** — **two NUL values per
      file**, within n-11's echoed count; Rust refuses a job dir of more than 1000 files before any
      ssh — so the server checks the upload itself and no Rust step sits inside the protocol. In order:
      1. `exec 9>"$HOME/.orcastudio-submit.lock"; flock -w 20 9` — one lock per account, outside
         every root, so submits from profiles aliasing one host serialise. Timeout → refuse "lock
         busy". (Measured, probe 5.3c: `-w` returns rc 1 after the wait across ssh sessions; the lock
         is released when the script exits; **a laptop-side kill does not release it** — the
         remote script keeps it until it ends, which the 20 s wait and the call's own bound limit.)
         The call's ssh timeout is 60 s (> wait + scan + enqueue).
         - **Every `tsp` invocation inside the lock (`-l` included) and every child that may outlive
           the script runs with `9>&-`**, like `</dev/null` (round 4 MED-A): a `tsp -l` on a stale
           socket starts a daemon by the same fork path (5.2b), and a daemon that inherits fd 9 keeps
           the account lock for life (5.3c C2). Negative control: a `tsp -l` on a stale socket inside
           the lock leaves `flock -n` succeeding after the script exits.
         - **The holder is bounded on the server** (Anton, round 4 MED-B): every child that can block
           inside the lock (`tsp`, `busctl`, `sha256sum`, the scan's reads) runs under `timeout N`
           (N per call, sum < 40 s); a timeout refuses (before the claim) or is `failed-after-claim`
           (after it). The laptop-side 60 s does **not** bound the remote script (5.3b fact 4, 5.3c
           C1) — only the waiters' 20 s is bounded by `flock -w`.
         - **A "lock busy" refusal names the holder PIDs** *(all openers, o13.4)* (own-uid `/proc/*/fd` entries resolving
           to the lock file) and their cmdlines, so a stuck holder can be found and killed by hand.
      2. `KillUserProcesses` re-check; the realpath asserts of item 1.
      3. **No marker** (`.started`, `.enqueued`, `.exit_code`, `.cancelled`, `.submitting`) and **no
         row holding this job dir** on the slot socket (after the `/proc/net/unix` check; `NoDaemon`
         = no rows; a failed `tsp -l` refuses).
      4. **Upload post-condition (rule #9):** the set of files under the job dir (excluding
         `.tsp-out/`) and their sha256 equal the expected list; a refusal **names** the missing,
         extra and differing files (an rsync temp left by a killed upload is an "extra"). Negative
         control: one corrupted uploaded byte → refused.
      5. **The slot check of item 9.**
      6. `mkdir -p .tsp-out` (not a marker; harmless if the submit later refuses).
      7. **Claim:** `ln -sT x .submitting` — no-clobber, rc 1 "File exists" on a second claim
         (measured, probe 5.3c; a symlink claim — (l) publishes `.started` by a hard-link `ln -T` and
         `.enqueued` by rename; all three are no-clobber). **Every refusal so far wrote no
         claim**, so the job stays "not on the server" and retry — or 5.4's pending re-check — remains
         possible (round 3 HIGH-B).
      8. Enqueue: `TMPDIR=<job>/.tsp-out TS_SOCKET=<sock> tsp bash <wrapper> <job> <mask> <orca>
         9>&-`. **The lock fd is closed for tsp** — without that a fresh daemon and its task inherit
         it and hold the account lock for their whole life (measured, probe 5.3c; `9>&-` and `flock
         -o` both prevent it). Then publish `.enqueued` (the (l) atomic form) and the (l)
         post-condition (the socket verbatim in `/proc/net/unix`, measured verbatim for a 90-byte
         path).
      9. The script reports one of *(o13.3 adds `refused-kup`)*: `refused <reason>` (nothing claimed), `enqueued <id>`, or
         `failed-after-claim <reason>` (claim stays; the label call of 3.4 decides from the server).
      Negative controls: a fresh daemon started by a submit does not hold the lock afterwards
      (`flock -n` succeeds); a slot-check refusal leaves no `.submitting`; a concurrent second submit
      for the same host waits on the lock and then sees the first one's claim/row.
   4. **The label call** (round 3 HIGH-A). For every remote `Queued` job, before any collect, the
      poller runs one **read-only** call reporting: the dir exists, which markers exist, and the
      rows holding the job dir on the recorded socket (after `/proc/net/unix`). Labels, **checked in
      this order**:
      1. no dir → **"not on the server"**;
      2. any of `.started`, `.exit_code`, `.cancelled`, `.enqueued`, or a row → **the classifier's**
         (collect + `classify`, item 4);
      3. `.submitting` → **"submit interrupted"** (claimed, never enqueued, or the call is still
         running on the server);
      4. otherwise → **"not on the server"**.
      A socket **`Error`** (a failed `tsp -l`) hands the job to the classifier (row 10
      `Indeterminate`), never to "not on the server" (round 4 LOW-4). It is checked **before**
      `.submitting`: an unreadable queue may hold the job, so "submit interrupted" (withdraw-only) is
      not claimed on missing evidence.
      "Not on the server" offers **retry** (3.2–3.3) and **withdraw** (item 2); "submit interrupted"
      offers **withdraw** only, never retry in 5.3 *(from 5.4a: Cancel instead of withdraw, o22.7; retry disabled while unreachable, o22.8)*. A withdraw's `.cancelled` moves the job to rule 2,
      so a lost withdraw reply is not a dead end. The collector is never run on a missing dir.
   5. **5.4's `ReEnqueue` is a different variant** (round 3 MED-3): a restart-dropped job already has
      `.submitting` and `.enqueued`, so this call would refuse it. The variant asserts no `.started`,
      `.exit_code`, `.cancelled` and no row, claims a name keyed by the v21 counter *(5.4b's version, o22.1)*
      (`.reenqueue-<n>`, `ln -sT`), re-checks the slot and replaces `.enqueued` by rename. Its exact
      form is 5.4's; it is never a bare `tsp` re-enqueue.
4. **Monitoring is polling while the host answers; it resumes on launch** (Anton). One loop over all
   non-terminal remote jobs, started at app launch; 5.4 **extends this loop**, it does not add a
   second one.
   - `poll_log` by byte offset (item 7) feeds the live convergence view while the job's view is open
     *(pushed as the local `job:log`/`job:convergence` events, opened by `watch_job_log` — o16)*.
   - The collector + classifier run on a timer for jobs the label call (3.4) hands to the classifier.
     **Collect is always passed the job's recorded socket (o1) plus the `.enqueued` socket**, never
     the profile's current sockets (n 6b) — so a withdrawn job without `.enqueued`, or a
     failed-after-claim job with a row but no `.enqueued`, still has a socket fact (`classify`
     errors with `NoSocketFacts` on none, `classify.rs:105`) (round 4 LOW-3). **The fetching
     outcomes** are exactly the rows that need `.exit_code`: `Completed{..}` (rows 2, 5) and
     `Failed{NonZeroExit | BadExitCode | NoNormalTermination}` (row 6). Both download (item 6); the
     final status comes from `detect_completion` on the **downloaded** files (rule #6). A failed
     job's output is downloaded too — it is the debugging evidence.
   - Every other outcome (`Queued`, `Running`, `Indeterminate`, `ReEnqueue`, `Lost`, `Cancelling`,
     `Cancelled` *(now terminal, o17; with `.started` fetched as evidence first, o22.6)*, `Failed{CorruptStarted}` *(with `.cancelled`: terminal `failed`, o22.4.5)*) *(`Cancelling` becomes a status, o22.5)* is **shown, with "handled in unit 5.4" where it needs an
     action**; the row stays `Queued`/`Running` (so it still blocks profile edits and deletion).
     5.3 passes `reenqueue_count = 0` (the column is v21's *(5.4b's version, o22.1)*), so `Failed{WrapperNeverStarted}` cannot
     occur in 5.3.
   - **One in-flight guard per job** covers submit, withdraw, collect and fetch *(also cancel, retry
     and label; a command refuses a busy job — o15)*: a tick skips a job with an operation still running.
   - **Bounded retries:** after 3 consecutive failed fetches of a fetching outcome, automatic retries
     stop, the reason is shown, and a manual retry is offered.
   - The retry counter, the in-flight guard and the log offset live in memory and **reset on
     launch**; the bound holds per launch.
   - Initial periods: log 2 s while the view is open, status 15 s. Tunable; not a contract.
5. **The run target is chosen next to Submit** (Anton): `Local` plus every profile that passes the
   same run-target predicate submit uses (n item 2); the others are listed disabled with their
   reason. Default `Local`; the last choice is remembered per project *(amended by o20: app-wide,
   since there is no project entity)*. **Nothing is deleted on the
   server in 5.3** (Anton): the remote job dir stays after download; cleanup is a later unit, once
   disk use is measured.
6. **rsync rules (measured, probe 5.3a):**
   - **Both transports get the same ssh options:** rsync runs with `-e 'ssh -o BatchMode=yes -o
     ConnectTimeout=10'` (as `ssh_bash_argv`), through `SystemRunner` (own process group, timeout
     kill). The timeout scales with the expected bytes (an opt-in `.gbw` is large).
   - **Up:** item 3.2. Without `--mkpath` a missing parent fails with rc 11 (measured); a repeated
     upload is a no-op (measured). Whether a partial multi-file upload leaves the earlier files
     complete under their final names is **inference** (only a single-file drop was measured) — the
     set-and-hash post-condition of item 3.3 makes that irrelevant.
   - **Down: never `--partial`; always `--checksum`** (Anton, 5.3 Part A1: a fetch retried after a
     hash mismatch must not skip a corrupted local file with the same size and mtime — mirror of the
     upload). With `--partial` an interrupted transfer leaves the half file under its
     **final name** (rc 12, measured). Without it, a killed ssh child left nothing (single file,
     measured; a real network drop is not measured).
   - **The download set has one source** (rounds 1 M1, 2 MED-4): one `pub const` list of glob
     patterns in one module. **Both** `export_group::curated_match` and the rsync filter args are
     derived from it (the readers use its names). It is not restated here — the code is the list.
     The markers (`.exit_code`, `.started`, `.enqueued`, `.cancelled`, `.submitting`), `stderr.log`
     and `.tsp-out/` (needs **both** `--include=.tsp-out/` and `--include=.tsp-out/**`, measured) are
     added for the download; `*.gbw` only when the job opts in. **Gates:** (a) a fixture dir with
     every pattern's example names plus negatives (`input.gbw`, `input.tmp`, `input.densities`,
     `.tmp/x`, `sub/deep.xyz`) goes through the **real local rsync** with the derived args, asserted
     in both directions; (b) **parity:** for every fixture name, `curated_match(name)` == "came down
     via rsync" (minus the download-only extras). Negative controls: drop one include → red; `*.gbw`
     without opt-in → red; a pattern added to `curated_match` only → parity red.
   - **Post-condition of the final download:** after rsync rc 0, one server call lists name + `sha256`
     of every file the same filter selects (excluding `.tsp-out/`); the **filter-selected local
     subset** must equal that set and every hash match. A mismatch is a failed fetch (item 4's
     bound), not a parse. That the files are static once `.exit_code` exists is **inference** (the
     wrapper writes `.exit_code` last, (l)); the hash check catches it if it is ever wrong.
   - **Residual (measured under a different kill):** a SIGKILL of the rsync *process alone* left a
     `.<name>.XXXXXX` temp file locally and an orphan remote sender alive ≥ 40 s. `SystemRunner`
     kills the *group*; that case is not measured. `export_group`'s Full mode skips `.*.??????` rsync
     temp names ([modules/group-export.md](../modules/group-export.md) updated in the same change).
7. **`poll_log` over ssh (measured):** one call, size first —
   `sz=$(stat -c %s f); echo "$sz"; head -c "$sz" f | tail -c +$((off+1)) | head -c CAP`, framed in
   the 5.2 length-prefixed wire format.
   - **`LogChunk` changes:** raw `bytes` and `reset: bool` instead of a lossily decoded `String`; the
     consumer keeps a byte carry and decodes complete lines only (66 polls rebuilt a file with Å/ü/→
     sha256-exact). `LocalBackend` implements the same shape.
   - `tail -c +K` past the end returns empty with **rc 0** (measured): **`size < offset` ⇒ `reset`** —
     offset to 0, the live state dropped. (Inference: a replacement that grows past the offset is
     invisible; `.started` refuses a second wrapper in one dir.)
   - Post-condition: ssh rc 0, the size header parses, and `len(bytes) == min(CAP, sz − off)` when
     `sz ≥ off`. An absent `output.out` returns no bytes at the same offset, not an error. `CAP` stays
     under `MAX_OUTPUT_BYTES` (1 MiB) minus framing.
   - Live parsing is an estimate; completion is decided by item 4 only.
8. **ssh multiplexing is the user's `~/.ssh/config`'s business** — OrcaStudio adds no `ControlPath`
   (≥ 108 bytes fails with rc 255, measured). Probe 5.3b (a Python emulation of the runner's pipes;
   its `communicate()` waits for EOF, so it is not exact on the kill path):
   - an app-spawned ssh that becomes the persisted master does not hold the caller's pipes after the
     session (separate reparented process, fds 0–2 on `/dev/null`): 0.71 s cold, 0.11–0.13 s warm;
     rsync with the item 6 `-e` options 0.95 s / 0.28 s;
   - during a session the master holds the session's pipes, so a reader that waits for EOF after a
     kill waits until the remote command ends. **`SystemRunner` already returns at the deadline right
     after `killpg` without draining** (`ssh.rs:225-228`); keep that. New negative control: a muxed
     `sleep 30` with a 2 s timeout returns in < 5 s. Reader threads are left until the remote command
     ends;
   - **a laptop-side kill does not stop remote work** (with or without the mux) — hence item 3.4's
     "submit interrupted" is never retried.
9. **The slot check scans the server** (Anton; restates (l) round-5 MED-1, which stays in force).
   Inside the submit call's lock (3.3.5):
   - **"Full set"** = the scanning shell's own `Cpus_allowed_list` (reasoning, not measured: so a cpuset
     cgroup would not mark everything pinned); unreadable → refuse (fail closed).
   - **Candidates** (own uid only; other users' cwd is EACCES — measured): every **wrapper** —
     argv element-wise `bash`, a path of the shape `*/bin/wrapper-<hex>.sh` (any root of this
     account), job dir, mask — with its argv[3] mask (the wrapper bash itself is **not** pinned,
     `0-47`, measured); and **every process whose `Cpus_allowed_list` differs from the full set,
     whatever its cwd** (rule #8; the list is inherited by children and grandchildren, measured).
     Match argv element-wise, never a grep of the joined cmdline (measured: the scanning `bash -c`
     holds the wrapper text). A failed read means "gone" (ENOENT races measured); unreadable-cwd
     own processes and zombies are skipped unless pinned.
   - **Queued work:** every socket in `/proc/net/unix` whose path has the layout
     `<dir>/tsp/slot<N>.sock` (`remote/mod.rs:49`) **and** is owned by us (`[[ -O path ]]` — the
     file has no uid column; `-O` measured true for ours, false for root's) is read with `tsp -l`;
     each **queued** row contributes its mask = the token two after the `wrapper-<hex>.sh` token
     (measured in queued, running and finished rows, lines up to 552 chars, no truncation with or
     without a tty/`COLUMNS`; parse from the command side, never by column index — the Output column
     is `(file)` for queued rows and a path otherwise). **A failed `tsp -l` on a qualifying socket
     refuses** (`Error`). Other paths are ignored.
   - **Accounted for:** a wrapper whose argv[2], or a pinned process whose **cwd equals** a job-dir
     token (exact equality, as (l)'s cwd filter), matches a `running` row of **this slot's own
     daemon**; and this daemon's own queued rows. **Blocks:** any other candidate whose cores
     intersect the slot's mask. `NoDaemon` on this slot → every intersecting candidate blocks;
     `Error` → the submit refuses. Refusals name the blockers. Until 5.4 a refused submit fails with
     the blockers named (m2); it wrote no claim (3.3.7), so it stays retryable.
   - Cost ≈ 0.65 s with bash builtins vs 1.2 s forking per pid (measured) — the builtin form is
     required.
   - Negative controls: a submit to a slot with a running job of the same slot is accepted; the same
     job with its row removed is refused; a queued row on another own socket with an overlapping mask
     refuses; a pinned `sleep` outside any root with an overlapping mask refuses; an unowned socket
     matching the layout is ignored.
   - **Residuals:** processes of **other accounts** on the same host are not checked (Anton's own-uid
     decision; M6 closed for one account only); a user's own pinned process whose cwd is a running
     job's dir counts as accounted for; a late queued row left by a withdraw that raced an enqueue
     blocks other profiles with an overlapping mask until it runs (its wrapper then exits at step 2);
     the (l) residual (daemon dies between check and submit) stays.
10. **Every script 5.3 sends through ssh stdin follows n item 11.**
11. **Propagation.** Superseded by this item and marked where they stand: m1's hand-listed fetch set,
    m3's per-profile slot scope and its collect-based mechanism (item 9: one scan inside the submit
    call, over the account), ROADMAP 5.3,
    `modules/execution-backends.md`, the `FetchPolicy` doc comments (the implementer updates code
    comments in the same change), and `modules/group-export.md` (item 6's temp-name skip and the
    shared pattern list).
12. **Measured for this item:** probes 5.3a, 5.3b, 5.3c. **Open questions:** what `timeout` does
    to a hung `tsp` client inside the lock; whether rsync's remote receiver leaves a temp file after a
    laptop-side kill (upload direction); SIGKILL of the lock holder on the server itself; a daemon already running during a second submit (lock inheritance
    was measured for a fresh daemon); `tsp -l` lines longer than 552 chars; real mpirun/ORCA rank
    allowed-lists and masks like `0,2,4-5`; scan cost with hundreds of own-uid processes; a master
    dying mid-session; whether `orca_2json`/`orca_plot` on the laptop read a `.gbw` from the server's
    ORCA when the recorded versions differ; `--timeout`, `ServerAlive` on a dead link and rsync rc
    23/24/30 under a real drop.
13. **Part A2 amendments** (2026-10-05). They refine items 3.2, 3.3, 3.4, 6 and 7 of (o), n7, and
    (l)'s upload name; nothing else changes. Accepted after DESIGN review (PASS WITH FINDINGS, findings
    applied) on 2026-10-05.
    1. **The wrapper is named by its sha, never by a path** (Anton; closes a gap: 3.3.3's list had no
       wrapper, but 3.3.8 runs it). The submit's NUL list is: job dir, root, slot socket, slot mask,
       ORCA path, **the wrapper's sha256** (exactly 64 lowercase hex digits; anything else refuses at
       the value check), then two values per file — **6 + 2n**. The script itself builds
       `<root>/bin/wrapper-<sha>.sh`; in step 2 (inside the lock, before the claim) it requires that
       path to be a regular file equal to its `realpath` (so neither the file nor `<root>/bin` is a
       symlink) and its `sha256sum` to equal `<sha>`, else it refuses (`refused`, never
       `failed-after-claim`). Every child of this check (`realpath`, `sha256sum`) runs under `timeout`
       within MED-B's budget (sum < 40 s). The client never sends a wrapper path, so the enqueued argv
       cannot point outside `<root>/bin/`.
       - **Wrapper upload precedes the submit** *(extended to cancel, collect and `tsp/`, o14.1/14.3)* (part of 3.2, outside the lock): the same read-only
         call that asserts 3.2's realpath shapes also asserts `realpath <root>/bin == <root>/bin` when
         it exists and reports whether `<root>/bin/wrapper-<sha>.sh` already hashes right; if not, the
         wrapper is uploaded per (l) — a unique temp name in `<root>/bin/`, then `rename` — so two
         aliased profiles uploading at once cannot clobber each other.
       - Negative controls: a wrapper whose bytes differ from its name refuses; a symlinked wrapper
         refuses; `<root>/bin` a symlink to a directory holding a correct-bytes wrapper refuses; a 6th
         value that is a path, or not 64 lowercase hex, refuses at the value check; an absent wrapper
         is `refused`, not `failed-after-claim`.
       - **Residual:** the check binds the bytes at submit time only; tsp opens the path at dequeue,
         possibly hours later. A later replacement or deletion is excluded only by (l)'s
         never-overwrite rule and by nothing being deleted in 5.3. **A future `bin/` cleanup must keep
         every wrapper referenced by a queued or running row.** A wrapper deleted anyway surfaces as
         a wrapper that never starts (the `NeverStarted` family), not as a refusal.
    2. **Upload names carry the full sha** (orchestrator; acknowledged by Anton 2026-10-05). (l)'s
       `<root>/bin/<name>-<sha256 prefix>.sh` is narrowed to the **full 64-hex** sha256 — the form the
       code already uses (`scripts.rs` `upload_path`) and the submit now requires. The slot scan's
       "ours" shape (`wrapper-<hex>.sh`, item 9) is unchanged and matches it.
    3. **`KillUserProcesses` has its own outcome** (Anton; item 7/n7 needs a signal that is not free
       text). **Every** result of the step-2 `busctl` other than rc 0 + exactly `b false` — `b true`,
       other output, any rc ≠ 0, a `timeout` (rc 124) — is reported as `refused-kup <len>` with the
       evidence (`rc`, stdout verbatim, first stderr line) instead of `refused`. Like `refused`, it
       means nothing was claimed and the job stays retryable. The parser maps it to its own
       `SubmitReply` variant and treats a `refused-kup` whose evidence is rc 0 + `b false` as a
       protocol error (rule #9). **Only that variant** sets `verified_at` to NULL; Part B never
       matches a refusal's text. Negative controls: a stub `busctl` printing `b true` yields the
       variant; a stub `busctl` exiting 1, and one sleeping past its timeout, yield it too; a plain
       `refused` whose text starts `KillUserProcesses:` does not; Part B: a non-KUP refusal leaves
       `verified_at` set.
    4. **"Lock busy" lists the lock file's openers, not "the holder"** (Anton). Refines 3.3.1's last
       bullet. The refusal keeps the step-prefix convention — shape `lock busy: lock file open in:
       <pid> <cmdline> …` (a shape, not a byte literal) — and lists every own-uid process with the
       lock file open: the holder, any other waiting submit, and a daemon that leaked fd 9 (5.3c C2,
       the case the list exists for). It does not claim which one holds the lock. If none is found
       (the holder exited before the scan), it says so.
    5. **Reply formats of the label (3.4) and listing (item 6) calls** (orchestrator; acknowledged by
       Anton 2026-10-05) were left open by (o); A2 defines them and `modules/remote-jobs.md` is their
       spec of record (a wire format between our own script and parser is a module interface, like
       o6's "the code is the list"). The decision-weight rules stay here: both follow item 10 (n-11
       framing, echo of every value), both carry the raw evidence and Rust re-derives the result
       (rule #9): the label parser re-checks the `/proc/net/unix` evidence and counts a row only on a
       whole-token match of the job dir; the listing describes a symlink by its `readlink` target,
       never following it, and Rust re-derives every selection from the shared pattern list.
    Not changed: item 9's "zombies are skipped unless pinned" is implemented literally. Measured on
    the laptop (kernel 6.14, DESIGN review 2026-10-05; **not measured on the server**): a pinned
    zombie keeps its `Cpus_allowed_list` and its cwd reads ENOENT, so it is never accounted for and
    blocks its cores until reaped. Consequence: a transient zombie of the slot's own running ORCA
    child refuses a same-slot submit as "slot busy" (retryable) until reaped — item 9's control "a
    running job of the same slot is accepted" must not leave such a zombie, or it is flaky.
14. **B1 amendments** (2026-10-05; forks found by the B1 Part A implementer, decided by Anton).
    They refine (l)'s transport and upload rules, items 2, 3.1–3.3 and o13.1, and n 11; nothing else
    changes. Accepted after DESIGN review (PASS WITH FINDINGS, findings applied; MED-3 decided by
    Anton) on 2026-10-05.
    1. **Uploaded job scripts are run through one stdin trampoline** (Anton, F1). *(Delivery readies the server the same way, o22.4.2.)* (l) uploads the
       cancel script — and, new here, `collect.sh` — and gives them positional arguments, but
       `ssh host cmd args` re-parses the joined string in the remote login shell ((l)'s own re-parse
       point). Every call of an uploaded script (`cancel`, `collect`; the wrapper stays tsp's) is one
       `bash -s` call whose stdin is the trampoline (head + body, n 11 framing, every value echoed per
       item 10) followed by the NUL list `<root> <name> <sha> <args…>`. The per-job values stay
       positional arguments of the static scripts ((l) holds); only the reader of the NUL list
       changes — it is the stdin-fed trampoline, not the uploaded script. The trampoline:
       - checks `<root>` against (l)'s one path rule, `<name>` against a **closed allow-list**
         (`cancel`, `collect`), `<sha>` = exactly 64 lowercase hex;
       - builds `<root>/bin/<name>-<sha>.sh` itself and requires a regular file equal to its
         `realpath` whose `sha256sum` equals `<sha>` (o13.1's check; each child under `timeout -k`);
         an absent file is a distinct "not installed" refusal;
       - runs it as `timeout -k 1 <N> bash <path> <args…> </dev/null` (N per script, stated in the
         module page; a hung `tsp` inside cancel/collect is thus bounded — B0 measured `timeout -k`
         ending a hung client), with stdout and stderr captured into temp files, each capped below
         the laptop's `MAX_OUTPUT_BYTES` (1 MiB);
       - replies with length-framed `rc`, `stdout`, `stderr` records and exits 0 whenever its reply is
         complete, whatever the script's rc (the script's rc lives in the record). The collector's
         snapshot is the verbatim payload of the `stdout` record; Rust unwraps it before
         `parse_snapshot`.
       None of **these calls'** values passes through the remote login shell (the rsync upload's remote
       path still does, made safe by (l)'s path rule). Residual, as o13.1: the sha binds the bytes only
       until `bash` opens the file; (l)'s never-overwrite rule closes that window.
       - **Prepare and install** (o3.2, o13.1) cover all three uploaded scripts (wrapper, cancel,
         collect), each by its own sha, plus `<root>/tsp/` (14.3).
       - **Withdraw's sequence** (item 2), with no `verified_at` gate (n 6a): prepare → install if any
         script is missing → prepare again as post-condition → one call that runs `mkdir -p <job dir>`
         and then re-asserts o1's shapes (`realpath <job> == <job>`, `realpath <job>/.. ==
         <root>/jobs`) **after** the `mkdir`, in the same call → cancel via the trampoline → collect
         via the trampoline → `classify`.
       - Negative controls: `<name>` = `wrapper` (outside the allow-list; it would start ORCA outside
         tsp) refuses; a sha that is not 64 lowercase hex refuses; a script whose bytes differ from its
         sha refuses; a symlinked script or `<root>/bin` refuses; a missing script is "not installed";
         a script exiting non-zero yields a complete reply with
         that rc; an argument containing `'`, `$`, a space and a newline reaches the script
         byte-identical (observed with a test-only allow-list entry that echoes its argv — `cancel`
         and `collect` themselves reject such values with `valid_path`).
       - Observation, not a control: a script that reads stdin gets EOF — the `bash -s` read loop drains
         stdin before the script runs, so removing `</dev/null` cannot turn a test red (verifier, B1);
         `</dev/null` stays as defence in depth against a future framing change.
    2. **The remote input's `%pal` is aligned downward only** (Anton, F2 + DESIGN MED-3; domain rule
       #8, `orca/performance.md` "Memory ceiling on nprocs"). On **every attempt** (first submit and
       each retry) the input is derived from `jobs.input_content` (the DB keeps the user's original)
       with the **attempt's** profile mask: `nprocs = min(the input's %pal nprocs, the number of
       distinct CPUs in core_mask)`; an input with no `%pal` gets the distinct-CPU count, as a local
       run does. A deliberately small `%pal` is never raised (raising it multiplies the memory of a
       per-rank `%maxcore`; 5.5's preflight owns `%maxcore`/RAM/disk). The result is written into the
       local job dir **before** `upload_expected`, so the local dir = the uploaded bytes = the hashed
       list and o3.3.4's server-side set check binds it. A change is announced visibly, like the local
       `[OrcaStudio] %pal nprocs aligned to N` notice. Post-condition (rule #9): the rewritten input
       holds exactly one `%pal` directive stating that N. Negative controls: `%pal nprocs 48 end` on a
       4-CPU mask uploads `nprocs 4`; `%pal nprocs 2 end` on a 12-CPU mask uploads `nprocs 2`; a retry
       after a mask change uploads the new N. Open (not measured; the local path shares it): which of a
       `%pal` block and a `PALn` simple keyword ORCA honours when both are present — `align_pal_nprocs`
       rewrites only the first `%pal`.
    3. **`<root>/tsp/` is created by the install call** (Anton, G2): `mkdir -p <root>/bin <root>/tsp`,
       then both asserted equal to their `realpath`; the prepare call asserts `tsp/`'s realpath too.
       What real tsp does when the socket's directory is missing is **not measured** (rule #10; probe
       5.2b had the parent present) — the B4 live run records it.
    4. **A failed attempt leaves the row `Queued` with its coordinates and the reason in
       `error_message`** (Anton): every failure after the o3.1 persist — prepare, install, upload,
       `refused`, `refused-kup`, `failed-after-claim`, a transport error — keeps `status = queued` and
       the coordinates; the label call (3.4) then decides "not on the server" / "submit interrupted" /
       the classifier's. `Enqueued` clears `error_message`. A failure **before** the persist (run
       target, `MAX_UPLOAD_FILES`, a bad name) leaves the **row** untouched (the local job dir may
       already be written; it is rewritten on the next attempt).
    5. **The local cancel path refuses a live remote job** (Anton) *(still true for the local path; the remote arm records and delivers from 5.4a, o22.3)*: `local_backend::cancel` refuses a
       non-terminal job with coordinates **at function entry**, before any branch, so no caller can
       reach a local kill or a local `Cancelled` for a remote job *(holds under concurrency through
       `cancel_job`'s guard, o15)*.
    6. **Propagation** (marked where they stand): (l)'s transport rule and upload rule; item 2's
       withdraw; items 3.1 and 3.2; o13.1. Moving with B1 (code and module pages):
       `modules/remote-jobs.md` and `modules/execution-backends.md` ("fork F1 … not decided"), the doc
       comment on `ssh_backend.rs`'s withdraw, the "input verbatim" comment in `ssh_backend.rs`, and
       `install.sh`'s comment on the tsp directory.
15. **B1 Part B amendment** (2026-10-06; found by the B1 Part B implementer, agreed by the CODE
    verifier, decided by Anton). It refines item 4's in-flight guard and completes o14.5 under
    concurrency; nothing else changes. Accepted after DESIGN review (PASS WITH FINDINGS, findings
    applied; MED-3 decided by Anton) on 2026-10-06.
    1. **The guard covers every operation on one job that reaches the server** *(remote cancel records without it and delivers under it; Undo takes none — o22.3)* — submit to a server,
       retry, withdraw, label, collect, fetch — **plus cancel**, local or remote *(the read-only log
       poll excepted, o16.5)*. Cancel needs it
       because of a race item 4's list leaves open: cancel reads a draft (dispatch picks the local
       backend; `local_backend::cancel`'s o14.5 entry check sees the draft and passes), a concurrent
       remote submit persists it as `Queued` with coordinates (o3.1), and the local cancel's later,
       separately locked status read sees `Queued` and records a local `Cancelled` on a live remote
       job. o14.5's entry check alone does not close this; the guard, claimed before the job is read,
       does *(the read that acts; o21's pre-check before it only refuses)*. A local `queued`/`running` job's id is otherwise claimed only for the instant a remote
       command takes to refuse it (the remote commands claim before they read; the B2 poller polls
       remote jobs only), so a local cancel is at worst refused with a conflict error, never
       misdirected. A cancel of a draft that a remote submit holds is refused, by design.
    2. **Two operations stay outside it, each decided atomically under the DB lock instead:** a
       **local submit** (its draft-status check and the remote persist's `WHERE status = 'draft' AND
       remote_host IS NULL` are each one locked step; the loser is refused) and, **until B2**,
       **delete** (`delete_job_conn` decides under one lock and refuses a non-terminal remote job,
       o2). That exclusion is safe only while no guarded operation writes to a job after it turns
       terminal; the B2 poller may (a results write after the terminal status, a manual fetch retry), so
       **B2 makes `delete_job` claim the guard too, refusing a busy job** (Anton, MED-3), with a
       negative control.
    3. **Shape:** the guard lives in memory and resets on launch (item 4). A command claims it before
       any work on the job — the remote commands in `guarded_blocking` before `spawn_blocking`,
       moving it into the task; `cancel_job` as its first statement *(amended by o21: a live remote
       job is refused by a read-only pre-check before the claim)* — and holds it until that work
       ends; a busy job is **refused** with a conflict error. The poller (B2) uses a non-blocking
       claim and **skips** a busy job. The DB lock is never held across an ssh or rsync call.
    4. Negative controls (B1 Part B, `wiki/log.md` 2026-10-06): the guard dropped at the start of the
       blocking work; the claim moved inside the task; a command bypassing the guarded path; cancel's
       claim moved below its job read — each turns a test red. Cancel's is a source-order pin (no
       `AppHandle` test harness), a weaker kind of evidence than a behavioural race test *(superseded by o21.5:
       behavioural tests through `cancel_job_guarded`, the pin retargeted to the guarded read)*.
    5. **Propagation:** item 4's guard bullet and o14.5 (marked); `modules/execution-backends.md`'s
       guard section; the header comment of `src-tauri/src/in_flight.rs`; ROADMAP B2 (delete joins the
       guard).
16. **B2: the remote live log is pushed** (2026-10-06; fork raised at B2 start, decided by Anton).
    It refines item 4's `poll_log` bullet, item 7's consumer and o15.1's guard scope; it adds one
    command (`watch_job_log`) and one event (`job:log-reset`), and it settles the earlier plan to
    flip the view from push to pull: the view stays push. Accepted after DESIGN review (PASS WITH
    FINDINGS, findings applied; MED-3 and HIGH-2 decided by Anton) on 2026-10-06.
    1. **One live path for both backends.** The poller turns a remote job's `poll_log` chunks into
       the **existing** `job:log` and `job:convergence` events, with the local payloads and the local
       emitters (`local_backend::emit_log` / `emit_convergence`, made `pub(crate)` with their
       payloads, as `emit_status` already is), so the view listens to the same events whatever the
       backend. Per job it keeps a `LineAssembler` (item 7: raw bytes in, complete lines out, split
       UTF-8 carried) and a `ConvergenceParser` fed line by line, as the local tail does. One poll is
       one batch (up to item 7's `CAP`), not the local 50-line / 100 ms batching. Rejected: the view
       pulling `poll_log` itself — a second code path in the view for remote jobs and a second
       convergence parse outside Rust.
    2. **The view says when it is open.** A command `watch_job_log(id, open)` keeps an in-memory
       count of open views **per job id, whatever the backend** (empty on launch, like item 4's
       state). Only the poller decides what to poll: the log of a job is polled every 2 s while its
       count is > 0 **and** it is remote **and** non-terminal **and** no fetching outcome has been
       classified for it (16.6). So a draft watched before its remote submit is polled once its
       coordinates exist, a watched local job gets no ssh call, and an open on a terminal job creates
       no state. The status tick (15 s, item 4) runs for every non-terminal remote job, watched or
       not.
    3. **Every open re-streams.** Each `watch_job_log(id, true)` increments the count and resets the
       job's log state to offset 0 (assembler and parser recreated), emitting `job:log-reset
       { job_id }` first if state existed, so every open view — new or existing — rebuilds from the
       start. A close decrements, saturating at 0 (a close with no matching open is a no-op). The
       last close drops the state, and so do the job turning terminal and its row vanishing, so a
       lost close (a webview reload, an IPC reorder — not measured) leaks at most until the job ends.
       Streaming from 0 is the remote job's backfill: nothing is downloaded before the fetch, and
       `read_job_output` returns an empty list while there is no local `output.out`. **Catch-up:**
       while a poll returns exactly `CAP` bytes, the job's next poll follows without the 2 s wait,
       still one call at a time and bounded by the file's size. Memory per job stays bounded: one
       `CAP` chunk, the assembler's carry (≤ 1 MiB) and the parser's state. **Generation:** the job's
       log state carries a generation, bumped by every open, reset and drop; the generation comes
       from one counter that never resets within a launch (it outlives a dropped state). A poll or
       drain step records it when it starts and applies its chunk only if the generation is
       unchanged — otherwise the chunk is discarded (the next poll starts from the state's offset).
       The generation check, the state update and the emit of that step's events happen under one
       mutex, and so does an open's reset and its `job:log-reset` emit (that a Tauri emit returns promptly is
       an inference, not measured; the rule holds even if it blocks briefly); the mutex is never held
       across an ssh call.
    4. **Reset** (item 7: `size < offset`): offset to 0, assembler and parser recreated, and
       `job:log-reset { job_id }` tells the view to drop the lines and convergence points it holds
       before the re-streamed ones arrive.
    5. **The log poll takes no in-flight guard** (Anton, MED-3) — the one exception to o15.1's "every
       operation that reaches the server". It is read-only and touches no state machine; guarding it
       would make Retry, Withdraw and Cancel in the watching view refuse whenever they landed inside
       a poll. Instead the loop runs each job's operations off the loop thread and **sequences, per
       job, the log poll and the status step itself** — never concurrent with each other, the status
       step first when both are due — so a log poll can never make the status step skip, and the
       guard arbitrates between the status step (label/collect/fetch) and commands, and among
       commands, as before. A slow job does
       not hold up other jobs. The log poll's ssh timeout is short (≤ 10 s, stated in the module
       page). A failed poll (transport or `PollError`) leaves the state and offset unchanged, emits
       nothing and does not touch the status. Like the rest of item 4, the log is polled only while
       the host answers. The DB lock is never held across a poll.
    6. **The tail is drained from the downloaded copy** (Anton, HIGH-2). The live stream is an
       estimate (item 7) and its convergence points are not stored (nor are a local job's: the
       local view re-parses `output.out`). Once the classifier returns a fetching outcome the remote
       log is final and is no longer polled. When the fetch has downloaded and hash-verified
       `output.out` (o6), the poller drains the job's live state from the **local** copy before it
       emits the terminal `job:status`: `read_log_chunk` from the stored offset until no bytes remain
       (the same `plan_log_read` rule; the copy is sha256-equal to the remote file), then the
       assembler's `take_partial`, each batch through the same emitters — so a watching view holds
       the whole log and every convergence point, as for a local job. Then the state is dropped. A
       fetch that fails leaves the state as it was (the row stays non-terminal, item 4's 3 strikes),
       and a watching view stays empty until a fetch succeeds. The drain runs only when state exists
       *(read: while the job is watched, o17.2)* (no watcher, nothing to emit). The "fetching outcome classified" flag lives in memory like
       item 4's state; status-first sequencing sets it again on the first tick after a launch.
    7. **Testability and B3's side.** The poller's log step is an `AppHandle`-free core that takes an
       event sink (as `status_to_emit` does), tested with a recording sink over the fake runner.
       B3's view: calls `watch_job_log(id, true)` only after its `job:log`, `job:convergence` and
       `job:log-reset` listeners have resolved (else the offset-0 chunk is lost); closes only if its
       open was sent; skips the `read_job_output` / `read_job_convergence` backfill for a **remote
       non-terminal** job (during a failing fetch a local `output.out` exists while the row is still
       non-terminal, and backfill plus re-stream would duplicate lines and convergence points).
    8. Negative controls: **parity** — a fixture `output.out` streamed through the remote path in
       `CAP`-sized and odd-sized chunks yields exactly the lines and `ConvergenceEvent`s that
       `read_convergence` gives on the same file; an unwatched job gets no `poll_log` call while its
       status step still runs; a watched local job gets no ssh call; a watched draft is polled once
       its coordinates exist; open, open, close keeps polling; a close at count 0 stays at 0; a
       second open emits one reset and the next poll asks for offset 0; a terminal job's state is
       dropped with the count > 0; a chunk boundary inside a multi-byte character yields the same
       lines as one chunk; a reset recreates the state and emits `job:log-reset` once; a failed poll
       leaves the offset unchanged; a due status step of a watched job is not skipped because of its
       own log poll; a fixture whose last lines (and an unterminated last line) arrive only in the
       downloaded copy has them emitted before the terminal `job:status`; a job with a fetching
       outcome gets no further `poll_log`; an unwatched job's successful fetch reads no log chunk; an
       open issued while a (fake) poll is blocked makes the next poll ask for offset 0 and no line of
       the stale chunk is emitted after the reset; open, a poll blocked, last close, open again,
       then the poll released — the stale chunk is discarded.
    9. **Propagation:** item 4's `poll_log` bullet and o15.1 (marked); ROADMAP B2 (fork decided:
       push) and B3 (the watch order, `job:log-reset`, no backfill for a live remote job). Moving
       with B2's code: ROADMAP 5.0's "push→pull flip rides with `SshBackend`";
       `modules/execution-backends.md` (the "push→pull flip" lines and "`poll_log` … refuse until
       the poller"); `modules/remote-jobs.md` (the poller's log step, the command, the event); the
       code comments that still predict the flip (`execution_backend.rs` module and trait docs,
       `local_backend.rs`'s `poll_log` doc); `in_flight.rs`'s header (the log poll is not guarded).
       ADR-003's "pull, not push" still holds at the backend layer; the UI stays push.
17. **B2 Part A amendments** (2026-10-06; found by the B2 Part A implementer, decided by Anton). They
    refine item 4's outcome list and o16.6's drain condition; nothing else changes. Accepted after
    DESIGN review (PASS WITH FINDINGS, findings applied) on 2026-10-06.
    1. **A classifier `Cancelled` from the poller is terminal** (Anton) *(evidence fetch first when `.started` exists: o22.6)*. Item 4 listed `Cancelled`
       among the outcomes that are only shown, the row staying `Queued`. Read literally, a withdraw
       whose reply is lost becomes a dead end — the label call then hands the job to the
       classifier, so a second withdraw is refused and cancel is refused until 5.4 — contradicting
       o3.4 ("a lost withdraw reply is not a dead end"). So the status step writes terminal
       `cancelled` (with `completed_at`) for a classifier `Cancelled` (row 4: `.cancelled`, nothing
       of the job runs), the same write `record_withdraw` makes, under one lock after re-checking
       that the row is still this live remote job; it drops the job's live-log state and emits the
       terminal `job:status`. **Nothing is downloaded** (o6 fetches only `Completed` and the `Failed`
       variants). Row 4 precedes row 6, so it **includes a job that ran and wrote `.exit_code`**
       (non-zero, unreadable, or 0 without normal termination) before the cancel: its output and
       exit code are **not** downloaded and `detect_completion` does not run; they stay on the server
       (item 5: nothing is deleted there in 5.3). In 5.3 this is rarely reachable (a withdraw follows
       only "not on the server" / "submit interrupted"); in 5.4 every running job killed by a cancel
       takes this arm, so **5.4 decides** whether a started-then-cancelled job's evidence is fetched
       while it still ends `cancelled` (open question, Anton). The withdraw's "nothing ran" message
       has the same imprecision. `Cancelling` (row 3) stays shown *(from 5.4a a status, `cancelling`, o22.5)*. Negative control: the arm
       reverted to "shown" turns its test red.
    2. **o16.6's "only when state exists" means "while the job is watched".** A view opened during
       the fetch window has a count > 0 but no state yet (no log poll runs once a fetching outcome is
       classified); the drain then creates the state at offset 0 and streams the whole verified
       local copy, so that view is not left empty. With no watcher nothing is read (o16.8's control
       holds).
    3. **Propagation:** item 4's outcome list and o16.6 (marked); `modules/remote-jobs.md` and
       `modules/execution-backends.md` move with B2's code.
18. **B2 Part B: exit leaves in-flight ssh/rsync to the OS** (2026-10-06; fork raised by the B2 Part B
    implementer, decided by Anton). Accepted after DESIGN review (PASS WITH FINDINGS, findings
    applied) on 2026-10-06. App exit joins neither the poller loop nor its step threads, so
    it never waits for a step (a download may take up to its unmeasured 300 s bound). The ssh and
    rsync children of in-flight steps run in their own process groups (`SystemRunner`) and are left
    to the OS — as the B1 remote commands already are. This is safe because every such call is
    read-only on the server or idempotent, the in-flight guard and the poller memory reset on launch
    (item 4), and the next launch redoes the step from the server's facts. A leftover rsync from the
    previous run can still rename a whole file into the local job dir **after** the new run's LIST +
    sha256 post-condition (o6), which is a point-in-time snapshot. That is content-neutral only
    because of **finality**: a fetching outcome is classified only after `.exit_code` is published,
    which `wrapper.sh` step 4 does only after ORCA has returned, right before the wrapper exits, so
    the selected artifacts on the server no longer change and both transfers bring the same bytes,
    each written to a temp file and renamed whole (`.tsp-out/` may still change but is outside the
    comparison and decides nothing; rsync temp names are not selected). A future change that lets a
    server artifact change after a fetching outcome breaks this argument. Rejected: tracking live
    process groups in `SystemRunner` and killing them on `ExitRequested` — extra shared state for no
    correctness gain. Residuals (not measured, B4): an orphaned ssh/rsync loses its 60 s / 300 s
    timeout (it was enforced by the dead process's thread; no `ServerAliveInterval`, no `rsync
    --timeout`), so on a silently dropped link it may hang for an unknown time — B4 decides whether
    to add those options; an orphaned rsync can recreate files under a job dir the new launch has
    just deleted (orphan files, not corruption); that the children survive the parent's exit is
    asserted, not yet observed; classifier row 6 does not inspect the job session, so ORCA children
    (MPI ranks, `orca_*` helpers) orphaned by a main process that died with a non-zero exit could, if
    they survive, still append to `output.out`/`stderr.log` after `.exit_code` exists — plausible,
    not seen in a run (rule #10). **Propagation:** `modules/execution-backends.md` ("On exit"),
    `modules/tauri-core.md` (startup and exit), ROADMAP B4 (the measurements).
19. **B3: the poller's per-job remote state reaches the view** (2026-10-06; fork raised at B3 start,
    decided by Anton). Accepted after DESIGN review (three rounds, each PASS WITH FINDINGS, findings
    applied; MED-1 / MED-3 / LOW-2 and round-2 LOW-d decided by Anton) on 2026-10-06. It refines item 4's "shown, with 'handled in unit 5.4'", item 4's
    bounded retries ("the reason is shown, and a manual retry is offered") and item 3.4's "not on the
    server" / "submit interrupted" offers. **The gap it closes:** the row's `error_message` already
    carries the words of every `Shown` outcome except `Indeterminate` (o19.3) (`shown_message`, with "handled in unit 5.4") and of
    every strike / stop (`Poller::strike`), and they reach the view through `job:status`. What the
    view cannot see is the **structure**: the labels of `Labelled` (which decide retry vs withdraw),
    `CheckFailed` (the host is away), and a structured "fetch stopped" flag for the retry button —
    the loop (`poller/app.rs` `spawn_step`) drops the `StatusStep`.
    1. **One in-memory record per job.** Beside `PollerMemory`, a `RemoteState` per non-terminal
       remote job: `known: Option<Known>` (`Labelled(Label)` or `Shown(Outcome)`; *amended by
       o19.11: or `Fetching`*), `check_error:
       Option<String>`, and `fetch: { strikes, stopped }` **re-read from `PollerMemory`'s strike
       counter on every apply** (Anton, MED-1; equality suppresses the emit) — the counter the bound acts on stays the
       one source; `stopped = strikes >= MAX_FETCH_STRIKES`, so the third `FetchFailed` already
       reads stopped. It lives for one launch and starts empty, like item 4's counter, guard and
       offset.
    2. **Words come from the row; structure from the record** (Anton, MED-3). The view shows the
       row's `error_message` as the text of a remote non-terminal job. While the record has no `known`
       state (no record before the first status step of a launch, or a record holding only
       `check_error`) that text is labelled **"from the last check"** beside
       "checking": it may be stale across a restart (e.g. "automatic fetches stopped" while the
       strikes have reset to 0 and automatic fetches resume), and no action button is offered from it. The record decides the badges and the
       offered actions; it carries text only where the row has none: the `check_error` reason. The
       view keeps **no** TypeScript copy of item 4's "handled in 5.4" list or of `shown_message`.
    3. **What each `StatusStep` does to the record** — one function, a `match` with **no wildcard**
       (a new variant fails to compile):
       - `Labelled(l)` → `known = Labelled(l)`, clear `check_error`;
       - `Shown(Indeterminate)` → **keep** `known`, set `check_error` to its `shown_message` (Anton,
         LOW-2: "could not be decided on this check" is a check error, not a new state). And
         `record_shown` **no longer writes the row's `error_message`** for `Indeterminate` (Anton,
         round-2 LOW-d — a code change in B3 Part A; it still re-checks the live row and returns
         `Written::Yes(None)`, so `RowChanged` keeps its meaning and no `job:status` is emitted): the row keeps the text of the last known state
         (e.g. `Lost`'s "handled in unit 5.4"), and the explanation lives only in `check_error`, so
         the view never shows one sentence twice;
       - `Shown(o)` otherwise → `known = Shown(o)`, clear `check_error`;
       - `FetchFailed` / `FetchStopped` *(amended by o19.11: they set `known = Fetching`)* → clear
         `check_error`; `known` kept (`fetch` is re-read on
         every apply, o19.1);
       - `CheckFailed(why)` → set `check_error`, **keep** `known` (a host away does not erase what
         was last seen; the view shows it as "last seen"). With no `known` yet (host away at
         launch) the record holds only `check_error` and the view says "server unreachable: why";
       - `Busy`, `RowChanged` → nothing;
       - `NotLive`, `Cancelled`, `Finalised` → **remove** the record (the terminal status arrives
         as `job:status`).
       `clear_strikes` (the manual `retry_remote_fetch`) refreshes `fetch`, so a retry whose own
       step is `CheckFailed` no longer reads stopped. **Any** `retry_remote_submit` or
       `withdraw_remote_job` *(removed by o22.7; its removal moves to `cancel_job`'s remote arm)* outcome — success, refusal or transport failure — removes the record
       (the next status step re-derives it): inside the guarded closure the removal runs **first**,
       whatever the inner `Result`, and only then is that result propagated (a `?` must not skip it).
       A refusal by the in-flight guard itself (`Conflict`, before the closure) leaves the record
       alone — writing without the guard would break o19.4; so no stale `NotOnServer` keeps offering a retry the
       server would refuse. The existing `label_remote_job` command does not feed the record and the
       B3 view does not call it.
    4. **Every write holds the job's in-flight guard** (review HIGH-1). The record change and its
       emit happen **before** the guard is released: in the loop inside the status step (a wrapper
       around the step's body applying it while the guard is alive), never in `spawn_step`, which
       sees the step only after the guard is gone; in `retry_remote_fetch` inside its guarded
       closure; for retry and withdraw inside the `run_guarded` work closure, not after its
       `.await`. So every writer of a job's record holds that job's guard, writes are serialised per
       job, and a step cannot land after a later command — e.g. a `Labelled(NotOnServer)` step
       finishing after a withdraw made the row `cancelled` cannot resurrect the record. The one
       unguarded writer is `sweep` (5), which touches only non-candidates and skips jobs with a step
       in progress.
    5. **Every exit path removes, and every removal emits** (review MED-2). `sweep` filters the
       records by the current candidates (keeping jobs with a step in progress), so a job that left
       the candidates without a step on it (another writer, a `RowChanged`) loses its record. All
       removals — the step mapping, `sweep`, the commands — go through the one emitting function;
       none drops a record silently (e.g. inside `forget`). Removing an absent record emits nothing.
       **Lock order** (review round 2 MED-A): the record map's lock R before `PollerMemory`'s M;
       nothing takes R while holding M or the database lock. `retry_fetch` calls `clear_strikes`,
       releases M, then applies; `sweep` snapshots the in-progress ids from M, releases it, then
       filters R (never inside `memory.retain`'s closure). The sink never calls back into R. Emitting
       under R relies on a Tauri emit returning promptly — the same inference as o16.3, measured in
       B4.
    6. **Push, read, and order** (review HIGH-2). Every record change (set or removal) takes the
       next value of one launch-wide sequence counter that never resets (like o16.3's generation),
       and is emitted under the record map's mutex, so emit order equals change order on the Rust
       side. The event is `job:remote-state { job_id, seq, state: RemoteState | null }` through a
       new `PollerSink` method (the `AppSink` emits it, its name pinned like `LOG_RESET_EVENT`; the
       recording sink records it). A command `remote_job_state(id) -> { seq, state: RemoteState |
       null }` returns the current record and the **global counter's current value**, taken under R,
       whatever the state (so any event emitted after the read has a greater seq by construction). A change that
       leaves the record equal emits nothing (a repeated `Shown(Running)` every 15 s). The view
       attaches its `job:remote-state` listener **first**, then reads `remote_job_state`, and
       applies any value — event or read — only if its `seq` is greater than the one it holds. So
       neither an older read nor an older event delivered late overwrites a newer state, whatever
       order Tauri delivers an event and a command response in (unmeasured — open question below).
    7. **What the view offers** (B3) *(from 5.4a: Cancel replaces withdraw and is offered for every live remote job; Retry is disabled while the last contact failed — o22.7, o22.8)*: `Labelled(NotOnServer)` → retry submit + withdraw;
       `Labelled(SubmitInterrupted)` → withdraw only; `Shown(o)` → the row's text (which already
       says "handled in unit 5.4" where an action is needed); `fetch.stopped` → a "retry fetch"
       button (`retry_remote_fetch`); `check_error` → "server unreachable / not decided: <why>"
       beside the last-seen state; no record → "checking". Cancel and delete stay refused by the
       backend for a non-terminal remote job (o2); the view shows the refusal text.
    8. **Not in the record.** A row with partial coordinates (B2 F-3, set aside by `candidates`
       before any step) gets no record; the view derives "corrupt coordinates" from the row itself
       (some but not all of `remote_host` / `remote_job_dir` / `remote_socket` set). The record is
       not persisted: it is a view of the server's facts, re-derived by the next status step after a
       launch (item 4).
    9. Negative controls (each must be shown to bite):
       - `Labelled(NotOnServer)` → `CheckFailed` keeps the label (removing the keep branch → red);
       - `Shown(Running)` → `Shown(Indeterminate)` keeps `Running` and sets `check_error`;
       - `Shown(Lost)` → `Shown(Indeterminate)` leaves the row's `error_message` equal to `Lost`'s
         text (restoring the `Indeterminate` write in `record_shown` → red);
       - `Busy` does not overwrite; `Finalised`, `Cancelled` and `NotLive` each remove;
       - the guard scope, behavioural: when `Poller::status_step` returns, the record and the
         recording sink already show its change; a following withdraw / `retry_remote_submit` removes
         it and later ticks do not set it again — moving the apply out of `status_step` leaves the
         record unset at return (red);
       - the guard scope, source pin (like `every_handed_out_step_is_ended`): `spawn_step` contains no
         record apply; `retry_remote_submit` and `withdraw_remote_job` *(removed by o22.7; `cancel_job`'s remote arm instead)* call the removal inside their
         `run_guarded` closure, before propagating the inner result;
       - the lock order, deterministic source pin: the bodies of `clear_strikes` and
         `PollerMemory::retain` never reference the record map, and `sweep` does not filter R inside
         `memory.retain`'s closure (a looped concurrency test may be added, stated as probabilistic);
       - `sweep`: a job dropped from the candidates with no step loses its record and emits `null`;
         a silent `forget`-style removal turns it red;
       - one `job:remote-state` per change, none for an unchanged repeat; seqs strictly increase;
       - a `retry_remote_fetch` whose step is `CheckFailed` leaves `fetch.stopped == false`;
       - the view: an event with a lower seq delivered after the read leaves the read's state;
         an event emitted between the listener's resolve and the read's resolve with a higher seq
         wins; dropping the seq comparison turns one of them red.
    10. **Propagation:** ROADMAP B3; `modules/execution-backends.md` (command table:
        `remote_job_state`; the `PollerSink`/`AppSink` section: the new sink method and the pinned
        event name); `modules/remote-jobs.md` ("Not built yet"); `modules/tauri-core.md` (command
        list); the `PollerSink` doc comment in `poller.rs`; Open questions (the cross-channel order). With the
        `Indeterminate` change: `modules/execution-backends.md` ("every other outcome writes
        `shown_message`"), and in `poller.rs` the `shown_message` doc ("What `error_message` shows")
        and the module doc's step 4 ("`error_message` says what the server reports").
    11. **B3 Part A amendments** (2026-10-06; found by the Part A verifier, decided by Anton).
        1. **`Known::Fetching`** (verifier MED; also the implementer's point A). o19.3's
           "`FetchFailed` / `FetchStopped` → `known` kept" let a job last seen `Shown(Running)` whose
           fetch then failed show "running" beside the row's "fetch failed" text, and a job with no
           `known` label its fresh strike text "from the last check". Amended: `FetchFailed` and
           `FetchStopped` set `known = Fetching` (the server reports a fetching outcome; the download
           is failing) and clear `check_error`; `fetch` is re-read as before. A later `CheckFailed`
           keeps `Fetching` like any known state. The view shows "fetching results" with the fetch
           state; since `known` is set, the "from the last check" label (o19.2) does not apply.
           Negative control: `Shown(Running)` → `FetchFailed` reads `known == Fetching`; keeping
           `known` turns it red.
        2. **A retry or withdraw whose row cannot be loaded** (verifier LOW): `run_guarded` loads
           the job before the work closure, so a missing row skips the closure's removal. Accepted:
           the row is gone, so the job is not a candidate and the next `sweep` removes the record
           with a `null` emit (o19.5).

20. **B3 Part B amendments** (2026-10-07; found by the B3 Part B implementer, decided by Anton;
    DESIGN review: two rounds, PASS WITH FINDINGS each, applied).
    1. **The run-target choice is remembered app-wide, not per project.** o5's "per project" has no
       referent: the app has no project entity (no projects table, no per-project preferences).
       Remembered is **the target of the last Create & Run**, one app setting `last_run_target`
       (named apart from `ServerProfile.run_target`), read and written through the existing
       `get_settings` / `set_setting` (a plain upsert, no key validation). Rules:
       - **Encoding:** a profile **id** (a rename keeps the choice; delete-and-recreate loses it);
         `""` is Local. No stored value is Local.
       - **Resolve on load** (pure, `resolveRunTarget`): the stored id is preselected only while it
         is among the current `runTargetOptions` **and** enabled; otherwise Local. A failed
         `get_settings` reads as nothing stored → Local; a failed `list_server_profiles` gives an
         empty list → Local, and its error is shown.
       - **The fallback never writes.** A stored profile that is disabled today is preselected again
         once it becomes a run target again. Nothing "self-heals" the stored value.
       - **When it is written:** on Create & Run, when the job is created and handed to the view,
         regardless of the submit's outcome (Anton: a retry after an unreachable server keeps the
         choice). Changing the select writes nothing; "Create Job" writes nothing. Accepted side
         effect: if a load failed and the user runs on Local, the stored server choice becomes `""`.
       - **Load race:** a choice the user makes while the load is pending wins over the stored value.
       Submit re-checks the run-target predicate in Rust (n2), so a preselect that went stale between
       load and submit is refused there. Rejected: keying the choice per destination group.
    2. **A run target is chosen only through Create & Run** (Anton). The Jobs table's Run on a draft
       and the re-opt fan-out submit locally (`target: null`) and **say so**: the table button reads
       "Run locally", and **every** fan-out that submits without a target (re-opt, 2D surface
       scan, scan profile, NEB band, connectivity — CODE verifier L5, Anton) says its jobs run
       locally where the user sees it before or as it runs: re-opt in its queued message; the
       refines that navigate straight to the child (scan profile, NEB band, 2D surface) next to
       their button or in their hint; connectivity in its pending line. These labels are
       checked in Anton's live gate, not by a unit test. Choosing a target there is
       recorded as open in `modules/remote-jobs.md` ("Not built yet") and ROADMAP; not built in 5.3.
    3. **Withdraw asks for confirmation like Delete job** (Anton) *(the confirm moves to the one remote Cancel, o22.7; only its first Cancel — Retry cancel and Undo have none, o22.14.2)*: the Tauri plugin-dialog `confirm`,
       the app's pattern for destructive actions (never `window.confirm`, unreliable in WebKitGTK).
       Declining sends nothing. `withdraw_remote_job` is called only if `withdraw` is still among the
       view's offered actions after the dialog closes (a record change during the dialog must not
       fire a withdraw from a stale offer): the check reads the view's **current** actions (a ref or
       the latest state) after `await confirm(...)` resolves, never the offer captured by the click
       handler *(for Cancel: re-read from the backend, o22.14.14.1)*. A `confirm` that rejects (a plugin error) counts as declining.
    4. **Negative controls** (each must turn its test red):
       a. `resolveRunTarget`: a stored disabled profile → Local, a deleted one → Local, an enabled one
          → preselected, a failed settings read → Local; dropping the enabled check turns the
          disabled case red. At the load path (not the pure resolver, which cannot write): a load
          with a stored disabled profile makes no `set_setting` call — a fallback write turns it red;
          a choice made before the load resolves survives it — removing the "touched" guard turns
          it red.
       b. Changing the select writes nothing and Create & Run writes the id (`""` for Local). Not
          unit-testable without rendering New Job (Monaco, 3Dmol; no screen harness): **checked in
          Anton's live gate**, not by a negative control.
       c. Withdraw: declining sends nothing, confirming sends exactly one call, a rejecting
          `confirm` sends nothing, and an offer withdrawn **while the mocked `confirm` is still
          pending** sends nothing; bypassing the confirm, or re-checking the click-time offer instead
          of the current one, turns it red.
    5. **Propagation:** o5 (marked); ROADMAP B3 ("per project" marked); `modules/remote-jobs.md`
       ("The view", "Not built yet"); `modules/frontend.md`.
21. **Cancel refuses a live remote job before the guard** *(the refusal is superseded by o22 in unit 5.4a; the pre-check read stays)* (2026-10-17; found in Anton's live gate of
    B3 Part B, decided by Anton). It refines o15.1, o15.3 and o15.4 for `cancel_job` only; nothing
    else changes. Accepted after DESIGN review (PASS WITH FINDINGS, findings applied) on 2026-10-17.
    1. **Symptom and cause.** While a remote job's server was unreachable, Cancel answered
       `conflict: an operation on job … is already in progress` on most clicks (Anton's live gate,
       `wiki/log.md` 2026-10-17), never the real reason. The poller's status step holds the job's
       guard through its ssh calls: the label call for a `queued` job, the collect for both. A connect
       that hangs (packets silently dropped) costs up to `ConnectTimeout` (10 s, `remote/ssh.rs`), a
       collect up to its run timeout. The step's period is 15 s, stamped at its start, so a 10 s hang
       keeps the guard busy about ⅔ of the time. A refused connection or an unresolvable host fails
       fast. `cancel_job` claimed the guard as its first statement (o15.3), so the conflict hid
       o14.5's refusal ("remote cancel arrives in unit 5.4").
    2. **Rule.** Before it claims the guard, `cancel_job` reads the row once under the DB lock (one
       `get_job_conn`). If that read fails (not found, a lock error), its error is returned as is.
       If `refuse_if_remote_live` refuses the job — remote and `queued`/`running`, the very predicate
       of o14.5 — `cancel_job` returns that refusal **without claiming the guard**. In every other
       case it claims the guard, reads the row again and dispatches, exactly as o15.3 says. That
       guarded path, and o14.5's entry check in `local_backend::cancel`, are unchanged.
    3. **Why o15.1's race stays closed.** The pre-check **only refuses; it never acts**. Its premise:
       **a remote-live row is monotone** — it can only turn terminal (withdraw, the poller's
       finalise) or be deleted, never back to a draft or a local job. No production code clears the
       coordinates; the v20 CHECK keeps them all set or all NULL. The only way to gain coordinates is
       the draft→`queued` persist, which is o15.1's race itself. There the pre-check reads a draft,
       passes, and the guarded path decides as before. A pre-check that sees a live remote job
       refuses; if the job turns terminal right after that read, the refusal is stale but harmless:
       a later Cancel takes the guarded path, which never acts on a terminal remote job.
    4. **Lifetime.** Unit 5.4 replaces the refusal with remote cancel. While the server is
       unreachable — inside or outside the window — that is Decision i's pending cancel (its
       2026-10-03 amendment; "inside the window" clarifies, it does not change). The cancel is
       stored and runs first on reconnect; the job's status does not change meanwhile; the view shows
       "cancel pending". Anton re-confirmed this on 2026-10-17 against the alternative of a local
       "forget" (mark the row cancelled while the job may still run on the server), which stays
       rejected: Decision c, and rule #9. **Left to 5.4's amendment:** how a pending cancel meets
       the guard. *Lean, not decided:* recording the intent needs no server call and is not refused
       while the poller holds the guard. Retry and Withdraw meet the same conflict while the server
       is unreachable; their true answer is a transport failure anyway, so 5.4 decides them too.
    5. **Seam and negative controls** (behavioural; o15.4's cancel pin was only source order).
       - **Seam:** `pub(crate) fn cancel_job_guarded(in_flight: &InFlight, db: &DbState, id: &str,
         dispatch: impl FnOnce(Job) -> Result<(), AppError>) -> Result<(), AppError>`, the twin of
         `delete_job_guarded`: the pre-check (a named helper), then `in_flight.acquire(id)?`, the
         guarded locked read, then `dispatch(job)` with the guard held. `cancel_job` passes the
         backend dispatch (`Backend::for_job(app, &job)?.cancel(…)`).
       - **Tests**, each with the job's guard held unless stated: (a) a remote `queued` row, then a
         `running` one → the o14.5 refusal, not `Conflict`, and `dispatch` is not called; (b) a local
         draft → `Conflict`, no dispatch; (c) a local `queued` row → `Conflict`, no dispatch (the
         pre-check never acts); (d) guard free, a terminal remote row → `dispatch` gets the job, and
         inside it `in_flight.try_acquire(id)` is `None` (the guard is held while the action runs).
       - **Pin, retargeted:** the claim precedes the *guarded* read (the core body has exactly one
         `get_job_conn(`, after `acquire`; the pre-check's read lives in its helper).
       - **Controls that must turn red:** the pre-check moved after the claim → (a); the pre-check
         dropped → (a); the local dispatch moved before the claim → (c); the guard bound as `let _ =`
         → (d); the guarded read moved above the claim → the pin.
    6. **Propagation:** o15.1, o15.3 and o15.4 (marked); `modules/execution-backends.md` (the guard
       section and its Tests paragraph); the header comment of `src-tauri/src/in_flight.rs`;
       `cancel_job`'s doc comment and the o15.4 pin test in `commands/jobs.rs`; ROADMAP unit 5.4
       (pending cancel while unreachable, and its meeting with the guard).
22. **Unit 5.4a: remote cancel, the pending cancel and the guard** (2026-10-17; forks raised by the
    orchestrator, decided by Anton). This amendment gives Decision i's pending cancel (2026-10-03
    amendment) its mechanics, closes the o17 open question and **supersedes** o21's refusal and
    o2's Withdraw for remote jobs. `Lost`, re-enqueue and the pending submit are not covered.
    DESIGN round 1: FAIL (H1 + 9 MED); round 2: PASS WITH FINDINGS (2 MED + 6 LOW), applied.
    1. **Unit 5.4 is split** (orchestrator; see ROADMAP).
       - **5.4a** (this item): remote cancel, the pending cancel and `Cancelling`.
       - **5.4b**: `Lost`, the orphan sweep of row 8, the re-enqueue counter and acting on
         `ReEnqueue`, and the `Indeterminate` counter of l row 10 (Decisions d, e, l).
       - **5.4c**: the pending submit and its refusal counter (Decision m2).

       Each unit migrates only its own columns: **v21 = 5.4a's two columns** (item 2), and the
       re-enqueue counter and the pending submit move to later versions with their units *(renumbers
       the "v21" of l, m2, o1, o3.5 and o4, each marked; o23.5: 5.4c = v22, 5.4b = v23)*.
    2. **Two DB columns**, so a cancel survives a quit and a closed laptop.
       - **Columns:** `jobs.cancel_requested_at TEXT NULL` (the user asked) and
         `jobs.cancel_sent_at TEXT NULL` (a delivery is about to run `cancel.sh`; item 4.5). Both
         hold laptop time, are only displayed, and are never compared with a server time
         (Decision j).
       - **Invariant:** `cancel_requested_at` is non-NULL only on a remote job in `queued`,
         `running` or `cancelling`; `cancel_sent_at` is non-NULL only where `cancel_requested_at`
         is. The schema states it, in the v20 form of an ADD-COLUMN CHECK that names earlier
         columns (`db.rs`): `CHECK (cancel_requested_at IS NULL OR (remote_host IS NOT NULL AND
         status IN ('queued','running','cancelling')))` and `CHECK (cancel_sent_at IS NULL OR
         cancel_requested_at IS NOT NULL)`.
       - **Every terminal write of a remote row NULLs both columns in the same statement:**
         `finalize_job_conn` (the poller's finalise) and `mark_cancelled_conn` (`record_cancelled`,
         and delivery). The CHECK turns a forgotten NULL into a failed write, not a silent breach.
         Part A tests each of these sites with the columns set. `record_shown`'s
         `queued`→`running` is not terminal and keeps them. The local writes
         (`update_job_status_conn`, the local `finalize_job_conn` callers) never meet a remote row.
       - Also: `CHECK (status <> 'cancelling' OR remote_host IS NOT NULL)`.
    3. **Cancel records first, needs no guard to record, and is never refused for being busy.**
       `cancel_job` reads the row once (o21's pre-check).
       - **A remote job in `queued`, `running` or `cancelling`:**
         1. One atomic `UPDATE … SET cancel_requested_at = now WHERE id = ? AND remote_host IS NOT
            NULL AND status IN ('queued','running','cancelling') AND cancel_requested_at IS NULL`
            *(narrowed by o22.12.1: `queued`/`running` only; a `cancelling` click is the manual
            sweep retry)*.
            0 rows means the row moved on or a cancel is already pending; re-read and report.
         2. `try_acquire` the guard. **Free:** run the delivery (item 4) now, in `spawn_blocking`
            holding the guard, like the other remote commands.
         3. **Busy** (a poller step, a fetch): return **"cancel pending"**; the poller delivers at
            the job's next status step.
         - The command returns once delivery has classified (item 4.5). An evidence fetch the
           verdict needs (item 6) runs in the poller's next step, not in the click.
       - **Any other job** (local, draft, terminal) takes o21's guarded path, unchanged.
       - **o15.1's race stays closed.** A draft has no coordinates, so the `UPDATE` matches nothing
         and the guarded local path decides, as in o21.3.
         - If the remote persist lands **before** the pre-read, the `UPDATE` matches and the job is
           cancelled on its server; that is the job the user cancelled.
         - If it lands **between** the pre-read (a draft) and the guarded read, the guarded read
           finds a remote-live row. The command then **goes once to the record path** (step 1)
           instead of refusing with stale text.
       - **Undo cancel** (Anton) is one atomic `UPDATE … SET cancel_requested_at = NULL WHERE id = ?
         AND cancel_requested_at IS NOT NULL AND cancel_sent_at IS NULL`, **with no guard**, so it
         is never refused for a busy poller.
         - It is offered only while `cancel_sent_at` is NULL. Once a delivery has marked the send,
           the button is gone, with the text "the cancel may already be on the server".
         - While the server is unreachable delivery never reaches `cancel.sh` (item 4.5), so Undo
           stays available exactly when a change of mind is likely.
         - **Why this is sound:** delivery marks the send under the DB lock **before** spawning
           `cancel.sh`, and re-reads `cancel_requested_at` in that same locked step. An Undo that
           lands first wins and nothing is sent. An Undo that comes after the mark is refused.
           A lost reply after a real `cancel.sh` can no longer be "undone" locally while
           `.cancelled` sits on the server (round 1 H1).
    4. **Delivery: one function, called by the command and by the poller, always holding the
       guard.**
       - **Order.** The poller reads `cancel_requested_at` in the step's own locked read, under the
         guard. While it is set, the poller runs delivery **instead of** the normal status step.
         Delivery comes **before** the `FetchStopped` short-circuit, so delivery runs first on
         reconnect (Decision i).
       - **Steps:**
         1. Re-read the row under the lock. If it is no longer remote-live with
            `cancel_requested_at` set, stop.
         2. **Ready the server**: prepare → install if needed → prepare, as o14.1 does for submit
            and withdraw. The uploaded `cancel.sh`/`collect.sh` are sha-named, so an app upgrade
            needs them reinstalled. A not-installed or refused prepare is reported as itself,
            never as "unreachable".
         3. **`queued` → label call.**
            - "Not on the server" or "submit interrupted" → o2's withdraw tail: `mkdir -p`, then
              step 5.
            - Otherwise go on.
         4. **One collect + `classify` before any cancel** (Anton).
            - **Already ended without `.cancelled`** (row 5 `Completed`, row 6 `Failed{…}`): clear
              **both** columns (a marked but unsent cancel may precede this, item 4.5), keep the
              result, take the normal fetch path. Tell the user
              "the job finished before the cancel reached the server".
            - **Every other outcome** — `Queued`, `Running`, `Lost`, `ReEnqueue`, `Cancelling`,
              `Indeterminate` — goes on *(narrowed by o22.11.4: an outcome showing `.cancelled` on
              the server — rows 2, 3, 4, 1+`cancelled` — is delivered, nothing re-sent)*. `Indeterminate` implies no `.exit_code` (rows 5–6 precede
              row 10), and `cancel.sh` publishes `.cancelled` first and skips tsp when it cannot
              read the queue, so it is safe there (round 1 M1).
            - **Row 1 `Failed{CorruptStarted}`** goes on too (step 5's verdict) *(with `cancelled`:
              delivered, o22.11.4)*.
         5. **Mark, then send.** In one locked step: re-check `cancel_requested_at IS NOT NULL`
            (an Undo may have won), set `cancel_sent_at = now`, and release the lock. Then run
            `cancel.sh cancel`.
            - **rc 0:** the server holds `.cancelled`; the cancel is **delivered**. One collect +
              `classify` decides, never a hard-coded `Cancelled` (Decision c):
              - **row 4 `Cancelled`** → item 6;
              - **row 3 `Cancelling`** → status `cancelling` (item 5);
              - **row 2 `Completed{late_cancel}`** → the normal fetching path, so a clean result is
                kept;
              - **row 1 `Failed{CorruptStarted}`** → terminal `failed`, as l row 1 intends ("without
                `sid` the job cannot be swept; the user must see it"). The rule is keyed on the
                **snapshot**, not on the path that delivered: row 1 gains `cancelled: bool`
                (`CorruptStarted { cancelled }`), and **any** status step that sees row 1 with
                `.cancelled` runs item 6's evidence fetch and writes terminal `failed`. A click's
                delivery (which returns after classifying, item 3) is therefore finished by the
                poller's next ordinary step. The reason says processes of the job may survive on
                the server. That is consistent with rule #8:
                - a surviving wrapper keeps a `running` row on its slot's one-slot daemon, which
                  queues any new job behind it;
                - pinned orphans are seen by o9's account-wide scan, which refuses a submit whose
                  mask overlaps them;
                - once the row is terminal, the profile becomes editable while survivors may run,
                  and o9's scan still blocks overlapping cores.
              - `Indeterminate` → shown; later status steps decide, since `.cancelled` is now on
                the server.
              - **Columns:** a terminal write NULLs both (item 2). A non-terminal verdict NULLs
                `cancel_requested_at` and `cancel_sent_at` in the same locked write that records it.
            - **A transport failure, a timeout or rc 2/3** (a usage error; or fail-closed — an
              unreadable or not-a-directory job dir): **not delivered**. Both columns stay set.
              The record shows the script's stderr or the transport error. The next step tries
              again (re-sending is safe, item 4.6).
              - **Transport failures** retry on every step, like any check while the host is away.
              - **rc 2/3** (the server answered, the script failed) is bounded: after **3**
                consecutive non-zero exits *(widened by o22.11.3: every answered failure of a
                delivery)*, automatic re-sends stop with a notice, and Cancel *(Retry cancel, o22.14.1)* is
                the manual retry. The counter lives in `PollerMemory`, per launch like the strikes. Undo is no longer offered: the send is marked, and a
              lost reply looks exactly like a transport failure.
         6. **Re-sending must be safe.** `cancel.sh cancel` must be **idempotent** on a dir that
            already holds `.cancelled`. Its only new effect may be `.cancelled`'s mtime, which no
            rule reads. **Part A pins this with script tests (the Lab harness):**
            - a second run gives rc 0, `.cancelled` present, `.tmp` absent, and no signal outside
              the job session;
            - the verdict is unchanged (row 4 stays row 4, row 2 stays row 2);
            - two concurrent runs behave the same;
            - a missing job dir gives rc 3.

            The B4-style live run confirms it on uni.
       - **`StatusStep` and the record** *(the variant list is replaced by o22.12.5)*. Delivery returns the existing variants where they fit:
         `CheckFailed` (transport, before or after the send), `Labelled`, `Shown`, `Cancelled`,
         `Finalised`, `FetchFailed`. It adds two:
         - `CancelNotSent(why)` (rc 2/3) → `check_error` in the record;
         - `CancelStopped` (the rc bound reached) → a "cancel re-sends stopped" flag in the record,
           like the fetch bound.

         o19.3's exhaustive `match` maps both.
       - **Accepted residual.** A job that fails between step 4's classify and step 5's
         `cancel.sh` ends `cancelled` (row 4), not `failed`, with its evidence fetched. The window
         is one ssh round-trip, and its files are kept either way.
    5. **`Cancelling` joins `JobStatus`**: transient, non-terminal, only ever on a remote job
       (Decision d; the CHECK of item 2).
       - `JobStatus::from_db` gets the explicit arm. It has a wildcard, so the compiler does not
         force this: a missing arm breaks every read of such a row.
       - It joins every remote-live predicate, at each site:
         - `poller/plan.rs` (`candidates`);
         - `poller.rs` (`is_live`, `live_row`, `set_message`'s SQL, `mark_cancelled_conn`'s
           `from` list);
         - `ssh_backend.rs` (`refuse_if_remote_live`);
         - `commands/server_profiles.rs` (`live_remote_jobs`, `refuse_while_live`);
         - `commands/jobs.rs` (`delete_job_conn`);
         - and in TypeScript, `types.ts`, `jobs/remote.ts`, `JobsScreen.tsx`,
           `JobDetailScreen.tsx`, `App.tsx` and `ConnectivityPanel.tsx`: the status union, the
           live/terminal splits, filters and badges.
       - The local paths (`try_start_next`, `reconcile_on_startup`) select explicit statuses with
         `remote_host IS NULL` and never see it.
       - **The sweep bound is l row 3's, kept** (round 1 M3). Each status step of a `cancelling`
         job re-runs `cancel.sh cancel` *(readies the server first, o22.13.3)*, then collects and classifies.
         - **Progress** = the job session shrinks. `Outcome::Cancelling` gains the session size
           (`Cancelling { session: usize }`) for this purpose.
         - After **3 sweeps without progress**, automatic sweeps stop. The job stays `cancelling`
           with a notice ("processes of the job still run after 3 sweeps") and a manual retry:
           Cancel on a `cancelling` job runs one delivery and restarts the count *(replaced by
           o22.12.1: the click records no request, restarts the sweep count, and the next step
           sweeps)*.
         - The counter and the last session size *(the smallest, o22.13.4)* live in `PollerMemory` (per launch, like the
           strikes).
         - Once automatic sweeps have stopped, status steps still **collect and classify** (without
           `cancel.sh`), so a process that finally exits is noticed and the job ends `cancelled`.
    6. **o17 decided: a cancelled job that ran is fetched as evidence** (Anton).
       - **When:** a classifier `Cancelled` (row 4) whose snapshot has `.started`, whether the
         verdict came from a delivery or from an ordinary status step. Row 1 with `.cancelled`
         (`CorruptStarted { cancelled: true }`, item 4.5) takes the same evidence path and ends
         `failed`. `Outcome::Cancelled` gains `started: bool` so the decision is
         made from the outcome.
       - **What:**
         - The step sets the fetching flag, which stops the log poll as for any fetch (o16.6).
         - It downloads with the shared filter, then runs the LIST check (o6).
         - The live view is drained from the local copy.
         - `detect_completion` decides nothing, and no results are parsed or written; the files are
           for the log and file views. o18's finality argument rests on `.exit_code`, which a
           cancelled job may lack. Here the guarantee is weaker and stated as such: the copy
           equals the server's listing at the moment of the LIST check. A process the sweep could
           not kill (row 1) may still write afterwards.
       - **Status:** terminal `cancelled` (or row 1's `failed`) is written only **after** the LIST
         check passes. A failed evidence fetch counts a strike. **The step that takes the
         `MAX_FETCH_STRIKES`-th strike writes the terminal status itself**, with "evidence not
         downloaded: <why>; it stays on the server". It never reaches `FetchStopped`, so a job is
         never left non-terminal waiting for a manual retry that item 8 may disable.
       - **Without `.started`** (nothing ran), row 4 downloads nothing and is written `cancelled` at
         once.
    7. **One Cancel for a remote job; Withdraw goes** (Anton).
       - A remote job in `queued`, `running` or `cancelling` shows one **Cancel** *(for a pending or `cancelling` row: Undo / Retry cancel, o22.14.1–2)*. It sits behind
         o20.3's plugin-dialog `confirm` and its re-check of the current actions.
       - The `withdraw_remote_job` command and its button are removed. Withdraw lives on as
         delivery's not-on-the-server branch (item 4.3). o19's record removal and its source pin
         move to `cancel_job`'s remote arm *(withdrawn by o22.11.1: a cancel never removes the
         record)*.
       - A pending cancel shows a **"cancel pending"** badge with the request time, and **Undo
         cancel** while the send is not marked (item 3). The row's status does not change until
         the server decides (Decision i).
       - User-facing texts change: `refuse_if_remote_live` (no "arrives in unit 5.4 … can be
         withdrawn") and `refuse_while_live` ("cancel it first").
       - Local cancel is unchanged and has no dialog.
    8. **Retry while the server is unreachable is disabled, with the reason** (Anton).
       - **Record field.** The remote record (o19) gains `last_contact: Ok | Failed(why)`. Every
         ssh/rsync call the poller or a command makes for the job writes it from its **transport**
         result: label, collect, prepare, install, mkjob, cancel, fetch, LIST, and the upload and
         submit calls of `retry_remote_submit` *(and of the first Create & Run attempt and the pending
         re-check, o23.9)*. Every one of them holds the job's guard (o19.4).
         `poll_log` is **not** a writer: it holds no guard (o16.5), and o19.4 stays whole. "The
         server answered" is `Ok`, even when the answer is a refusal or an `Indeterminate`. A
         connect failure, timeout or ssh exit 255 is `Failed`.
         - `check_error` keeps its o19 meaning.
         - A job with no record yet reads as unknown, and Retry is offered.
         - `retry_remote_submit` **no longer removes the record at all** *(amends o19.3's "removal
           runs first" for retry)*. Its outcome writes `last_contact`, and its other fields as
           o19's mapping says. So a retry that failed to connect leaves Retry disabled.
       - **Offer rule.** `retry_remote_submit` and `retry_remote_fetch` are offered only while
         `last_contact` is not `Failed`. Otherwise the button is disabled with "server unreachable
         — retry when it answers".
       - **The Rust commands gain one predicate:** both refuse while `cancel_requested_at` is set
         ("undo the cancel first"). The check is made in the command's locked read, since the
         guard alone does not express it. Otherwise they still refuse a busy job (o15).
       - Cancel stays enabled while unreachable: it is the action that is useful then *(for a pending row the actions are o22.14.1's table: Undo, or Retry cancel once stopped)*.
    9. **Seams and negative controls** (each control must turn its test red).
       - **Seams, `AppHandle`-free:**
         - the cancel core (`in_flight`, `db`, `runner`, `sink`, `id`, the local dispatch);
         - `deliver_cancel(&Poller-like ctx, &InFlightGuard)`;
         - the undo core (`db`, `id`).
       - **Controls:**
         - with the guard held, recording still records and returns "cancel pending", not
           `Conflict`;
         - an `UPDATE` without the remote predicate makes a draft's cancel record an intent;
         - delivery that skips the pre-cancel classify turns a **row-6** job (failed, no
           `.cancelled`) `cancelled` instead of keeping `failed`;
         - delivery that marks the send **after** spawning `cancel.sh`, plus a lost reply, lets
           Undo succeed while `.cancelled` is on the server; Undo after the mark must be refused;
         - clearing `cancel_requested_at` before rc 0 loses the cancel after a transport failure;
         - rc 3 treated as delivered;
         - delivery without the ready step fails on a not-installed `cancel.sh`;
         - `Indeterminate` that never sends;
         - a terminal write that keeps either column fails the CHECK (each site of item 2);
         - `from_db` without the `cancelling` arm;
         - each live predicate of item 5 without `cancelling`;
         - a 4th automatic sweep without progress;
         - evidence written `cancelled` before the LIST check;
         - the third evidence strike that does not write the terminal status;
         - a row 4 without `.started` that downloads;
         - Retry offered while `last_contact` is `Failed` (frontend pure logic), and Retry accepted
           while a cancel is pending;
         - `cancel.sh` run twice on one dir changes its verdict;
         - a marked but unsent cancel on a job that then completes must reach `completed` (both
           columns cleared, M1 of round 2);
         - a click's delivery that leaves row 1 + `.cancelled` must end `failed` at the next
           ordinary step (keying on the path instead of the snapshot turns it red);
         - a 4th automatic re-send after three rc 3;
         - a `poll_log` that writes `last_contact`.
    10. **Propagation** (all marked where they stand):
        - Decision i (the mechanics); l row 1 and row 3 (realised here) and row 10 (5.4b);
        - the "v21" of l, m2, o1, o3.5 and o4;
        - m2's and o3.2's "withdraw";
        - o2 (Withdraw → Cancel), o3.4, o14.1, o14.5, o15.1 (the remote arm), o17 (closed);
        - o19.3 (`withdraw_remote_job`; retry no longer removes the record), o19.7 and o19.9, o20.3
          (the confirm moves to Cancel);
        - o4's "shown … handled in unit 5.4" list and o17.1's "`Cancelling` stays shown";
        - o21 (superseded);
        - Consequences "Job state machine" (`Cancelling`); Open questions (o17 closed);
        - ROADMAP 5.4 (split).

        Moving with 5.4a's code: `modules/remote-jobs.md` (its v21 text),
        `modules/execution-backends.md`, `modules/frontend.md`, and the doc comment at
        `ssh_backend.rs` that names v21.
    11. **Clarifications from Part A2a** (2026-10-17; found by the A2a implementer, settled by the
        orchestrator from Decision c and l row 3's bound philosophy; no new fork).
        1. **The record is never removed by a cancel.** o22.7's "o19's record removal moves to
           `cancel_job`'s remote arm" is withdrawn. A removal would erase both the verdict and
           `last_contact` (o22.8). Instead, the cancel core applies its delivery's `StatusStep` to the
           record under the guard, like any step (o19.4).
        2. **rc 0 clears both columns for every verdict** (item 4.5's "a non-terminal verdict NULLs
           both"). Rows 4, 1+`.cancelled` and 2 are finished by the next ordinary status step, which
           re-derives them from the server (item 4.5, row 1 keyed on the snapshot).
        3. **The rc bound counts every answered failure of a delivery**, not only `cancel.sh`'s.
           Before the mark, that includes ready (not installed, refused) and `mkjob`; after it,
           `cancel.sh` rc ≠ 0, a not-installed trampoline and a broken reply. A transport failure
           (no answer) is never counted. A click on an already-pending cancel is the manual retry:
           it restarts the count and delivers if the guard is free.
        4. **A stopped delivery still classifies** (Decision c: the server is the truth). With
           `CancelStopped` reached, each status step still runs delivery's steps 1–4: re-read,
           ready, label, pre-classify. It sends nothing.
           - If the pre-classify (at any time, stopped or not) shows that `.cancelled` is already on
             the server — row 2, row 3, row 4 or row 1 with `cancelled` — the cancel **is
             delivered**. Both columns are cleared in the step's locked write, and the verdict is
             handled as after an rc 0. Nothing is re-sent unless the verdict is row 3, whose
             re-sweep is item 5's.
           - An ended job (rows 5/6) is still caught as ended-first.
        5. **`last_contact` and rsync exit codes.** ssh exit 255 and rsync exit 255 (measured:
           rsync 3.2.7 exits 255 when ssh cannot connect — `wiki/orca/remote-sync-probe.md`) are
           `Failed`. rsync exit 12 writes **no** contact: a dropped transfer gives 12 (probe 5.3a),
           but its other causes are unmeasured, so it is left undetermined rather than guessed
           (rule #10).
        6. **Propagation:** item 7's record-removal sentence (marked); B's source pin keeps the
           record.
    12. **Clarifications from the A2a review** (2026-10-17; CODE + DESIGN verifier; D1 decided by
        Anton, the rest settled by the orchestrator from the verifier's fixes).
        1. **A click on a `cancelling` job records no request** (Anton, D1). The cancel already
           sits on the server, so the click is item 5's **manual sweep retry**:
           - it restarts the sweep counter (A2b's, in `PollerMemory`) and answers "the cancel is
             already on the server; sweeping again";
           - Undo is never offered;
           - the next status step sweeps.

           The record `UPDATE` admits `queued` and `running` only (item 3.1 narrowed). The CHECK
           stays as it is. **Every write of `cancelling` NULLs both columns in the same statement**,
           like the terminal writes of item 2: delivery's write does so today, and A2b's
           ordinary-step write of row 3 must as well. So a `cancelling` row never holds a request.
           As a belt, Undo also requires `status IN ('queued','running')`.
           - **Accepted residual (Anton):** after a delivery on a `queued`/`running` row, the next
             ordinary step finishes the verdict. Until then a second click records a fresh request
             and offers an Undo that cannot stop the cancel already on the server. How long that
             lasts depends on the server:
             - **while it answers:** up to one status period (≤ 15 s). With the guard free, the
               click's own delivery pre-classifies `.cancelled` and clears the request at once.
             - **while it does not answer:** for as long as it does not.

             The next step that reaches the server clears the request (pre-classify: `.cancelled`
             present → delivered). The rejected alternative was a third
             column, a persisted "delivered" marker (v22).
        2. **`CancelStopped` carries what was seen** (D3): `CancelStopped { seen: Option<Seen>,
           unready: Option<String> }`, where `Seen` is the pre-classify's `Outcome` or, on the
           not-on-server path, the `Label`, and `unready` is the prepare's reason when nothing could
           be seen.
           - The record maps it like `Shown` / `Labelled`, plus the "re-sends stopped" flag.
           - So a stopped job's view keeps showing the server's current verdict (`Lost`,
             `ReEnqueue`, `Indeterminate`), and 5.4b's `Indeterminate` counter is fed.
           - `seen` is `None` only when a stopped delivery's prepare-only ready was **answered** with
             a failure (12.3; a ready with no answer is `CheckFailed`) and nothing could be
             classified; then `unready` holds the reason and the record keeps `known`, with
             `check_error` set *(wording corrected by o22.13.6)*.
        3. **While stopped, ready is prepare-only** (D4): no `install` (a server write), so nothing
           is written on every period. A stopped delivery whose prepare is **answered** with a failure
           cannot classify; the record says so (`CancelStopped { seen: None, unready: Some(reason) }`).
           A prepare that gets **no answer** is `CheckFailed`, as everywhere else.
        4. **A recovered server still needs a click** (D5): a successful ready does not reset the
           bound. A transient answered failure (e.g. an install on a full quota) stops automatic
           delivery after 3 steps (~45 s); after recovery the user's Cancel *(Retry cancel, o22.14.1)* restarts it.
           Accepted: bounded, and visible.
        5. **`StatusStep` variants, as built** (supersedes item 4's list): `EndedBeforeCancel(Outcome)`
           (A2b takes the normal fetch path *(at the next ordinary step, o22.13.2)*), `CancelDelivered(Outcome)` (the record reads it like an
           ordinary step), `CancelNotSent(String)`, and `CancelStopped { seen, unready }`. `CancelResult` (the cancel
           core's answer) gains `AlreadyOnServer` for 12.1. Plus the
           existing `CheckFailed` (transport) and `RowChanged`.
        6. **A2b keeps the pending-cancel predicate in the read that acts.** `retry_fetch`'s check
           takes its own lock today. Once delivery replaces the normal step while a request is set,
           the run-status read must carry the predicate, so a cancel recorded between the two reads
           cannot let one fetch run.
        7. **Propagation:** item 3.1's status set; item 4.4's "`Cancelling` / row 1 go on" (narrowed
           by 11.4: `.cancelled` on the server = delivered); item 4.5's "3 consecutive non-zero exits"
           and item 4's variant list (widened by 11.3; replaced by 12.5); item 5's "Cancel on a
           `cancelling` job runs one delivery" (replaced by 12.1); all marked. `poller/plan.rs`'s
           counter docs move with A2a's code.
    13. **Clarifications for Part A2b** (2026-10-18; found by the A2b archaeology, settled by the
        orchestrator from l row 3's bound, o22.11.2 and o22.4; 13.2's notice and 13.3's ready decided
        by Anton). DESIGN round 1: PASS WITH FINDINGS (4 MED + 5 LOW), applied; round 2: PASS WITH
        FINDINGS (6 LOW), applied.
        1. **One read decides.** `run_status`'s first locked read (`live_remote`) also returns
           `cancel_requested_at`. While it is set, the step runs the delivery body **instead of**
           everything after that read, before the `FetchStopped` short-circuit, inside the same
           `record_step` — one record apply per step. `retry_fetch` keeps its pre-check (the "undo
           the cancel first" refusal, before `clear_strikes`); a cancel recorded after it is caught
           by the step's own read and delivered, so no fetch runs (12.6).
        2. **Delivery never downloads; the next ordinary step finishes.** `EndedBeforeCancel` and
           `CancelDelivered(_)` are the step's result. Both columns are already NULL, so the next
           step is an ordinary one: it re-collects, re-classifies and takes rows 5/6's fetch, row 4's
           or row 1's evidence path (item 6), row 2's fetch, or row 3's sweep (11.2, keyed on the
           snapshot). The cost is at most one status period.
           - **The ended-first notice survives the hand-over** (Anton, option a; item 4.4's "tell the
             user"). Delivery's ended-first write also sets `JobMemory.ended_before_cancel` (both
             paths: the click's and the poller's). The job's next `finalise` adds "the job finished
             before the cancel reached the server" to the message it writes (the whole message for
             `completed`, appended after `detect_completion`'s for `failed`), and keeps it when a
             results-parse failure then rewrites the message (`set_job_parse_error_conn`).
             **Accepted residual:** the flag is per launch and lives until the job's `finalise` —
             possibly many periods (failed fetches, `FetchStopped` waiting for a retry, a server that
             does not answer) — so a restart before that `finalise` loses the note (the result is
             kept either way).
        3. **The sweep of a `cancelling` job** (item 5). While the bound is not reached, the step
           **readies the server as delivery does** (prepare → install if needed → prepare; Anton,
           option a — so after an app upgrade, whose sha-named scripts read "not installed", a
           sweep reinstalls them, `collect.sh` included), runs `cancel.sh cancel`, then collects and
           classifies. Once the bound is reached, the step collects and classifies only; the click
           of 12.1 restarts the count, so the next step sweeps and readies again.
           - **Counting is one rule** (round 1 MED-1): a sweep step in which the server **answered
             any call** (ready, `cancel.sh`, collect) adds 1 to `sweeps_without_progress`, unless its
             classify shows progress (13.4). A step whose calls got **no answer at all** is
             `CheckFailed` and counts nothing. So a sweep that may have written to the server is
             always counted, whether the failure was the script's, the collect's or the classify's.
           - An answered failure is recorded as the step's `check_error`; the step still collects and
             classifies when it can. An answered **ready** failure sends nothing (no `cancel.sh`) and
             goes straight to collect + classify; the step counts +1.
           - **A stopped step** (bound reached) makes no ready, install or `cancel.sh` call: it is an
             ordinary collect + classify whose result is `Shown(outcome)` (a collect failure is
             `CheckFailed`, as everywhere), the record's `sweeps.stopped` staying set from `Bounds`.
             Its classify changes neither `min_session` nor the count, so a shrink seen while stopped
             does not resume sweeps; a terminal outcome (row 4 …) still ends the job (13.5).
           - `cancel_failures` stays the delivery's counter (11.3); a sweep never touches it.
           - **The step's result** (round 1 MED-3) is a new `StatusStep::Swept { outcome:
             Option<Outcome>, error: Option<String> }`: `outcome` is the classify's `Cancelling` (or
             `None` when nothing could be classified), `error` the answered failure. `next` maps it
             explicitly (known ← outcome when present, `check_error` ← error). The record's `Bounds`
             gain the sweep counter (`sweeps { count, stopped }`), read from `PollerMemory` like the
             fetch and cancel bounds; TS mirrors both (types only). A sweep whose classify shows a
             **non-`Cancelling`** outcome (row 4, row 1 + `.cancelled`, row 2 …) is dispatched like
             an ordinary step's outcome in the same step (13.5), and that path's result is the step's.
        4. **Progress has a baseline.** `JobMemory` gains `min_session: Option<usize>`, the
           **smallest** session size seen for the job (named for what it holds; a "last size" would
           let an oscillating session sweep forever). Every write of `cancelling` (delivery's and the
           ordinary step's) sets it from that outcome. A sweep whose classify gives
           `Cancelling { session }`:
           - `min_session` is `None` (e.g. after a restart) → set it, count nothing;
           - `session < min_session` → progress: set it, count restarts at 0;
           - otherwise → counted (13.3), `min_session` unchanged.

           Because the baseline only falls, a session that grows and shrinks back is not progress.
           The sweeps are bounded: from a first baseline `b` the baseline can fall at most `b` times,
           and each fall allows at most 3 further sweeps, so at most `3·(b + 1)` sweeps (plus the one
           that sets the baseline after a restart); with no real shrinking, 3. The click's
           `restart_sweeps` (12.1) zeroes the count and keeps the baseline. At the 3rd sweep without
           progress the record's `sweeps.stopped` is set, with the text "processes of the job still
           run after 3 sweeps".
           - **Atomic updates** (round 1 LOW-5): the count's increment, its reset and the baseline's
             minimum are each **one** `PollerMemory` operation under its lock (as `cancel_failed`
             is), never "value read before the ssh calls + 1", so a `restart_sweeps` click during a
             sweep is not overwritten.
        5. **The ordinary-step outcomes, as A2b builds them:**
           - **row 3** `Cancelling { session }` on a `queued`/`running` row → one write: status
             `cancelling`, the message, **both columns NULL** (12.1), guarded on the row still being
             live; plus the baseline (13.4). The CHECK does not catch a forgotten NULL here (it admits
             a request on `cancelling`), so a test pins it (13.7);
           - **row 4** `Cancelled { started: false }` → `cancelled` at once, no download (unchanged);
           - **row 4** `Cancelled { started: true }` and **row 1** `CorruptStarted { cancelled: true }`
             → item 6's evidence fetch: the fetching flag, `fetch_remote` (download + LIST), then the
             terminal write, then the live view drained from the local copy (the same order as
             `fetch_and_finalise`). The write is `cancelled` through `write_cancelled`, or for row 1
             `failed` with "processes of the job may survive on the server" through `finalise` (its
             live-row re-check). No `detect_completion`, no parse — a copy holding `.exit_code` 0
             still ends `cancelled` (rule #6: the evidence path never claims completion). The step's
             result is `Cancelled` or `Finalised`.
           - **A failed evidence fetch** counts a strike with its own text; the step's result is
             `FetchFailed`. The step that takes the `MAX_FETCH_STRIKES`-th strike writes the terminal
             status itself, with "evidence not downloaded: <why>; it stays on the server", and
             forgets the job — also when that write finds the row changed — so `FetchStopped` is never
             reached for an evidence job.
        6. **Propagation** (marked where they stand): 12.2's "`seen` is `None`" wording (corrected);
           item 5's "the last session size" (now the smallest, 13.4); 12.5's "(A2b takes the normal
           fetch path)" (the next step, 13.2). With A2b's code: `poller.rs`'s stale `shown_message`
           comment on `Cancelled` and its `Cancelling` text ("handled in unit 5.4"), its module doc's
           "row 4 … with no download", the "caller takes the normal fetch path" doc on the
           ended-first variant; `poller/cancel.rs`'s "the caller fetches" docs;
           `modules/remote-jobs.md`.
        7. **Negative controls for 13.x** (each must turn its test red):
           - delivery placed after the `FetchStopped` check → a pending cancel on a job with the
             fetching flag and strikes at the maximum sends no `cancel.sh`;
           - the pending predicate dropped from the read that acts → a cancel recorded after
             `retry_fetch`'s pre-check lets an rsync run; a second, separate read of it in
             `run_status` is caught by a source pin, since no seam lands a click between two reads
             inside one step *(wording corrected by 13.8.1)*;
           - a delivery step that downloads → an rsync call is counted;
           - a poller-delivered row 5 that loses `ended_before_cancel` → `completed` without the note;
           - a sweep with no ready step → a not-installed `cancel.sh` is never reinstalled;
           - transport-only failures counted → the 4th sweep does not run after three; answered
             failures (of `cancel.sh`, or of the collect after an rc-0 sweep) not counted → a 4th
             sweep runs;
           - a baseline tracking the last size → an oscillating session (5, 6, 5, 6 …) never stops;
             after `restart_sweeps`, a session equal to the baseline counted as progress;
           - a non-atomic increment → a `restart_sweeps` between the sweep's read and its increment
             leaves the count above 1;
           - the row-3 write without the NULLs → a request recorded between the step's read and that
             write survives on the `cancelling` row;
           - the evidence path through `detect_completion` → a row 4 with `.exit_code` 0 and no
             `ORCA TERMINATED NORMALLY` ends `failed`, not `cancelled`;
           - the terminal write before the LIST check; a row 4 without `.started` that downloads;
           - the `MAX_FETCH_STRIKES`-th evidence strike that does not write the terminal status, or
             that leaves memory at the maximum with the fetching flag after a changed row;
           - a permanently failing install → the 4th sweep step makes an install call;
           - the ready step moved before the "stopped" check → a stopped, collect-only step makes a
             prepare or install call;
           - a results-parse failure after an ended-first `finalise` → the note is lost.

        8. **Clarifications from the A2b implementation** (2026-10-18; found by the A2b implementer,
           settled by the orchestrator; no new fork). Reviewed with A2b's CODE verification.
           1. **The 13.7 control for 13.1** is two: a behavioural test (the predicate dropped from the
              read that acts → an rsync runs) and a source pin (no second lock or pending read in
              `run_status`). A separate read *before* the deciding one stays green behaviourally,
              because no seam lands a click between two reads of one step; only the pin guards it.
           2. **The stopped text lives in the row's `error_message`**, written by the sweep that
              reaches the bound when it classifies `Cancelling`, and by every stopped step that
              classifies `Cancelling`. If the 3rd count comes from a sweep that classified nothing,
              the text appears one step later; the record's `sweeps.stopped` shows the stop at once.
           3. **A non-`Cancelling` verdict after an answered sweep still counts +1** (13.3's one rule).
              So a `cancelling` row whose verdict is row 2 with a failing fetch re-runs the idempotent
              `cancel.sh` (item 4.6) at most until the bound stops it. Accepted: bounded, and harmless.
           4. **`FetchStopped` precedes the sweep** in the run order (it follows only the pending
              delivery, 13.1). It can reach a `cancelling` row only through a row-2 fetch that failed
              `MAX_FETCH_STRIKES` times; that row is then shown as fetch-stopped and is not swept,
              which is acceptable: `.cancelled` is on the server and the job has ended.
           5. **A drain error after an evidence `Cancelled`** is logged, not carried: `StatusStep::
              Cancelled` has no field, and the copy on disk is complete (the LIST check passed). A
              field would change the TS wire shape for no user-visible gain. Accepted.
    14. **Part B: the commands and the view** (2026-10-18; forks raised by the orchestrator from the
        Part B archaeology; 14.1–14.4's choices decided by Anton, the rest settled by the
        orchestrator). DESIGN round 1: FAIL (H1 + 5 MED + 6 LOW), applied; round 2: PASS WITH FINDINGS (3 MED + 8 LOW),
        applied.
        1. **"cancel pending" in the list and the detail** (Anton). A remote job with
           `cancel_requested_at` set shows a **"cancel pending since <time>"** badge in the Jobs table
           and in the detail's remote panel; once `cancel_sent_at` is set it reads "cancel pending —
           the cancel may already be on the server" (item 3). **The action set of a pending row**
           (round 1 H1; 11.3: a click on a pending cancel is the manual delivery retry):

           | `cancel_sent_at` | `cancel_sends.stopped` | offered |
           |---|---|---|
           | NULL | no | **Undo cancel** |
           | NULL | yes | **Undo cancel** + **Retry cancel** |
           | set | no | nothing (delivery re-sends by itself on every step) |
           | set | yes | **Retry cancel** |

           Undo and Retry cancel replace the Cancel button in the row and in the panel. Retry cancel
           calls `cancel_job` (the already-pending click: the failure count restarts, delivery runs if
           the guard is free) with no confirm, like 14.2. *(Amends o22.7's "shows one Cancel" for a
           pending row.)*
        2. **On a `cancelling` job the button is "Retry cancel", with no confirm** (Anton). It calls
           the same `cancel_job` (12.1: no request, the sweep count restarts); the answer
           `AlreadyOnServer` shows the notice "the cancel is already on the server; sweeping again".
           No Undo (12.1). The confirm of item 7 / o20.3 stays on the first Cancel of a `queued` or
           `running` remote job, with o20.3's re-check of the current actions after the dialog:
           title "Cancel job", text "Cancel "<name>" on <host>? The server stops it; files already
           written stay on the server and are downloaded for viewing. You can undo until the cancel is
           sent." Local Cancel is unchanged and has no dialog. *(Amends o22.7's confirm for
           `cancelling` and o22.12.4's "the user's Cancel restarts it": Retry cancel.)*
        3. **Cancel problems are separate badges; an error says whether the server answered**
           (Anton; round 1 M3).
           - The record's `check_error` becomes `{ text, answered: bool }`, set **at each site that
             writes it** (`next` / `after_retry` in `remote_state.rs`):
             - `Shown(Indeterminate)`, `CancelNotSent`, `CancelStopped` (its `Indeterminate` and its
               `seen: None` with `unready`), `CancelDelivered(Indeterminate)` → `answered: true`;
             - `after_retry`'s `Failed` → `answered: false`;
             - **`CheckFailed` → from the step's own contact** (the tracker's `last()` that
               `record_step` already passes to `apply_with_contact`; the failing call is the step's
               last call), not from the record's `last_contact` — `CheckFailed` also carries answered
               failures (a broken reply, "collect exited N after a complete snapshot", a second
               retake, a label parse error) (round 2 MED-1);
             - **`Swept`**: its `error` carries the flag of the **last failing call** (a joined
               "earlier; then why" text takes the later call's flag).

             The view prefixes "Server unreachable: …" when `answered` is false and "Server answered:
             …" when true. The old "Server unreachable / not decided" goes. **Wire shape** (round 2
             LOW-2): TS `check_error: string | null` becomes `{ text: string; answered: boolean } |
             null`; the serde pin in `remote_state.rs` and the TS readers in `remote.ts` change with
             it.
           - `cancel_sends.stopped` **while a cancel is pending** → a warn badge "cancel re-sends
             stopped — Retry cancel to send again". `undo_cancel`'s `Undone` also clears the
             failure count (round 1 L3).
           - **The record's bounds are refreshed by the commands that change them** (round 2 MED-3):
             after `cancel_job` clears `cancel_failures` (Retry cancel, a new Cancel) or restarts the
             sweeps, and after `undo_cancel`'s `Undone`, the command applies a **bounds-only refresh**
             to the record — `cancel_sends` and `sweeps` re-read from `PollerMemory`, `known`,
             `check_error` and `last_contact` untouched — and emits it like any record change. It
             needs no guard: it reads memory at the moment it writes, so it can never write a bound
             older than memory holds, and any later step re-reads memory too. So "stopped" is not
             shown next to the "cancel pending" notice after a Retry cancel that answered `Pending`.
           - `sweeps.stopped` → a warn badge "sweeps stopped — processes still run; Retry cancel to
             sweep again". The row's own text (13.8.2) is not shown a second time as a red banner.
           - No counters are shown before a bound is reached.
        4. **Retry is hidden while a cancel is pending** (Anton). Retry submit / Retry fetch are not
           offered while `cancel_requested_at` is set; the badge and 14.1's actions explain why.
           "Disabled with a reason" stays only for the unreachable case of item 8 ("server
           unreachable — retry when it answers"). The Rust refusals stay as the belt.
        5. **The seam: `Job` carries both columns; every column write emits** (orchestrator; round 1
           M2).
           - `Job::COLUMNS`, the struct, `from_row` and the TS `Job` gain `cancel_requested_at` /
             `cancel_sent_at` (no migration: v21 has them). Rejected: a `pending_cancel(id)` command
             (N+1 on the list); the in-memory record (invisible after a restart until the first step).
           - **Every write of either column emits `job:status`** for that job, **after the write's
             lock is released, from the same step** (the existing pattern of `record_delivered` and
             `run_guarded`): `record_intent`, `revoke_cancel_core` (`Undone`), `mark_sent`,
             `record_ended_first`, `record_delivered` (both branches), and the writes that already
             emit (terminal, `record_cancelling`). These are every write of the two columns (grep,
             round 2). `status_to_emit` stops being the only trigger for these paths. So that each
             emit is testable without an `AppHandle`, the cancel core's record path and the undo
             core take a `sink` (and the undo core `memory` and the record store, for 14.3's
             refresh) *(amends o22.9's undo-core seam `(db, id)`)*. The list and the detail already reload on `job:status`; the clicking screen also
             reloads when `cancel_job` / `undo_cancel` returns.
        6. **Times** (round 1 M1; rule #10). SQLite's `datetime('now')` is **UTC** — measured
           2026-10-18: `datetime('now')` = `06:18:01` while the laptop read `09:18:01 EEST` (SQLite
           3.45.1); B1 pins it for the app's own SQLite with a test against the system clock. The
           view therefore converts: a pure `formatLaptopUtc(ts)` parses `"YYYY-MM-DD HH:MM:SS"`
           explicitly as UTC (`ts.replace(" ", "T") + "Z"`, never `new Date(ts)` on the bare form,
           whose parse is implementation-defined) and shows local time; an unparsable value is shown
           raw, never "Invalid Date". It replaces `formatTimestamp` **everywhere** (`created_at` in
           the Jobs, Molecules and Reactions tables, which today show raw UTC — 3 h off here) and
           the detail's unformatted `created_at`, `started_at` and `completed_at` (all
           `datetime('now')`; grep found no displayed time that is not SQLite UTC, and no server
           time); `formatTimestamp` is retired. A vitest under a non-UTC `TZ` pins it; the Rust
           test of the app's own SQLite (rusqlite `bundled`, not the system 3.45.1 measured above)
           runs with a non-UTC `TZ` too, so "≠ system UTC" can bite. The measurement is logged as a
           `probe` entry (2026-10-18). `db.rs`'s
           "laptop time" doc says "UTC, laptop clock" (Decision j unaffected).
        7. **Commands.**
           - `cancel_job` becomes async and runs `cancel_job_core` inside `spawn_blocking` +
             `with_poller` — **not** `guarded_blocking`: the core claims the guard itself
             (`try_acquire` remote, `acquire` local), and a pre-claimed guard would turn every remote
             cancel into `Pending` and every local one into `Conflict`. It returns `CancelResult`
             (a local job: `{"result":"dispatched"}`). The shell is a thin, source-pinned wrapper;
             the behaviour is tested on the core. `cancel_job_guarded` and
             `refuse_cancel_if_remote_live` go (superseded by the core).
           - New `undo_cancel(id) -> UndoResult` over `revoke_cancel_core` (no guard).
           - **Every answer has a defined notice** (round 1 M4), from pure `cancelNotice` /
             `undoNotice`: `Delivered{step}` maps by its step — `CancelDelivered`/`Cancelled`/
             `Finalised` → "cancel sent"; `EndedBeforeCancel` → "the job finished before the cancel
             reached the server"; `CancelDelivered(Completed{late_cancel})` → "the job completed
             before the cancel took effect; its result is kept"; **`CheckFailed` by the reloaded row
             and the error's flag** (round 2 MED-2): `cancel_sent_at` set → "no answer after the
             cancel was sent; it may already be on the server; checked again at the next step";
             not set and unanswered → "server unreachable — the cancel stays pending and is sent
             when it answers"; not set and answered → "the server's answer could not be read: <why>;
             the cancel stays pending"; `CancelNotSent` → "the cancel failed on the server: <why>; it
             stays pending" (it may follow a real `cancel.sh` whose reply was broken);
             `CancelStopped` → "cancel re-sends stopped"; `RowChanged`/`NotLive` → "the job changed
             meanwhile"; every other `StatusStep` (`Busy`, `Labelled`, `Shown`, `FetchFailed`,
             `FetchStopped`, `Swept`; delivery returns none of them) → no notice, never "sent"
             (the TS switch stays exhaustive); `Pending` → "cancel pending — sent at the next check";
             `AlreadyOnServer` → 14.2's text; `MovedOn{status}` → "the job is already <status>";
             `Dispatched` → none. `Undone` → "cancel undone"; `SendMarked` → "too late to undo: the
             cancel may already be on the server"; `NothingPending` → "no cancel to undo". "sent" is
             never shown for a step that did not deliver.
           - `withdraw_remote_job` is removed with its registration, its button, its dialog,
             `confirmThenWithdraw`, `SshBackend::withdraw`, `withdraw_remote`, `record_withdraw`,
             `WithdrawReport` (Rust and TS), the `"withdraw"` `RemoteAction`, the fake-runner
             `withdraw_remote` tests and their `withdraw_server` helper (`ssh_backend/tests.rs`), the
             `ssh_backend.rs` module doc's Withdraw section, and `forget_after` / `forget_in_app` if
             nothing else needs them (`poller/tests.rs` uses `forget_after`: retarget or drop).
             `refuse_if_remote_live`'s third caller, `cancel_remote`, stays as the local backend's
             belt (unreachable for a live job once the core routes). The three
             `backend_e2e_tests.rs` withdraw tests are **ported** (round 1 L1: they are the only
             real-script coverage of label → mkjob → `cancel.sh` → collect for a job not on the
             server), driving `cancel_job_core` and then **one ordinary status step** (delivery
             alone ends at `CancelDelivered`, the status unchanged, 13.2) before asserting
             `cancelled`; e2e1's "unit 5.4" assertion inverts (a queued job is now cancellable). o19's removal pin becomes a **behavioural** test: with a record seeded,
             `cancel_job_core` answering `Pending`, `AlreadyOnServer` and `Delivered` leaves it
             present (the step mapping's own `NotLive`/terminal removal is exempt).
           - Texts: `refuse_if_remote_live` (its callers: delete and the local path's belt) says
             "job … is <status> on server <host>: cancel it first (the cancel goes through the
             server)"; `refuse_while_live` says "cancel them first, or wait until they finish";
             `cancelling_message`'s "Cancel sweeps again" says "Retry cancel sweeps again" (and the
             doc comments at `poller.rs` ~227 and `plan.rs` ~42 follow);
             `queued_remote` says "only a queued remote job can be retried"; the poller's "handled in
             unit 5.4" (`Lost`, `ReEnqueue`, the shown `Failed`) says "handled in unit 5.4b".
        8. **`cancelling` is live in the view** (item 5's TS sites). `remoteView`, `backfillsLog`,
           `RemotePanel`, `RemoteJobBadges`, the detail's live/terminal split, the Jobs table's live
           branch (Retry cancel, no Delete) and its status chips, and the status bar's active count
           all treat `cancelling` like `running`. `backfillsLog` keeps its no-duplicate-line rule for
           a `cancelling` job (the poller's candidates already include it); a test pins it. **The
           Cancel click's in-flight state** is labelled "Sending cancel…" (not "Cancelling…", now a
           status) and, for a remote job, **ends when `cancel_job` returns** (round 1 M5), not on a
           terminal `job:status`; a local job keeps today's behaviour.
        9. **Accepted:** an answered sweep error followed by a non-`Cancelling` verdict is not shown
           (13.3's last bullet; the job has moved on).
        10. **Split** (round 1 M5). **B1**: the Rust commands and texts, the emits (14.5), the `Job`
            columns, the record pin, the removals and the e2e port, plus the pure TS logic —
            `remoteView`'s actions with a per-action `confirm: bool` (`cancel`, `retry_cancel`,
            `undo_cancel`, the hidden and disabled retries), the badges and prefixes,
            `cancelNotice` / `undoNotice`, `formatLaptopUtc`, `backfillsLog`, and a
            `confirmThenCancel` with o20.3's re-check, and the Jobs table's row actions as a pure
            function (`rowActions(job, record)`: Retry cancel and no Delete for `cancelling`) —
            **and the minimal screen edits that keep B1
            safe and building**: the withdraw branch and button go, and every remote Cancel already
            goes through `confirmThenCancel` (a B1 tree never cancels a running remote job without
            the dialog), with the in-flight reset of 14.8. A B1 commit has no badge, Undo or notice yet
            (accepted: B2 follows before the live gate). Then STOP. **B2**: the rest of the
            screens (badges, Undo/Retry cancel in the row and panel, notices, chips, counts, time
            formatting), then Anton's live gate (WebKitGTK) and the uni run.
        11. **Negative controls** (each must turn its test red; "user-facing string" = a non-comment
            string literal in `src-tauri/src` or `src`, tests excluded):
            - `remoteView` with Undo but no Retry cancel for a stopped re-send with `sent_at` NULL,
              or nothing for a stopped one with `sent_at` set (H1); Undo offered once `sent_at` is set;
            - Retry offered while a cancel is pending; Retry offered (not disabled) while
              `last_contact` is `Failed`;
            - the prefix keyed on `last_contact` (a transport `check_error` followed by an answered
              call → "Server answered"); a `Swept` that loses its error's flag;
            - `cancelNotice(Delivered{CheckFailed})` containing "sent"; `SendMarked` → "undone";
            - a column write without its emit (one per site of 14.5);
            - `formatLaptopUtc` via `new Date(ts)` on the bare form, or without the `Z` (red under
              `TZ=Europe/Kyiv`); the app's SQLite `datetime('now')` ≠ system UTC;
            - `Job::from_row` with the two new columns swapped;
            - after Retry cancel → `Pending`, the record still reading `cancel_sends.stopped: true`
              (no bounds-only refresh); after `Undone`, the record still reading it (round 2 MED-3);
            - `CheckFailed` flagged from the record's `last_contact` instead of the step's contact (a
              broken reply → "Server unreachable"); a joined `Swept` error taking the earlier
              call's flag;
            - `cancelNotice(Delivered{CheckFailed})` saying "is sent when it answers" when
              `cancel_sent_at` is set (rc 0 then a failed collect); `CancelDelivered(Completed{
              late_cancel})` → "cancel sent";
            - a ported e2e: a not-on-server delivery without `mkjob` → `cancel.sh` rc 3 (Lab);
            - each screen's Cancel invoking `cancel_job` other than through the one shared helper
              behind `confirmThenCancel` (a source pin; the dialog itself is a live-gate item);
              `rowActions` offering Delete for `cancelling`;
            - a cancel path that removes the record (the behavioural pin); the shell not calling
              `cancel_job_core` inside `spawn_blocking` without `guarded_blocking` (a source pin —
              no `AppHandle`-free seam exists for the shell);
            - `remoteView` treating `cancelling` as terminal; `backfillsLog` re-streaming a
              `cancelling` job's log twice;
            - `confirmThenCancel` sending when the offer vanished during the dialog; `retry_cancel`
              or `undo_cancel` with `confirm: true`; a remote `cancel` with `confirm: false`;
            - the remote in-flight state not ending when `cancel_job` returns (a pure reducer or the
              wiring helper);
            - a stray "withdraw", "unit 5.4" (matched as `unit 5\.4(?![a-c])`) or "Cancel sweeps
              again" in a user-facing string (the text guard);
        12. **Propagation** (round 1 L5, round 2 LOW-7; marked where they stand): o22.4.5 ("Cancel is
            the manual retry"), o22.7 ("one Cancel", its confirm), o22.8 ("Cancel stays enabled
            while unreachable"), o22.9 (the undo-core seam), o22.12.4 ("the user's Cancel restarts
            it"), o20.3 (the confirm moves to the
            first remote Cancel; Retry cancel and Undo have none). Moving with B's code:
            `modules/frontend.md`, `modules/tauri-core.md`, `modules/execution-backends.md`,
            `modules/remote-jobs.md`, `db.rs`'s time doc, ROADMAP 5.4a.
        13. **Clarifications from the B1 implementation** (2026-10-18; found by the B1 implementer,
            settled by the orchestrator; reviewed with B1's CODE verification).
            1. `cancelNotice(Delivered{CheckFailed})` reads the flag from the **reloaded** record's
               `check_error.answered`; when the record cannot be read, the notice is the neutral
               "The cancel stays pending: <why>" (it claims neither "unreachable" nor "answered").
            2. A `CheckFailed` with **no call made** (an argument-build failure, practically
               unreachable) is flagged `answered: false` and so reads "Server unreachable". Accepted:
               the step made no contact, which is what that prefix reports to the user.
            3. **What a B1 tree shows** (14.10's "no badge, Undo or notice yet", corrected): the new
               badges and the prefixed check error already render, because `RemotePanel` /
               `RemoteJobBadges` draw every `view.badges` entry; the retries render disabled with
               their reason. A pending, unsent, not-stopped row has **no button** until B2 wires
               Undo (accepted for the B1 commit; B2 follows before the live gate). Retry-cancel offers
               keep the label "Cancel" until B2 relabels them.
        14. **Decisions from the B2 review** (2026-10-18; B2 CODE verifier F1 + the implementer's
            questions; decided by Anton).
            1. **The cancel helpers trust nothing the screen hands them** (Anton, F1, option b).
               `sendCancel` and `sendUndo` take **only the job id** (plus the in-flight callbacks).
               Each reads the row (`get_job`) and the record (`remote_job_state`) itself and derives
               its offer from that read; after the dialog it reads both **again** and re-derives
               (o20.3's re-check, now from the backend, not from the view). The dialog `confirm` is
               the Tauri plugin's, **imported inside `live-wiring.ts`**, never injected by a screen
               (tests mock the module). So a screen cannot forge a job, a record, an offer or a
               dialog — a structural guard, not a text pin. The Rust refusals stay the belt.
               Rejected: (a) a wider source pin (text, not structure); accepting the gap.
            2. **In the job view, a remote job's cancel buttons live only in its panel** (Anton); a
               local job keeps them in the header. The Jobs table row keeps its own (14.1).
            3. **The Jobs notice banner is dismissed by hand** (✕), never by a timer (Anton); the
               **next answer replaces it** (Anton, B2 round 2 G6). A click that produces no answer
               (a declined dialog, nothing offered) leaves the banner as it is; an **error** clears it,
               so an older notice never stands beside the error banner (orchestrator, B2 round 3).
            4. **`cancelling` gets its own badge tone** (Anton), distinct from `cancelled` (e.g. an
               amber outline instead of a fill): "still being cancelled" must not read as
               "cancelled".
            5. **A Retry in flight disables the cancel buttons too** (verifier F2, orchestrator), as
               a cancel in flight already disables the retries.
            6. **Clarifications from B2 round 2** (orchestrator): the in-flight state starts **before**
               the helper's first backend read, so a double click cannot send twice (G3); a failed
               read after the dialog rejects and sends nothing, and a test pins it (G1); the
               one-helper source pin scans every non-test `src/**/*.{ts,tsx}` except
               `live-wiring.ts` for the `cancel_job` / `undo_cancel` literals (G2).
23. **Unit 5.4c: the pending submit** (2026-10-19; forks raised by the orchestrator from the 5.4c
    archaeology and the DESIGN review; 23.1–23.4 and 23.6's `Unknown` and socket-error rules decided
    by Anton, the rest settled by the orchestrator). Realises Decision m item 2. DESIGN round 1:
    PASS WITH FINDINGS (6 MED + 7 LOW), applied; round 2: PASS WITH FINDINGS (4 MED + 6 LOW),
    applied as 23.13 (which wins over the items above where they differ).
    1. **Scope: only a slot the server cannot clear** (Anton). A submit becomes **pending** only when
       `submit.sh` answers that the slot check of Decision l / o9 blocks it:
       - **slot busy** (step 5: live blockers on the mask);
       - **slot check error** — the scan itself failed (step 5: timeout, a bad `tsp -l` of another
         socket, unexpected output) **or** the slot's own socket gave l's `Error` (step 3: `tsp -l`
         on the slot socket failed, `/proc/net/unix` unreadable) (Anton, round 1 MED-5).

       Every other outcome stays what it is today: a transport failure of the first attempt, a
       refused prepare, install, upload, lock or values, `KillUserProcesses` leave the row "not on
       the server" with the manual Retry submit (o22.8's offer rule). A pending submit exists only
       after the server **answered** a submit the user started (Create & Run or Retry), so the app
       never writes to a server it has not heard from for this job, and B4's "step 0 before any
       write" holds (no submit is allowed before step 0).
    2. **A free slot submits by itself** (Anton). The pending state exists to submit when the slot
       frees; nothing is ever freed by force (m2, rule #8): the app waits for the blocking work to
       end on its own. Two pending jobs on one slot: the first to enqueue holds the slot; the second's
       next attempt is accounted by the slot daemon's running row (l round-5 MED-1) and queues behind
       it. The order between pending jobs is the poller's candidate order (`created_at`).
    3. **The re-check is the submit itself** (Anton). Each status step of a pending job:
       1. **derives the plan exactly as `resubmit_remote` does** (round 1 MED-1) — `retry_checks`
          (the profile's run target with its verified stamp, host/root/socket unchanged) and
          `write_local_dir` + `submit_args` with the profile's **current** mask (o14.2: nprocs ≤ the
          mask's CPUs) — **minus** the label call (step 3's marker and row checks under the account
          lock replace it) **and minus the upload**;
       2. runs the ready step (prepare → install if needed → prepare);
       3. runs `submit.sh`. Its upload post-condition (step 4: the exact file set and every sha256)
          proves the server copy matches the plan; only if it answers **upload mismatch** (23.7) does
          the step upload once and run `submit.sh` once more, in the same step *(a mask edit does not
          reach this path: it clears the profile's stamp and ends the pending state, 23.13.3)*.

       While blocked, an attempt refuses at step 3 or 5, **before** the `.tsp-out` dir (step 6) and
       the claim (step 7), so it writes **nothing in the job dir** (pinned by a Lab script test). It
       does touch, outside the job dir (round 1 LOW-1): the account lock file
       (`$HOME/.orcastudio-submit.lock`, created on the first submit, opened for write each time); a
       `mktemp -d` dir removed by the EXIT trap (whether the trap runs on SIGHUP or a kill is **not
       measured**); and `tsp -l` on sockets `/proc/net/unix` lists (the stale-socket daemon start of
       probe 5.2b is guarded by that check; a daemon dying between the read and `tsp -l` is l's
       harmless residual, now met once per period). A free slot is the submit, under the same
       account lock as its check, so there is no window between "checked" and "submitted" beyond l's
       accepted residual (the daemon dying between the scan and `tsp`).
       - **Cost, accepted without backoff** (round 1 LOW-3): one prepare + one `submit.sh` per period
         (~15 s), about 1 s measured (scan 0.6–0.8 s, `orca/remote-sync-probe.md` items 8 and 10),
         plus a `sha256sum` of the uploaded inputs (small) under the account lock. m2 asks for a
         re-check on every reconcile; the notice (23.7) tells the user it is waiting.
       - No new script and no new probe: the scan and its refusal are the ones measured in
         `orca/remote-sync-probe.md` item 10.
    4. **The refusal counter is per launch** (Anton), in `PollerMemory` like the fetch strikes,
       the re-send failures and the sweeps (a deviation from m2's "in the local DB", recorded at m2).
       The **pending flag** is in the DB (it must survive a restart). The counter counts slot-busy and
       slot-check-error answers; it is **not** reset by a transport failure, only when the pending
       state ends (and by a restart). After a restart the notice comes back after 3 more refusals.
    5. **Schema v22: `jobs.submit_pending_since TEXT NULL`** (UTC, laptop clock, o22.14.6), with
       `CHECK (submit_pending_since IS NULL OR (remote_host IS NOT NULL AND status = 'queued'))` in
       v21's ADD-COLUMN form. 5.4b takes v23. `Job` carries the column (o22.14.5's seam).
       - **Set** by the write that records a slot-busy / slot-check-error attempt (`record_attempt`,
         same statement), as `COALESCE(submit_pending_since, now)`: a later refusal keeps the first
         time (round 1 LOW-4).
       - **NULLed** by every write that leaves the pending state: the enqueue, a refusal that ends it
         (23.6), `record_delivered` (both branches — the not-on-server delivery leaves `queued` and
         the terminal write is the next step's, round 1 MED-4), `record_ended_first`, and every
         terminal or `cancelling` write (`finalize_job_conn`, `mark_cancelled_conn`,
         `record_cancelling`). The CHECK catches a forgotten NULL on a terminal or `cancelling`
         write; on the two `queued`-keeping writes (`record_delivered`, `record_ended_first`) it
         cannot, so tests pin them (23.11).
       - `record_shown`'s queued→running write (`poller.rs` ~568) and `update_job_status_conn`
         (local) never meet a flagged row: the pending branch runs before the label/collect path,
         and a local row has no coordinates. The CHECK would fail loudly if they did (round 1
         LOW-5).
       - Every write of the flag emits `job:status` after its lock (o22.14.5's rule).
    6. **While pending, per status step** (keyed on the step's contact, not on the step name —
       round 1 MED-3):
       - a **pending cancel comes first** (o22.13.1): delivery's not-on-server branch (`mkjob` →
         `cancel.sh`); `record_delivered` NULLs the flag; the next ordinary step writes `cancelled`.
         Undo resumes the pending submit (the flag is untouched by an Undo);
       - **no answer** — the step's `ContactRunner` saw a transport failure (ssh/rsync 255, a
         timeout) or rsync 12 (no contact, o22.11.5) on its last call, before `submit.sh` answered
         → stays pending, `CheckFailed`, nothing counted;
       - **the submit reply lost** (`Unknown`: the call was sent, no reply) → **stays pending**
         (Anton): the next attempt's step 3, under the account lock, sees the claim or the slot row
         if the first one enqueued and refuses ("marker" / "row") — that refusal ends the pending
         state (below) and the label decides from then on (o14.4); if it did not enqueue, the next
         attempt simply submits;
       - **slot busy / slot check error** → stays pending; one refusal counted; the record carries
         the blockers or the reason (23.7);
       - **lock busy** (another submit holds the account lock) → stays pending *(counted, with its
         own notice: 23.13.1)*;
       - **enqueued** → the flag is NULLed, the row is an ordinary queued job;
       - **a local refusal of the plan** (23.3.1: the run target unverified — e.g. another job's
         `KillUserProcesses` cleared the stamp —, host/root/socket changed) and **any other answered
         refusal** (prepare, install, upload after its one retry, values, markers, a slot row,
         `KillUserProcesses`, failed after the claim) → the pending state **ends**: the flag is
         NULLed, the row shows the refusal with the manual Retry submit, as after a first attempt.
         These need a person.
       - After a restart (memory lost, flag set) the step simply re-checks; the counter starts at 0.
    7. **Structured outcomes from `submit.sh`** (round 1 MED-2; no caller decides on reason text,
       `remote/submit.rs`). The refusals 23.6 distinguishes get their own reply kinds, in the
       length-prefixed records of the existing `Reader` format (not `%q`-quoted text):
       - **blocked** — one record per blocker: a process (pid, kind wrapper|pinned, cores, job dir or
         "cwd not readable") or a queued row (tsp id, socket, job dir);
       - **slot-check-error** — the reason (step 3's socket `Error` or step 5's scan failure);
       - **lock-busy**;
       - **upload-mismatch** — missing / extra / differing names (step 4's comparison only; its other
         failures, e.g. listing the dir, stay ordinary refusals).

       Rust parses them into `SubmitOutcome::NotClaimed` reasons with a typed `Refusal`
       (`Blocked { blockers }`, `SlotCheckFailed(why)`, `LockBusy`, `UploadMismatch`, `Other(why)`).
       A blocker whose job dir is `<root>/jobs/<id>` of a local job is shown by that job's title; any
       other (including a job dir outside `<root>/jobs/`) by pid and cores. At the **3rd** refusal the
       record's notice reads "Waiting for the slot: held by <blockers>" (or "the slot could not be
       checked: <why>"); before it, the view shows "Waiting for the slot" only. The pending job keeps
       waiting until the slot frees or the user cancels. `submit.sh` is stdin-fed (`STDIN_SCRIPTS`),
       so the change needs no reinstall (round 1 LOW-2).
    8. **The user's actions on a pending row:** **Cancel** (the one remote Cancel, o22.7, with its
       confirm): delivery takes the not-on-server branch. Retry submit is **not offered** while
       pending (the poller re-checks every period), and the Rust `retry_remote_submit` **refuses** on
       a pending row ("a submit is pending; the slot is re-checked every period") as the belt (round 1
       LOW-7). There is no local withdraw (m2's sentence superseded, marked). While the server is
       unreachable, Cancel stays pending and is delivered on reconnect, before any re-check.
    9. **Every attempt writes `last_contact`** (drift with o22.8 found by the archaeology): the first
       Create & Run attempt (`SshBackend::submit_attempt`, today a hard-coded `SystemRunner`) gains a
       runner seam and runs through the `ContactRunner` like the retry; the pending re-check runs
       inside the poller's `record_step`.
    10. **Split.** **Part A** (Rust, tested; then STOP): v22; the `submit.sh` reply kinds with their
        Lab tests; the typed `Refusal`; the poller's re-check in `run_status` (after the pending-
        cancel branch, before `FetchStopped` and the label); the counter; the record fields and a
        `StatusStep` for the re-check with its TS mirror (types only); the NULLs and emits; the
        `retry_remote_submit` refusal; 23.9's contact. **Interim (Part A alone):** the view shows a
        pending row's record as today's queued row — no "Waiting for the slot" badge yet, and Retry
        hidden only because the Rust command refuses; accepted while uni is offline. **Part B**: the
        view (badge, notice, blockers by title, Retry hidden by the offer rule), then Anton's live
        gate and the uni run.
    11. **Negative controls** (each must turn its test red):
        - a blocked attempt that writes `.tsp-out` or the claim (Lab); a slot-busy answer that does not
          make the row pending; a step-3 socket `Error` that ends it instead (MED-5);
        - a transport failure that ends the pending state or counts; an `Unknown` that ends it; a
          lock-busy that counts; a non-slot answered refusal or a local plan refusal that keeps it;
        - a terminal write that keeps the flag (the CHECK); `record_delivered` (unchanged-status
          branch) or `record_ended_first` keeping it; a later refusal overwriting
          `submit_pending_since`; a flag write that emits no `job:status`;
        - the re-check placed before the pending-cancel branch; a re-check that uploads when the
          post-condition passes, or never re-uploads on an upload mismatch; a re-check that reuses
          the old args after a mask change (o14.2's nprocs post-condition);
        - the notice before the 3rd refusal; after a restart, the notice before the 3rd new refusal;
          a blocker mapped to the wrong local job (a job dir outside `<root>/jobs/`);
        - `retry_remote_submit` accepted on a pending row; a first Create & Run that writes no
          `last_contact` (through the new runner seam); a free slot that does not enqueue at the
          next step.
    12. **Propagation** (marked where they stand): l ("5.4 tells the user after 3 refusals"); m2 (the
        re-check "only reads state", "withdraw the pending submit", the counter's home); o22.1's
        v-order; o22.8's `last_contact` writers. Moving with the code: `modules/remote-jobs.md`,
        `modules/execution-backends.md`, `modules/frontend.md`, ROADMAP 5.4c.
    13. **Clarifications from DESIGN round 2** (2026-10-19; 13.1–13.3 decided by Anton, the rest
        settled by the orchestrator). Where these differ from items 1–12, these win.
        1. **No pending loop is silent** (Anton, round 2 MED-A). Besides slot busy and slot check
           error, two more answers **stay pending and are counted**, each toward the 3rd-refusal
           notice with its own text: **lock busy** ("Waiting: the account lock is held by <the
           lock's openers>") and **an unreadable answer** — the server answered (the call's contact
           `Ok`) but the reply did not parse ("Waiting: the server's reply could not be read:
           <why>"). The notice shows the most recent kind. The job keeps waiting; Cancel is the exit.
        2. **Transient refusals stay pending** (Anton, round 2 MED-B). A **timeout of a bounded child
           before the claim** (rc 124 of `realpath`, the wrapper's sha, `find`, `sha256sum`, the
           scan) is a **slot check error**: pending, counted. A **local** failure of the re-check —
           a DB or IO error in `write_local_dir` (e.g. disk full) — is `CheckFailed`: pending, not
           counted. The `busctl` timeout stays **fail-closed** as today (`refuse_kup`: it clears the
           profile's verified stamp and so ends every pending job of that profile via 13.3) —
           accepted: safe, if annoying.
        3. **An unverified profile ends the pending state** (Anton, round 2 MED-C). Editing the
           mask (or any target field) clears the stamp (`server_profiles.rs`), so the next re-check's
           `run_target` refuses locally and the pending state ends with "profile not verified"; after
           re-verifying, the user's Retry submit starts again (and goes pending again if the slot is
           still busy). The 23.11 mask-change control becomes: a re-check after a mask edit that
           submits instead of ending.
        4. **"No answer" is the failing call's own contact** (round 2 MED-D, LOW-3). The step
           classifies by **outcome first**: the `submit.sh` outcomes of 23.7 / `Unknown` /
           enqueued; "no answer" applies only to the calls **before** `submit.sh` (prepare,
           install) and to the one re-upload, and is decided from that call's own contact
           (`Failed`, or none for rsync 12), not from `ContactRunner::last()`. A submit call that
           times out or exits 255 is `Unknown` (stays pending, 23.6), never both.
        5. **The local dir is rewritten only when the derived bytes differ** (round 2 LOW-1):
           `write_local_dir` compares before writing, so a per-period re-check writes nothing
           locally while the plan is unchanged.
        6. **No upload in the step right after an `Unknown`** (round 2 LOW-2): that step runs
           `submit.sh` only; an upload mismatch there ends nothing and is retried at the next step.
           So a lagging first call cannot have its dir overwritten by the re-upload.
        7. **Delivery's branch is the label's** (round 2 LOW-4): a pending cancel goes to delivery,
           which labels first; for a job never enqueued that is the not-on-server branch. A cancel
           recorded **during** a re-check does not stop that re-check's `submit.sh`; the next step
           delivers it (the job may start briefly). Accepted: the one remote Cancel then sweeps it
           like any running job.
        8. **The reply kinds, precisely** (round 2 LOW-5): `slot_scan` hands its blockers to `main`
           through a byte-exact channel (length-prefixed records, not `%q` lines), since a cwd can
           hold a newline; the queued-row record carries the mask too, and its fallback text is "tsp
           row <id> on <socket>"; slot-check-error from step 3 covers all three branches (the socket
           absent or unreadable in `/proc/net/unix`, `tsp -l` failed, the header mismatch); the
           tests that move off reason text move with it (`call_script_tests.rs`,
           `ssh_backend/tests.rs`, `remote/submit.rs`). An upload mismatch made only of **extra**
           files cannot be fixed by an rsync without `--delete`: its one retry fails and the pending
           state ends — correct, and stated.
        9. **Writers and controls** (round 2 LOW-6): the local-plan-refusal NULL is its own guarded
           write (before `record_attempt`); `record_attempt` gains a sink for its emit. Added
           controls: a v22 CHECK test (a flag on a local, `running` or `draft` row is rejected;
           `SCHEMA_VERSION` = 22); an Undo that clears the flag; a lock-busy loop that reaches its
           notice; an unreadable answer that ends the pending state or never notifies; a DB/IO error
           that ends it; a re-upload ended by rsync 12 treated as an answered refusal; a rewrite of
           an unchanged local dir; an upload in the step after an `Unknown`.
    14. **Clarifications from the Part A implementation** (2026-10-19; found by the Part A
        implementer, settled by the orchestrator; reviewed with Part A's CODE verification).
        1. **An unreadable reply is not a `Refusal`.** It does not prove nothing was claimed, so it
           is a wait kind (`Wait::Unreadable`): `SubmitOutcome::Unknown` with the submit call's own
           contact `Ok`. A lost reply (contact `Failed`) is `Lost`. Both stay pending (23.6, 13.1).
        2. **The attempt that makes a job pending is refusal 1.** The notice comes at the 3rd
           refusal overall (l, m2: "after 3 refusals"); after a restart, at the 3rd new one.
        3. **The re-check's plan leaves out the retry's pending-cancel refusal.** The pending-cancel
           branch already runs first (o22.13.1); a cancel recorded after the step's read must not
           *end* the wait (then an Undo could not resume it, 23.6, 13.7) — the next step delivers it.
        4. **Any plan error other than a local IO/DB error ends the wait** (broader than 23.6's list),
           so nothing deterministic loops silently; an `Unknown` with no call made ends it too.
        5. **The lost-reply mark is cleared by the next *answered* `submit.sh`**, not strictly by the
           next step — so 13.6's "no upload right after an `Unknown`" holds across transport failures.
        6. **rc 137** (`timeout`'s kill-after) stays an ordinary refusal **for the children whose
           other failures are ordinary refusals** (realpath, the wrapper's realpath and sha256sum,
           `find`, the upload's sha256sum): only rc 124 is measured (probe B0 item 5; rule #10). For
           the slot's `tsp -l` and the scan, any non-zero rc — 137 included — is a slot check error,
           as 23.1 requires.
        7. On the command path `job:status` may be emitted twice (`run_guarded` and
           `record_attempt`'s sink): harmless, a reload is idempotent.
        8. The slot-check-error notice reads "Waiting: the slot could not be checked: <why>"; Part B
           may reword the view texts.

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
  still apply to the rsync phases.) **o22.5** adds the transient **`Cancelling`** (remote only, unit 5.4a);
  `Lost` comes with 5.4b.
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
- **Job survival depends on a host setting** (Amended 2026-10-03, probe review). A tsp job outlives
  the ssh session only because logind leaves the abandoned session scope alone —
  **`KillUserProcesses=false`** (probe A). This is not a guarantee given by tsp. So the ADR-023
  **connection test checks `KillUserProcesses=false` as a mandatory precondition**: a profile whose
  host would kill user processes at logout is not offered as a run target. *Candidate hardening (not
  now):* enable linger for the profile user and run the tsp daemon as a `systemd --user` service. That
  would take the queue out of the login-session scope and remove the dependency.

## Open questions

- **(o19) Does Tauri deliver an event emitted before a command's response ahead of that response?**
  Events and command responses travel different IPC paths; the order is unmeasured. o19.6 does not
  depend on it (the seq comparison), and no other design relies on this order (o16.7 needs only the
  listeners registered before the emit). Measure in B4 or B3's live gate if it ever matters.
- **(o17) Fetch a started-then-cancelled job's evidence?** Classifier row 4 (`Cancelled`) includes a
  job that ran and wrote `.exit_code` before the cancel; o17.1 makes it terminal `cancelled` with no
  download, so its output stays on the server. Rarely reachable in 5.3; in 5.4 every cancelled
  running job lands here. Options: keep (no download) or treat `Cancelled` with `.started` /
  `.exit_code` as a fetching outcome that still ends `cancelled`. **Anton decides at 5.4.** *(Decided 2026-10-17, o22.6: fetched as evidence when `.started` exists; ends `cancelled`.)*
- **(e) `lost`-restart seed selection.** How restart-from-last-geometry picks and validates its
  seed (last `_trj.xyz` frame? last `.xyz`? a post-condition that the geometry is sane?), given
  it bypasses `resolveCarryForwardGeometry`. Unresolved.
- **Exact nightly cutoff window in UTC** — stated as 08:00–22:00 Europe/Kyiv, to confirm
  (uni-server.md).
- **UPS → clean shutdown** — does the UPS signal the server (`lsusb` / `nut`)? Affects how often
  `lost` actually occurs (uni-server.md).
- **Parallel slots on the university server** — whether 2×12-core slots are safe is unmeasured;
  the profile stays at 1 slot until a rule-#10 measurement (f, uni-server.md). *Partly measured
  2026-10-03:* the **binding** half is settled (two concurrent queues with masks `0-11`/`12-23` stay
  inside their masks — [task-spooler-uni-probe.md](task-spooler-uni-probe.md#probe-d--two-queues-two-masks-decision-b-rule-8-not-performance)).
  The **throughput** half is still unmeasured.
- **(a) Does the `tsp` daemon survive the ssh session that launched it exiting?** (Amended
  2026-10-02, review.) **Resolved by probe 2026-10-03:** **yes**. A job was enqueued over a one-shot
  ssh, the ControlMaster was closed, and the job ran on for over 2 minutes with no connection, then
  finished normally. The reason is logind's `KillUserProcesses=false` (the session scope is left
  `abandoned`), not tsp itself — see Decision b's 2026-10-03 amendment and
  [task-spooler-uni-probe.md](task-spooler-uni-probe.md#probe-a--survival-after-the-ssh-session-exits-open-question-a).
- **(b) `tsp` behaviour across a server restart** (Amended 2026-10-02, review). We assert the
  in-memory queue is lost (Decision b) and reconcile via `never-started` (Decision d), but the
  exact post-reboot `tsp` state — does the daemon auto-restart, does `tsp -C`/socket survive,
  are partially-written job dirs left clean — is **unmeasured** (probe). *Still open after the
  2026-10-03 probe* (it needs an author-run reboot). Measured since then: a queued task also holds a
  live runner process, and `TS_SAVELIST` saves the queue only on SIGTERM of the server (`man tsp`).
  **Amended 2026-10-03 (probe review) — covered by simulation.** The reconciliation logic does not
  need a real reboot to be tested. In the implementation unit, (1) **`tsp -K`** simulates the lost
  in-memory queue — queued jobs must reconcile to `never-started` and re-enqueue at most once — and
  (2) a **substituted `boot_id`** in `.started` simulates "this run began in an earlier boot" — the job
  must reconcile to `lost`. A real restart is observed at the **first natural occasion** (e.g. a power
  event) and recorded then. It is **not** triggered on purpose: an unattended HP ProLiant may stop at
  POST after a reboot, and nobody has physical access to it.
- ~~**(d′) Running-cancel classification and the cancel/start race**~~ — **Resolved 2026-10-03.**
  Resolved by the "d′ resolution" amendments:
  - one cancel script for every state that writes `.cancelled` first (Decision i);
  - a wrapper that checks `.cancelled` right after writing `.started` (Decision b);
  - `.cancelled` classified as `cancelling` / `cancelled` regardless of `boot_id` (Decision d).
  The race is closed by the mirror-image write-then-check order on one local filesystem.
- **(c) Process-group kill for an MPI job over SSH** (Amended 2026-10-02, review). **Resolved by probe
  2026-10-03** (mechanism later revised by Decision l: no `tsp -k`): `tsp -k` plus a session-id sweep (Decision i's 2026-10-03 amendment) left no survivors,
  including when `mpirun` and the ranks were SIGSTOPped. This differs from the laptop (`debugging/004`) —
  see [task-spooler-uni-probe.md](task-spooler-uni-probe.md#probe-c--cancel-open-question-c).
