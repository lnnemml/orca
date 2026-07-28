# ORCA gotchas (accumulating)

Living page. Add every trap encountered, newest at top, format:
**symptom → cause → fix**.

---

- **`%pal nprocs` larger than the pinned core count → job runs *slower*, not faster** → when ORCA
  is pinned with `taskset -c <mask>`, an `%pal nprocs` that exceeds the masked core count
  oversubscribes: e.g. 12 ranks fighting over 4 cores is ~3× slower than 4 ranks. Fix: OrcaStudio
  rewrites `%pal nprocs N end` to match the pinned rank count before every run
  (`align_pal_nprocs`), and emits a log line saying it did. Also disable OpenMPI's own binding
  (`OMPI_MCA_hwloc_base_binding_policy=none`) so it doesn't fight taskset (domain rule #8,
  `wiki/orca/performance.md`).
- **ORCA MPI ranks escape the parent's process group** → the intuitive fix for "killing ORCA
  leaves ranks alive" — spawn in a new process group (`process_group(0)`) and `killpg` the group —
  **only half works**. `mpirun` calls `setpgid` on every rank it forks so terminal signals can't
  reach them, so each rank (`orca_*_mp`) ends up in **its own** process group (`PGID == its own
  PID`). Verified with `ps -eo pid,pgid,cmd` on `%pal nprocs 4`: only `orca` + `sh` + `mpirun`
  share the leader's group; the 4 ranks each have their own. So `killpg(pgid, …)` reaches mpirun
  but not the ranks. It *appears* to work because a SIGTERM'd mpirun reaps its ranks cooperatively
  — but on the SIGKILL path mpirun dies before it can, leaving N orphaned ranks burning N cores.
  **Fix:** after `killpg(SIGTERM)`, also **sweep by working directory** — every job process has
  `cwd` = the job dir, so signal every PID whose `/proc/<pid>/cwd` matches, before escalating to
  SIGKILL. See `debugging/004-mpi-ranks-escape-process-group.md`. Same problem hits `SshBackend`
  remotely (Phase 5) — a parent `.pid` marker is not enough; sweep by cwd there too
  (`fuser -k <dir>` / `pkill -f <dir>`).
- **Graceful "stop after current optimization cycle" — UNCONFIRMED for 6.1** → ORCA is *said* to
  support stopping a geometry optimization cleanly via a marker file in the job dir (preserving a
  valid `.gbw` + last geometry), which would beat a hard kill. This could not be verified: the
  ORCA 6.1 manual is not indexed locally yet (Phase 4; `resources/manual/` currently holds only a
  README). Until confirmed against the real manual, OrcaStudio implements **only** hard kill
  (killpg). Do not add a "Stop after current cycle" button on the strength of memory — re-check
  the manual first.
- **`orca --version` "fails" / does something weird** → ORCA has **no CLI flags**; it treats
  its first argument as the name of an input file, so `orca --version` tries to open a file
  literally called `--version`. There is no version subcommand → read the version from the
  **banner printed at the top of every run's output** (e.g. `Program Version 6.1.0`), or from
  the release/install directory. Implication for OrcaStudio: detect the ORCA version by parsing
  the output banner, never by shelling out `orca --version`.
- **%pal ignored, runs on 1 core** → ORCA invoked via bare name from PATH → always invoke
  with full absolute path (see orca-basics.md).
- **MPI errors at startup on a machine where serial runs work** → OpenMPI version mismatch
  with the ORCA build → install the exact version the release notes specify.
- **Huge outputs freeze naive viewers** → never read output files whole; tail/stream only.
- *(add as encountered during Phase 0+)*
