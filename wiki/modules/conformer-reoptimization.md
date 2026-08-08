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

## The read/aggregate side — `read_conformer_reoptimization` + `reopt-aggregate.ts` (D2b)

The comparison view over the SAME GOAT job's detail. The set is DERIVED, never stored:
`read_conformer_reoptimization(source_job_id)` (Rust, `commands/jobs.rs`) is
`SELECT … FROM jobs WHERE source_ensemble_job_id = ?1 ORDER BY source_conformer_index`
LEFT JOIN `results` — so a child that hasn't parsed yet still appears (with `None` energies,
never dropped). Per child it returns raw facts only: status, DFT electronic energy
(`results.final_energy_eh`), Gibbs G (`results.free_energy_g_eh`, `None` unless Freq ran),
`imaginary_count`, whether the input requested Freq, and an `element_mismatch` flag. **No
weighting in Rust** — one Boltzmann implementation lives in `ensemble.ts`.

**Mode auto-detect (derived, not stored).** A child "intended ΔG" iff its input requested
`Freq` (`input_requested_freq`, scanning `!` lines). The set is ΔG-mode iff *every* child
requested Freq, ΔE-mode iff none did, and **`mode_inconsistent`** iff mixed (D2a shouldn't
produce a mixed set; the TS side then refuses to pick a single mode and weights on electronic
E with a warning). **Element-list post-condition (rule #9):** each child's `* xyz` composition
is compared to the source ensemble job's; a mismatch is flagged, never silently ranked across.

`aggregateReopt(raw, ensemble)` (TS, `src/scene/reopt-aggregate.ts`) reuses `boltzmannWeights`
/ `deltaEKcal` and applies **honest-or-absent**: a child that is not a terminal success, has a
composition mismatch, optimized to a saddle (`imaginary_count > 0` — a saddle is NOT a
minimum), or lacks the mode's usable energy is EXCLUDED from the weighting and returned in
`excluded` with a reason — never a fabricated weight. xTB and DFT populations are computed over
the SAME included subset (same conformers, same normalization; only the energy LEVEL differs),
so the columns are comparable and are never combined by arithmetic. Ranks are within that
subset; a conformer whose xTB rank ≠ DFT rank is flagged (`rankChanged`) — the teaching moment.

**The comparison UI** sits on the GOAT `JobDetail`, below the D1 populations panel and the D2a
trigger, recomputed on open (read-time). It shows per conformer `xTB #/ΔE/pop | DFT #/ΔG-or-ΔE/pop`,
the DFT column **labelled by the detected mode** (never unlabelled, never `ΔG` on a set where a
contributing child lacks G), highlights reordered rows, lists excluded children with reasons,
carries a **"n of k complete — provisional, not for decisions"** banner until every child is
terminal, and a **"not frequency-validated as minima"** caveat in ΔE-mode. Nothing is stored.

## Status

**Create side (D2a) + read/aggregate side (D2b) complete.** The full loop — fan out k DFT
re-opts, then read them back re-ranked/re-weighted vs the xTB populations — is in. **Not built
(D3):** "use the best conformer" wiring (feed the DFT winner into a reaction center) and the
C2b-2b reference convergence. D2b is read-only: it never creates a job, never migrates, never
caches a weight or rank.
