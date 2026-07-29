# Module: Result parsing

**Status:** two Rust extractors built — minimal result extraction and an incremental streaming
convergence parser. The authoritative sidecar/cclib tier is not started.

## Responsibilities & boundaries

Turn ORCA output into structured data. Two tiers, by design:

1. **Streaming tier (Rust, during a run)** — lightweight, tolerant regexes over incoming log
   lines for live UI only: SCF iteration energies, per-cycle geometry energy, gradient norms vs
   convergence criteria. Tolerant to partial lines; **never authoritative**.
2. **Authoritative tier (sidecar, after a run)** — cclib full parse of the final output → SQLite
   `results`. Everything the Results screen shows comes from here. **Not built** (Phase 3).

Formats and the ORCA-6 output gotchas these parsers rely on are documented in
`wiki/orca/output-files.md`.

## Streaming convergence parser (`src-tauri/src/convergence.rs`)

Tier 1, feeding the live convergence dashboard. A `ConvergenceParser` is fed the **same** stdout
stream `local_backend::drive_job` already tails, one line at a time, so `output.out` is never
re-read while running (domain rule #5). `feed(line)` returns `Option<ConvergenceEvent>`
(`Scf(ScfPoint)` | `Opt(OptPoint)`, internally tagged on `kind` for the frontend). `drive_job`
batches events on the same cadence as logs and emits `job:convergence`; the `read_job_convergence`
command replays a finished/running job's `output.out` (via `BufReader`, line by line) through a
fresh parser for backfill.

What it parses:

- **SCF iterations** — tolerant row parse (int iter, negative-decimal energy, ≥3 numeric fields),
  **gated to inside an `Iteration … Energy (Eh)` table**. The gate is essential: Freq normal-mode
  eigenvector rows have the *identical* shape and would otherwise leak in as bogus SCF points
  (verified against a real Opt+Freq run — the ignored `real_full_outputs_parse_sanely` test
  asserts no near-zero "SCF" energy leaks). `cycle` = current opt cycle (0 for a single point),
  read from `GEOMETRY OPTIMIZATION CYCLE N` markers.
- **Geometry convergence** — a state machine over the `|Geometry convergence|` block; each
  `name value tolerance YES|NO` row → a `Criterion`; the closing dashed rule (after ≥1 criterion)
  emits an `OptPoint { cycle, energy, criteria }`. Criterion count is **not** hardcoded (cycle 1
  has 4, later cycles 5, OptTS more).
- **Per-cycle energy** — reuses `result_extraction::extract_final_energy` on the
  `FINAL SINGLE POINT ENERGY` line (one line at a time), attached to the cycle's `OptPoint`.

**Not in SQLite** — convergence data is derived on demand from `output.out` (cheap to re-parse,
avoids schema churn and a write path during the hot streaming loop). Fixture:
`src-tauri/tests/fixtures/opt_output_excerpt.txt` (two real C₂H₆ cycles). The frontend mirror +
dashboard live in `src/convergence/` (see `modules/frontend.md`).

## Result extraction (`src-tauri/src/result_extraction.rs`)

Minimal Rust extraction, run once when a job completes (in `local_backend::drive_job`, **before**
the terminal `job:status` event) over a 64 KB tail of `output.out` (`RESULT_TAIL_BYTES`).
Persisted via `set_job_results_conn` into `jobs.energy` / `jobs.wall_time`.

- **`extract_final_energy(tail) -> Option<f64>`** — regex `FINAL SINGLE POINT ENERGY\s+(-?[\d.]+)`,
  takes the **last** match (optimizations reprint it each cycle; the last is the converged value).
  Hartree.
- **`extract_wall_time(tail) -> Option<f64>`** — regex over
  `TOTAL RUN TIME: <d> days <h> hours <m> minutes <s> seconds <ms> msec`, returns total seconds.

Both regexes compiled once via `std::sync::LazyLock`. Unit-tested with fixture strings; also
asserted against genuine ORCA output in the ignored `real_orca_water_single_point_completes` e2e
(energy ≈ −76.419 Eh).

**Why a 64 KB tail, not the 5 KB completion tail:** a Freq / Opt+Freq run prints the final energy
well before EOF (normal modes + thermochemistry follow), so a small tail can miss it.

**Deliberately minimal** — only the two numbers the job list needs today. The rich, authoritative
parse (geometries, frequencies, spectra) is the sidecar/cclib tier, not yet built.

## Notes

- Keep a fixtures library of real outputs (per ORCA version, per job type) in
  `sidecar/tests/fixtures/` — regression safety when ORCA formatting shifts between versions.
- cclib gaps (if any ORCA 6 block is unsupported) get targeted supplemental parsers in Python,
  documented here as they appear.
