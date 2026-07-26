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
