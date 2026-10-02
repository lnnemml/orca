# Cross-version fixtures — ORCA 6.1.0 vs 6.1.1

Matched pairs for the parser cross-version regression
(`src-tauri/src/parse/cross_version_6_1_1.rs`). Each `<case>/` holds `v610/` and
`v611/`: the **same input** run by `/opt/orca-6.1.0/orca` and `/opt/orca-6.1.1/orca`.

## Provenance (rule #10 — from real runs, 2026-10-02, dev laptop)

- Both binaries invoked by absolute path (rule #1), one isolated dir per run (rule #3),
  `%pal nprocs 4`, explicit `%maxcore 2000`, **sequential** (6.1.0 then 6.1.1, never
  concurrent). Every run reached `ORCA TERMINATED NORMALLY` (rule #6).
- Only the files the readers consume are committed — no `.gbw`/`.densities`/`.tmp`.
  `mayer_tail.out` is the final `MAYER POPULATION ANALYSIS` block through EOF (the
  reader takes the last block); the full `output.out` is not committed.

## Cases

| case | system | keywords | readers exercised |
|---|---|---|---|
| `opt_freq_water` | H₂O | `r2SCAN-3c Opt Freq TightSCF` | property, hess, xyz (`_trj`), mayer |
| `smd_water` | H₂O | `r2SCAN-3c TightSCF SMD(water)` | property (implicit solvation energy) |
| `dlpno_hcn` | HCN | `DLPNO-CCSD(T) def2-TZVP def2-TZVP/C def2/J RIJCOSX` | property (WF energy) |
| `scan_ethane` | C₂H₆ | `r2SCAN-3c Opt` + `%geom Scan B 0 1` | relaxscan (`.relaxscanact/.scf.dat`) |
| `optts_hcn` | HCN↔HNC | `r2SCAN-3c OptTS Freq` + `Calc_Hess true` | property, hess (imaginary mode) |

`scan_ethane` reuses the verbatim input of the existing `scan-ethane-cc/` fixture.

## Result (2026-10-02)

Every compared field was **bit-identical** between 6.1.0 and 6.1.1 — Δ = 0, no
tolerance consumed. The old 6.1.0 fixtures elsewhere in `tests/fixtures/` are **not**
replaced; these are additional.

## Not here: NEB-TS

NEB-TS HCN↔HNC at r2SCAN-3c does not finish within the 10-minute laptop guard (the
climbing-image TS + numerical-Hessian tail). No clean 6.1.1 pair exists yet; the gap
is kept visible as an `#[ignore]` test (`neb_ts_cross_version_deferred`), a candidate
for a server run. The existing `neb/` (Menshutkin, 6.1.0) fixture is untouched.
