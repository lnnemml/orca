# ADR-024: Remote execution under intermittent connectivity

**Status:** Accepted · 2026-10-02 (Proposed → Accepted after the review + acceptance amendments below) · amended 2026-10-03 (probe; probe review; d′ resolution — ready for implementation; l — unit 5.2 script and classifier shape; m — 5.3 submit rules; n — 5.1 Part B profile and connection test) · amended 2026-10-05 (o — 5.3 transport, sync and monitoring; accepted after DESIGN round 4; o13 — Part A2 amendments, accepted after DESIGN review 2026-10-05; o14 — B1 amendments, accepted after DESIGN review 2026-10-05) · amended 2026-10-06 (o15 — B1 Part B: the in-flight guard's scope, accepted after DESIGN review 2026-10-06; o16 — B2: the remote live log is pushed, accepted after DESIGN review 2026-10-06)

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
  | 1 | `.started` exists but does not parse (empty or missing fields — a disk-full write can publish an empty file, whether by `rename` or by `ln -T`) | `Failed` ("corrupt `.started`"). The cancel sweep is impossible without `sid`; the reason says so. |
  | 2 | `.cancelled` + `.exit_code` = 0 + `ORCA TERMINATED NORMALLY` | `Completed { late_cancel: true }` (Anton, M6). A clean result is never discarded, and the UI shows that the cancel came too late. |
  | 3 | `.cancelled`, `boot_id` current, and the wrapper PID is alive and ours **or** the job session is non-empty | `Cancelling` (transient). The reconcile re-runs the cancel script, which sweeps on the same trigger. 5.4 counts the sweeps. **Progress** = the job session shrinks. After 3 sweeps without progress (e.g. a D-state process), the **automatic sweeps stop**, and the job stays `Cancelling` with a user-visible notice and a manual retry (round 2 MED-2, round 3 LOW-D). |
  | 4 | `.cancelled` (anything else) | `Cancelled` |
  | 5 | `.exit_code` parses to 0 + `TERMINATED NORMALLY` | `Completed` |
  | 6 | `.exit_code` present (non-zero, empty or not a number; or 0 without `TERMINATED NORMALLY`) | `Failed` (rule #6) |
  | 7 | `.started`, `boot_id` current, wrapper PID alive and ours | `Running` |
  | 8 | `.started` (any other case: a different `boot_id`; or current `boot_id` with the wrapper dead or not ours — OOM, an outside kill) | `Lost { orphans }` (Anton, H2: at once, with no re-check). `orphans` = the job-session PIDs, which are non-empty only with a current `boot_id`. 5.4 sweeps them, with the same cwd-filtered sweep (Anton, round 2 MED-1): an orphan with no wrapper never writes `.exit_code`, so its result can never pass rule #6, and it holds the slot's cores that tsp is already handing to the next job. |
  | 9 | no `.started`, a `tsp` row in state queued or running | `Queued` (the job may be starting right now; the next reconcile sees `.started`) |
  | 10 | no `.started`, and any socket fact in the snapshot is `Error` | `Indeterminate` — no action this pass (round 2 MED-4). Re-enqueueing a job that may still be queued would put two wrappers in one job dir. 5.4 counts consecutive `Indeterminate` passes and tells the user after 3 (round 3 LOW-D). |
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
- **The re-enqueue count is an input.** The `jobs` column (schema v19 *(renumbered v20 by Decision n: v19 is the 5.1 Part B profile columns; then v21 by Decision o: v20 is the 5.3 job coordinates)*) lands in **unit 5.4**. The bound
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
      user after 3 refusals.
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
     state; it signals nothing and changes nothing.
   - After 3 refusals the user is told, and the notice **names the blocking job(s)**.
   - The user may withdraw the pending submit, cancel the blocking job, or keep waiting.
   - **No automatic action** ever frees a slot: a blocked slot almost always means something really
     is still computing on those cores (rule #8).
   - **Persistence and sequencing** (round 1 MED-9): the pending submit and its refusal counter live
     in the local DB. Unit **5.4** adds them with the v21 migration *(renumbered from v20 by Decision o: v20 is the 5.3 job coordinates)*, next to the re-enqueue counter,
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
   (re-enqueue counter, pending cancel, pending submit) move to **v21**.
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
   - **Withdraw** (Anton, round 2 fork, closes round 2 HIGH-2): a job in the **"not on the server"**
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
      refuse with that file named; the exit is **withdraw** and a new job (round 4 LOW-5).
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
      offers **withdraw** only, never retry in 5.3. A withdraw's `.cancelled` moves the job to rule 2,
      so a lost withdraw reply is not a dead end. The collector is never run on a missing dir.
   5. **5.4's `ReEnqueue` is a different variant** (round 3 MED-3): a restart-dropped job already has
      `.submitting` and `.enqueued`, so this call would refuse it. The variant asserts no `.started`,
      `.exit_code`, `.cancelled` and no row, claims a name keyed by the v21 counter
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
     `Cancelled`, `Failed{CorruptStarted}`) is **shown, with "handled in unit 5.4" where it needs an
     action**; the row stays `Queued`/`Running` (so it still blocks profile edits and deletion).
     5.3 passes `reenqueue_count = 0` (the column is v21's), so `Failed{WrapperNeverStarted}` cannot
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
   reason. Default `Local`; the last choice is remembered per project. **Nothing is deleted on the
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
    1. **Uploaded job scripts are run through one stdin trampoline** (Anton, F1). (l) uploads the
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
    5. **The local cancel path refuses a live remote job** (Anton): `local_backend::cancel` refuses a
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
    1. **The guard covers every operation on one job that reaches the server** — submit to a server,
       retry, withdraw, label, collect, fetch — **plus cancel**, local or remote *(the read-only log
       poll excepted, o16.5)*. Cancel needs it
       because of a race item 4's list leaves open: cancel reads a draft (dispatch picks the local
       backend; `local_backend::cancel`'s o14.5 entry check sees the draft and passes), a concurrent
       remote submit persists it as `Queued` with coordinates (o3.1), and the local cancel's later,
       separately locked status read sees `Queued` and records a local `Cancelled` on a live remote
       job. o14.5's entry check alone does not close this; the guard, claimed before the job is read,
       does. A local `queued`/`running` job's id is otherwise claimed only for the instant a remote
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
       moving it into the task; `cancel_job` as its first statement — and holds it until that work
       ends; a busy job is **refused** with a conflict error. The poller (B2) uses a non-blocking
       claim and **skips** a busy job. The DB lock is never held across an ssh or rsync call.
    4. Negative controls (B1 Part B, `wiki/log.md` 2026-10-06): the guard dropped at the start of the
       blocking work; the claim moved inside the task; a command bypassing the guarded path; cancel's
       claim moved below its job read — each turns a test red. Cancel's is a source-order pin (no
       `AppHandle` test harness), a weaker kind of evidence than a behavioural race test.
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
       (no watcher, nothing to emit). The "fetching outcome classified" flag lives in memory like
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
- **Job survival depends on a host setting** (Amended 2026-10-03, probe review). A tsp job outlives
  the ssh session only because logind leaves the abandoned session scope alone —
  **`KillUserProcesses=false`** (probe A). This is not a guarantee given by tsp. So the ADR-023
  **connection test checks `KillUserProcesses=false` as a mandatory precondition**: a profile whose
  host would kill user processes at logout is not offered as a run target. *Candidate hardening (not
  now):* enable linger for the profile user and run the tsp daemon as a `systemd --user` service. That
  would take the queue out of the login-session scope and remove the dependency.

## Open questions

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
