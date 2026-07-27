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

### Rule 1 — Composite ("3c") methods are self-contained
`r2SCAN-3c`, `B97-3c`, `PBEh-3c`, `wB97X-3c`, `HF-3c` **already include their own basis set,
dispersion correction, and (where needed) geometric/BSSE corrections.** When a 3c method is
chosen, emit ONLY its name — do **not** add a basis, dispersion, or RI keyword. Adding
`def2-TZVP` to `r2SCAN-3c` is a real error that corrupts the calculation, so the builder disables
those fields in composite mode.

```
! r2SCAN-3c Opt Freq TightSCF        ← correct
! r2SCAN-3c def2-TZVP D4 Opt         ← WRONG (double basis + dispersion)
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

### Rule 6 — Coordinates are preserved
The builder generates only the `!` line and `%` blocks; the existing `* xyz charge mult ... *`
atom rows are carried through verbatim (`buildOrcaInput` wraps them, using the form's charge /
multiplicity for the header). When no geometry is present it emits a commented placeholder.

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
