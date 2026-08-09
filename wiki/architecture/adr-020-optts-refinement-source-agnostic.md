# ADR-020 — OptTS refinement is a source-agnostic engine

**Status:** accepted (Phase 4.5 Stage E1a, 2026-08-09).
**Context builds on:** ADR-007 (reaction modeling; ΔE‡ vs ΔG‡), ADR-016 (emit ownership; Fork 2 =
reuse, no golden pair), ADR-018 (comparability — a barrier is only meaningful on one method+solvation).

## Context

A relaxed scan gives an **approximate** TS (its maximum — a ΔE‡ estimate on the relaxed surface,
ADR-007). Turning that estimate into a **located** transition state (exactly one imaginary mode) is a
separate ORCA run: `! OptTS Freq` seeded from a TS *guess* geometry plus a start Hessian
(`wiki/orca/optts.md`, measured on the real Menshutkin SN2).

The question this ADR settles: **what owns "refine a TS guess into a located TS"?** A scan is only one
way to produce the guess. The two others are already on the roadmap:
- a **NEB / NEB-CI / NEB-TS climbing image** (Stage E3) — the standard route when there is **no clean
  1-D scan coordinate** (a concerted reaction, e.g. BH₄⁻ hydride transfer: a C–H bond forms while B–H
  and O–B break — its scan maximum is *not* the barrier; NEB finds the saddle);
- a **hand-authored guess** later.

Anton's forward requirement (2026-08-09): **NEB→OptTS is the more common TS workflow** than
scan→OptTS, so the engine must not bake in "scan."

## Decision

**OptTS refinement is a single source-agnostic engine**, consumed by per-source entry points:

- **Engine (generic):**
  - `buildOptTSInput(sourceInput, seedGeometry, options?)` — pure, `src/scene/optts.ts`. Takes a
    generic **context input** (`sourceInput`: any job's `!` line + `* xyz`, for method/solvation/charge)
    and a generic **seed** (`seedGeometry: { elements, xyz_angstrom }` — a `TsGuessGeometry`, deliberately
    NOT `ScanGeometry`). REUSES `buildOrcaInput` (Fork 2 of ADR-016 — no atom-index-bearing directive is
    emitted, so no `orcastudio-core` golden pair). Emits `! <method> <solvation> OptTS Freq TightSCF` +
    `%geom Calc_Hess true end`.
  - `create_optts_job(source_job_id, title, input_content)` — Rust, `commands::jobs`. `source_job_id`
    is **any job**. Creates a normal `draft` child (caller submits it); if the source is on a pathway,
    the TS **joins that same pathway** via the shared `attach_job_to_pathway_conn`. **No lineage
    column** — the TS↔source relation is the shared pathway + the **`! OptTS` role** derived from the
    child's own input.
- **Entry points (per source):** each extracts its own guess and calls the one engine. Today: the
  **scan panel** (`ScanProfilePanel`) — enabled on the approx-TS maximum, seeds from that point's
  geometry. Tomorrow (E3): the **NEB band viewer** — seeds from the climbing image. Same two calls.

## Two load-bearing invariants (rule #9)

1. **Charge footgun.** `(charge, mult)` come ONLY from `sourceInput`'s `* xyz <c> <m>` (via
   `sceneFromOrcaInput`, not a regex, not a default) and are **asserted back out** of the emitted
   input. A wrong-charge OptTS terminates normally and is garbage (the exact class that made CREST QCG
   useless on the anion). Same discipline as `reopt.ts`.
2. **Comparability.** Method + solvation **default to the source's**, extracted verbatim via
   `methodSolvationKeywords` (the same `!`-line reader the comparability guard uses — one definition of
   "what is method," so re-emit and compare cannot drift). The source's opt keyword (the scan's
   `LooseOpt`) and its `Scan`/`Constraints` `%geom` block must NOT leak — `buildOrcaInput` builds fresh.

## Rejected alternative

**A scan-specific OptTS builder, re-written for NEB in E3.** Rejected: it would duplicate the
charge-safe emit, the pathway attach, and (later) the located-TS parsing across two code paths that
must stay identical — guaranteed drift (the ADR-018 comparability guard and the charge assertion would
have to be re-implemented and kept in sync). One generic engine + thin per-source entry points keeps
the invariants in one place.

## Consequences

- E3's NEB entry point calls `buildOptTSInput` / `create_optts_job` **unchanged** — it only supplies a
  different seed + context. This ADR is the reason those signatures carry no "scan."
- **E1b (next):** consume the located TS — ΔG‡ from OptTS+Freq+thermochemistry, and retire the
  "approximate TS" label on a pathway once a real TS exists. Deliberately not in E1a.
- **Named limit (`wiki/orca/optts.md`, Pattern-2):** method/solvation inheritance reads the **inline
  `SMD(<solvent>)`** keyword form (what the app emits). A `%cpcm … smd true … end` **block**-form source
  would re-emit `SMD` without the solvent — no current source writes that; when a NEB/hand source can,
  the engine must be extended (or the entry point passes `options.solvation`).
