# Module: Result parsing

**Status:** both tiers are built. The **streaming tier** (minimal result extraction + the
incremental convergence parser) is described in full on this page. The **authoritative tier is
COMPLETE** — all four Rust artifact readers (`.property.txt`, `.hess`, `_trj.xyz`/`.xyz`,
`orca_2json`) are built, tested, and wired end-to-end into the job pipeline via `results.rs` (job →
`parsed`). That tier has its own page — **[artifact-readers.md](artifact-readers.md)** — and is not
re-described here beyond the boundary below. Per
[ADR-012](../architecture/adr-012-output-parsing-ownership.md) it is **Rust over ORCA's structured
artifacts**, not a sidecar/cclib parse (cclib crashes on ORCA 6.1.0 output — see
[parse-sources.md](../orca/parse-sources.md)).

## Responsibilities & boundaries

Turn ORCA output into structured data. Two tiers, by design:

1. **Streaming tier (Rust, during a run)** — lightweight, tolerant regexes over incoming log
   lines for live UI only: SCF iteration energies, per-cycle geometry energy, gradient norms vs
   convergence criteria. Tolerant to partial lines; **never authoritative**.
2. **Authoritative tier (Rust, over structured artifacts — ADR-012) — COMPLETE.** After a run, own
   parsers read `.property.txt` (energies, geometry, charges, dipole, thermochemistry), `.hess`
   (signed frequencies, normal modes, IR), `_trj.xyz`/`.xyz` (trajectory, final geometry), and
   `orca_2json` over `.gbw` (MO energies/occupations) → SQLite `results`. **All four built, tested,
   wired** — the detail lives in **[artifact-readers.md](artifact-readers.md)**. Everything the
   Results screen shows comes from here. `output.out` is **not** an authoritative source (rule #5 —
   the multi-MB *log* is never parsed whole; the small structured artifacts are read whole and
   capped). cclib is **not** used. Part A of the parse-sources probe verified every artifact's atom
   order equals the input order, so each artifact's position→`AtomId` map is the identity today; the
   readers enforce that with per-block/per-frame order + Bohr→Å post-conditions.

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

**Deliberately minimal** — only the two numbers the job list needs *during/right after* a run. The
rich, authoritative parse (geometries, frequencies, spectra, MOs) is the **Rust artifact-reader
tier** (ADR-012, [artifact-readers.md](artifact-readers.md)), which runs on completion and is what
the header energy is re-sourced from (`jobs.energy` is overwritten from `results` — a 64 KB tail can
miss the final energy on a large molecule; see [debugging/007](../debugging/007-phase1-decisions-phase3-outgrew.md)).

## Notes

- Keep a fixtures library of real artifacts (per ORCA version, per job type) — `.property.txt`,
  `.hess`, `_trj.xyz`, `orca_2json` output — as regression safety when ORCA formatting shifts
  between versions. Format stability across ORCA versions is **unmeasured** (ADR-012 caveat);
  `sidecar/probes/parse_sources.py` re-runs on every ORCA upgrade to catch a format change
  before the parsers silently misread it.
