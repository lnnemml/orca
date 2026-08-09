# ADR-018: Reaction energy reference model

**Status:** accepted · 2026-08-07
**Relates:** [ADR-007](adr-007-reaction-modeling.md) (reaction as a first-class object; this ADR
supplies the energy-reference half of the comparative view), [ADR-004](adr-004-sqlite-storage.md) (the
SQLite normalization this mirrors)
**Stages:** ROADMAP Phase 4.5 Stage C2b. This ADR is the **decision**; the `reaction_reference_jobs`
table (migration v14) and the ΔΔE‡/overlay UI land in C2b. **Methodology:**
[`chemistry/reaction-barriers.md`](../chemistry/reaction-barriers.md) (the three barriers this ADR
serves, and which reference is for what).

## Context

A relaxed scan yields a curve `E(coordinate)`; C2b compares pathways off it. Three different barriers
come off one curve (full derivation in `chemistry/reaction-barriers.md`):

1. **ΔΔE‡ (si/re selectivity)** = `E(max_si) − E(max_re)`. The reactant reference is shared between
   the two faces of the same substrate+reagent, so it **cancels** — the number is
   **reference-independent**. This is the mission deliverable (a screening estimate from scan maxima,
   not localized saddles, not ΔG‡).
2. **Intrinsic barrier** = `E(max) − E(scan minimum)`. A scan started far enough (Nu···C ≈ 2.5 Å+)
   captures the **pre-reaction complex (RC)** as its own minimum, so this comes off the scan for free.
3. **Barrier vs separated reactants** = `E(max) − [E(substrate) + E(reagent)]`, each reactant
   optimized **separately, in its own job** (BH₄⁻ with its own −1 charge). This is the reference
   comparable to **solution kinetics** (reactants are separated before they collide) — the one needed
   to line up against Arrhenius/experiment.

Only barrier 3 needs a **reactant reference** the app must model. Barrier 1 needs none; barrier 2
usually reads off the scan minimum. The question this ADR settles: **how does a reaction store its
reactant reference for absolute barriers?**

## Decision

A reaction's **reactant reference** is a **list of references to optimized-reactant jobs whose final
energies SUM** to `E(ref)` — and it is **optional**.

- **One** reference job → the reference is a pre-reaction complex (intrinsic barrier, variant 2),
  though the scan minimum usually already gives that for free.
- **Two+** reference jobs → separated reactants, `E(ref) = Σ E(job)` (variant 3, the
  experimental-comparison reference; e.g. substrate + BH₄⁻ optimized separately).
- The **semantics** — what these jobs represent — is the user's labelled choice; the app **sums and
  labels**, it does not infer.

### Why a summed list, not a single FK and not a cached scalar

- **A single `reference_job_id`** cannot express separated reactants — substrate + reagent are **two**
  jobs. The common, mission-critical case (BH₄⁻ + ketone) needs ≥2.
- **A cached scalar** (`reactions.reference_energy_eh`) loses provenance and can **silently drift**
  from the jobs it was computed from — the exact two-sources-of-truth trap this project keeps
  refusing (same reason `jobs` carries `pathway_id` only and derives the reaction by join, ADR-007
  amendment). A list of **job references** keeps **one source of truth**: each energy is read from its
  job's parsed result at read time, never copied.

### Representation (confirmed at C2b build)

A lean join table, mirroring the `reactions`/`pathways` normalization:

```
reaction_reference_jobs
  reaction_id TEXT REFERENCES reactions(id),
  job_id      TEXT REFERENCES jobs(id)
  -- (+ an ordering/label column if the build needs it; kept minimal)
```

A reaction has **0+** reference jobs. Migration **v14** when C2b's data touch lands. Deletion follows
the same **jobs-survive** rule as C1 (ADR-007 amendment): a `reaction_reference_jobs` row is
**grouping metadata** — deleting a reaction removes its reference rows, and neither a reaction-delete
nor a reference-remove ever deletes the underlying job; the job stays standalone in the Jobs list.

**Rejected alternative:** a JSON job-id array on `reactions`. Rejected for the same normalization
reason `jobs.pathway_id` is a column and not a JSON blob — a join table is queryable, referentially
checkable in the commands, and does not bundle a mutable list into one cell.

### ΔΔE‡ is reference-free — the mission deliverable is independent of this machinery

The si/re number needs **no** reference (it cancels); the reference is only for **absolute** barriers.
So **C2b ships ΔΔE‡ + intrinsic barriers (both from the scans) with the reference optional**:

- **No reference set** → show ΔΔE‡ + intrinsic barriers + a note: *"absolute (vs separated reactants)
  barriers need a reactant reference."*
- **Reference set** → additionally show the absolute barrier `E(max) − Σ E(ref job)`, labelled with
  what the reference represents.

This keeps the stereoselectivity screen (the Phase 4.5 "done-when") working before any reference is
configured.

### Comparability guard (belongs with this ADR)

Barriers and ΔΔE‡ are shown **as numbers** only when the compared pathways **and** the reference jobs
share **method / basis / dispersion / solvation**. A mismatch shows the **curves** but **refuses the
number**, naming the reason. (You cannot subtract energies computed on different scales — see
`chemistry/reaction-barriers.md` "Спільність методу".) For the ionic BH₄⁻ case, the reference and
pathway solvation should be **SMD, not ALPB** — surfaced here, enforced at C2b / Stage F.

## Consequences

- C2b gains a small reference-management surface (add/remove reactant jobs to a reaction) reusing the
  C2a jobs-survive pattern; the ΔΔE‡ + intrinsic-barrier view does not depend on it.
- The absolute barrier is always recomputed from live job energies — no stored scalar to invalidate.
- Full experimental comparison (ΔG‡ via OptTS + Freq + thermochemistry, association entropy, standard
  state) is **Stage E+**; this ADR lays only the reference infrastructure, deliberately.

### Amendment (2026-08-09, Stage E1b) — the barrier now spans **E and G**; a located TS supersedes the scan max

This ADR's machinery generalizes from `E` to `{E, G}` without a schema change. The reference-energy
seam gains a **ΣG(ref)** alongside Σ E(ref), on the **identical honest-or-absent discipline**: `ΣG` is
non-null only when the list is non-empty AND **every** reference job ran Freq (a partial ΣG is `None`,
never summed — a wrong ΔG‡ denominator is as poisonous as a wrong E(ref)). Where a pathway has a
**located TS** (an OptTS child parsed with Freq → G, Stage E1a / [ADR-020](adr-020-optts-refinement-source-agnostic.md)),
the compare view shows a **real ΔG‡ = G(TS) − ΣG(ref)** and a located ΔE‡ = E(TS) − ΣE(ref), and
**retires the "approximate TS" label** for that pathway; the scan-max ΔE‡ remains the screening
fallback where no TS exists. **Reference-free ΔΔG‡** = G(TS_A) − G(TS_B) is the free-energy sibling of
the reference-free ΔΔE‡ (shared reactants cancel; the 1 atm→1 M standard-state correction also cancels
for equal molecularity). Standard state is **named, not auto-applied** (Fork A — auto-applying embeds a
molecularity/reference-definition assumption). Same comparability guard governs the ΔG‡ barriers.
Pure logic + honest-or-absent tests: `reactions/compare.ts`; the ΣG sum + its partial-is-None test:
`commands/reactions.rs`. Story: `chemistry/reaction-barriers.md` §"Барʼєр 4".
