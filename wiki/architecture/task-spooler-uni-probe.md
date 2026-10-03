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

> **Note (2026-10-03):** ADR-024 Decision l later **replaced `tsp -k`** in the running-cancel path.
> It now sends TERM to the verified wrapper's process group and runs a cwd-filtered SID sweep with a
> SID-reuse guard. The measurements above stay valid; only the adopted mechanism changed.

## Probe 5.2 — argv, cmdline fixtures, script replacement, `tsp -l` (2026-10-03)

Settles the four facts ADR-024 (l) depends on (DESIGN review round 1: H1, H3, M1, M2). Measured on
`uni` (user `anton`, tsp 1.0.1, bash 5.2.21) under `/home/anton/.orcastudio/probe-5.2/`, with its own
`TS_SOCKET`s, never the default socket; P3 was also run on the laptop (bash 5.2.21). Cleaned up: every
probe daemon was stopped with `tsp -K`, and survivors were killed by PID.

**P1 — argv.**
- **`tsp` execs the argv verbatim.** A script uploaded with `ssh uni 'cat > f' <<'EOF'` ran
  `tsp bash …/args-wrapper.sh 'a b' '$HOME' "it's" ';' '*' '' 'x;touch …/INJECT' '`id`'`. The stub
  (`printf '%s\n' "$#" "$@"`) recorded `8`, then each argument byte-for-byte. No `INJECT` file was
  created.
- **The ssh hop is the lossy step.** `ssh uni bash $W 'a b' '$HOME' "it's" …` failed with
  `unexpected EOF while looking for matching '''`. Without the quote, the wrapper saw
  `3 / a / b / /home/anton`, and `*: command not found` was printed: word-splitting, expansion and
  command separation all happened. This is injection-capable.
- **Safe forms:**
  - a NUL-separated list on stdin — `printf '%s\0' … | ssh uni 'bash nul-reader.sh'`, read with
    `while IFS= read -r -d '' a`. It preserved all 9 arguments, including an embedded newline and an
    empty one;
  - `printf '%q '` into one command string — also preserved them, but depends on bash being the
    remote login shell.

  ADR-024 (l) adopts stdin.

**P2 — cmdline fixtures** (verbatim, NUL → `|`). The wrapper was launched as
`tsp bash $D/bin/wrapper.sh $D/jobs/j1 0-3 /opt/orca`:
```
bash|/home/anton/.orcastudio/probe-5.2/bin/wrapper.sh|/home/anton/.orcastudio/probe-5.2/jobs/j1|0-3|/opt/orca|
```
Its child, started as `taskset -c 0 sleep 60`, shows `sleep|60|`. `taskset` execs: the child's `comm`
is `sleep`, its `Cpus_allowed_list` is `0`, and its PID equals `$!`. `ps -o pid,ppid,pgid,sid,cmd`:
```
 376681  376679  376681  376681 bash /home/anton/.orcastudio/probe-5.2/bin/wrapper.sh /home/anton/.orcastudio/probe-5.2/jobs/j1 0-3 /opt/orca
 376682  376681  376681  376681 sleep 60
```
- The parent, 376679, is the per-task tsp runner. `tsp -p` printed the wrapper PID.
- The tsp **daemon's** argv is rewritten to the first enqueued job's command (`tsp bash …`), and each
  runner shows as `tsp <job argv>`. No sweep may assume a fixed daemon cmdline or anchor on
  `taskset`.

**P3 — replacing a running bash script** (gotcha). Test script: `sleep 3; echo A` and then `echo B`,
`C`, `D`. The file was replaced at t = 1 s.
- **In-place overwrite** (`cp new s.sh` or `cat new > s.sh`; the inode is unchanged): bash resumes at
  its old byte offset **in the new file**.
  - Aligned new content: `A NEW-B NEW-C`, so the new lines ran.
  - Misaligned new content on the laptop: `A`, then a parse error.
  - Misaligned new content on uni: `A`, then `XXXXXXXXXXXXXXXXX: command not found`, so a mid-line
    fragment ran as a command.
- **`mv` over the script** (new inode): the old content ran to the end (`A B C D` / `A B C`).
- **Rule:** deploy scripts by temp file + rename, or under a new name, never by `cp`/`>` over a script
  that may be running.
- Not run: `rsync`'s default (temp + rename), hardlinks, NFS.

