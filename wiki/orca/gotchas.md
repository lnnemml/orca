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
- **Any element-keyed table taken from outside MUST be checked against the ADR-007 metals
  (Pd, Pt, Rh, Ru, Ir, Os) BEFORE relying on it.** This has now bitten the project **three times**,
  same shape each time — a lookup table keyed by element symbol, complete for organic elements,
  with a hole exactly where the reaction-modeling work lives (4d/5d transition metals):
  1. **2.5.0** — our own `atomicNumber` table stopped at **H–Kr (Z ≤ 36)**, so Pd(46)/Pt(78) threw
     on electron counting. Extended to H–Rn.
  2. **2.5.2e-1** — 3Dmol's `GLModel.vdwRadii` needed a fallback; the copy was checked to include
     Pd/Pt, and off-table elements got an explicit fallback radius.
  3. **2.5.2e-3b** — 3Dmol's default colour table (`rasmol`, 28 elements) has **no Pd/Pt/Rh/Ru**,
     so 3Dmol painted them `defaultColor` #ff1493 — which collided with the pink selection halo,
     making every metal atom look permanently selected. Fixed by adding the metals from 3Dmol's
     `Jmol` table and moving the halo off pink.
  The rule: when adopting an element-keyed table (colours, radii, masses, basis defaults, anything),
  **explicitly verify coverage of Pd/Pt/Rh/Ru/Ir/Os** and decide the fallback deliberately. A
  drift/coverage guard must check BOTH directions — "our copy is stale" AND "the source has an
  element we don't" — because iterating only our own keys is blind to the missing-element case by
  construction (that is exactly how case 3 slipped through). See `wiki/modules/visualization.md`.
- **Bond perception is a GUESS from geometry, not a fact — and OUR OWN editor can create the
  geometries where it is wrong.** Bonds are inferred from interatomic distance vs covalent radii ×
  a multiplier (`ase.neighborlist.natural_cutoffs`). A **stretched** bond can vanish from the guess;
  two **close** atoms that aren't bonded can appear bonded — and the geometry editor routinely makes
  both (an inter-fragment reaction distance of ~2.2 Å is exactly the ambiguous zone for heavy
  atoms). So `POST /geometry/rotatable-mask` (2.5.3a) makes the multiplier an **explicit parameter**
  (default 1.2 — ASE's own 1.0 is too tight, misses C–H/C–C), checks perception against **known
  valence** in tests (butane 13, benzene 12, BH₄⁻ 4 — the charged trap — water 2), and **refuses with
  an explanation** when the guess looks odd (unbonded cut, ring bond, >2 components) rather than
  returning a silently-wrong mask. Rule: never trust perceived bonds blindly on geometries this app
  can distort; make the cutoff visible, test it against valence, and refuse rather than guess. See
  `wiki/modules/sidecar.md`.
- **`%geom Constraints` index base — SETTLED: 0-based** (was the open "Question C").
  An off-by-one here doesn't crash; it freezes the *wrong* coordinate on a run that
  finishes `ORCA TERMINATED NORMALLY` — so it was resolved by a real ORCA 6.1.0
  run (2026-07-29), not by memory. Chloromethane (order `Cl,C,H,H,H`) with
  `{B 1 2 1.234 C}` froze the **C–H** bond (atoms 1,2 = C,H under 0-based) and left
  C–Cl free; ORCA's own internal-coordinate table printed `B(H 2, C 1)` — carbon =
  atom 1, chlorine = atom 0. Bonus trap: an **out-of-range index segfaults** (`{C 5 C}`
  on a 5-atom molecule died at "Evaluating the coordinates") — ORCA does no bounds
  check, so range-check indices before writing a constraint. Full evidence and the
  in-range/out-of-range control in `wiki/orca/constraints.md`.
- **TWO constraint index bases — ORCA is 0-based, xtb is 1-based. DO NOT confuse them.**
  Both were settled by real runs (never memory), each with the chloromethane experiment
  (order `Cl,C,H,H,H`, an explicit-value constraint on the pair whose bond *type* a one-index
  shift changes):
  - **ORCA `%geom Constraints` → 0-based** (2.5.4a; `{B 1 2 …}` froze the C–H pair;
    `wiki/orca/constraints.md`).
  - **xtb `$constrain` → 1-based** (2.5.5; `distance: 1, 2, …` froze the Cl–C pair;
    `wiki/orca/xtb.md`).
  OrcaStudio stores constraints **0-based** (ADR-008), so it writes ORCA indices **as-is** and
  xtb indices **`+1`**. An off-by-one here freezes the WRONG coordinate on a calculation that
  finishes normally — both code paths therefore carry a runtime guard (ORCA: range-check +
  segfault block; xtb: the post-condition that the constraint was actually held, which also
  catches a base mismatch because the intended pair would then drift).
- **xtb holds constraints with a SPRING, not rigidly.** `$constrain force constant=k` is a
  harmonic restraint, so the held coordinate deviates from the target (measured: 0.011 Å on a
  realistic reaction-coordinate hold at k=1.0; up to 0.12 Å when fighting a stiff bond). The
  post-condition tolerance (0.1 Å) is sized from this — see `wiki/orca/xtb.md`.
- **xtb 6.6.1 HANGS on an EMPTY `--input` file** → given `--input <file>` where the file is empty,
  xtb spins at 99 % CPU and never reaches the first optimization cycle (verified: 300 s timeout,
  0 cycles; independent of molecule, `OMP_*`, opt level — `wiki/debugging/006`). A foreign-binary
  bug we don't own. **Rule: never pass `--input` unless the detailed-input file has content** —
  OrcaStudio's `build_xcontrol` returns `Option` and one value drives both "write the file" and
  "pass `--input`". This bit exactly the first no-constraint run (all prior runs had constraints,
  so a non-empty `xcontrol`). Candidate upstream xtb issue (not filed from here).
- *(add as encountered during Phase 0+)*
