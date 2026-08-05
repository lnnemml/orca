# Does `! UseSym` reorder atoms in ORCA 6.1.0 output artifacts? — measured

**Verdict (within the measured scope below): NO.** For the molecules, job types and
artifacts probed, `! UseSym` **reorients and symmetrizes** the geometry but leaves the
atom order **identical to the input order** in every output artifact. Permutations of
symmetry-**equivalent** atoms are unobservable in principle and are *not* claimed either
way.

This page records a **run**, not a manual claim (domain rule #10). The ORCA manual
(`resources/manual/.../essentialelements/symmetry.md.txt`) says `UseSym` "reorient[s] and
center[s] the molecule … and replace[s] the input structure by a geometry that corresponds
exactly to this point group" — it says **nothing** about atom *ordering*. The gate for
Phase 4.2 Stage 1 (ADR-016) is exactly that unstated question, so it was measured.

- **Probe:** [`sidecar/probes/usesym_atom_order.py`](../../sidecar/probes/usesym_atom_order.py)
  (re-runnable against the next ORCA version). Artifacts under `sidecar/probes/_usesym_runs/`.
- **ORCA version:** 6.1.0 (`Program Version 6.1.0`, from the run banner), `/opt/orca/orca`.
- **Invocation:** full absolute path, serial (`%pal nprocs 1`, so MPI is not a variable),
  one isolated dir per calculation (rules #1, #3).

## Why a naive coordinate compare is wrong here (both failure modes are silent)

1. `UseSym` **legally reorients** the molecule into the symmetry frame and **symmetrizes**
   the geometry. A raw coordinate diff shows "everything moved" and gives a false verdict in
   *both* directions. The detector therefore compares **rigid-motion-invariant fingerprints**
   — per atom, the sorted vector of distances to every other atom — with a tolerance for
   symmetrization, and reports the measured drift as a number.
2. A permutation of symmetry-**equivalent** atoms (the two H of water; the two mirror methyl
   H of methanol) is **unobservable in principle** — no fingerprint can distinguish them. The
   detector names the equivalence classes and reports a swap inside one as *unobservable*,
   never as "no reorder". **This is the boundary of the claim, stated in the claim.**

## The systems (input atom order verbatim)

| system | keywords | detected PG | note |
|---|---|---|---|
| formaldehyde, order **H C H O** | `! HF def2-SV(P) UseSym TightSCF` | **C2v** | mixed multi-element order — an element-reorder would show instantly |
| methanol, order **C O H(–O) H(mirror+) H(in-plane) H(mirror–)** | `! HF def2-SV(P) UseSym TightSCF` | **Cs** | decisive case: two mirror methyl H are **equivalent**, the in-plane one is **not** — the only way to see a reorder *inside* one element |
| water, order **H O H** | `! r2SCAN-3c UseSym TightOpt Freq` | **C2v** | covers motion artifacts: `_trj.xyz`, `.hess $atoms`, final `.xyz` |
| **methanol (same xyz), Opt+Freq** | `! r2SCAN-3c UseSym TightOpt Freq` | **Cs** | **the gap closer**: distinguishable one-element atoms **×** motion artifacts — the one combination neither the methanol-SP nor the water-Opt+Freq run reached |
| *control:* formaldehyde (same xyz), **no symmetry** | `! HF def2-SV(P) TightSCF` | — | ID baseline: detector on a known identity must report identity |

Formaldehyde input, verbatim:

```
! HF def2-SV(P) UseSym TightSCF
%pal nprocs 1 end

* xyz 0 1
  H      0.00000000     0.94300000    -0.58800000
  C      0.00000000     0.00000000     0.00000000
  H      0.00000000    -0.94300000    -0.58800000
  O      0.00000000     0.00000000     1.20500000
*
```

## What ORCA's own symmetry module printed — the most direct evidence

ORCA echoes a **"Symmetry-perfected Cartesians"** table with an explicit **0-based `Index`**
column. It preserves the input order exactly (only reorients + symmetrizes coordinates):

```
Index  Symbol         Symmetry-perfected Cartesians (x, y, z; Ang)   [formaldehyde]
   0     H      0.000000000000000   0.943000000000000  -1.190590654765870
   1     C      0.000000000000000   0.000000000000000  -0.602590654765870
   2     H      0.000000000000000  -0.943000000000000  -1.190590654765870
   3     O      0.000000000000000   0.000000000000000   0.602409345234130
```

```
Index  Symbol         Symmetry-perfected Cartesians (x, y, z; Ang)   [methanol]
   0     C     -0.733207977030148  -0.027998252293864   0.000000000000000
   1     O      0.696792022969852  -0.027998252293864   0.000000000000000
   2     H      0.956792022969852   0.862001747706136   0.000000000000000
   3     H     -1.093207977030148   0.487001747706136   0.892000000000000
   4     H     -1.093207977030148  -1.057998252293864   0.000000000000000
   5     H     -1.093207977030148   0.487001747706136  -0.892000000000000
```

Methanol confirms the decisive point: the interleaved methyl-H input order (mirror at 3,
in-plane at 4, mirror at 5) is preserved — the mirror pair lands at `z = ±0.892` (indices
3, 5; the measured equivalence class), the in-plane H at `z = 0` (index 4). No intra-element
reorder. Note ORCA's `Index` is **0-based**, consistent with the 0-based `%geom` base
(`constraints.md`).

## Per-artifact result (element sequence + fingerprint match vs input)

Every artifact's element sequence equals the input order, and every fingerprint match is the
identity permutation (modulo the named equivalence classes):

| artifact | formaldehyde SP | methanol SP | water Opt+Freq | **methanol Opt+Freq** |
|---|---|---|---|---|
| `.out` `CARTESIAN COORDINATES (ANGSTROEM)` block 0 | identity | identity | — | — |
| `.property.txt` `$Geometry` block(s) | identity (1) | identity (1) | element-seq ✓ (5) | element-seq ✓ (8) |
| `_trj.xyz` frame 0 vs input (fingerprint) | — | — | identity | **identity** |
| `_trj.xyz` all frames (element seq) | — | — | ✓ (5) | ✓ (8) |
| final `.xyz` (element seq) | — | — | ✓ | ✓ |
| `.hess $atoms` (element seq) | — | — | ✓ | ✓ |
| `.hess $atoms` vs final trj frame (fingerprint) | — | — | identity | **identity** |

The **methanol Opt+Freq** column is the decisive gap closer: it is the only run that crosses
**distinguishable atoms of one element** (the in-plane methyl H vs the mirror pair) with the
**motion artifacts**. In its `.hess $atoms`-vs-final-frame fingerprint the in-plane H (index 4)
matches **only** ref candidate `[4]` while the mirror pair `{3, 5}` is the sole equivalence
class — so the order of the *distinguishable* H is confirmed preserved through `_trj.xyz` (8
frames), the final `.xyz`, and `.hess $atoms`, not merely for water where every H is
equivalent and an intra-element reorder would have been invisible.

- **Equivalence classes measured:** formaldehyde `{H0, H2}`; methanol (SP & Opt+Freq)
  `{H3, H5}` (mirror methyl pair); water `{H0, H2}`. Order within these is unobservable —
  not claimed.
- **ID control — formaldehyde WITHOUT `UseSym`** (`! HF def2-SV(P) TightSCF`, no symmetry,
  point group not detected): the detector on this known identity reports **identity** on both
  the `.out` block and the `.property.txt` `$Geometry` block — the positive baseline that the
  UseSym runs are compared against. (It ran but was unrecorded in the first draft; an
  unrecorded control does not exist.)
- **Symmetrization drift:** ≈ 0 for the SP inputs (they were constructed already-symmetric,
  and reorientation is fingerprint-invariant). The `.hess`-vs-final-frame fingerprint drift
  was **1e-6 Bohr** on both Opt+Freq runs — the `.hess` prints fewer decimals than `_trj.xyz`,
  not a real motion. A grossly asymmetric input would drift more (bounded by `SymThresh`,
  default 1e-4 a.u.); that regime is not probed here.

## Negative controls — the detector demonstrably bites (CLAUDE.md convention)

A gate whose ability to fail is not shown is green for an unknown reason. All three passed:

| control | construction | expected | measured |
|---|---|---|---|
| **NC-elem** | formaldehyde, swap C(1)↔O(3) | element-sequence check bites | ✅ `OBSERVABLE REORDER`, `element_sequence_exact=false` |
| **NC-fp** | methanol, swap in-plane methyl H(4)↔mirror methyl H(3) — **same element, inequivalent** | fingerprint check bites where element check **cannot** | ✅ `OBSERVABLE REORDER`, `element_sequence_exact=true` (test atom 3 matches only ref 4) |
| **NC-null** | methanol, swap the two **equivalent** mirror methyl H (3↔5) | **not** flagged (would be crying wolf) | ✅ reported unobservable, equivalence class `{3,5}` |

NC-fp is the important one: it proves the detector catches an intra-element reorder that the
element-sequence check is blind to, while NC-null proves it does not false-positive on an
in-principle-invisible permutation.

## Scope of the claim (exactly the measurement — Pattern 2 guard)

Measured: **formaldehyde (C2v), methanol (Cs), water (C2v)**; job types **SP** (formaldehyde,
methanol) and **Opt+Freq** (water *and* methanol) — the Opt+Freq claim rests on **methanol**,
whose in-plane vs mirror methyl H are distinguishable, not only on water where all H are
equivalent, so an intra-element reorder on the motion path would have been *seen* had it
occurred; artifacts **`.out` coordinate echo, `.property.txt` `$Geometry`, `_trj.xyz`,
final `.xyz`, `.hess $atoms`**; **ORCA 6.1.0**. Not measured / not claimed: larger systems;
other point groups (in particular the D-groups and cubic groups whose reorientation is more
aggressive); explicit `%Symmetry PointGroup "..."`; grossly asymmetric inputs with large
symmetrization drift; and — in principle unobservable — the order **within** an equivalence
class of symmetry-equivalent atoms.

## Consequence for the architecture (ADR-016, ADR-010, ADR-008)

Within this scope the `parse_output` boundary may treat the artifact order as the input
order: the `IndexMap` for a `UseSym` job is the **identity**, and the type-level pairing of
ADR-010 carries an **identity-map post-condition** here rather than a mandatory permuted map.
The post-condition (rule #9) is the fingerprint/element-sequence check itself, re-run on real
output — so a *future* ORCA version that starts reordering fails loudly instead of silently
animating a mismatched geometry. This does **not** license assuming identity for unprobed
point groups: the open Phase-4.5 symmetry work must re-run this probe on the specific system
before trusting order there. See [ADR-016](../architecture/adr-016-emit-input-ownership.md).
