# ORCA 6.1.0 parse sources — measured

Every fact on this page comes from a **run** against the author's real job directories
(`~/.local/share/orcastudio/jobs/`), not from documentation or memory. The probe that
produced it lives in the repo and re-runs on the next ORCA version:

```
sidecar/.venv/bin/python sidecar/probes/parse_sources.py \
    --sp <dir> --optfreq <dir> --saddle <dir> --goat <dir>
```

**Environment at measurement (2026-07-30):** cclib **1.8.1** (the latest on PyPI —
`pip index versions cclib` → `1.8.1` is LATEST), Python 3.12.3, ORCA **6.1.0**
(`Program Version 6.1.0` in every output; gbw `Git 679e74b`), ASE 3.29.0.

Jobs used (all `ORCA TERMINATED NORMALLY`, `.exit_code = 0`):

| role | job id | keywords | atoms |
|---|---|---|---|
| SP | `09de617c` | `! r2SCAN-3c TightSCF` | 17 |
| Opt+Freq (min) | `d7992449` | `! r2SCAN-3c Opt Freq TightSCF` | 8 (ethane) |
| Opt+Freq (saddle) | `99e805f5` | `! B3LYP def2-TZVP … CPCM(dmso) Opt Freq` | 19 |
| GOAT | `04aeca22` | `! XTB GOAT` | 33 |

---

## Headline finding — cclib 1.8.1 does NOT parse ORCA 6.1.0

`ccread` **raises on every one of the four outputs**. It is not a soft failure returning
partial data — it throws:

```
IndexError: list index out of range
  cclib/parser/orcaparser.py:2799  _append_scfvalues_scftargets
    self.scfvalues[-1].append([deltaE_value, maxDP_value, rmsDP_value])
```

for the SP, both Opt+Freq runs, and `AssertionError:` (empty message) for the GOAT run.
The crash is in the SCF-convergence-table parser, i.e. cclib's model of the ORCA 6.1 SCF
output section is wrong for this release. cclib 1.8.1 predates ORCA 6.1.

**What cclib manages before it crashes** (harvested off the parser instance — cclib fills
`self.<attr>` incrementally and only assembles `ccData` at the end):

| harvested | value | note |
|---|---|---|
| `atomnos` | full (17/8/19/33) | element list, complete |
| `atomcoords` | `len == 1` | **initial geometry only** — the crash is in the first SCF, so the optimized geometry is never reached |
| `natom` | correct | |
| `scfenergies` | `len == 1` | **first SCF cycle only**, in eV; not the converged/final value for an Opt |
| `metadata` | present | package/version/keywords/solvent etc. |

**Never reached before the crash:** `vibfreqs`, `vibirs`, `vibdisps`, `vibsyms`,
`atomcharges`, `moenergies`, `homos`, `mocoeffs`, `temperature`, `enthalpy`, `entropy`,
`freeenergy`, `zpve`, `etenergies`, `etoscs`. On these outputs cclib delivers **none** of
the quantities the Results screen needs beyond the initial geometry.

---

## Atom order & count — the Phase-3 seam (measured)

Comparing cclib's `atomnos` and `atomcoords[0]` against the `* xyz … *` block of the very
`input.inp` that launched each job (exact element-sequence equality; coordinate match to
`< 1e-4 Å`):

| role | count match | element order exact | max coord Δ (Å) | first mismatch |
|---|---|---|---|---|
| SP (17) | ✅ | ✅ | 0.0 | none |
| Opt+Freq min (8) | ✅ | ✅ | 0.0 | none |
| Opt+Freq saddle (19) | ✅ | ✅ | 0.0 | none |
| GOAT (33) | ✅ | ✅ | 0.0 | none |

**PASS on all four.** Caveat, stated because it matters: the coordinates cclib exposes are
`atomcoords[0]` = the **initial** geometry (cclib crashed before the optimized one), so this
confirms only cclib's atom **ordering** equals the input ordering. It does **not**, by itself,
say anything about the order in the structured artifacts we actually read — that is measured
separately, per artifact, below. None of these runs used symmetry (`PointGroup: C1` in the
gbw), which is the case ORCA is known to reorder under — flagged as an open UseSym probe in
ROADMAP Phase 4.5.

---

## Atom order in EACH structured artifact — the real seam (measured, Part A)