**P4 — `tsp -l` format, and the daemon restart.** Output under a non-tty `ssh uni 'bash p4.sh'` with 1
slot, verbatim:
```
ID   State      Output               E-Level  Times(r/u/s)   Command [run=1/1]
3    running    /tmp/ts-out.S0SWpm                           bash -c sleep 100 /home/anton/.orcastudio/probe-5.2/jobs/job-with-a-very-long-name-0123456789-0123456789-0123456789-0123456789-0123456789-0123456789-0123456789-0123456789-0123456789 4-7 /opt/orca
4    queued     (file)                                       bash -c sleep 100 /home/anton/.orcastudio/probe-5.2/jobs/j_queued 8-11 /opt/orca
0    finished   /tmp/ts-out.QG5j6G   0        0.00/0.00/0.00 bash -c exit 0 /home/anton/.orcastudio/probe-5.2/jobs/j_ok 0-3 /opt/orca
1    finished   /tmp/ts-out.NlzNMk   3        0.00/0.00/0.00 bash -c exit 3 /home/anton/.orcastudio/probe-5.2/jobs/j_fail 0-3 /opt/orca
2    finished   /tmp/ts-out.JzAIe7   -1       1.00/0.00/0.00 bash -c sleep 100 /home/anton/.orcastudio/probe-5.2/jobs/j_killed 0-3 /opt/orca
```
- **Rows:** ordered running → queued → finished, not by id.
- **E-Level:** `0`, the exit code, or `-1` after `tsp -k`.
- **No truncation:** the 256-char line came through intact, and the output was identical under
  `COLUMNS=40` and under `ssh -tt` with `stty cols 60`.
- **The command is argv joined by single spaces, unquoted**, so it is lossy for arguments with
  spaces. A job dir is matched as a whole token, and job-dir paths must contain no whitespace.
- `tsp -s <id>` prints the state alone.
- **Output files:** the Output column is a `/tmp/ts-out.XXXXXX` file per task, which litters `/tmp`.
  5.3 must stop that, e.g. with a `TMPDIR` for `tsp` itself or the no-output option — **not probed**.
- **`tsp -K`, then a fresh daemon on the same socket path:**
  - `tsp -K` returned rc 0 and removed the socket.
  - The **running** job survived: its runner and its `sleep` stayed alive.
  - The **queued** task's runner died.
  - The fresh `tsp -l` showed only the header, so the old rows were gone, and the next id was **0**.
  - `TS_SAVELIST` was not tested.

## Probe 5.2b — missing/stale socket, session cwd (2026-10-03)

Settles DESIGN round 3 MED-A and LOW-C for ADR-024 (l). Measured on `uni` (user `anton`, tsp 1.0.1)
under `/home/anton/.orcastudio/probe-5.2b/` with dedicated sockets. Cleaned up: every daemon was
stopped with `tsp -K`, and the only SIGKILLs went to server PIDs the probe itself had started.

**Q1 — `tsp` on a missing or stale socket.**

| Case | `-l` | `-s 0` / `-r 0` | Daemon started? |
|---|---|---|---|
| path absent, parent exists | rc 0, header only | rc 255, `Error in the request: The job 0 cannot be stated.` / `… removed.` | **yes** (ppid 1, own SID; socket created) |
| stale socket file (its daemon was SIGKILLed; the file survives) | rc 0, header only | rc 255, same text | **yes** (socket rebound) |
| parent dir absent | rc 255, `The server didn't come up.` | same | no |

- `-s`/`-r` give the same text for "no daemon" and "no such job", so rc cannot tell them apart.
- **Telling a stale socket from a live one without spawning:**
  - `test -S` is true for both, so it cannot.
  - Both `ss -xlp` and `/proc/net/unix` were read. A **live** socket appears in both, as
    `ss`: `u_str LISTEN … …/live.sock … users:(("tsp",pid=…))` and `/proc/net/unix`:
    `…: 00000002 00000000 00010000 0001 01 2960949 …/live.sock`. A **stale** one appears in neither
    (nor in `ss -xa`).
  - A Python `AF_UNIX` connect gets `ECONNREFUSED` on a stale socket.
- **Rule:** never run `tsp` to test liveness. Socket paths stay short: `sun_path` is 108 bytes including the NUL (sourced: `man 7 unix`
  *"char sun_path[108]"*, `linux/un.h` `UNIX_PATH_MAX 108`; not measured), and
  `ss` truncates long ones.

