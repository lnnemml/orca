# ADR-024: Remote execution under intermittent connectivity

**Status:** Accepted · 2026-10-02 (Proposed → Accepted after the review + acceptance amendments below) · amended 2026-10-03 (probe; probe review; d′ resolution — ready for implementation; l — unit 5.2 script and classifier shape)

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
1. write `.started` — still the first action (atomically, via a temp file + `rename`); *(superseded by Decision l: step 0 is now `cd "$job_dir"`, and a self-check follows step 1)*
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
`rename` in the same directory on one local filesystem, so visibility is immediate. The argument would
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
**dedicated user without sudo**. OrcaStudio's server root is **`/home/<user>/.orcastudio/`**: the tsp
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
    by an uploaded script (`while IFS= read -r -d '' a`). That form preserved every argument,
    including an embedded newline and an empty one, and it does not depend on the remote login shell.
    `printf '%q'` into one command string also worked, but needs bash as the remote login shell, so it
    is not adopted.
- **Upload: content-addressed, never overwritten** (review M1). 5.3 uploads each script as
  `/home/<user>/.orcastudio/bin/<name>-<sha256 prefix>.sh` via a temp file + `rename`, then checks
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
- **Every marker is written by temp file + `rename`** — `.started`, `.cancelled`, `.enqueued`, and
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
    a zombie, per probe 5.2c) is **not** in the job session; this is not an `Error` (round 5 LOW-3);
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
      whole space-separated token. **Job-dir paths must match `[A-Za-z0-9._/-]+`**, which the submit
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
  `.orcastudio/bin/` **and** carries **this job's dir** as its positional argument. Without the second
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
  | 1 | `.started` exists but does not parse (empty or missing fields — a disk-full `rename` can publish an empty file) | `Failed` ("corrupt `.started`"). The cancel sweep is impossible without `sid`; the reason says so. |
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
  (round 2 LOW-4). After the `rename`, the wrapper reads `.started` back and parses it. If that fails,
  it exits without launching ORCA and writes `.exit_code` = 97 (by `rename`), so row 1 or row 6
  explains the failure. Exit code 97 is our own choice, not an ORCA code.
- **The outcome type is the classifier's own enum:** `Queued`, `Running`, `Completed { late_cancel }`,
  `Failed { reason }`, `Lost { orphans }`, `Cancelling`, `Cancelled`, `Indeterminate`, `ReEnqueue`.
  `Lost` and `Cancelling` join `JobStatus` only in 5.4, so 5.2 does not change `JobStatus`.
- **The re-enqueue count is an input.** The `jobs` column (schema v19) lands in **unit 5.4**. The bound
  holds only if 5.4 **persists the increment before** it issues the re-enqueue (review M5). Otherwise
  a crash between `tsp` submit and the DB write would re-enqueue again, without limit.

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
  - **Also for 5.3:** tsp writes a `/tmp/ts-out.*` file per task (P4); not yet prevented.
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
