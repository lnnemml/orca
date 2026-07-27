# ADR-007 — From molecular modeling to reaction modeling

**Status:** accepted 
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

A Reaction contains one or more Pathways. A Pathway generates multiple Jobs
(one per scan point, or a single NEB/OptTS job). Jobs remain standalone-capable:
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
jobs.pathway_step  INTEGER         (nullable, index within sweep)
```

Standalone jobs (reaction_id = NULL) remain fully functional.

## Execution model: pathway as batch orchestration

```
User defines ReactionCenter (d, angle, dihedral)
    │
    ▼
Sweep reaction coordinate: d = [3.0, 2.7, 2.4, 2.1, 1.8, 1.5]
    │
    ▼
For each d:
  ┌─ geometry kernel: generate xyz with these params
  ├─ constraint manager: fix d, export %geom
  ├─ [optional] xTB pre-optimize with constraint
  └─ create ORCA Job (constrained Opt or SP)
    │
    ▼
Run all (sequential, concurrency=1 per backend)
    │
    ▼
Collect energies → energy profile plot
    │
    ▼
[optional] identify maximum → TS guess → OptTS / NEB-TS
    │
    ▼
Comparative view: overlay Pathway A vs Pathway B
```

Each step in this pipeline is a primitive we build in earlier phases.
The orchestration layer is new but thin.

## Phasing (how this integrates with the ROADMAP)

| Phase | What it builds | Reaction-modeling relevance |
|---|---|---|
| **2** (current) | 3Dmol.js viewer, molecule import, input builder, convergence | Foundation: viewing + structure input |
| **2.5** (new) | Geometry editor: measure, set distance/angle/dihedral, fragment placement, constraint → `%geom`, xTB pre-opt | **Core primitives** for reaction center construction |
| **3** | Results dashboard: energies, trajectories, spectra, orbitals | Energy profile plot (reused for reaction coordinate) |
| **4** | Manual integration | Teaching: what is Bürgi-Dunitz angle, what is NEB |
| **4.5** (new) | Reaction Modeling: Reaction/Pathway data model, reaction center editor, parametric sweep, batch orchestration, comparative pathway view, reaction energy diagram | **The integration layer** |
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

## References

- Bürgi, H. B.; Dunitz, J. D. "From crystal statics to chemical dynamics."
  Acc. Chem. Res. 1983, 16, 153–161. (The original Bürgi-Dunitz trajectory paper.)
- Felkin, H. "The stereochemistry of the addition of organometallic reagents to ketones."
  Top. Curr. Chem. 1968, 18, 1–61.
- Author's bachelor thesis: stereoselectivity proof for NaBH₄ reduction (the motivating
  use case for this ADR).
