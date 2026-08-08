# Conformer re-optimization fan-out (Stage D, DFT re-opt)

The scientific-rigor layer over a GOAT conformer search (ADR-007): xTB ensemble energies
are coarse, so the lowest-energy conformers are re-optimized at DFT before any reaction
center is built on them. This page describes the **create side** (unit D2a — landed
2026-08-08); the aggregate read + DFT re-rank/re-weight display is D2b (next). See
`wiki/log.md` for history.

## Data model — two FKs, no table (Fork 1)

A re-opt child is a normal `jobs` row tagged back to where it came from, via two nullable
columns added in **migration v15**:

- `jobs.source_ensemble_job_id TEXT` — the GOAT ensemble job it was fanned out from;
- `jobs.source_conformer_index INTEGER` — which conformer of that ensemble (0-based).

There is **no `reopt_batch` table**: the conformer set of one fan-out is *derived* by
`GROUP BY source_ensemble_job_id` in D2b, never stored (one source of truth). Both columns
are NULL on every non-child job. **Jobs-survive** (as v13/v14): deleting the source GOAT job
nulls these links in the commands, it never cascades to delete the children.

## The child-input builder — `src/scene/reopt.ts`

`buildReoptInput(sourceInputText, conformer, opts)` emits one child's ORCA input, PURE and
TS-only. It REUSES the existing scene→input machinery — `sceneFromOrcaInput` (extract the
source's charge/multiplicity — **the existing parser, never a new charge regex**),
`sceneFromAtomLines` (the conformer's own atoms, order preserved), and `buildOrcaInput` (the
same `!`-line / SMD / `* xyz` emit as the New Job form). Because it emits a *proposal* input
by reuse — not an order-bearing scan golden pair — there is deliberately **no
`orcastudio-core` Rust mirror** (unlike `emit_scan_block`); the reason is stated in the file
header so a future reader doesn't "fix" it by adding one.

- `opts.method` — DFT method keyword, default `r2SCAN-3c` (`DEFAULT_REOPT_METHOD`).
- `opts.freq` — emit `Freq` (the defensible ΔG path), default `true`. `false` → `Opt` only.
- `opts.solvation` — `{ model: "smd", solvent }` → the keyword-line `SMD(<solvent>)` emit,
  **verified real SMD in ORCA 6.1.0** (rule #10 determiner run, `wiki/orca/solvation.md`).

### The charge footgun (rule #9) — the load-bearing invariant

Charge and multiplicity live ONLY in the input text's `* xyz <c> <m>` line — never a jobs
column, never a builder default. Each child MUST inherit (c, m) from the SOURCE GOAT job; a
BH₄⁻ child (`* xyz -1 1`) that silently became `* xyz 0 1` would terminate normally and be
garbage (the exact class that made CREST QCG useless on the anion). The builder therefore:

1. throws if the source has no inline `* xyz` block (no silent fall-back to charge 0);
2. propagates the extracted (c, m) into the emitted child;
3. **post-condition (rule #9, in our terms):** re-parses the EMITTED child and asserts its
   (c, m) equals the source's AND its atoms are exactly the conformer's — same count and
   element order. Any mismatch throws, so a wrong-charge or wrong-geometry child is never
   handed to the create boundary.

## The trigger UI + wiring — `JobDetailScreen`

The ensemble panel carries a **"Re-optimize top-k at DFT"** form: `k` (default 4, with the
D1 cumulative-% at k shown beside it so the user SEES how much xTB population those k cover —
Fork 3), method (default `r2SCAN-3c`), a **mode toggle** (Opt+Freq default = the ΔG path /
Opt-only = an explicit quick screen), an SMD toggle + solvent, and an honest "creates k jobs"
note. **Mode lives in the child input** (`! … Opt` vs `! … Opt Freq`); D2b auto-detects it
from the input rather than a stored flag.

On trigger (`reoptTopK`): the k lowest-energy conformers' inputs are **built + charge-checked
FIRST**; if any `buildReoptInput` throws, the whole fan-out aborts having created nothing (the
create boundary refuses). Only once all k are proven does it create each via the
`create_reopt_job` command (status `draft`, tagged with the two FKs) and `submit_job` it into
the sequential queue (concurrency 1). All k share one mode — no mixing within a fan-out.

`create_reopt_job` (Rust, `commands/jobs.rs`) reuses `create_job_conn` (so the child mints its
OWN `index_map` from its OWN input) then stamps the two linkage FKs; its create-boundary
post-condition is **referential integrity** — the source ensemble job must still exist, else a
clean `NotFound` and no child is created. No scene snapshot: the child is a normal DFT job whose
input fully defines it (ADR-008 #5).

## Status

Create side complete (D2a): migration v15, the pure builder + charge post-condition, the
trigger UI, and charge-safe queued job creation. **Not yet built (D2b):** reading the children's
DFT energies back, re-ranking/re-weighting vs the xTB populations, and any comparison UI. Until
then nothing reads `source_ensemble_job_id` — it is write-only.
