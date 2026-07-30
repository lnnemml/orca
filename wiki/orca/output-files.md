# ORCA output files — what each artifact is

| File | What it is | Our handling |
|---|---|---|
| `output.out` | Main text log | **Not** an authoritative parse source ([ADR-012](../architecture/adr-012-output-parsing-ownership.md)): stream during run (`convergence.rs`), search (`output_search.rs`), two tail regexes (final energy, wall time). Rich results come from structured artifacts, not this file |
| `.gbw` | Wavefunction/orbitals (binary) | Needed by orca_plot; large — remote fetch opt-in |
| `_trj.xyz` / `.xyz` | Optimization trajectory / final geometry | Always fetch; trajectory player |
| `.hess` | Hessian (frequencies) | Fetch for Freq jobs |
| `.cube` | Volumetric data from orca_plot | Generate lazily; cache; never in DB |
| `.relaxscanact.dat` / `.relaxscanscf.dat` | Relaxed-scan surface: 2 cols `coordinate energy` (`act` = composite/actual, `scf` = bare SCF), one row per scan point | **The** structured per-point source for Phase 4.5 energy profiles ([parse-sources.md](parse-sources.md)) |
| `.allxyz` | Relaxed-scan geometries, `>`-separated; comment line carries `Step N E <energy>` | Geometry + energy per point (not the coordinate value) |
| `.NNN.xyz` / `.NNN.gbw` | Per-scan-point final geometry / wavefunction | Per-point artifacts |
| `.densities`, `.tmp*` | Scratch | Delete on cleanup |
| `.property.txt` / property files | Machine-readable results (ORCA 6) | **Measured** parse source: `$Block…$End` with energies, `$Geometry`, population `&AtomicCharges`, `$SCF_Dipole_Moment`, `$THERMOCHEMISTRY_Energies`, `$Hessian` — see [parse-sources.md](parse-sources.md) |

## Which artifact each result comes from (ADR-012)
Measured — full per-quantity table + evidence in [parse-sources.md](parse-sources.md):

- **`.property.txt`** — energies, geometry (Bohr), atomic charges (Mulliken/Loewdin/Mayer,
  each element-labelled via `&ATNO`), dipole, thermochemistry (ZPE/H/S/G).
- **`.hess`** — signed frequencies (`$vibrational_frequencies`, 3N), normal modes
  (`$normal_modes`, 3N×3N), IR (`$ir_spectrum`); `$atoms` coords are Bohr.
- **`_trj.xyz` / `.xyz`** — trajectory / final geometry.
- **`orca_2json` over `.gbw`** — MO energies + occupations (Eh), HOMO/LUMO.
- **`output.out`** — live streaming + search only; not authoritative.
- **Scan (`%geom Scan` + `! Opt`)** — per-point `coordinate energy` in the structured
  `.relaxscanact.dat` / `.relaxscanscf.dat`; `.out` `RELAXED SURFACE SCAN RESULTS` is the text
  mirror; `.property.txt`/`_trj.xyz` are per-opt-cycle, **not** per-point.

**Units are two systems** (measured — [parse-sources.md](parse-sources.md) "Units"): geometry in
`.property.txt`/`.hess` is **Bohr** (`&Units "Bohr"`), while `orca_2json`/`.xyz`/`_trj.xyz`/scan
are **Å**. Energies **Eh**, frequencies **cm⁻¹** (`&Units "cm^-1"`), IR **km/mol**, dipole
**a.u.** (`&Units "a.u."`), gradient **Eh/Bohr**; `$THERMOCHEMISTRY_Energies entropyS` is **T·S
in Eh**, not S. Each reader converts Bohr→Å once at its boundary (CLAUDE.md rule #11).

## Completion signals
- Success: `ORCA TERMINATED NORMALLY` near end of output + `.exit_code` = 0 (our marker).
- `.exit_code` present but nonzero / string absent → failed; surface the tail of the
  output in the UI (ORCA's error messages are usually in the last ~50 lines).

## Imaginary frequencies
Negative frequencies in Freq output = saddle point, not a minimum. The Results screen must
flag this loudly — core teaching moment.

**Measured (ORCA 6.1.0, saddle job `99e805f5`, see [parse-sources.md](parse-sources.md)):**
the 6 rigid-body (translation/rotation) modes are printed as **exactly `0.0000000000000000`**
in `.hess`, `.out`, and `.property.txt &FREQ` — already projected out, no small residue to
threshold. The imaginary mode appears with its **negative sign preserved** in `.hess`
(`6  -33.6608873883281419`) and `.out` (`6:  -33.66 cm**-1  ***imaginary mode***`), and ORCA
itself emits the `***imaginary mode***` marker. So a cutoff is not needed to drop trans/rot
modes (they are literally 0); ORCA's own marker is the measured signal for a real imaginary.
Note: cclib 1.8.1 cannot report any of this — it crashes before reaching `vibfreqs` (see
parse-sources.md §1).

## What to look for in an output (search presets)
The output-search feature (`output_search.rs`, Phase 2.7) ships one-click chips for the things a
chemist actually greps for. It's a **learning instrument**: the presets teach *what* to look for
and *what each banner means*. All wording below is **verified against real ORCA 6.1 output** on the
dev machine unless noted.

| Preset | Query (matcher) | What the banner means |
|---|---|---|
| **Warnings** | `WARNING` (literal, ci) | Advisory messages — often the reason a result is subtly off. Confirmed: the `WARNINGS` header and `WARNING:` lines. |
| **Errors** | `ERROR\|error termination\|aborting\|ABORTING` (regex, **case-SENSITIVE**) | Fatal problems / aborts. Case-sensitive on purpose: a case-insensitive `error` matches the benign `Last DIIS Error` / `Startup error` on **every** SCF (12+ hits in a *successful* run) — noise that buries real aborts. The case-sensitive query fires 0× across 12 successful outputs. |
| **SCF not converged** | `SCF NOT CONVERGED` (literal, ci) | The SCF failed → any energy after it is meaningless. (Positive case not reproducible locally — all sampled runs converged — but it's the standard failure banner, analogous to the `SCF CONVERGED AFTER N CYCLES` success line.) |
| **Imaginary modes** | `imaginary mode` (literal, ci) | A negative vibrational frequency = a saddle point, not a minimum. Matches ORCA's real marker, e.g. `6:  -33.66 cm**-1  ***imaginary mode***`. **Not** bare `imaginary`, which hits `imaginary perturbations` (a CPHF count present in every Freq run, even at a true minimum). |
| **Final energies** | `FINAL SINGLE POINT ENERGY` (literal) | One per optimization cycle; the last is the converged value. For an Opt run the count ≈ cycles + 1 (the post-convergence recompute). |
| **Geometry convergence** | `Geometry convergence` (literal) | The per-cycle convergence tables (`\|Geometry convergence\|`); count = optimization cycles. |
| **Timings** | `TOTAL RUN TIME\|Sum of individual times` (regex) | Where the wall time went. |
| **Basis set info** | `Basis Dimension\|Number of basis functions` (regex) | How large the calculation actually was. |

`ci` = case-insensitive (the default; the UI `Aa` toggle is off unless a preset turns it on).

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
