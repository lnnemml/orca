# Anatomy of an ORCA input file

```
! B3LYP def2-TZVP def2/J RIJCOSX D4 CPCM(water) Opt Freq TightSCF
%pal nprocs 8 end
%maxcore 3000
* xyz 0 1
O   0.000000   0.000000   0.117790
H   0.000000   0.755453  -0.471161
H   0.000000  -0.755453  -0.471161
*
```

- **`!` simple input line** — method, basis, aux basis, approximations, job type, keywords.
  Order-insensitive. This is what the input-builder form generates.
- **`%` blocks** — detailed settings (`%pal`, `%maxcore`, `%tddft`, `%geom`, `%cpcm`, ...).
- **`* xyz charge multiplicity ... *`** — geometry. Also `* xyzfile 0 1 mol.xyz`.

## The `!` keyword line — builder reference

The Phase 2.4 input builder (`src/input-builder/`) generates the `!` line from dropdowns.
The rules below are baked into `build-input.ts` and checked against the ORCA 6.1 manual
(https://www.faccts.de/docs/orca/6.1/manual/).

### Rule 0 — The method comes from one of three families
`buildKeywordLine` branches on `state.methodFamily: "composite" | "dft" | "wavefunction" | "xtb"`
(N1a added composite/dft/xtb; N1b added wavefunction):

| Family | Emits | Basis / aux / RI / disp | Solvation + SCFConv tail |
|---|---|---|---|
| `composite` | the 3c method name only | never (self-contained, Rule 1) | applies |
| `dft` | functional + basis (+ aux/RI/dispersion) | per Rules 1–2 | applies |
| `wavefunction` | correlated method + basis (RI/DLPNO add `/C`+Coulomb-aux+RIJCOSX) | per Rule 2c; **never dispersion** | applies |
| `xtb` | the xTB method keyword **only** (`XTB` = GFN2-xTB) | **never** | **suppressed** |

`xtb` is fully self-contained: a `! XTB` line carries the method + job type and **nothing else** —
no basis, aux, RI, dispersion, solvation, or SCFConv. `! XTB def2-TZVP SMD(water) TightSCF` is
invalid, so the whole basis block *and* the solvation+scfConv tail are guarded off for that family
(`state.methodFamily !== "xtb"`). Job type is still emitted for every family (`! XTB Opt`,
`! XTB NEB-TS`). See `wiki/orca/xtb-method.md` for the probe facts (ORCA's `! XTB` = GFN2-xTB via
the bundled `otool_xtb`, distinct from the standalone `xtb.md` binary).

The `wavefunction` family keeps the solvation + SCFConv tail (it is **not** xtb — C-PCM/CCSD is
valid, and post-HF *needs* a tight SCF) but emits **no dispersion keyword** (the correlation *is*
the dispersion). Its aux chain is Rule 2c. See `wiki/orca/correlated-methods.md` for the probe.

### Rule 1 — Composite methods and dispersion-inclusive functionals are self-contained
`r2SCAN-3c`, `B97-3c`, `PBEh-3c`, `wB97X-3c`, `HF-3c` **already include their own basis set,
dispersion correction, and (where needed) geometric/BSSE corrections.** When a 3c method is
chosen, emit ONLY its name — do **not** add a basis, dispersion, or RI keyword. Adding
`def2-TZVP` to `r2SCAN-3c` is a real error that corrupts the calculation, so the builder disables
those fields in composite mode.

```
! r2SCAN-3c Opt Freq TightSCF        ← correct
! r2SCAN-3c def2-TZVP D4 Opt         ← WRONG (double basis + dispersion)
```

The same self-containment applies to **dispersion-inclusive functionals** used in
Functional + Basis mode. Some functional names bake the dispersion correction into the name
itself: `wB97X-D4` (the `-D4` *is* the D4 correction) and `wB97M-V` (the `-V` *is* the VV10
non-local term). For these, emit the functional but **not** a separate dispersion keyword —
appending `D4` again double-counts the correction. The builder marks such functionals with
`builtInDispersion: true` (see `orca-options.ts`), `buildKeywordLine` skips the dispersion token
via `functionalHasBuiltInDispersion()`, and the form disables the Dispersion dropdown with the
hint "Included in the functional".

```
! wB97X-D4 def2-TZVP def2/J RIJCOSX Opt Freq   ← correct (D4 is in the name)
! wB97X-D4 def2-TZVP D4 Opt Freq               ← WRONG (D4 counted twice)
```

### Rule 2 — RI needs a matching auxiliary (fitting) basis
The RI approximations fit the Coulomb (and optionally exchange) integrals and require an
auxiliary basis. For `def2-*` orbital bases:

| RI keyword | Aux basis | Meaning |
|---|---|---|
| `RIJCOSX` | `def2/J` | Coulomb via RI-J, exchange via seminumerical COSX. Hybrid-DFT speedup. |
| `RI` (RI-J) | `def2/J` | Pure (non-hybrid) functionals only. |
| `RI-JK` | `def2/JK` | Coulomb **and** exchange fitted. Accurate; larger aux basis. |

The builder adds the aux basis automatically when an RI method is chosen with a `def2` basis.

```
! B3LYP def2-TZVP def2/J  RIJCOSX Opt      ← RIJCOSX / RI-J → def2/J
! B3LYP def2-TZVP def2/JK RI-JK   Opt      ← RI-JK        → def2/JK
```

**Non-def2 bases (Dunning, Pople) → `AutoAux`.** The tuned `def2/J`/`def2/JK` fit sets exist only
for the Karlsruhe def2 family. When an RI method is chosen with a `cc-pV*Z`, `aug-cc-pV*Z`, or
`6-31G*`…`6-311++G**` basis, `auxBasisFor` emits ORCA's general **`AutoAux`** — an auto-generated
auxiliary. It is the honest choice: ORCA fails loud on a bad/absent aux, never silently wrong, so
`AutoAux` is safe as the fallback rather than guessing a mismatched def2 fit set.

```
! B3LYP cc-pVTZ AutoAux RIJCOSX Opt        ← non-def2 basis + RI → AutoAux
```

### Rule 2b — Basis families offered by the builder
`BASIS_GROUPS` (`orca-options.ts`) groups the catalog as `<optgroup>`s: **Karlsruhe def2**
(`def2-SVP`…`def2-QZVPP`, the diffuse `def2-*D`, and minimally-augmented `ma-def2-SVP/TZVP/TZVPP`),
**Dunning** (`cc-pVDZ/TZ/QZ`, `aug-cc-pV{D,T,Q}Z`), and **Pople** (`6-31G*`, `6-31G**`, `6-311G**`,
`6-311+G**`, `6-311++G**`). A flat `BASIS_SETS = BASIS_GROUPS.flatMap(...)` is kept for importers.
Only def2-* gets a tuned aux (Rule 2); everything else pairs with `AutoAux` under RI.

### Rule 2c — Correlated methods need a *correlation* aux (`/C`), and only RI/DLPNO variants
Post-HF RI needs **two** fit sets: `/J` (Coulomb, as Rule 2) **and** `/C` (correlation — fits the
MP2/CC amplitudes, a different basis you cannot substitute for `/J`). The rule splits by method:

- **RI/DLPNO** (`RI-MP2`, `DLPNO-MP2`, `DLPNO-CCSD(T)`, `DLPNO-CCSD(T1)`) → emit
  `<basis>/C <Coulomb-aux> RIJCOSX`. `<Coulomb-aux>` follows Rule 2 (def2 → `def2/J`, else nothing
  because — see below — the `/C` also falls away for non-def2).
- **Canonical** (`MP2`, `CCSD`, `CCSD(T)`) → emit **no aux at all**. A spurious `/C RIJCOSX` on a
  canonical method is a *different, RI-approximated* calculation, not the one requested.

Only the **def2** family has a probed native `/C`, so the builder emits `<basis>/C` **for def2
only**; every non-def2 basis (Dunning, Pople) falls back to bare **`AutoAux`** (covers J+C
together) — consistent with Rule 2's non-def2 path and guaranteed valid.

```
! DLPNO-CCSD(T) def2-TZVP def2-TZVP/C def2/J RIJCOSX TightSCF   ← measured (ORCA 6.1, HCN SP)
! CCSD(T)       def2-TZVP TightSCF                              ← canonical: NO aux chain
! DLPNO-CCSD(T) cc-pVTZ AutoAux RIJCOSX                         ← non-def2 → bare AutoAux
```

No dispersion keyword ever joins a wavefunction line. The tighter Dunning form
(`cc-pVTZ/C def2/J RIJCOSX`) is a deliberate future refinement pending its own probe — see
`wiki/orca/correlated-methods.md`.

### Rule 3 — Canonical keyword order (for readability)
ORCA is order-insensitive, but the builder emits a fixed order so the line reads consistently:

```
method  basis  auxbasis  RI  dispersion  solvation  jobtype  scfconv
```

Example: `! B3LYP def2-TZVP def2/J RIJCOSX D4 CPCM(water) Opt Freq TightSCF`

### Rule 4 — `%maxcore` is a directive, `%pal` is a block
`%maxcore` (MB per core) takes **no** `end`; `%pal` does. (Established Phase 1.2 — don't break it.)

```
%pal nprocs 4 end     ← block, WITH end
%maxcore 3000         ← directive, NO end
```

### Rule 5 — Solvation syntax
`model(solvent)` — the solvent in parentheses, no space. Nothing is emitted in gas phase.

```
CPCM(water)
SMD(acetonitrile)
```

### Rule 6 — Coordinates come from the Scene
The builder generates only the `!` line and `%` blocks; the geometry is supplied by a Scene.
`buildOrcaInput(state, scene)` takes a **Scene** or `null` (the string branch was removed in
2.5.0d — it existed only to keep two tests, and tests must not dictate the API):
- **Scene** — coordinates are the canonical merged rows (`mergeToAtomLines`, `toFixed(8)`); charge
  and multiplicity come from the Scene (see below).
- **null** — a commented placeholder, with the form's own charge / multiplicity fallback.

### Rule 7 — Charge and multiplicity come from the Scene
When a Scene drives generation (ADR-008), the `* xyz charge mult` header is **derived**, not typed:
- **charge = `totalCharge(scene)`** — the sum of the per-fragment formal charges. In the builder
  form the Charge field is read-only and shows this sum; you change it by setting a fragment's
  charge (2.5.0d-2 fragment UI), not by typing into the header.
- **multiplicity = `scene.multiplicity`** — a genuine physical choice, so it stays user-editable;
  the form writes the user's value into the Scene before generating.

**Electron parity** (`checkElectronParity`, `src/scene/parity.ts`): the electron count
(Σ Z − totalCharge) fixes the *parity* of the allowed multiplicity — even electrons ⇒ odd
multiplicity (singlet/triplet/…), odd electrons ⇒ even multiplicity (doublet/quartet/…). A
mismatch is the error ORCA reports cryptically ~30 s into a run; the form surfaces it instantly as
an inline, explanatory warning (not a blocker — Generate still works).

> **Important — what parity does *not* check.** It validates only the *arithmetic* possibility of a
> multiplicity for a given electron count. It says nothing about whether that spin state is a
> physically sensible ground state for the molecule (a triplet may be arithmetically allowed and
> chemically absurd). Physical reasonableness is the chemist's judgement; we only catch the
> provably-impossible parity mistake.

## Dispersion keywords
`D4` (newest, recommended), `D3BJ` (Becke-Johnson damping), `D3Zero` (zero damping),
`NL` (non-local VV10, more expensive). None for the composite methods (built in).

## Template library seeds (Phase 1)
- `SP`: `! r2SCAN-3c TightSCF`
- `Opt`: `! r2SCAN-3c Opt`
- `Opt+Freq`: `! r2SCAN-3c Opt Freq`
- `DFT quality`: `! wB97X-D4 def2-TZVP def2/J RIJCOSX Opt Freq`
- `TD-DFT` (Phase 6): `! wB97X-D4 def2-TZVP` + `%tddft nroots 15 end`

## Not yet modelled by the builder (deliberate)
- Double-hybrid functionals (need an AuxC correlation-fitting basis — separate logic).
- `%geom Scan` / NEB (Phase 4.5, reaction modeling).
- The full 179-solvent list — a curated 20 covers routine work.
- Reverse parsing (input text → form) — the form → text flow is one-way (see ROADMAP).
- Fragment annotation `(1)` / `(2)` on coordinate lines — deliberately unsupported.
  It serves compound methods (DFT-SAPT, counterpoise BSSE), not standard Opt/Freq/Scan,
  so OrcaStudio merges multi-fragment scenes into one flat `* xyz ... *` block (ADR-008).
  Forward note: ORCA 6 also uses `%geom` fragments for rigid-body optimisation and
  fragment constraints — when we generate those blocks, our internal SceneFragment model
  becomes the source, rather than round-tripping through this input annotation.
