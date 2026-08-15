# ORCA geometry-optimization convergence verdict (rule #10, measured)

Two exact markers ORCA prints in the **output tail** decide whether a geometry optimization
converged. Both measured from real runs on the dev machine — the manual is not the source.

## The two markers

| Verdict | Exact marker (substring) | Measured in |
|---|---|---|
| **Converged** | `*** OPTIMIZATION RUN DONE ***` | `dexketoprofen_output_tail.out:1061` (a converged r2SCAN-3c Opt) |
| **Not converged** | `The optimization did not converge but reached the maximum` | the Diels-Alder OptTS job `e710c8b8…`, `output.out:44735` |
| **Not applicable** | *(neither present)* | a single point / relaxed scan / GOAT — no optimization to judge |

- Both live in the **tail** (rule #5: the completion detector already tails `output.out`; the
  verdict adds no unbounded read). The not-converged marker sits ~115 lines before
  `****ORCA TERMINATED NORMALLY****` (the failed optimization is the last thing that runs), so a
  64 KB tail catches it with wide margin.
- The not-converged sentence is **wrapped** by ORCA (`… reached the maximum \n number of
  optimization cycles.`), so we match only the stable **leading prefix** — verified byte-for-byte
  against the real line (which even has a trailing space after `maximum `, correctly ignored).
- **Exact-substring, tail-only** — never fuzzy. A single point prints `SCF CONVERGED` (a *different*
  convergence), which must NOT be read as an optimization verdict; keying on the exact optimization
  markers is what keeps an SP/scan `NotApplicable` and never flagged.

## Why this is a first-class verdict: TERMINATED NORMALLY ≠ converged

The measured trap (the whole reason for this unit): the Diels-Alder OptTS **exhausted its 50-cycle
budget without converging**, yet ORCA still printed `****ORCA TERMINATED NORMALLY****` and **exited
0**. So the completion detector (domain rule #6 — marker + exit code) correctly marked the job
`Completed`, and it read as a clean success. The optimization verdict is the **quality flag** the
completion status alone cannot carry.

## The `.hess` mismatch is a CONSEQUENCE, not a separate bug

The DA OptTS was `! r2SCAN-3c OptTS Freq TightSCF` with `%geom Calc_Hess true end` — ORCA computes
the Freq Hessian **at the seed** at the START. When the optimization then runs 50 cycles without
converging, the **final** geometry is many steps from the seed, so the `.hess`-vs-final-geometry
post-condition (`artifact-readers.md`, the distance-based rule #9 check) legitimately fires a
geometry mismatch. That mismatch is the **expected downstream consequence of non-convergence**, not
an independent parser defect — so when the verdict is `NotConverged`, the parse flow **skips `.hess`**
(frequencies off a non-stationary point are meaningless anyway) and reports "did not converge"
instead of the misleading `.hess: geometry mismatch`.

## How the app uses it

`optimization_verdict(output_tail) -> OptVerdict {Converged, NotConverged, NotApplicable}`
(`src-tauri/src/convergence.rs`) is read in `results::parse_and_store` **before** the `.hess` branch,
stored as `ParsedResults.converged: Option<bool>` (`Some(true)` / `Some(false)` / `None`). Only
`Some(false)` drives a UI state — a "did not converge (max cycles)" banner on the results card; a
converged opt (`Some(true)`) and a non-optimization job (`None`) render as a clean completed job.
See `modules/artifact-readers.md` (the parse-flow ordering) and `wiki/orca/optts.md`.

**Not fixed here (named, separate concern):** *why* the r2SCAN-3c OptTS did not find the TS from the
XTB Diels-Alder seed is a **chemistry** question (seed quality / the TS search), not a status-honesty
one. This unit makes the tool tell the truth about the run; it does not change the OptTS recipe.
