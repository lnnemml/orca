# ADR-007 — From molecular modeling to reaction modeling

**Status:** accepted; **amended 2026-08-07** (Phase 4.5 Stage C1 — ratified normalized schema; see
the Amendment section below)  
**Date:** 2026-07-27  
**Supersedes:** —  
**Context:** author's research experience (stereoselectivity proof for NaBH₄ reduction,
bachelorʼs thesis) exposed a gap no existing tool fills.

---

## Decision

OrcaStudio's mission expands from "GUI for ORCA" to **reaction mechanism workstation**:
a tool where a computational chemist constructs and investigates reaction mechanisms
with precise geometric control, automated pathway comparison, and (later) AI assistance.

The core shift: **Reaction** (not Job) becomes the central intellectual object.
Job remains the execution unit — a single ORCA run — but it serves the Reaction.

## Motivation

### The problem no tool solves today

To computationally prove stereoselectivity of a reaction (e.g. NaBH₄ reduction of a
ketone), a researcher needs to:

1. Build the substrate with correct stereochemistry.
2. Place the reagent (BH₄⁻) at a specific distance, attack angle (Bürgi-Dunitz ~107°),
   and dihedral angle (selecting the *si* or *re* face).
3. Pre-optimize with constraints (xTB) to get a physically reasonable TS guess.
4. Run TS search (OptTS / NEB-TS) via ORCA.
5. Repeat steps 2–4 for the competing pathway (opposite face).
6. Compare activation energies: ΔG‡(si) vs ΔG‡(re) → stereoselectivity proof.

No existing tool (Avogadro, Chimera, GaussView, IQmol) supports this workflow end-to-end.
Each step requires switching software, manual file editing, and mental bookkeeping.
The researcher knows the scientific question but cannot express it as a computational
experiment within a single environment.

### Scaling to complex reactions

The same pattern applies to catalytic cycles (Sonogashira, Suzuki, Buchwald-Hartwig, etc.)
where each elementary step (oxidative addition → transmetalation → reductive elimination)
has its own reaction center, TS, and energy barrier. The number of pathways to compare
grows combinatorially with stereochemistry, conformers, and ligand variations.

### The principle

Good scientific software lets the researcher say: *"I want to test exactly this
hypothesis"* — and the system lets them construct the experiment precisely, rather than
forcing them to reshape their scientific question to fit the GUI's limitations.

## Domain objects (first-class entities)

| Object | What it represents |
|---|---|
| **Molecule** | A 3D structure: xyz + metadata (name, formula, charge, multiplicity) |
| **Fragment** | A reusable molecular piece (BH₄⁻, PdL₂, H₂O, common ligands) |
| **Reaction** | A transformation: substrate(s) + reagent(s) → product(s) |
| **ReactionCenter** | The geometric relationship defining the reactive encounter: nucleophile atom, electrophile atom, distance, attack angle, dihedral |
| **Constraint** | A geometric parameter held fixed during optimization (distance, angle, dihedral → ORCA `%geom Constraints`) |
| **ReactionCoordinate** | The variable swept along: e.g. nucleophile–electrophile distance from 3.0 → 1.5 Å |
| **Pathway** | One approach geometry explored end-to-end: geometry sweep → jobs → energy profile → optional TS |
| **Job** | A single ORCA (or xTB) calculation — the execution primitive |

A Reaction contains one or more Pathways. A Pathway generates one Job
(native ORCA scan), or a small set for NEB/OptTS workflows. Jobs remain standalone-capable:
a user can still create and run a Job without any Reaction context.

## Data model impact

New tables (introduced in Phase 4.5, not before):

```
reactions
  id, project_id, name, notes, created_at

reaction_molecules
  id, reaction_id, molecule_id, role ('substrate' | 'reagent' | 'product')

reaction_centers
  id, reaction_id,
  nucleophile_atom_serial, electrophile_atom_serial,
  distance, angle, dihedral,
  fragment_id (nullable — for placed reagent fragments)

pathways
  id, reaction_id, name,
  coordinate_type ('distance' | 'angle' | 'dihedral'),
  coordinate_param (which atoms define it),
  coordinate_start, coordinate_end, coordinate_steps,
  status ('draft' | 'running' | 'complete' | 'failed')
```

Existing `jobs` table gains nullable FKs:

```
jobs.reaction_id   → reactions.id  (nullable)
jobs.pathway_id    → pathways.id   (nullable)
```

Standalone jobs (reaction_id = NULL) remain fully functional.

## Execution model: native ORCA scan per pathway

The primary mechanism for exploring a reaction coordinate is ORCA's built-in
relaxed surface scan (`%geom Scan ... end`), not N separately orchestrated jobs.