**Q2 — cwd of every process in a job session.** Wrapper:
`cd "$job_dir"; OMPI_MCA_hwloc_base_binding_policy=none HWLOC_COMPONENTS=-gl TMPDIR="$job_dir/.tmp" taskset -c 0-3 /opt/orca/orca input.inp`,
launched via `tsp`. Two runs, sampled in a tight loop:
- (a) water `! HF def2-QZVPP`, `%pal nprocs 4`, 18 samples, 8.2 s;
- (b) water `! HF def2-TZVP NumFreq`, `%pal nprocs 4`, 34 samples, 14.6 s.

Both ended with exit 0 and `ORCA TERMINATED NORMALLY`.

These members had cwd **exactly the job dir** in every sample, and **all of them had the wrapper's
SID**:
- the wrapper;
- `orca`;
- `sh -c -- mpirun`, then `mpirun`;
- the ranks `orca_{guess,startup,util,prop,leanscf,scfgrad}_mpi`;
- `orca_numfreq` (whose parent is `orca`, wrapper PGID/SID);
- the per-displacement `orca_leanscf input_D000NN.gbw`.

An independent scan of `/proc/*/cwd` found no process outside the SID with the job dir as cwd. A few
empty `sid=` rows were processes that exited mid-sample (`/proc/<pid>/cmdline: No such file`), not
escapes.

**Consequence:** on this host, a cwd sweep and a SID sweep select the same set.

**Not measured:** Opt/Freq, other ORCA tools, and processes that `chdir`/`setsid` themselves.

## Probe 5.2c — `/proc/<pid>/stat` start time, zombies (2026-10-03)

Supports ADR-024 (l) N-2: the SID-reuse guard compares start times. Measured on the laptop (Linux
6.14, bash 5.2.21). On uni (Linux 6.8.0, via tsp) only the parts noted below were measured.
- **Reading its own stat from bash.**
  - Correct: `read -r l </proc/$$/stat` (and `/proc/self/stat` via a builtin redirect in the main
    shell).
  - Wrong: `/proc/self` read inside `$(…)`, via `cat`, or via `readlink`. Each reports the child's
    PID.
- **Parsing.** A script named `w q) x.sh` gave the line
  `66186 (w q) x.sh) R 66182 66186 66182 0 -1 … 1890507 …`. The parse
  `rest="${l##*) }"; set -- $rest; state=$1 pgrp=$3 session=$4 starttime=${20}` returned
  `state=R pgrp=66186 session=66182 starttime=1890507`. Field N = token N−2 after the last `) `.
- **Zombie** (a perl parent that does not wait):
  - live: `66401 (bash) S … 1891976 13090816 991 …`;
  - zombie: `66401 (bash) Z … 1891976 0 0 …`.

  So field 22 is **unchanged**. In the zombie state:
  - `/proc/<pid>/status` shows `State: Z (zombie)`;
  - the cmdline is 0 bytes (30 when live);
  - `readlink /proc/<pid>/cwd` fails with ENOENT;
  - `ls -d /proc/<pid>` still succeeds.

  After reaping, `/proc/<pid>` is gone.

  On uni the zombie showed the same Z / empty cmdline / cwd ENOENT, but its start time was not
  compared against a recorded live value.
- **Resolution.** `getconf CLK_TCK` = 100 (also on uni). Three reads of one live process gave the same
  value, and two `sleep`s 50 ms apart differed by 5 ticks. Forced PID reuse was not tested (`pid_max`
  is 4194304).
- **Under tsp on uni.** The same parse works: `pgrp = session = own PID`, `starttime=319395485`.

## Probe 5.3 — shipped scripts on uni (2026-10-03)

Run on `uni` as `anton` under `/home/anton/.orcastudio/probe-5.3/` (now removed), with its own
`TS_SOCKET`. Host: bash 5.2.21, coreutils 9.4, procps-ng 4.0.4, kernel 6.8.0-138, tsp 1.0.1, `strace`
present. Raw outputs were kept only in the session scratchpad (temporary). Claims marked
*(prober-reported)* below have no retained raw output.

**Q1 — the error messages the readers in `head.sh` match** (C locale). **Every ENOENT case matches**
the literal strings, and EACCES / EISDIR / EINVAL are read as errors, never as "absent" (all rc 1).
The readers' ESRCH alternatives (`cat: <p>: No such process`, `readlink: <p>: No such process`) were
**not exercised**; a different ESRCH message would fail closed.