cclib is dead, so the Phase-3 seam moved onto the structured artifacts. This section replaces
an earlier **unverified** claim ("the structured artifacts carry the same atom count in the
same order") — which had evidence only for *count*, not *order* — with a per-artifact,
per-frame, per-block measurement. The element sequence of every artifact is compared to the
`* xyz … *` block of the launching `input.inp`; exact equality, first divergent index on
failure. Produced by `probe_artifact_order()` in `sidecar/probes/parse_sources.py`.

| Artifact | what is compared | SP | min | saddle | GOAT |
|---|---|---|---|---|---|
| `.hess $atoms` | element column vs input | — | ✅ | ✅ | — (no `.hess`) |
| `.hess` dims | `vibfreqs = 3N`, `normal_modes = 3N×3N` | — | ✅ 24, 24×24 | ✅ 57, 57×57 | — |
| `orca_2json` `Atoms` | `ElementNumber` seq vs input; `Idx` = 0…N-1 | ✅ | ✅ | ✅ | ⚠ no JSON (xTB gbw) |
| `_trj.xyz` | element column of **every** frame | — | ✅ 5/5 | ✅ 8/8 | ✅ 18/18 |
| `input.xyz` | element column (final) | — | ✅ | ✅ | ✅ |
| `.property.txt $Geometry` | element column of **every** block | ✅ 1/1 | ✅ 5/5 | ✅ 8/8 | ✅ 18/18 |

**Gate verdict: PASS** — every artifact's element order equals the input order, on every
frame and every block, across all four jobs. No reorder anywhere.

**Units, measured (not assumed):** `.hess $atoms` coordinates are **Bohr** — the ratio of the
largest `.hess` coordinate to the largest input coordinate is **1.8886** (ethane) / **1.6579**
(saddle), i.e. ≈ the Bohr/Å factor 1.889, not ≈ 1.0. `.property.txt $Geometry` states its unit
in the file: `&Units "Bohr"`. `orca_2json` states `CoordinateUnits: "Angs"` and its coords are
the **final** geometry (z 0.7635 vs input 0.768) — a coordinate difference there is
final-vs-initial, **not** a reorder; order is compared by element only.

### Per-atom array labelling — the one place order is *assumed*

Which per-atom arrays carry their own element label, and which are bare positional (order must
be assumed from the co-located `$Geometry` block)? Measured directly:

| Array | labelled? | measured |
|---|---|---|
| `$SCF_Mulliken/Loewdin/Mayer_Population_Analysis` `&AtomicCharges` | **element-labelled** | each block carries a co-located `&ATNO` array whose order was verified == input on all jobs |
| `$SCF_Nuc_Gradient &grad` | **BARE positional** | `&Dim (3N,1)` flattened xyz, no per-atom label → order **assumed** from the block's `$Geometry` (same `&GeometryIndex`, `&NAtoms`) |
| `$THERMOCHEMISTRY_Energies &FREQ` | not atom-indexed | `&Dim (3N,1)` is a **mode** index, not an atom index (modes map to atoms via `.hess $normal_modes`, itself 3N×3N in atom order) |

So the atomic **charges** are self-describing (labelled, verified) — an earlier note that
called them "bare positional" was wrong and is corrected here. The only bare positional
*atom-ordered* array is the SCF gradient, and its order source is named explicitly: the
co-located `$Geometry` block. The decision that turns this measurement into a rule is
**[ADR-012](../architecture/adr-012-output-parsing-ownership.md)**.

---

## Imaginary & near-zero frequencies (measured)

cclib cannot answer the "does the negative sign survive" question here — it never reaches
`vibfreqs`. The sign is measured **directly from the artifacts** instead:

**Saddle (`99e805f5`)** — `input.hess` `$vibrational_frequencies` (57 = 3N for 19 atoms):

```
    5        0.0000000000000000
    6      -33.6608873883281419      ← negative sign present in .hess
    7       54.2201437270207052
```

and `output.out`: `6:  -33.66 cm**-1  ***imaginary mode***` — ORCA **itself** flags it.

**Near-zero (translation/rotation) modes:** ORCA prints the 6 rigid-body modes as **exactly
`0.0000000000000000`** — in `.hess`, in `.out` (`0.00 cm**-1`), and in `.property.txt`
`&FREQ`. They are already projected out; there is **no small-nonzero residue** to threshold
away. Minimum run (`d7992449`): 6 exact zeros, then mode 6 = `318.41 cm⁻¹`. Consequence for a
cutoff: the trans/rot modes need no threshold (they are literally 0); a threshold only serves
to decide whether a small negative like `-33.66` is a genuine imaginary or numerical noise —
and for that, ORCA's own `***imaginary mode***` marker is the measured signal.

---

## Domain rule #5 — cclib parse cost (measured)

| item | value |
|---|---|
| largest `output.out` available | **647 818 B** (GOAT `04aeca22`) |
| cclib import floor (import cclib+numpy, parse nothing) | **≈ 85 340 KiB ≈ 83 MiB** peak RSS |
| full-parse peak RSS | **UNMEASURABLE** — no parse completes (crash at 3–21 ms) |
| crash wall time | SP 3.0 ms · min 3.1 ms · saddle 3.7 ms · GOAT 21.6 ms |

Rule #5 (never load a whole output into memory) cannot be evaluated against cclib on ORCA
6.1.0 because cclib produces no successful parse to measure. The only hard number is the
~83 MiB the library costs just to import.

---

## Structured-artifact inventory (measured)

### `input.hess` (Freq jobs) — the frequency source
Sections (`^\$…`), verbatim, in order:
`orca_hessian_file, act_atom, act_coord, act_energy, multiplicity, hessian,`
`vibrational_frequencies, normal_modes, atoms, actual_temperature,`
`frequency_scale_factor, dipole_derivatives, ir_spectrum, end`.
Contains frequencies (`$vibrational_frequencies` 24/57 rows, signed), normal modes
(`$normal_modes`), and IR (`$ir_spectrum`, 24 rows). Sizes: 30 KB (min) / 150 KB (saddle).

### `input_trj.xyz` / `input.xyz`
Multi-frame trajectory measured: **min 5 frames**, **saddle 8**, **GOAT 18**; natom per
frame = input natom (8/19/33). `input.xyz` = **1 frame** = the final geometry.

### `input.property.txt` — machine-readable ORCA 6 property file
`$Block … $End` with `&prop [&Type …]` entries. Blocks measured per job type:

- **Opt+Freq** (17 unique blocks): `Calculation_Status, Geometry (×5), SCF_Energy,`
  `DFT_Energy, SCF_Mulliken/Loewdin/Mayer_Population_Analysis (&AtomicCharges),`
  `SCF_Dipole_Moment, SCF_Nuc_Gradient, Hessian (&Dim (24,24)), THERMOCHEMISTRY_Energies,`
  `Single_Point_Data, VdW_Correction, gCP_Energy, Calculation_Info, Calculation_Timings`.
  `$THERMOCHEMISTRY_Energies` carries `temperature, elEnergy, zpe, innerEnergyU, enthalpyH,`
  `entropyS, freeEnergyG, numOfFreqs, &FREQ[24]` (the FREQ array zeros the 6 trans/rot modes).
  `elEnergy = -79.7918513760713` matches `.out` `FINAL SINGLE POINT ENERGY -79.791851376071`.
- **SP** (14 blocks): same minus `Hessian`, `THERMOCHEMISTRY_Energies`, `SCF_Nuc_Gradient`;
  one `$Geometry`. Has `$SCF_Dipole_Moment` and the three population analyses.
- **GOAT** (2 blocks only): `Geometry (×18)`, `Single_Point_Data`. No charges/dipole/thermo.

IR: `.property.txt` contains IR references (11 grep hits) but the clean structured IR source
is `.hess $ir_spectrum`; `.out` also prints an `IR SPECTRUM` table (line 3704 in the min run).

### `/opt/orca/orca_2json` — gbw → JSON converter (present, runs)
Invocation (measured): `orca_2json input.gbw` **with the extension** (`input` alone →
`Error: Cannot open GBW file`), and `LD_LIBRARY_PATH=/opt/orca` set. `returncode 0`, emits
`input.json` (198 176 B for the 8-atom min run). Top keys: `ORCA Header, Citations, Molecule`.
`Molecule` gives:

- `Atoms`: per atom `Coords` (**Angs**, the *final* geometry — z = 0.7635 vs input 0.768),
  `ElementNumber`, `Idx`, `NuclearCharge`, full basis.
- `MolecularOrbitals`: `EnergyUnit = Eh`, `MOs` (68 for the min run) each with
  `OrbitalEnergy` + `Occupancy` (2.0 … 0.0 → HOMO/LUMO derivable) + `MOCoefficients`.
- `Charge`, `Multiplicity`, `PointGroup` (`C1`).

**Not** in the gbw-json: frequencies, thermochemistry, Hessian (grep for
`freq|hessian|thermo|enthalp|entropy|gibbs|vibration` → 0 hits). It is the wavefunction/MO +
final-geometry source, nothing vibrational.

Other converters present in `/opt/orca` (not run here): `orca_2aim`, `orca_2mkl`, `orca_mapspc`
(spectrum broadening), `orca_chelpg`. Named for future probes, not measured.

#### `orca_2json` scaling — the rule-#5 gate (measured, unit 3.7)
The JSON is dominated by `MolecularOrbitals.MOs[].MOCoefficients` — an n×n matrix (n = basis
functions) we do **not** need (we want only `OrbitalEnergy` + `Occupancy`).

| job | atoms / basis | `.gbw` | `.json` | nMO | MOCoeff bytes | % of json |
|---|---|---|---|---|---|---|
| ethane (def2-SVP) | 8 | 1.08 MB | 198 KB | 68 | 104 KB | **52.5%** |
| saddle (def2-TZVP) | 19 | 1.97 MB | **3.5 MB** | 314 | 2.17 MB | **62%** |

(`MOCoeff bytes` = sum of `len(json.dumps(MOCoefficients))` over MOs.) **Extrapolation** (shown,
not felt): fit `nBF ≈ 31·heavy + 5·H` to the saddle (8 heavy·31 + 11 H·5 = 303 ≈ measured 314);
json ∝ nBF². A 50-atom organic (~30 heavy + 20 H) → 1030 BF → 3.5 MB·(1030/314)² ≈ **38 MB**; a
60-atom (~35 heavy + 25 H) → 1210 BF → ≈ **52 MB**, of which ~60% (~23–31 MB) is coefficients we
discard. Tens of MB for the author's realistic case — a real rule-#5 hazard, not hundreds of MB.

**Flags (from `orca_2json`'s own `-h` output, not memory):** `-json/-bson/-ubjson/-msgpack`
(format only), `-gbw`, `-property*` (property txt↔json). **No flag omits MO coefficients.**

**Resolution (gate PASS):** stream the file with `serde_json::from_reader` (already a dependency)
into a struct that simply **omits** `MOCoefficients` — serde consumes it as `IgnoredAny`
(tokenized past, never allocated), so peak memory is the two small per-MO arrays, not the whole
file. Reading it whole into a `serde_json::Value` would be the `.out` mistake in JSON clothing.

### `_trj.xyz` comment line (measured — unit 3.7)
Verbatim, **identical format on every frame of every job type** (Opt / GOAT / scan) and on `.xyz`:

```
Coordinates from ORCA-job input E -79.791800280837   ← Opt+Freq _trj frame
Coordinates from ORCA-job input E -45.164880639800   ← GOAT _trj frame
Coordinates from ORCA-job input E -79.781552926389   ← scan _trj frame
Coordinates from ORCA-job input E -79.791851376071   ← .xyz (final geometry)
```

So the comment carries the **frame energy in Eh** after `" E "` — parseable to `Option<f64>`,
uniform across job types. `.allxyz` is **different** (`… Relaxed Surface Scan Step N E …`, and
`>`-separated) — out of scope (Phase 4.5), not fed to the xyz reader.

### Relaxed scan artifacts (measured — unit 3.3)
A real 6-point relaxed C–C scan of ethane was run **from the terminal** (not the app — the scan
generator is Phase 4.5), `! r2SCAN-3c Opt TightSCF` + `%geom Scan B 0 1 = 1.4, 2.4, 6 end end`,
indices 0-based and range-checked before the run (`constraints.md`). Note: a relaxed scan needs
`! Opt` — without it ORCA runs a single point and silently ignores the `Scan` block (measured:
1 energy, 1 geometry). Files that appear in a scan and **not** in an Opt+Freq run (verbatim
listing):

- `input.relaxscanact.dat` / `input.relaxscanscf.dat` — **structured**, 2 columns
  `coordinate  energy`, one row per scan point (6). `act` = the actual (final composite) energy,
  `scf` = the bare SCF energy; the two differ (r²SCAN-3c has gCP+D4 terms).
- `input.allxyz` — the 6 relaxed geometries, `>`-separated; each comment line carries
  `Relaxed Surface Scan Step N E <energy>` (energy, **not** the coordinate value).
- `input.001.xyz … input.006.xyz` + `input.001.gbw … input.006.gbw` — per-point final geometry
  and wavefunction.

**Where per-point energies + the scanned coordinate live** (the Phase-4.5 question): the clean
**structured** source is `.relaxscanact.dat` / `.relaxscanscf.dat` (coordinate + energy, 6 rows).
The **text** mirror is the `.out` `RELAXED SURFACE SCAN RESULTS` summary table ("The Calculated
Surface using the 'Actual Energy'" / "using the SCF energy"). `.property.txt` here holds **26**
`$Geometry` blocks (every opt cycle across all points, **not** 6 scan points) and `_trj.xyz` 26
frames — so property/trj are **per-cycle, not per-point**; they are *not* the scan source.
Atom-order gate ran on the scan dir: **PASS** — all 26 trj frames and all 26 `$Geometry` blocks
keep the input element order (best available test for a silent mid-scan reorder).

---

## Units — measured per array (unit 3.3)

The one risk the Part-A gate touched but did not formalise: the authoritative tier spans **two
length-unit systems**. Every unit below is set by exactly one method — (1) a file literal quoted
verbatim, (2) a numeric cross-check with the **ratio as a number**, or (3) a determiner run —
never from convention. `UNDETERMINED` is a real, allowed result. Produced by `probe_units()` in
`sidecar/probes/parse_sources.py`.

| Quantity (artifact) | Unit | Method | Number |
|---|---|---|---|
| length — `.property.txt $Geometry`, `.hess $atoms` | **Bohr** | (1) `&Units "Bohr"` (property); (2) for `.hess`, ratio vs input | 1.8886 / 1.6579 |
| length — `orca_2json` | **Å** | (1) `CoordinateUnits "Angs"` | — |
| length — `.xyz`, `_trj.xyz`, `.allxyz` | **Å** | (2) ratio 1.0 vs `orca_2json`/input | 1.0 |
| energy — `.property.txt` SCF/DFT/Single_Point | **Eh** | (2) equals `.out FINAL SINGLE POINT ENERGY` | 1.0 |
| energy — `orca_2json` MO | **Eh** | (1) `EnergyUnit "Eh"` | — |
| frequency — `.hess $vibrational_frequencies` | **cm⁻¹** | (2) equals `.out` `cm**-1` listing | 1.0 |
| frequency — `.property.txt &FREQ` | **cm⁻¹** | (1) `&Units "cm^-1"` | — |
| normal modes — `.hess $normal_modes` | **Cartesian** normalized displacement (dimensionless) | (3) determiner run: `orca_pltvib` frame minus equilibrium ÷ raw column, per atom — 2.0000 for **all 8** ethane atoms (H/C = 1.0000, not √12≈3.46) → Cartesian, not mass-weighted | H/C = 1.0000 |
| IR intensity — `.hess $ir_spectrum` col2 | **km/mol** | (1) `.out` `IR SPECTRUM` header names it; (2) col2 = `.out` Int | 1.0 |
| dipole — `.property.txt $SCF_Dipole_Moment` | **a.u.** | (1) `&Units "a.u."` | — |
| gradient — `.property.txt $SCF_Nuc_Gradient &grad` | **Eh/Bohr** | (1) `.out` labels `Eh/bohr` (property has no literal) | — |
| thermo — `zpe / innerEnergyU / enthalpyH / freeEnergyG` | **Eh** | (1) `.out` prints each in Eh | — |
| thermo — `entropyS` | **T·S, in Eh** (not S) | (2) `entropyS == enthalpyH − freeEnergyG` | exact |
| `.hess $hessian` / `.property.txt $Hessian` | **UNDETERMINED** (same numbers both files) | (2) `.hess[0,0] == property[0,0]`; absolute unit unmeasured | 1.0 |
| `.hess $dipole_derivatives` | **UNDETERMINED** | none of (1)/(2)/(3) | — |
| scan coordinate — `.relaxscanact.dat` col1 | **Å** | (2) = the C–C distance in that point's geometry | 1.0 |
| scan energy — `.relaxscanact.dat` col2 | **Eh** | (2) = the point's FINAL SINGLE POINT ENERGY | 1.0 |

**Canonical app units** (rule #11): length **Å**, energy **Eh**, frequency **cm⁻¹**, IR
intensity **km/mol**. The one conversion every geometry reader owes at its boundary is
**Bohr→Å** (`.property.txt`, `.hess`); everything else is already canonical. `$hessian` and
`$dipole_derivatives` stay `UNDETERMINED` — determiners: reconstruct frequencies from the
Hessian + atomic masses (expected Eh/Bohr²); cross-check `$dipole_derivatives` against IR
intensities. Neither is a Results-screen display quantity.

`$normal_modes` is **Cartesian** (settled by the unit-3.6 gate — Σ² = 1.0 only proved
*normalization*, and mass-weighted eigenvectors are unit-norm too, so it needed a determiner run).
Determiner: `orca_pltvib m.hess 9` (mode 9 = 997 cm⁻¹, non-degenerate, C and H both move); the
first block's displacement columns (Å) ÷ the raw `$normal_modes` column, per atom. Measured
per-atom ratio = **2.0000 for all 8 atoms** (both C at 12.011 u and H at 1.008 u), i.e. **H/C =
1.0000, not √(12/1) ≈ 3.4519** — a single scalar (pltvib's animation amplitude) independent of
mass. Mass-weighted modes would have split the ratio by 3.46. **Consequence for the `.hess`
reader:** normal modes are consumed as-is (Cartesian displacement vectors); **no ÷√m** and no atomic-
mass table is needed.

---

## Summary table — where each Results-screen quantity can be read (measured)

Legend: ✅ measured available · ❌ measured NOT available · — not applicable.
"cclib" = cclib 1.8.1 as installed (crashes; only what it harvests before dying).

| Quantity | cclib 1.8.1 | Structured artifact | `.out` text |
|---|---|---|---|
| Final SCF energy | ❌ (only 1st-cycle SCF before crash) | ✅ `.property.txt $SCF/DFT_Energy`, `$Single_Point_Data` | ✅ `FINAL SINGLE POINT ENERGY` (Rust already does this) |
| Final geometry | ❌ (only `atomcoords[0]` = initial) | ✅ `input.xyz` (1 frame); `.property.txt` last `$Geometry`; `orca_2json` `Atoms.Coords` (Angs) | ✅ `CARTESIAN COORDINATES` |
| Trajectory | ❌ | ✅ `input_trj.xyz` (5/8/18 frames); `.property.txt` N×`$Geometry` | ✅ per-cycle coords |
| Frequencies (signed) | ❌ (never reaches `vibfreqs`) | ✅ `.hess $vibrational_frequencies`; `.property.txt &FREQ` (zeros trans/rot) | ✅ `VIBRATIONAL FREQUENCIES` |
| IR intensities | ❌ | ✅ `.hess $ir_spectrum` (24 rows) | ✅ `IR SPECTRUM` table |
| Normal modes | ❌ | ✅ `.hess $normal_modes` | ✅ `NORMAL MODES` |
| Atomic charges | ❌ | ✅ `.property.txt $SCF_*_Population_Analysis &AtomicCharges` (Mulliken/Loewdin/Mayer) | ✅ population sections |
| Dipole | ❌ | ✅ `.property.txt $SCF_Dipole_Moment` | ✅ `DIPOLE MOMENT` |
| MO energies & occupations | ❌ | ✅ `orca_2json` `MolecularOrbitals` (Eh, Occupancy) | ✅ `ORBITAL ENERGIES` |
| HOMO/LUMO gap | ❌ | ✅ derived from `orca_2json` occupancy boundary | ✅ derived from `ORBITAL ENERGIES` |
| Thermochemistry (ZPE/H/S/G) | ❌ | ✅ `.property.txt $THERMOCHEMISTRY_Energies` | ✅ `GIBBS FREE ENERGY` etc. |

Every ❌/✅ above rests on a probe run in §1–§5, not on expectation.

---

## Gaps (runs the probe could NOT make)

- ~~No scan job~~ **CLOSED (unit 3.3):** a real relaxed scan was run; per-point energy +
  coordinate live in the structured `.relaxscanact.dat` / `.relaxscanscf.dat` (see the scan
  inventory above). The scan dir lives at `~/.local/share/orcastudio/probe-scans/scan-ethane-cc`
  (terminal run, **not** an app job — no SQLite row).
- **No TD-DFT / excited-state job** → `etenergies` / `etoscs` sources unmeasured (separate unit,
  Phase 6).
- **cclib full-parse peak RSS** unmeasurable (no successful parse) — rule #5 cannot be checked
  against cclib until cclib parses ORCA 6.1 at all.
- **Final-geometry atom order via cclib** not directly confirmed (cclib only exposed the
  initial geometry); confirmed indirectly via constant natom in `_trj`/`.property.txt` and
  `C1` point group.
- Whether a **cclib development build** (git `master`, not on PyPI) parses ORCA 6.1 was not
  tested — only the released 1.8.1 was, and it fails.

---

## Re-running on the next ORCA version

`sidecar/probes/parse_sources.py` takes `--sp/--optfreq/--saddle/--goat/--scan <job_dir>` and
re-emits every number above. Run it after any ORCA or cclib upgrade; if cclib starts parsing
6.x cleanly, the crash rows here flip and the summary table is regenerated from evidence.
