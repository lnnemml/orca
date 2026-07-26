# Module: Result parsing

**Status:** Rust minimal extraction built (Phase 1.4). Sidecar/cclib tier not started.

## As built (Phase 1.4) — `src-tauri/src/result_extraction.rs`
Minimal Rust extraction, run once when a job completes (in `local_backend::drive_job`, before
the terminal `job:status` event) over a 64 KB tail of `output.out` (`RESULT_TAIL_BYTES`).
Persisted via `set_job_results_conn` into `jobs.energy` / `jobs.wall_time`.

- **`extract_final_energy(tail) -> Option<f64>`** — regex
  `FINAL SINGLE POINT ENERGY\s+(-?[\d.]+)`, takes the **last** match (optimizations reprint it
  each cycle; the last is the converged value). Hartree.
- **`extract_wall_time(tail) -> Option<f64>`** — regex over
  `TOTAL RUN TIME: <d> days <h> hours <m> minutes <s> seconds <ms> msec`, returns total seconds.

Compiled once via `std::sync::LazyLock`. Unit-tested with fixture strings; also asserted against
genuine ORCA output in the ignored `real_orca_water_single_point_completes` e2e (energy
≈ -76.419 Eh).

**Why 64 KB tail, not the 5 KB completion tail:** a Freq/Opt+Freq run prints the final energy
well before EOF (normal modes + thermochemistry follow), so a small tail can miss it.

**Deliberately minimal:** only the two numbers the job list needs today. The rich, authoritative
parse (geometries, frequencies, spectra) is the sidecar/cclib tier below — not yet built.

## Two-tier design
1. **Streaming tier (Rust, during run):** lightweight regexes over incoming log lines for
   live UI only — SCF iteration energies, geometry cycle energy, gradient norms vs
   convergence criteria. Tolerant to partial lines; never authoritative.
2. **Authoritative tier (sidecar, after run):** cclib full parse of the final output →
   SQLite `results`. Everything the Results screen shows comes from here.

## Notes
- Keep a fixtures library of real outputs (per ORCA version, per job type) in
  `sidecar/tests/fixtures/` — regression safety when ORCA formatting shifts between versions.
- cclib gaps (if any ORCA 6 block is unsupported) get targeted supplemental parsers in
  Python, documented here as they appear.