```
Substrate (SMILES / import)
    │
    ▼
Conformer ensemble: ! XTB GOAT  ← MANDATORY first step
  Boltzmann-weighted ensemble in minutes at xTB level;
  re-optimise the 3-4 lowest at DFT level
    │
    ▼
Pathway setup (geometry editor, Phase 2.5):
  build starting geometry with correct approach angle/face
    │
    ▼
Generate ORCA input with native scan block:
  %geom Scan
    B 0 1 = 3.0, 1.5, 15    ← distance, start → end, N points
  end end
    │
    ▼
One ORCA Job per Pathway (two Jobs for si/re comparison)
    │
    ▼
Parse scan output: table of (step, energy, coordinate value)
    │
    ▼
Overlay two scan profiles → ΔΔE‡ (electronic energy barrier estimate)
    │
    ▼
[optional] TS refinement: scan maximum → OptTS → Freq → ΔG‡
```

### Conformer sampling is not optional

Optimising a single arbitrary starting conformer is not reproducible science.
A flexible substrate has chain rotamers and, often, additional isomerism
(e.g. syn/anti of a nitrite group, ~1–3 kcal/mol apart with a low rotation
barrier — both populated in solution). One geometry does not describe the system.

For reaction modeling the error compounds: substrate conformers × approach face
× reagent orientation. Starting two pathways from unrelated conformers means the
resulting ΔΔE‡ compares points on different conformational surfaces — a number
comes out, but it means nothing.

ORCA 6.1 makes this one keyword: `! XTB GOAT` produces a Boltzmann-weighted
conformer ensemble in minutes. Re-optimise the lowest 3–4 at the DFT level and
build reaction centers on those.

Related: use `TightOpt` before `Freq`. Default optimisation convergence can leave
artefactual low-frequency modes that corrupt the thermochemistry.

Why native scan over N-job orchestration:
- **Wavefunction chaining:** SCF at each scan point starts from the converged
  orbitals of the previous point (2–5× faster convergence, avoids SCF instabilities).
- **Geometry chaining:** the optimizer at each point starts from the previous
  geometry, preventing jumps to different conformers that break the profile.
- **Simplicity:** one job, one output, one entry in the queue.

The N-job parametric approach remains as a fallback for:
- Rigid (unrelaxed) scans at the SP level (rare).
- NEB path setup (requires a set of discrete input images).
- Recovery when a native scan fails at a specific point.

### ΔE‡ vs ΔG‡ — terminology precision

The maximum of a relaxed scan gives an **electronic energy barrier estimate** (ΔE‡),
not a free energy. For comparing two stereofacial pathways, ΔΔE‡ from two scans is
a valid first filter. For publication-quality results, the full pipeline is:

1. Relaxed scan → identify approximate TS geometry (energy maximum).
2. `OptTS` starting from the scan maximum → true saddle point.
3. `Freq` on the optimized TS → confirm exactly one imaginary frequency +
   thermochemistry corrections → ΔG‡.

Phase 4.5 targets step 1 (scan profiles + ΔΔE‡). Steps 2–3 (TS refinement) are
a late Phase 4.5 item or Phase 6 — documented explicitly so the wiki never
implies the app produces ΔG‡ when it only produces ΔE‡.

## Phasing (how this integrates with the ROADMAP)

| Phase | What it builds | Reaction-modeling relevance |
|---|---|---|
| **2** (current) | 3Dmol.js viewer, molecule import, input builder, convergence | Foundation: viewing + structure input |
| **2.5** (new) | Geometry editor: measure, set distance/angle/dihedral, fragment placement, constraint → `%geom`, xTB pre-opt | **Core primitives** for reaction center construction |
| **3** | Results dashboard: energies, trajectories, spectra, orbitals | Energy profile plot (reused for reaction coordinate) |
| **4** | Manual integration | Teaching: what is Bürgi-Dunitz angle, what is NEB |
| **4.5** (new) | Reaction Modeling: Reaction/Pathway data model, reaction center editor, native ORCA scan per pathway, scan output parser, comparative energy profiles (ΔΔE‡), TS refinement pipeline | **The integration layer** |
| **5** | SSH remote execution | Critical for sweep jobs (6–10+ heavy DFT calculations) |
| **6** | NEB-TS, IRC, scans, TD-DFT, batch parametric | NEB-TS is the natural next step after TS guess |

Key insight: every Phase 2–3 deliverable is independently useful AND is a building block
for reaction modeling. We don't build "infrastructure for the future" — we build
useful tools that compose into something greater.

## AI integration (Phase 6+ / long-term)

The reaction-centric domain model creates a natural surface for AI assistance:

**Level 1 — Reaction setup assistant.** User describes the reaction in natural language
("I want to study stereoselectivity of NaBH₄ reduction of this ketone"). AI, seeing the
molecular structure, identifies the reaction center (C=O carbon as electrophile, H of
BH₄⁻ as nucleophile), proposes Bürgi-Dunitz approach geometry, and suggests two pathways
(si-face, re-face). User reviews and adjusts.