| Reader | ENOENT | Other cases (all treated as errors, never as "absent") |
|---|---|---|
| `cat` | `cat: <p>: No such file or directory` | `Permission denied` (`/proc/1/environ`, a mode-000 file); `Is a directory` |
| `readlink -v` | `readlink: <p>: No such file or directory` | `Permission denied` (`/proc/1/cwd`); `Invalid argument` (target is not a symlink) |
| `stat` | `stat: cannot statx '<p>': No such file or directory` | `Permission denied` |
| `tail` | `tail: cannot open '<p>' for reading: No such file or directory` | `tail: cannot open '<p>' for reading: Permission denied`; `tail: error reading '<p>': Is a directory` |

- `ps -o pid= -s <sid>` on a session with no processes: rc 1 with empty stdout and stderr, which the
  readers treat as an empty session.
- `ps` with a non-numeric argument: rc 1 plus `error: process ID list syntax error`, which fails
  closed.
- An unreadable `output.out` makes the collector emit an `error` record and exit 3.

**Q2 — `ln -T`.** It fails with rc 1 `File exists` on an existing file, an existing directory and a
dangling symlink, and leaves the target unchanged. Without `-T`, `ln` onto a directory links **into**
it (rc 0). `strace` shows exactly **one `linkat(AT_FDCWD,"src",AT_FDCWD,"dst",0)`**, with no
stat/lstat/newfstatat call, and no access call on the target, before it. The only other traced call
is the loader's `access("/etc/ld.so.preload")`; `statx` was not in the trace filter. It returned
`-1 EEXIST` for an existing **file**; for the directory and the dangling symlink, EEXIST is inferred
from the `File exists` message (its strerror text).

**Q3 — the shipped scripts, end to end.** The scripts were composed exactly as `scripts.rs` composes
them and uploaded as `bin/<name>-<sha256>.sh`. ORCA was a stub. The wrapper was enqueued as
`TS_SOCKET=… TMPDIR=… tsp bash <wrapper> <job> 0-3 <stub>`.

| Step | Result |
|---|---|
| (a) run j1 | `.started` written with pid = pgid = sid; `.exit_code` `0`; `.tmp` left in place after a normal finish *(prober-reported)*. |
| (b) j1 mid-run | collect exit 0: `proc collected`, 3 members, all with cwd = the job dir, 1 `running` row. `cancel.sh check`: `alive=yes ours=yes leader=yes sid_reused=no own_session=no`. |
| (c) j1 finished | `members 0`, row `finished`, `exit_code 0`. |
| (d) j2 cancelled mid-run | `cancel.sh` printed `queued: skip no verified queued row`, then `group: TERM -<pgid>`, `sweep: TERM+CONT <3 pids>`, `tmp: removed`. Afterwards the collector saw `members 0`, `exit_code -` and `cancelled yes`; the tsp row was `finished` with E-Level −1. That `.tmp` was gone rests on the script's own `tmp: removed` line plus the prober's `ls` *(prober-reported)*. |
| (e) foreign `boot_id`, stale and absent sockets | `proc skipped`, `nodaemon` for both sockets (wire). **No tsp daemon was started**: `ps` before and after was identical, and `absent.sock` was never created *(prober-reported)*. |

- The wire files were parsed **locally with the real Rust parser and classifier** (from a scratch copy
  of the crate at HEAD `53996f8`). Results:
  - j1 mid-run → `Running`;
  - j1 finished → `Completed { late_cancel: false }`;
  - j2 before the cancel → `Running`;
  - j2 after the cancel → `Cancelled`;
  - j3 (foreign boot) → `Lost { orphans: [] }`;
  - j4 (no `.started`, no daemon) → `ReEnqueue`.
- The parser's slot consistency check correctly rejected one run in which the slot list given to
  the parser was not a prefix of the collected socket facts.

**Q4 — the `/tmp/ts-out.*` litter.**
- `tsp -n` leaves `/tmp` clean. Its Output column says `stdout`, but where a `-n` task's output goes
  was **not measured**, so it is not adopted.
- **`TMPDIR` on the `tsp` call** puts `ts-out.*` in that directory instead. It is the **client's**
  `TMPDIR` at enqueue time that counts, not the daemon's. **Proposal for 5.3** (not yet decided; the
  directory under the root and its cleanup go to Anton with 5.3): set `TMPDIR` on **every** `tsp`
  enqueue.

**Probe hygiene.** The cleanup glob `rm -f /tmp/ts-out.*` also removed one pre-existing,
anton-owned, 174-byte `/tmp/ts-out.RMtkcp` (mtime 09:41). That it was left by an earlier probe is an
inference. A stray `cp -r` to `/tmp/ignore` was removed after checking its contents
*(prober-reported)*. Lesson: delete only paths recorded at creation.
