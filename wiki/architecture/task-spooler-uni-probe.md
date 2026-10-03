# task-spooler on the uni server — survival, cancel, slot masks (probe 2026-10-03)

Rule-#10 measurement record for [ADR-024](adr-024-remote-execution-intermittent-connectivity.md)
(Open questions **a** and **c**, Decision **b** core masks, Decision **i** cancel). Every fact below
was measured on the `uni` host ([uni-server.md](../infrastructure/uni-server.md)) on 2026-10-03,
07:17–07:40 UTC, on the shared `yats` account, using the probe scripts in
`scripts/probes/uni-tsp/`. **Open question b (behaviour across a server restart) was NOT probed**
— it needs an author-run reboot and stays open.

## Setup

- `task-spooler` **1.0.1+dfsg1-1** (Ubuntu package, binary `/usr/bin/tsp`), installed by the author.
- Everything ours lives in `/home/yats/.orcastudio/probe/` — job dirs `jobs/<name>/`, sockets
  `slot0.sock` / `slot1.sock`, tsp output dir `tsp-tmp/`. The account's **default** socket was never
  used (`/tmp/socket-ts.*` did not exist before or after).
- Probe scripts (copied to `probe/bin/`):
  - `orca-job-wrapper.sh` — prototype of the ADR-024 wrapper: **first** action writes `.started`
    (`pid`, `pgid`, `sid`, `boot_id`, `started_at`); runs
    `OMPI_MCA_hwloc_base_binding_policy=none taskset -c <mask> /opt/orca/orca input.inp > output.out 2> stderr.log`
    (rules #1, #8; ADR-024 b); **last** action writes `.exit_code`.
  - `tq.sh <slot> …` — `tsp` with `TS_SOCKET=probe/slot<N>.sock` and `TMPDIR=probe/tsp-tmp`;
    `tq.sh <slot> submit <job> <input> <mask>` makes the job dir and enqueues the wrapper.
  - `ourtree.sh` — inventory of **our** processes only (tsp daemon on our socket, tsp runners with our
    `TS_SOCKET`, their descendants, cwd under `probe/jobs`, or a session id recorded in a `.started`).
    **Never matches by process name** — `yats` hosts legacy ORCA installs that may be someone's job.
  - `affinity-sample.sh <job_dir> <a-b>` — per-thread `psr` + `Cpus_allowed_list` for a job's session.
- Inputs: `water.inp` (= the `opt_freq_water` parity fixture, r2SCAN-3c Opt Freq, 4 procs);
  `benzene_numfreq.inp` (r2SCAN-3c NumFreq, 4 procs, `%maxcore 2000`); `benzene_sp_mpi.inp`
  (B3LYP/def2-QZVPP SP, 4 procs — long-lived MPI ranks, the debugging/004 shape).

## `man tsp` — the env variables 1.0.1 actually supports

Read from the server's man page (not memory): **`TS_SOCKET`** (queue socket; default
`$TMPDIR/socket-ts.<uid>`), **`TMPDIR`** (tsp output files *and* default socket dir; `/tmp` if
unset), **`TS_SLOTS`** (initial slot count, read only by the first `tsp` that starts the server),
**`TS_MAXFINISHED`**, **`TS_MAXCONN`**, **`TS_ONFINISH`**, **`TS_SAVELIST`** (queue saved to that
file **on SIGTERM** of the server only), **`TS_ENV`**, **`TS_MAILTO`**, `USER`. Relevant actions:
`-k [id]` = *"kill the process group of the named job (SIGTERM) — equivalent to `kill -- -$(ts -p)`"*;
`-K` kills the server and *"will not kill the command being run"*; `-r [id]` removes a job;
`-p [id]` prints the job's PID. `/tmp/ts.error` is a fixed internal-error path.

## Smoke test — the wrapper is correct (rule #9)