**Level 2 — Literature-informed defaults.** AI pulls typical TS geometries from literature
or training data: Bürgi-Dunitz angle for nucleophilic addition, typical Pd–C distances for
oxidative addition, common ligand arrangements for cross-coupling. These become smart
defaults in the reaction center editor instead of the user guessing.

**Level 3 — Result interpretation.** After pathways complete, AI interprets: "ΔΔG‡ = 3.2
kcal/mol favoring si-face attack, consistent with Felkin-Anh model. The TS geometry shows
the phenyl group in pseudo-equatorial position, minimizing steric strain."

**Level 4 — Multi-step mechanism exploration.** For complex catalytic cycles (Sonogashira,
Buchwald-Hartwig), AI proposes the full mechanism: elementary steps, intermediates, which
steps are likely rate-determining. Researcher validates each step, adjusts, runs.
The system becomes a collaborative mechanism-exploration environment.

This is NOT a near-term priority. The foundation (geometry editor, reaction center,
pathway comparison) must be solid and manually usable first. AI amplifies a tool that
already works; it cannot substitute for missing primitives.

## What changes now

**Nothing breaks.** Phase 2 proceeds as planned. The only immediate action is:

1. Add this ADR to the wiki (ingest by Claude Code).
2. Update ROADMAP to include Phase 2.5 and Phase 4.5 placeholders.
3. Update `CLAUDE.md` non-goals: replace "Replacing Avogadro as a full molecular editor"
   with "Full-featured molecular drawing from scratch (symmetry-aware bond drawing,
   ring perception, etc.) — structure creation uses import + fragment placement +
   geometric manipulation, not free-hand sketching."
4. Every future design decision passes the test: "is this compatible with reaction
   center construction and pathway comparison?"
5. **Geometry kernel uses ASE, not custom math.** `ase.Atoms` provides
   `set_distance()`, `set_angle()`, `set_dihedral()` with mask arrays (which
   atoms move). ASE is already a planned sidecar dependency. Custom trigonometry
   for local coordinate systems is a classic source of week-long bugs —
   unnecessary when ASE covers the exact operations we need. Fragment placement
   starts minimal: attach along one vector (distance + angle + dihedral relative
   to three selected atoms) — the Bürgi-Dunitz case.
6. **Numerical control, not drag-editing.** The geometry editor uses typed
   numerical values (pick atoms → enter distance/angle → apply), not mouse
   dragging of atoms in 3D space. Drag-editing is imprecise, hard to implement
   over 3Dmol.js (a viewer, not an editor), and doesn't match the research
   workflow (the chemist knows the Bürgi-Dunitz angle is 107° — they want to
   type it, not guess it by dragging).
7. **Native ORCA scan for pathway sweeps, not N-job orchestration.** See the
   updated Execution Model section above.

## Risks

- **Scope creep.** Mitigation: reaction modeling features arrive in Phase 4.5, not before.
  Each earlier phase delivers standalone value. The author uses the app for real research
  throughout — if it's useful without reaction modeling, the project succeeds regardless.
- **3Dmol.js limitations for editing.** 3Dmol.js is a viewer, not an editor. Atom picking
  and measurement are supported; dragging atoms is not native. The editing layer (set
  distance/angle/dihedral) works through the geometry kernel (sidecar), not through
  3Dmol.js manipulation. 3Dmol.js re-renders; it doesn't edit.
- **xTB integration complexity.** Mitigated by using standalone `xtb` binary (already
  installed) with its native constraint file format, rather than wrapping through ORCA.
- **Catalytic cycles (Sonogashira etc.) are multi-step.** Each elementary step is its own
  Reaction with its own TS. A catalytic cycle is a sequence of Reactions. This meta-layer
  (Mechanism = ordered list of Reactions) is Phase 6+, not Phase 4.5.
- **MMFF force field gaps.** RDKit's MMFF may lack parameters for exotic reagent
  fragments (e.g. BH₄⁻). Mitigated by UFF fallback (`AllChem.UFFOptimizeMolecule`)
  which has broader atom-type coverage. Documented for Phase 2.5 fragment library.

## Amendment (2026-08-07, Phase 4.5 Stage C1) — ratified normalized schema

The "Data model impact" section above is a **sketch**. Stage C1 implements the first
concrete slice of it (migration v13) and **deviates deliberately** on two points. This
amendment records the ratified schema; the sketch is not edited (ADR history is
append-only).

**What C1 builds (migration v13):**

```
reactions
  id TEXT PK, name TEXT NOT NULL, description TEXT,
  created_at TEXT NOT NULL DEFAULT (datetime('now'))

pathways
  id TEXT PK, reaction_id TEXT NOT NULL REFERENCES reactions(id),
  label TEXT NOT NULL,
  created_at TEXT NOT NULL DEFAULT (datetime('now'))

jobs
  + pathway_id TEXT  (nullable, REFERENCES pathways(id))   -- and NOTHING else
```

