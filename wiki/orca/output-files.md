# ORCA output files — what each artifact is

| File | What it is | Our handling |
|---|---|---|
| `output.out` | Main text log; everything cclib parses | Stream during run; keep; parse after |
| `.gbw` | Wavefunction/orbitals (binary) | Needed by orca_plot; large — remote fetch opt-in |
| `_trj.xyz` / `.xyz` | Optimization trajectory / final geometry | Always fetch; trajectory player |
| `.hess` | Hessian (frequencies) | Fetch for Freq jobs |
| `.cube` | Volumetric data from orca_plot | Generate lazily; cache; never in DB |
| `.densities`, `.tmp*` | Scratch | Delete on cleanup |
| `.property.txt` / property files | Machine-readable results (ORCA 6) | Evaluate as parse source alongside cclib |

## Completion signals
- Success: `ORCA TERMINATED NORMALLY` near end of output + `.exit_code` = 0 (our marker).
- `.exit_code` present but nonzero / string absent → failed; surface the tail of the
  output in the UI (ORCA's error messages are usually in the last ~50 lines).

## Imaginary frequencies
Negative frequencies in Freq output = saddle point, not a minimum. The Results screen must
flag this loudly — core teaching moment.

## Convergence blocks (for live parsing)
Formats confirmed against real ORCA 6.1 output (r²SCAN-3c). Parsed incrementally by
`src-tauri/src/convergence.rs` for the live dashboard; will be reused in Phase 3 (results)
and Phase 4.5 (scan energy profiles).

### SCF iteration table
There is **no `SCF ITERATIONS` banner** in ORCA 6 — each SCF algorithm prints its own column
header, and one SCF solve can span several sub-tables (DIIS → SOSCF → …) with a **continuous**
iteration counter:

```
----------------------------------------D-I-I-S--------------------------------------------
Iteration    Energy (Eh)           Delta-E    RMSDP     MaxDP     DIISErr   Damp  Time(sec)
-------------------------------------------------------------------------------------------
    1    -79.6871232228694026     0.00e+00  6.89e-03  3.47e-02  1.88e-01  0.700   0.0
    2    -79.7391069690580849    -5.20e-02  5.01e-03  2.21e-02  9.48e-02  0.700   0.0
                              *** Initializing SOSCF ***
---------------------------------------S-O-S-C-F--------------------------------------
Iteration    Energy (Eh)           Delta-E    RMSDP     MaxDP     MaxGrad    Time(sec)
--------------------------------------------------------------------------------------
    6    -79.7948730896764999    -2.14e-04  2.12e-04  8.13e-04  1.92e-03     0.0
```

Row fields: `iter energy delta_e …`. `iter` starts at **1** (not 0); `delta_e` is in
**scientific notation** and is `0.00e+00` on the first iteration (positive/zero, not negative).
The column set differs by algorithm (DIIS has `DIISErr Damp`, SOSCF has `MaxGrad`) — parse
**tolerantly** (int iter, negative-decimal energy, ≥3 numeric fields) rather than by column.

**Gotcha — Freq eigenvector rows look identical.** The normal-mode displacement matrix in
Freq output has the exact same shape (`int  -0.000014  0.048084 …`). Only a state gate
(inside an `Iteration … Energy (Eh)` table, closed by `SCF CONVERGED` / `TOTAL SCF ENERGY`)
keeps them out. Never rely on line shape alone.

### Geometry convergence table
```
          ----------------------|Geometry convergence|-------------------------
          Item                value                   Tolerance       Converged
          ---------------------------------------------------------------------
          Energy change      -0.0000431163            0.0000050000      NO
          RMS gradient        0.0003022636            0.0001000000      NO
          MAX gradient        0.0011580689            0.0003000000      NO
          RMS step            0.0013159917            0.0020000000      YES
          MAX step            0.0049021954            0.0040000000      NO
          -------------------------------------------------------------------------
```

Per-criterion: a (1–2 word) name, then `value tolerance YES|NO`. **The criterion count
varies** — the first cycle has **no `Energy change`** (only 4 rows; no previous energy yet),
`OptTS` adds more — so don't hardcode 5. The block is bounded by dashed rules: the header
underline comes *before* any criterion, the closing rule comes *after* ≥1 criterion. The
cycle's energy is the `FINAL SINGLE POINT ENERGY` printed earlier in the same cycle. Gradient
and step values are magnitudes (always > 0 → safe for a log axis); `Energy change` is signed.
