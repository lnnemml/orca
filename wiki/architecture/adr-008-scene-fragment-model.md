# ADR-008 — Scene / SceneFragment model for multi-molecule geometry

**Status:** accepted  
**Date:** 2026-07-28  
**Supersedes:** —  
**Context:** ADR-007 makes reaction modeling the mission, but the entire Phase 2
geometry path is single-fragment. `MoleculeViewer` takes one xyz string;
`extractXyzFromInput` / `injectXyzIntoInput` handle one `* xyz ... *` block;
importing a molecule *replaces* the geometry instead of adding to it. Reaction
setup needs substrate + reagent in one scene, with known fragment boundaries —
you cannot place BH₄⁻ at a Bürgi-Dunitz angle relative to a carbonyl carbon if
the two molecules cannot coexist as distinct pieces in the same coordinate space.

---

## Decision

A **Scene** is an ordered list of **SceneFragment**s. It is OrcaStudio's own
abstraction — ORCA never sees it. On export the fragments are merged into one
flat `* xyz totalCharge multiplicity ... *` block; fragment identity lives only
in our state and (as a snapshot) in the DB. The input file stays a plain,
hand-editable ORCA input; the Scene annotates it, it does not replace it.

Numbered decisions, each with its rationale:

1. **Naming: `SceneFragment`, never `Fragment`.** `Fragment` collides with
   `React.Fragment` in imports and — just as importantly — in reader/agent
   attention. The longer name costs nothing and removes a whole class of
   confusion. (Note: ADR-007's domain table uses "Fragment" for the reusable
   library piece; the *scene-instance* type is `SceneFragment`. See #9.)

2. **Viewer: one 3Dmol model, styled by atom index range** — not one model per
   fragment. A single model keeps one index space end to end: picking returns
   the same index that indexes the merged xyz that indexes the ASE mask.
   Per-model indices reset to 0 per model and would need an extra indirection
   layer (model-id + local-index → global-index) at every boundary — picking,
   measurement, constraint export, xTB round-trip. One model, one index space.

3. **Use `atom.index` (0-based, stable for xyz models), not `atom.serial`.**
   `serial` comes from PDB-like formats and may be 1-based or absent; `index` is
   the position in the model's atom array, which for an xyz model is exactly the
   line order of the merged block. That is the number we want everywhere.

4. **Canonical merged-xyz format.** Coordinates `toFixed(8)`, element symbol
   right-padded, fixed column width. Needed twice: (a) float-tolerant comparison
   against the Monaco buffer (see #6), and (b) stable golden-string diffs in
   tests. A canonical serializer makes both deterministic instead of
   whitespace-sensitive.

5. **Persistence = one nullable TEXT column `jobs.scene_json`** (schema v4), a
   versioned JSON snapshot (`"version": 1`) written on job creation and restored
   on open/clone. Explicitly **NOT** relational tables: Phase 4.5 replaces this
   with the real Reaction/Pathway schema (ADR-007), and the migration path is
   trivial — read the JSON, expand into rows. Rationale for persisting at all
   *now*: the Phase 2.5 workflow is iterative — build a TS guess in TS, run it,
   adjust the angle, rerun. Without a snapshot, cloning a job yields one flat
   fragment and the user re-splits the scene by hand on every iteration, dozens
   of times per TS.

6. **Source of truth: Scene → Monaco for geometry, Monaco → everything for
   keywords.** The Scene owns the coordinate block; the input text owns the
   keywords and blocks. Scene resets to a single fragment ONLY when the
   *coordinate block* changed — detected by parsing both sides and comparing
   floats with tolerance `1e-6`, **never** by string comparison (number
   formatting differs: `1.0` vs `1.00000000`). The reset shows a notification
   with **Undo**, backed by the previous Scene held in a ref: one variable that
   protects half an hour of fragment placement from a stray space in a
   coordinate. This is consistent with the Phase 2 rule "form ↔ text one-way is
   fine".

7. **New-fragment placement: bounding box + 3–4 Å gap** along the axis with the
   most free space. Centre-of-mass + 5 Å along +X was the naive first idea and
   collides for elongated substrates — the offset can land inside a carbon
   chain. Bounding-box separation along the freest axis is still coarse; exact
   positioning is the geometry editor's job (set_distance / set_angle /
   set_dihedral). This only has to produce a non-overlapping starting point.

8. **Electron-parity validation.** Σ Z − total charge gives the electron count;
   its parity constrains the allowed multiplicity parity (even electrons →
   odd multiplicity, and vice versa). Odd electron count with a singlet is an
   error ORCA reports cryptically ~30 s into the run; the form can say it
   instantly and in plain language. Cheap to compute, and a teaching moment.

9. **Fragment library holds curated fixed geometries with provenance** (e.g.
   BH₄⁻: T_d symmetry, B–H 1.24 Å), each with a comment saying where the
   geometry came from — hand-built or a one-off xTB run. **No runtime RDKit
   generation:** MMFF silently lacks parameters for exotic ions like BH₄⁻, and
   runtime generation is a moving part for what is really five hardcoded
   structures. (This refines ADR-007's UFF-fallback note: for the *library*
   fragments we ship geometries, we do not generate them.)

10. **State: a Zustand store wrapping the pure functions**, which stay
    React-free and node-testable. The merge / index-mapping / comparison logic
    lives in plain functions with no React import; the store is a thin reactive
    shell over them. `zustand` is not yet a dependency — it is added in the task
    that first needs a store (2.5.0d), not in 2.5.0a.

11. **ORCA's `(1)`/`(2)` fragment annotation stays out of scope.** It serves
    compound methods (DFT-SAPT, counterpoise BSSE), not standard Opt/Freq/Scan,
    so the merged flat block is correct for everything Phase 2.5 does. Forward
    note: ORCA 6 also uses `%geom` fragments for rigid-body optimisation and
    fragment constraints, so our internal model will eventually be the *source*
    for generating those blocks. One more reason we track fragments ourselves
    rather than leaning on the input format.

## Why this is a prerequisite, not a feature

Every Phase 2.5 deliverable consumes something concrete from the Scene model:

| Phase 2.5 item | What it consumes from the Scene |
|---|---|
| Atom picking + measurement | `locateAtom(globalIndex)` so the UI can say "atom 3 of BH₄⁻ (B)"; inter- vs intra-fragment distances become distinguishable |
| Geometry kernel (ASE) | `fragmentAtomIndices()` **is** the mask argument to `set_distance` / `set_angle` / `set_dihedral` |
| Edit mode | pick substrate atom + reagent atom ⇒ mask = reagent fragment, so the reagent moves and the substrate stays |
| Fragment placement | Bürgi-Dunitz = `set_distance` + `set_angle` + `set_dihedral` with the mask on the newly added fragment |
| Constraint manager | constraints reference cross-fragment atom pairs |
| xTB integration | merged xyz in, optimised xyz out; fragment boundaries preserved because atom ordering and count are invariant across the round-trip |

Without the Scene model each of these would grow its own ad-hoc "which atoms
belong to the reagent" bookkeeping. The model centralises that once.

## Task breakdown

- **2.5.0a — pure core:** Scene / SceneFragment types + pure functions (merge,
  index mapping, immutable updates, serialization, float-tolerant comparison) +
  tests. Zero React.
- **2.5.0b — input-builder integration:** total charge from fragments,
  coordinate injection, electron-parity validation.
- **2.5.0c — multi-fragment viewer:** one 3Dmol model, index-range styling,
  per-fragment colours.
- **2.5.0d — Add Fragment UI:** Zustand scene store + `scene_json` migration
  (schema v4).

`b` and `c` can run in parallel after `a`.

## Risks

- **Scene / Monaco divergence** — mitigated by the float-tolerant coordinate
  check + Undo (#6).
- **Coarse placement collisions** — bounding-box separation reduces but does not
  eliminate them; the geometry editor is the real fix (#7).
- **`scene_json` going stale** relative to a hand-edited `input_content` — the
  snapshot is *provenance, not authority*: geometry always re-derives from the
  input text on open, fragments only annotate it. If the coordinate block was
  edited by hand, the float-tolerant check collapses the Scene to a single
  fragment rather than trusting a stale snapshot.

## References

- ADR-007 — reaction modeling mission, ASE geometry kernel, Fragment domain
  object, Phase 2.5 / 4.5 phasing.
- `wiki/orca/input-format.md` — "Not yet modelled": `(1)`/`(2)` annotation.
- `wiki/modules/visualization.md` — `atom.index` vs `atom.serial`, one-model
  multi-fragment rendering.

---

## Amendment (2026-07-28, implemented in 2.5.0d-3)

Decision #5 (persist a `scene_json` snapshot) **stands** — this amendment corrects
its *justification*, it does not change the decision (per the no-rewrite-history
convention).

**What was wrong.** #5 argued the column was needed because "cloning a job
otherwise yields one flat fragment and the user re-splits the scene by hand every
iteration." But **no clone/duplicate action existed in the app** when d-1–d-2b
shipped: New Job is reachable only from its own tab and from Molecules (via
`initialMolecule`). A snapshot column with no reader would have been dead weight —
written on every job, read by nothing.

**What changed.** 2.5.0d-3 makes the snapshot live by adding its reader in the same
unit: a **"New iteration"** action on the job detail screen that seeds a fresh New
Job draft from an existing job's `input_content` + `scene_json` (nothing from its
results, status, or directory; iterating a running job is fine — only its input is
taken). This is the clone #5 assumed. The iterative TS-guess workflow it enables —
build a scene, run, tweak the approach angle, run again with the fragment layout
intact — is now real and hand-verifiable.

**The reconciliation rule from the Risks section is now code.** `input_content` is
authoritative for geometry; `scene_json` only *annotates* it. `restoreScene`
(pure, `src/scene/restore.ts`) honours a snapshot only when
`xyzMatchesScene(snapshot, input geometry)` holds — the same primitive that guards
the live Scene↔Monaco sync, not a second comparison. A missing / malformed /
wrong-version / stale snapshot falls back to a single fragment parsed from the
input, and a `snapshotRejected` flag distinguishes a *discarded* snapshot (worth a
UI note) from a plain pre-v4 job (`scene_json = NULL`, no note).

**Storage.** Schema v4 adds a nullable `jobs.scene_json TEXT`, written once at job
creation (the input is immutable, so is the snapshot — no update path). Still
deliberately NOT relational tables; Phase 4.5's Reaction/Pathway schema supersedes
it, and the JSON→rows migration stays trivial.

**Forward note — "continue from the result" (Phase 3+).** Iteration currently
re-uses the job's *starting* geometry. Taking the *optimised* geometry from the
output instead needs output parsing (cclib, Phase 3) and is out of scope here — but
the fragment snapshot is already built to support it: after an optimisation the
atom count and order are invariant (`replaceFragmentAtoms`'s guarantee), so the
fragment boundaries transfer onto the optimised coordinates with no guessing.