**Deviation 1 — normalized: `jobs.pathway_id` ONLY, no `jobs.reaction_id`.** The sketch
proposed *both* FKs on `jobs`. We normalize: a job carries `pathway_id` only; its reaction
is derived by joining `pathways`. Two columns that can disagree (a job whose `reaction_id`
and `pathway_id.reaction_id` differ) is exactly the class of bug this project keeps
refusing — **one source of truth** wins over the one-join saving. A pathway always belongs
to exactly one reaction, so the join is total and cheap.

**Deviation 2 — a pathway is lean; `reaction_centers` and coordinate/method/profile are
not in C1.** The sketch put `coordinate_type/param/start/end/steps/status` on `pathways`
and added a `reaction_centers` table. C1 stores **none** of that on `pathways` — a pathway
is `{ id, reaction_id, label }`. The scan coordinate, method, and energy profile already
live in the attached job's input/results (Stages A/B); C2 reads them there. Storing them
again on the pathway would be a second copy that can drift (same reason as Deviation 1).
`reaction_centers` belongs with the reaction-center editor and is deferred to that unit.

**Load-bearing invariant — jobs survive grouping-row deletion.** Jobs are the work;
reactions/pathways are grouping metadata. `delete_reaction` removes its pathway rows and
**nulls** the `pathway_id` of every job attached to them; `delete_pathway` nulls its jobs'
`pathway_id` and removes the row. **Neither ever deletes a job.** A scan job that was
promoted into a pathway and then un-grouped is exactly the standalone scan job it was
before (Stages A/B still operate on it). `pathway_id = NULL` is the normal state for every
job today and nothing about the standalone job/scan/results path changes — the reaction
model is purely additive.

**Referential integrity is enforced in the Rust commands**, not by SQLite: this DB leaves
`PRAGMA foreign_keys` off (as elsewhere — even the `results` `ON DELETE CASCADE` is
documentation), so `create_pathway` under a missing reaction and `attach_job_to_pathway`
with a missing job/pathway return `AppError::NotFound` with no orphan/partial write. The
`REFERENCES` clauses in the DDL document intent.

> **Correction (measured 2026-08-08, rule #10 — mirrors `modules/tauri-core.md`:151):** the
> aside above — "this DB leaves `PRAGMA foreign_keys` off … even the `results` `ON DELETE
> CASCADE` is documentation … the `REFERENCES` clauses document intent" — is **false**. The
> bundled SQLite is compiled with `SQLITE_DEFAULT_FOREIGN_KEYS=1` (`PRAGMA foreign_keys` reads
> **1**; `pragma_compile_options` lists `DEFAULT_FOREIGN_KEYS`), so the `REFERENCES` clauses are
> **actively enforced** on every connection and `results ON DELETE CASCADE` **does fire** —
> proven by the Phase 4.7.1 test `delete_job_removes_it_and_cascades_results` (delete a job → its
> `results` row is removed automatically). The command-level checks are therefore
> **belt-and-suspenders** (clean `NotFound` errors + correct child-first delete order so a parent
> is never removed before its children — error 787), **not** the sole integrity mechanism. The
> ADR's decision (integrity-in-commands, jobs-survive, normalized `pathway_id`) stands; only this
> factual aside was wrong.

Commands: `create_reaction`,
`list_reactions`, `rename_reaction`, `delete_reaction`, `create_pathway`, `list_pathways`,
`delete_pathway`, `attach_job_to_pathway`, `detach_job_from_pathway`. Full schema +
command semantics: `modules/tauri-core.md` (v13). C2 (comparative ΔΔE‡ view) builds on this.

**Reactant reference for absolute barriers — [ADR-018](adr-018-reaction-energy-reference.md).** The
comparative view's *absolute* barriers (vs separated reactants) need a reactant reference; ADR-018
ratifies it as an **optional, summed list of reference-job references** (`reaction_reference_jobs`,
migration v14 at C2b) — same normalization and jobs-survive rule as the pathway model above. The
mission ΔΔE‡ number is **reference-free** (it cancels), so it ships without any reference configured.
The three barriers and which reference each needs: [`chemistry/reaction-barriers.md`](../chemistry/reaction-barriers.md).

## References

- Bürgi, H. B.; Dunitz, J. D. "From crystal statics to chemical dynamics."
  Acc. Chem. Res. 1983, 16, 153–161. (The original Bürgi-Dunitz trajectory paper.)
- Felkin, H. "The stereochemistry of the addition of organometallic reagents to ketones."
  Top. Curr. Chem. 1968, 18, 1–61.
- Author's bachelor thesis: stereoselectivity proof for NaBH₄ reduction (the motivating
  use case for this ADR).
