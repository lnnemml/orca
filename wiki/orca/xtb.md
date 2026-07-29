# xtb — standalone GFN2-xTB pre-optimization

Reference for OrcaStudio's **standalone xtb** integration (Phase 2.5.5). This is
the semi-empirical tight-binding pre-optimizer used to relax a reaction geometry
cheaply before handing it to ORCA. It lives in `orca/` because that is our catalog
of the external tools we drive, alongside ORCA itself.

> **Not xtb-via-ORCA.** ORCA can call xtb as `! XTB` (the GOAT conformer path,
> `wiki/orca/goat.md`). That is a different thing and does not mix with this. Here
> we shell out to the standalone `xtb` binary directly, from Rust.

**Version verified on this machine:** `xtb 6.6.1` (`/usr/bin/xtb`), 2026-07-29.
The binary is **never bundled** (domain rule #7) — the path is a user setting.

## Why Rust, not the sidecar

The ROADMAP said "sidecar endpoint `/xtb-optimize`". We put it in **Rust** instead
(logged as a decision):

- Rust owns process spawning, and with it the **isolated-directory** rule (#3) and
  the **kill-the-whole-group** discipline (`wiki/debugging/004`) — xtb can leave
  children/threads, same class of problem;
- the **binary path is a setting**, and settings live in SQLite under Rust;
- the sidecar is deliberately ignorant of the jobs dir and of settings. It is the
  thing that understands the *chemistry of files*, not the thing that *runs
  binaries*.

## Index base — **1-based** (xtb 6.6.1, verified 2026-07-29)

**xtb's `$constrain` block is 1-based. This is DIFFERENT from ORCA's `%geom
Constraints`, which is 0-based.** Getting it wrong freezes the wrong coordinate on
an optimization that finishes cleanly — the same "successful run, wrong chemistry"
trap as ORCA, so it was settled by a run, not by memory.

### The experiment (same design as the ORCA one, 2.5.4a)

Chloromethane, atom order `Cl, C, H, H, H` (a one-index shift changes the bond
*type*: C–Cl ≈ 1.78 Å vs C–H ≈ 1.09 Å), constraint on the pair `1, 2` at an
explicit 1.234 Å:

```
$constrain
  force constant=1.0
  distance: 1, 2, 1.234
$end
```

xtb echoed:

```
constraining bond 1 2 to    1.2340000 Å, actual value:    1.7780002 Å
```

The **initial** value of the constrained pair is **1.778 Å** — the Cl–C distance.
So `1, 2` selected atoms **Cl (atom 1)** and **C (atom 2)**: **1-based**. Under a
0-based reading, `1, 2` would be C and the first H (initial ≈ 1.09 Å). It was not.
OrcaStudio stores constraints 0-based (ADR-008), so it writes every index **`+1`**.

## Constraints hold by a **spring**, not rigidly — the tolerance

xtb applies constraints as a **harmonic restraint** (`force constant` = the spring
stiffness in the `$constrain` block), so the held coordinate does not land exactly
on the target — it balances against the molecule's own restoring force.

Measured (chloromethane, compressing C–Cl by 0.54 Å — a deliberately extreme case):

| force constant | held Cl–C | deviation from 1.234 |
|---|---|---|
| 1.0 | 1.354 | 0.120 Å |
| 5.0 | 1.273 | 0.039 Å |
| 10.0 | 1.256 | 0.022 Å |
| 25.0 | 1.243 | 0.009 Å |
| 50.0 | 1.239 | 0.005 Å |

That is the **worst case** (fighting a stiff covalent bond). On a **realistic**
reaction-coordinate hold it is far tighter — see below.

**OrcaStudio uses `force constant = 1.0`** and a post-condition tolerance of
**0.1 Å** (distance). The tolerance is justified by the realistic run: 0.011 Å
held, so 0.1 Å is a 10× margin. It is deliberately loose enough not to false-fail
a reasonably-posed constraint, yet tight enough to catch (a) a gross non-hold like
the artificial 0.12 Å over-compression, and (b) an **index-base mistake** — if the
`+1` were wrong, xtb would constrain a *different* pair and the intended pair would
drift far more than 0.1 Å. Angle/dihedral tolerances are set generously (5°, not
separately measured); a Cartesian `$fix` is a hard constraint, checked near-exact
(0.01 Å).

## Realistic run (the author's working scene)

Ibuprofen (33 atoms, RDKit/ETKDG) + BH₄⁻ (5 atoms), boron placed 2.23 Å from the
carbonyl carbon, constraint holding **C(#12)···B(#33)** at 2.2 Å — exactly what the
constraint panel writes. Our 0-based `12, 33` → xtb `distance: 13, 34, 2.2`.

- **Constraint held:** target 2.200 Å → final **2.211 Å** (deviation **0.011 Å**).
- **The rest relaxed:** ibuprofen block per-atom RMSD 0.67 Å (GFN2 reshaped the
  MMFF geometry).
- **Atom order preserved** (38 → 38, element sequence unchanged).
- **Wall time 1.5 s**, converged after 66 cycles.

## Charge and multiplicity

xtb takes total charge via `--chrg` and the number of **unpaired electrons** via
`--uhf` (= `2S` = multiplicity − 1). OrcaStudio passes the scene's total charge and
`--uhf = multiplicity − 1`. GFN2 (`--gfn 2`) is the default and only Hamiltonian
for now (GFN-FF, solvation, Hamiltonian choice are deliberately out of scope).

## What OrcaStudio runs

```
<xtb_path> input.xyz --input xcontrol --opt --gfn 2 --chrg <c> --uhf <mult-1>
```

in an isolated dir (removed after reading `xtbopt.xyz`, rule #3), by full path,
in its own process group (cancel/timeout → killpg + cwd-sweep, `debugging/004`).
The `xcontrol` file holds the `$constrain` / `$fix` blocks in 1-based indices. See
`wiki/modules/tauri-core.md` for the command and its post-conditions.

## See also

- `wiki/orca/constraints.md` — ORCA's constraint block (**0-based** — contrast!).
- `wiki/orca/gotchas.md` — the two index bases side by side.
- `wiki/modules/tauri-core.md` — `xtb_optimize` command, isolation, post-conditions.
- `wiki/modules/scene.md` — `replaceAllAtoms` (applying the result).
