# CREST 3.0.2 — QCG microsolvation probe (Phase 4.5 Stage F probe)

**Measure-only probe (rule #10), 2026-08-08.** No production code was written — this page records
exactly what was measured by running CREST/QCG on this machine. The sibling external-tool pages are
`wiki/orca/xtb.md` (standalone xtb) and `wiki/orca/goat.md` (GOAT); this mirrors their "learned by
running it, not from docs" discipline. Online CREST docs were **not** trusted for behavior — only the
installed binary's `--help` and the runs below.

CREST = **C**onformer–**R**otamer **E**nsemble **S**ampling **T**ool; **QCG** = Quantum Cluster Growth
(explicit microsolvation: grow a solvent shell around a solute, then optionally sample its ensemble).
It is the intended Stage F tool for putting explicit solvent around the ionic reactant (BH₄⁻).

## Install & environment (verified, not re-installed)

- **CREST 3.0.2** at `/opt/crest/crest` — GNU **static** build, `commit (af7eb99)`, compiled 2024-08-25.
  Not on `PATH`; invoked by full path (rule #1 discipline). Never bundled (rule #7) — a user setting.
- **xtb 6.6.1** at `/usr/bin/xtb` (the sole xtb, also the only one on `PATH`) — the same binary the
  standalone-xtb integration uses (`wiki/orca/xtb.md`). Compiled `builduser@buildhost` 2023-08-07.
- `crest --help qcg` present → QCG compiled in. Precondition met; this probe confirms **runtime
  linkage + artifacts + cost**, not availability.

## Rule #2 linkage — DOES QCG invoke xtb 6.6.1? YES (quoted proof)

The load-bearing question (rule #2: version-match, and rule #9: never trust a third party's "done").
**CREST 3.0.2 QCG shells out to the external `xtb` binary** (it does *not* only use an internal
engine for these steps). In the kept tmp dir (`-keepdir`) each xtb step writes its own output whose
banner and program-call line are the proof. From the **ionic** RUNG 1 run
(`qcg_tmp/solute_properties/xtb.out`):

```
   * xtb version 6.6.1 (unknown) compiled by 'builduser@buildhost' on 2023-08-07
          program call               : xtb solute --gfn2 --sp
```

and every distinct xtb invocation CREST made in that run (grep `program call` over the kept dirs):

```
  xtb solute --gfn2 --sp                                              # solute single point (gas)
  xtb solvent --gfn2 --sp                                             # solvent single point (gas)
  xtb coord --gfn2 --opt                                        (×2)  # solute + solvent preopt (gas)
  xtb dock cluster.coord solvent --gfn2 0.00 --nfrag1 5 --qcg --input xcontrol   # xtbiff docking
  xtb cluster.xyz --opt normal --gfn2 --alpb methanol                 # CLUSTER opt (WITH ALPB) ✓
```

`xtb` here resolves via `PATH` to the only xtb, `/usr/bin/xtb` 6.6.1. **Version-match confirmed:**
the QCG help itself prints *"This requires xtb version 6.6.0 or newer"* and the linked binary is 6.6.1.

**Nuance for a future gate (measured):** the string `normal termination of xtb` is **not** grep-able
in the kept dirs — CREST captures xtb **stdout** into `xtb.out`/`xtb_opt.out`, and (as `xtb.md`
records) xtb prints `normal termination of xtb` to **stderr**, which CREST does not keep. So the
xtb-ran-and-succeeded evidence is: the **version banner** + **program-call** line, the `.xtboptok`
marker file xtb writes on a good optimization, the `xtbopt.*` geometry, and the `* finished run on …`
line. Do not build a CREST post-condition on `normal termination of xtb` from a kept dir — it is not
there. (CREST's own success line is `CREST terminated normally.`)

## The v3.0.2 flag list (from the binary — rule #10)

**`crest --help qcg`** — QCG options (verbatim from the installed 3.0.2):

```
General usage:  <solute> -qcg <solvent> [options]
  -keepdir          keep the tmp folder
  -nopreopt         do not perform preoptimization (only for qcg)
  -xtbiff           use the xTB-IFF standalone for docking of solvent
  -grow             cluster generation
  -fixsolute        fix the solute during the growth (recommended for rigid ones); auto for water
  -nofix            do NOT fix the solute during growth (needed only for water)
  -nsolv <INT>      number of solvent molecules to add
  -normdock         more extensive docking during grow
  -maxsolv          convergence limit if no -nsolv given (default 150)
  -wscal <FLOAT>    scaling factor for the outer wall potential
  -samerand         same random number for every xtbiff run
  -fin_opt_gfn2     GFN2-xTB optimizations for final grow + ensemble structures
  -directed <FILE>  directed solvation at positions in <FILE>
  -ensemble         ensemble generation
  -qcgmtd | -ncimtd NCI-MTD CREST ensemble generation (default)
  -mtd | -md        MTD / normal-MD for QCG ensemble generation
  -enslvl [method]  method for ensemble search (any gfn method)
  -clustering       clustering for the ensemble search (qcgmtd/ncimtd only)
  -esolv            reference cluster generation + solvation energy
  -gsolv            reference cluster generation + solvation FREE energy
  -nclus            #clusters for reference cluster generation (default 4)
  -nocff            switch off the CFF algorithm
  -freqscal / -freqlvl [method]   frequency scale factor / method (thermo)
```

**`crest --help general`** — the identity/solvation/charge flags used above:

```
  -T <int>          total CPUs/threads (else from OMP_NUM_THREADS)
  -g <string>       GBSA implicit solvent for <string>
  -alpb <string>    ALPB implicit solvent for <string>          ← what we use
  -chrg <int>       molecular charge                            ← the ionic footgun
  -uhf <int>        Nα − Nβ electrons
  -opt <lev>        opt level for ALL xtb opts (default vtight)
  -gfn1 | -gfn2 | -gfn0 | -gfnff | -gfn2//gfnff   Hamiltonian (default gfn2)
  -cinp <file>      extra xtb-format constraints for ALL xtb calls
  -xnam <"bin">     name of the xtb binary to use              ← how to pin /usr/bin/xtb explicitly
  -dry              print settings and stop before any calculation
```

There is **no SMD** at this level — CREST/xtb implicit solvation is **ALPB or GBSA only** (both
appear above). SMD is the *later ORCA-refinement* step (ADR-018 comparability guard wants SMD for
ions), **not** something QCG can do. Do not look for it here.

**Invocation syntax confirmed:** `<solute> -qcg <solvent> [options]`, e.g.
`crest bh4.xyz -qcg methanol.xyz -grow -nsolv 3 -alpb methanol -chrg -1 -fixsolute`.

## Per-rung results

Inputs built for the probe: benzoic acid (RDKit ETKDGv3+MMFF, C₇H₆O₂, 15 atoms), water (hand),
BH₄⁻ (hand-built tetrahedral, B–H 1.240 Å, 5 atoms), methanol (RDKit, 6 atoms). Each rung in its own
clean dir (rule #3), `-T 4`, `-keepdir`.

| rung | system | mode | result | wall | peak RSS |
|---|---|---|---|---|---|
| 0 | benzoic acid + 3 H₂O | `-grow -alpb water -nofix` | **CREST terminated normally**, 24-atom cluster | **10.15 s** | 26 MB |
| 1 | **BH₄⁻** + 3 MeOH | `-grow -alpb methanol -chrg -1 -fixsolute` | **CREST terminated normally**, 23-atom cluster | **8.68 s** | 26 MB |
| 2 | BH₄⁻ + 3 MeOH | `-ensemble …` (as rung 1) | grow OK → **ENSEMBLE SEGFAULTS** (reproducible) | 57 s to crash | — |

### RUNG 0 — known-good linkage (benzoic acid + water)

The documented QCG example, run first so a rung-1 failure would be diagnosably *ours*. Terminated
normally in 10.15 s; xtb 6.6.1 linkage as above; `-alpb water` **reached the cluster optimization**
(`* Solvation model: ALPB / Solvent water`). Final `grow/cluster.xyz` = **24 atoms** = benzoic (15) +
3×H₂O (9) → 3 solvent added. Clean baseline.

### RUNG 1 — the ionic target (BH₄⁻ + methanol), the charge footgun

Terminated normally in 8.68 s. CREST **reads `-chrg -1`** (`crest.out`: `Molecular charge : -1`) and
writes `.CHRG = -1` into the solute dir; the **solute preopt runs at −1** (`:: total charge
-1.000000000000 e ::`). `-alpb methanol` reached the cluster opt (`Solvation model: ALPB / Solvent
methanol / mass 3.2040E+01`). Final `grow/cluster.xyz` = **23 atoms** = BH₄⁻ (5) + 3×CH₃OH (18).

> ⚠️ **MEASURED FOOTGUN (rule #9) — QCG grows/optimizes the ionic cluster as NEUTRAL.** The −1
> charge is applied to the **solute monomer preopt only**. In the **grow** phase there is **no
> `.CHRG` file** in `qcg_tmp/tmp_grow/`, the docking reports `charge of molecule A : 0.0` (A = the
> solute), and the cluster optimization
> `xtb cluster.xyz --opt normal --gfn2 --alpb methanol` (no `--chrg`) computes
> `:: total charge 0.000000000000 e ::`. So the microsolvated **cluster geometry is that of a
> neutral cluster**, not the −1 anion — a "terminated normally, wrong chemistry" outcome, exactly the
> class rule #9 exists for. Four independent evidences (absent `.CHRG`, docking A=0.0, cluster-opt
> total charge 0.0, program call without `--chrg`). Probed with `-grow` only; whether `-esolv/-gsolv`
> or another flag carries the charge into the cluster was **not probed**.

**Artifact inventory — `grow/` (what a future reader would parse):**

| file | format / units | purpose |
|---|---|---|
| `cluster.xyz` | XYZ, **Å** | final grown cluster geometry (solute + n solvent) |
| `cluster.coord` | TURBOMOLE `$coord`, **Bohr** | same geometry, atomic units (rule #11: Bohr, not Å) |
| `cluster_optimized.xyz` | XYZ, Å; comment `energy: … gnorm: … xtb: 6.6.1` | cluster optimized **without** wall potential |
| `qcg_grow.xyz` | multi-frame XYZ | growth trajectory (frames 11 → 17 → 23 atoms — one per added solvent) |
| `qcg_energy.dat` | text table | per-size energy: `size  E(Eh)  ΔEtot` across the growth |
| `qcg_conv.dat` | text table | growth convergence: `# Energy  Run.Aver.  Diff/au` |
| `*_cavity.coord`, `twopot_*.coord`, `wall_potential` | TURBOMOLE `$coord` / `$wall` | the wall/cavity potentials that confine the shell |

The authoritative outputs are **`cluster.xyz`** (grown geometry) and **`cluster_optimized.xyz`**
(relaxed), plus `qcg_energy.dat`. xtb per-step logs live under `qcg_tmp/{solute,solvent}_properties/`
and `qcg_tmp/tmp_grow/` (`xtb.out`, `xtb_opt.out`, `xtb_dock.out`, `.xtboptok`, `xtbopt.coord`).

### RUNG 2 — ensemble sampling (BH₄⁻ + methanol): CRASHES

`-ensemble` = grow, then NCI-MTD shell sampling. The **grow phase completed** ("Growth finished after
3 solvents added"), then the **ensemble phase segfaulted** (SIGSEGV, exit 139), **reproducibly** (two
runs, identical trace):

```
#10  qcg_ensemble_        at src/qcg/solvtool.f90:1029
 #9  confscript2i_        at src/legacy_wrappers.f90:148
 #8  crest_search_imtdgc_ at src/algos/search_conformers.f90:135
 #7  crest_multilevel_oloop_
 #6  sort_and_check_      at src/legacy_algos/confscript2_misc.f90:1045
 #5  confg_chk3_
 #4  newcregen_           at src/cregen.f90:265        ← crash in CREGEN (conformer sort/check)
```

The ensemble search **defaults to GFN-FF** (`Method for ensemble search: --gff`). Forcing GFN2
(`-enslvl gfn2`) does **not** rescue it — the MTD fails to converge instead: *"Trial MTD 1…6 did not
converge … Automatic MD restart failed 6 times! Please try other settings manually."* (exit 1). So
**QCG `-ensemble` is unusable for this ionic BH₄⁻+methanol system at v3.0.2** by either route; the
`ensemble/` output dir is created but **empty**. (`-grow` alone is unaffected.)

## Caveats surfaced (all measured or from the binary)

1. **Ionic cluster grown as neutral** — the RUNG 1 footgun above. The single biggest blocker for
   using QCG on the mission's anion.
2. **`-ensemble` crashes** on the ionic system (CREGEN segfault / MTD non-convergence). Only `-grow`
   is dependable here today.
3. **Monomer preopts are gas-phase** — the solute/solvent single-point + preopt run without
   solvation; ALPB enters only at the **cluster** optimization. (Design choice, recorded so a reader
   doesn't expect solvation on the monomers.)
4. **ALPB/GBSA only, no SMD** at this level (built-in). SMD is the later ORCA step.
5. **Floppy solvent shell** (the roadmap caveat) — many near-degenerate arrangements; the ensemble
   would over-sample them even if it ran. Low-frequency modes make the eventual QCG thermochemistry
   soft (`-freqscal`/`-freqlvl` exist for this). Not reached (ensemble crashes).
6. **Completion signal** — trust `CREST terminated normally.` + the expected artifacts, not
   `normal termination of xtb` (stderr, not kept). Exit code is reliable for CREST itself here
   (0 grow / 139 crash / 1 MTD-fail), unlike the xtb-standalone exit-code lie (`xtb.md`).

## E-vs-F ordering recommendation (what this probe informs)

**Do Stage E (gas-phase ΔG‡: OptTS + Freq + thermochemistry) BEFORE Stage F (explicit QCG
microsolvation).** Rationale from the measurements:

- The mechanics of QCG are cheap and correctly linked (xtb 6.6.1, ALPB, ~10 s for a 3-solvent shell),
  so F is *reachable* — but **not production-trustworthy for the mission's ionic reactant** as-is:
  the grow cluster is optimized at the **wrong charge** (neutral, not −1), and `-ensemble` **crashes**.
- Both are blocking for a *defensible* solvated-anion result, and neither is OrcaStudio's bug to fix.
  Making F usable is a research spike (candidate mitigations, **none probed**: manually place a
  `.CHRG` in the grow dir / `-cinp`; a newer CREST; or — the pragmatic path — take QCG's *neutral*
  grown shell only as a **geometry seed** and re-optimize the cluster **in ORCA at the correct charge
  with SMD**, which is where the real ionic solvation energy should come from anyway).
- Stage E has no such dependency and directly upgrades the ΔΔE‡/absolute-barrier story (Phase 4.5
  C2b) toward true ΔG‡. So E is the higher-certainty next step; F stays a probed-but-gated spike.

This matches the ratified reorder (CREST probe → Stage D → E/F) and sharpens it to **E before F**.

## See also

- `wiki/modules/crest-microsolvation.md` — the production code (Stage F F1a–F2) built on this probe:
  the grow parse + completion, the ephemeral runner, the setup panel, and the ORCA re-opt handoff.
- `wiki/orca/xtb.md` — standalone xtb 6.6.1 (same binary CREST drives); the stderr `normal
  termination` fact reused above; the exit-code-lies discipline.
- `wiki/orca/goat.md` — the sibling external-tool probe (ORCA `! XTB GOAT` conformer search).
- `wiki/architecture/adr-018-reaction-energy-reference.md` — SMD-not-ALPB for ions (the later ORCA
  refinement F feeds); `wiki/chemistry/reaction-barriers.md` — where solvated barriers sit.
- `ROADMAP.md` Stage F — annotated "CREST installed + QCG probed" (probe, not the feature).
