# 004-mpi-ranks-escape-process-group.md — Cancel leaves ORCA's MPI ranks orphaned, burning N cores

**Date:** 2026-07-28 · **Area:** rust-core (LocalBackend)
**Symptom:** Cancelling a heavy `%pal nprocs N` job can leave N `orca_*_mp` processes alive after
the app thinks it killed the job — each pinned to a core, burning 100% CPU indefinitely.
**Root cause:** MPI ranks are not in the ORCA parent's process group, so `killpg` misses them.
**Fix:** sweep job processes by working directory as a safety net behind `killpg`; non-blocking
cancel; kill the tree on app exit too. (commit `fix: cancel leaves orphaned MPI ranks …`)

---

## What we believed

Cancellation spawned ORCA with `CommandExt::process_group(0)` and killed the job with
`killpg(pgid, SIGTERM→SIGKILL)`, on the assumption that **"`process_group(0)` puts ORCA and every
MPI rank it forks into one process group."** The gotchas page even claimed *"Verified: `kill -TERM
-<pgid>` reaps the entire tree."* That verification was too shallow — it only checked that the job
*stopped*, not *how*.

## The actual mechanism

`ps -eo pid,pgid,psr,comm` during a real `! B3LYP def2-QZVPP` run with `%pal nprocs 4`:

```
  PID    PGID  PSR  COMM
  30943  30943   6   orca              ← group leader (PGID == PID)
  30991  30943  11   mpirun            ← in the leader's group
  30997  30997   7   orca_leanscf_mp   ← OWN process group (PGID == its own PID)
  30998  30998   1   orca_leanscf_mp   ← OWN
  30999  30999   2   orca_leanscf_mp   ← OWN
  31000  31000   5   orca_leanscf_mp   ← OWN
```

Only `orca`, its `sh`, and `mpirun` are in group `30943`. The four MPI ranks each have
`PGID == PID` — they are **not** in the group. `mpirun` calls `setpgid` on every rank it launches,
precisely so a Ctrl-C / terminal signal to the launcher doesn't tear the ranks down mid-collective.

So `killpg(30943, …)` signals `orca` + `sh` + `mpirun` — never the ranks.

## Why it *seemed* to work

`mpirun`, on receiving SIGTERM, reaps its own ranks before exiting. So the graceful path
(SIGTERM → mpirun cleans up) happened to leave nothing behind — pure OpenMPI cooperation, not
our doing.

## Where it breaks

`terminate_pgid` waited a grace period then sent `killpg(SIGKILL)`. `mpirun` dies **instantly** on
SIGKILL and cannot forward anything → the ranks are orphaned. This is exactly the scenario in which
a user hits Cancel: a long, heavy job that does not exit promptly on SIGTERM. Reproduced live by
`SIGSTOP`-ing `mpirun` (so it cannot forward), then `killpg(SIGTERM)`+`killpg(SIGKILL)` on the
group: the `orca_*_mp` ranks stayed alive and had to be reaped by PID.

## The fix

A process's **working directory** is a reliable membership signal: every process ORCA spawns for a
job inherits `cwd` = the job directory (confirmed: `readlink /proc/<rank>/cwd` == the job dir).

- `sweep_job_processes(job_dir, sig)` — walk `/proc/<pid>/cwd`, signal every PID whose cwd matches
  the canonicalized job dir (skip our own PID; skip unreadable entries).
- `terminate_job(pgid, job_dir)` — `killpg(SIGTERM)` → wait up to 10 s → `sweep(SIGTERM)` (before
  any SIGKILL, so ranks get a clean-exit chance) → 2 s → `killpg(SIGKILL)` + `sweep(SIGKILL)`.
  Logs if the final sweep had to hard-kill anything (means the graceful path failed).
- `cancel` spawns `terminate_job` off-thread (it can take ~12 s) and returns immediately — the
  `cancelled` flag already drives correct finalization; the UI shows "Cancelling…" until the
  terminal `job:status`.
- `terminate_on_exit` runs the same routine **synchronously** in the `ExitRequested` handler — a
  spawned thread would die with the app before the ranks do.

Unit tests (`sweep_job_processes_matches_cwd`, `sweep_ignores_other_dirs`, `sweep_never_kills_self`)
cover the sweep with real `sh -c 'sleep 30'` children.

## Lesson / rule

- Durable rule added to `orca/gotchas.md`: **ORCA MPI ranks escape the parent's process group** —
  `killpg` is necessary but not sufficient; sweep by cwd.
- "Verified" must mean *verified by the right observation*. The old claim checked "job stopped",
  not "how the ranks died" — and the difference was a whole class of orphan bug.
- Same trap awaits `SshBackend` (Phase 5): killing the remote `.pid` parent won't reap the ranks;
  sweep by cwd remotely (`fuser -k <dir>` / `pkill -f <dir>`).