Water through `tsp` + wrapper (mask `0-3`): `.exit_code = 0`, `ORCA TERMINATED NORMALLY`,
**E = −76.418938720745 Eh — bit-identical to the parity reference**, freqs 1653.28 / 3813.59 cm⁻¹,
25.4 s wall. Wrapper environment recorded in `.wrapper_env`: `PATH` = system default (no conda,
no stale ORCA entries), `mpirun = /usr/bin/mpirun` (rule #2 holds through tsp).

## What tsp actually builds (process anatomy)

```
PID     PPID    PGID    SID     COMMAND
315807  1       315807  315807  tsp …            ← queue server (daemon), listens on slot0.sock
315808  1       315808  315808  tsp …            ← per-task "runner": a forked tsp client, parent of the job
315809  315808  315809  315809  bash orca-job-wrapper.sh …   ← PID = PGID = SID: tsp setsid()s the task
315818  315809  315809  315809   \_ /opt/orca/orca input.inp
…                                 \_ sh -c mpirun …  \_ mpirun -np 4 …
318481  318401  318481  315809        \_ orca_*_mpi   ← own PGID (mpirun setpgid), SAME SID
```

- **Every task is a session leader.** tsp starts the wrapper with `setsid` semantics: wrapper
  `PID == PGID == SID`. So the "setsid in the wrapper" the ADR considered is **already done by tsp**.
- **MPI ranks leave the process group but not the session.** Each `orca_*_mpi` rank has its own
  PGID (as in `debugging/004`) but keeps the wrapper's **SID**, and its **cwd is the job dir**
  (measured, `readlink /proc/<rank>/cwd`). So a session-id sweep and a cwd sweep both cover the ranks.
- Each **queued** task also holds a live runner process (`tsp` client with our `TS_SOCKET`), reparented
  to init — the queue is partly "in processes", not only in the daemon's memory.
- NumFreq phases spawn 4 serial `orca_numfreq` workers **without `mpirun`**, all in the wrapper's
  PGID and SID.
- **`TMPDIR` leaks into ORCA.** The `TMPDIR` set for tsp is inherited by the task, and OpenMPI puts
  its session dir (`ompi.<host>.<uid>/`) and `pmix-gds-shmem.*` files there. A normal run cleans
  them; **every killed run left 2 `pmix-gds-shmem.*` files + session-dir entries** behind.
  → The production wrapper should set its own `TMPDIR` (e.g. the job dir) so MPI litter dies with
  the job dir (rule #3), not accumulate in a shared dir.

## Probe A — survival after the ssh session exits (Open question a)

1. Fresh daemon: enqueued `a01_benzene` (NumFreq, mask `0-3`) through a **one-shot, no-tty** `ssh uni '…'`.
2. Closed the ControlMaster explicitly: `ssh -O exit uni` → `Control socket connect(…): No such file
   or directory`, and **no master process** on the laptop. (Checked beforehand that the master carried
   only our own `notty` channel — `-O exit` kills every multiplexed session.)
3. **≥2 min** (140 s) with no connection, then a new connection:
   - `tsp -l`: `running`; daemon, runner, wrapper, `orca`, 4 `orca_numfreq` workers all alive
     (ELAPSED 02:55); job progressing (displacement files up to `D00043`).
   - Job then finished on its own: `.exit_code = 0`, `TERMINATED NORMALLY`,
     E = −232.183011543644 Eh, **273.5 s wall** (`TOTAL RUN TIME 4 min 33 s`).
4. **Why it survives** — the cgroup and logind configuration:
   - daemon, runner and every job process: `0::/user.slice/user-1000.slice/session-7023.scope` —
     i.e. **the ssh login session's scope** (tsp does not escape it).
   - After logout: `loginctl list-sessions` → session 7023 `closing`;
     `systemctl status session-7023.scope` → **`active (abandoned)`**, tasks still inside.
   - `KillUserProcesses`: `#KillUserProcesses=no` in `/etc/systemd/logind.conf` (no drop-ins) and the
     effective value `busctl get-property … KillUserProcesses` → **`false`**.
   - `loginctl show-user yats -p Linger` → **`Linger=no`**. (`yats` also has a permanent graphical
     session `c1` on seat0, so its user manager is up regardless.)

   **Conclusion:** the job survives because **logind does not kill an abandoned session scope**
   (`KillUserProcesses=false`, Ubuntu's default) — **not** because of anything tsp does. The
   dependency is a host setting: if an admin ever sets `KillUserProcesses=yes` (or `KillUserProcesses`
   becomes the distro default), every tsp job dies at logout. This is a precondition to re-check in
   the `SshBackend` connection test, not a property of tsp.

## Probe C — cancel (Open question c)

Every cancel was on a fresh `benzene_sp_mpi` job, issued **15 s after `mpirun` appeared** (4 live
`orca_leanscf_mpi` ranks). Survivors counted 10 s later over the job's whole session (`ps -s <sid>`)
plus `ourtree.sh`.

| # | Mechanism | Condition | Survivors after 10 s | Job dir |
|---|---|---|---|---|
| c01 | `tsp -k <id>` | normal | **0** | `.started`, no `.exit_code` |
| c02 | `kill -TERM -- -<pgid>` (wrapper's group, from `.started`) | normal | **0** | `.started`, no `.exit_code` |
| c03 | `kill -STOP mpirun`; `killpg TERM`; 2 s; `killpg KILL` | mpirun can't forward | **0** | `.started`, no `.exit_code` |
| c05 | as c03, **ranks also SIGSTOPped** | nobody cooperates | **0** | `.started`, no `.exit_code` |
| c06/c07 | ranks SIGSTOPped, **SIGKILL to mpirun's PID only** (no group signal) | — | **0** (ranks gone < 50 ms) | **`.exit_code = 0`**, see below |

Tree before each cancel (c01; the others identical in shape):

```
    PID    PPID    PGID     SID COMMAND
 326308  326307  326308  326308 /bin/bash …/orca-job-wrapper.sh …/jobs/c01_tspk 0-3
 326317  326308  326308  326308  \_ /opt/orca/orca input.inp
 326354  326317  326308  326308      \_ sh -c -- mpirun -np 4  /opt/orca/orca_leanscf_mpi input.gbw input
 326355  326354  326308  326308          \_ mpirun -np 4 /opt/orca/orca_leanscf_mpi input.gbw input
 326358  326355  326358  326308              \_ /opt/orca/orca_leanscf_mpi input.gbw input
 326359  326355  326359  326308              \_ /opt/orca/orca_leanscf_mpi input.gbw input
 326360  326355  326360  326308              \_ /opt/orca/orca_leanscf_mpi input.gbw input
 326361  326355  326361  326308              \_ /opt/orca/orca_leanscf_mpi input.gbw input
```
After: `remaining in sid: 0`; `ourtree.sh` lists only the queue daemon.

**How the ranks die (measured, c04/c06/c07).** In c03/c05 the group signals never reach the ranks
(own PGIDs), yet they die. Timeline: with mpirun stopped and then SIGKILLed via its group, ranks were
still `Rl` at +0.5 s and gone at +1 s (c04). With the **ranks themselves SIGSTOPped** (state `Tl`) and
**only mpirun's PID** SIGKILLed, the ranks were gone at the first sample, **+50 ms** (c07). A stopped
process cannot die from any signal except SIGKILL, and we sent none to the ranks → **the kernel
delivers SIGKILL to the ranks when their parent `mpirun` dies.** This is the behaviour of a
parent-death signal (`PR_SET_PDEATHSIG`); that OpenMPI 4.1.6 sets it is **inferred from this
behaviour**, not read from source.

Consequences:
- On this host **any** mechanism that kills `mpirun` (graceful forwarding *or* SIGKILL) takes the
  ranks with it. The ADR-assumed failure mode (ranks orphaned by a group kill) **did not reproduce**.
- This **differs from `debugging/004`** (laptop, 2026-07-28), where SIGSTOP-ing mpirun + group
  TERM/KILL left the ranks running. The difference is **not explained** — it was not re-measured on
  the laptop in this unit. So "ranks always die with mpirun" is a uni-host fact, not a general one.
- **Killing only `mpirun` makes ORCA write `ORCA finished by error termination in LEANSCF` and exit
  with status 0** → the wrapper recorded **`.exit_code = 0` without `TERMINATED NORMALLY`** (c06, c07).
  Recorded in [orca/gotchas.md](../orca/gotchas.md). Rule #6 (completion = `.exit_code` **and** the
  normal-termination marker) is exactly what keeps this from reading as `completed`.
- `tsp -l` shows a killed job as `finished` with **E-Level −1** (c01, c02, c08); it does not
  distinguish "cancelled" from "crashed" — the local `Cancelled` record (ADR-024 i) remains the
  authority for an app-initiated cancel.

**Mechanism adopted for Decision i (measured):** `tsp -k <id>` (= SIGTERM to the wrapper's group =
its session leader's group) is sufficient in every case measured. As a belt-and-braces step that does
**not** depend on OpenMPI's behaviour (the laptop counter-example), follow it with a **session sweep**:
read `sid` and `boot_id` from `.started`; if `boot_id` equals the current one and `sid` is not the
sweeping shell's own session, `kill -TERM` + `kill -CONT` every PID in `ps -o pid= -s <sid>`, wait,
then `kill -KILL` whatever remains. The session sweep is the remote analogue of the local cwd sweep
(`debugging/004`) and is cheaper (one `ps -s` instead of a `/proc/*/cwd` walk); both cover the ranks.
Coverage, demonstrated on the c01–c05 trees: `killpg` reaches `{wrapper, orca, sh, mpirun}`; the
session contains those **plus the 4 ranks**. (A rank that outlives `mpirun` could not be produced on
this host — the negative control c05 shows even uncooperative ranks die — so the sweep's
*necessity* here is argued from the laptop case, not shown on uni.)

**Queued cancel (C.3).** `c08` running + `c09` queued behind it; `tsp -r <c09>` → rc 0, `c09`
vanished from `tsp -l` **and its runner process exited**; after `c08` was killed the queue moved on to
nothing. `c09`'s dir contains **only `input.inp`** — no `.started`, never ran. That is exactly the
on-disk shape ADR-024 d calls `never-started`, so the app must mark a cancelled-while-queued job
`cancelled` locally *before* the next reconcile, or the reconciler would read it as `never-started`
and re-enqueue it.

## Probe D — two queues, two masks (Decision b, rule #8; NOT performance)

Two `benzene_sp_mpi` jobs at once: `slot0.sock` mask `0-11`, `slot1.sock` mask `12-23` (physical cores
of NUMA node0 / node1). Two independent daemons (slot1's first job got tsp id 0). Six samples of every
thread in each job's session, 5 s apart (`affinity-sample.sh`), plus `ps -L`:

```
07:33:39 d01_slot0: threads=18 psr={0,1,2,3,4,5,6,8,9,10,11,45} allowed={0-47;0-11;} violations=1
  VIOLATION pid=356717 comm=orca-job-wrappe psr=45 allowed=0-47
07:33:40 d02_slot1: threads=18 psr={12,13,14,15,16,17,18,20,21,22,23,44} allowed={0-47;12-23;} violations=1
  VIOLATION pid=356728 comm=orca-job-wrappe psr=44 allowed=0-47
… (identical in all 6 samples)
-- d01_slot0 busy threads:  orca_leanscf_mp psr 1,0,4,3   (~99 % each)
-- d02_slot1 busy threads:  orca_leanscf_mp psr 14,16,15,13 (~99 % each)
```

- **Every thread of `orca`, `mpirun` and all ranks** (incl. their helper threads) had
  `Cpus_allowed_list` **exactly the slot mask** and ran only inside it, in all samples.
- `mpirun`'s environment carried `OMPI_MCA_hwloc_base_binding_policy=none` and its own affinity was
  the mask; OpenMPI did **not** narrow or re-bind any rank (allowed list stayed the whole mask, not a
  single core). **taskset holds; OpenMPI with `binding_policy=none` respects it.**
- The only out-of-mask thread is the **wrapper's own `bash`** — it is `taskset`'s *parent*, so it is
  not pinned (`allowed=0-47`). It sleeps in `wait` the whole run (no CPU), so it does not violate the
  no-shared-cores intent, but a production wrapper that wants a clean invariant can pin itself first
  (`taskset -cp <mask> $$`) or be launched as `taskset -c <mask> wrapper`.
- Throughput/timing of concurrent slots was **not** interpreted (separate unit). The ADR-024 f
  "1 slot until measured" default is unaffected by this probe.

## Cleanup (verified)

`tsp -k` on the Probe-D jobs, then `TS_SOCKET=… tsp -K` for each of our sockets: both sockets gone;
`ourtree.sh` empty; an independent scan of every `/proc/<pid>` for cwd under `probe/` or our
`TS_SOCKET` in the environment → empty. Removed 654 `*.tmp`/`*.gbw` files from the job dirs and the
stale `pmix-gds-shmem.*` + `ompi.*` leftovers from `tsp-tmp/` (1.3 G → 51 M). Job dirs kept on the
server for inspection.

## Side findings

- **Server clock is ~2 min 52 s ahead of the laptop and `NTPSynchronized=no`** (laptop: `yes`). The
  boot-id liveness logic does not use time, but any cross-host time comparison (`started_at` vs the
  laptop's clock, "stale for N minutes") would be skewed. Recorded as an open item in uni-server.md.
- The server runs in `Etc/UTC`; the 08:00–22:00 Kyiv window is 05:00–19:00 UTC in summer time (EEST).
