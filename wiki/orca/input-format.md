# Anatomy of an ORCA input file

```
! B3LYP D4 def2-TZVP def2/J RIJCOSX Opt Freq TightSCF CPCM(water)
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

## Template library seeds (Phase 1)
- `SP`: `! r2SCAN-3c TightSCF`
- `Opt`: `! r2SCAN-3c Opt`
- `Opt+Freq`: `! r2SCAN-3c Opt Freq`
- `DFT quality`: `! wB97X-D4 def2-TZVP def2/J RIJCOSX Opt Freq`
- `TD-DFT` (Phase 6): `! wB97X-D4 def2-TZVP` + `%tddft nroots 15 end`

Grow this page into a full keyword reference as keywords.json grows.
