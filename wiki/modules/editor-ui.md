# Module: editor UI (the New Job workspace)

**Status:** Phase 4.2 unit 2c1. The New Job screen (`src/screens/NewJobScreen.tsx`) is a
**viewer-first workspace**: the 3Dmol canvas is the primary surface, and the geometry panels live in
a **right dock** (`src/scene/EditorDock.tsx`) that is a thin icon rail expanding per section. This
page records the **layout principle and where future panels go** — not the current CSS pixel values
(those live in `src/styles/app.css`).

## The principle: viewer-first, one dock, fullscreen = workspace

- **The canvas is primary.** In the editor/viewer split the viewer takes the width; the dock is a
  thin always-visible **icon rail** on its right. Opening a section expands the dock (~300px, ~340px
  in fullscreen) and Monaco yields width to the viewer+dock (`.editor-viewer-split.dock-open`). When
  no section is open the canvas has the whole area. This is the arrangement rule for the editor —
  **new panels are dock sections, not another stack under the viewer.**
- **One dock, both modes.** There is exactly **one** `EditorDock` instance, shared by normal and
  fullscreen mode (never duplicated — two `AtomInspector`s would be two copies of selection state).
  Fullscreen is the **workspace mode**: the same dock is inside it, so every section — including Add
  Fragment — is reachable without leaving fullscreen. The fullscreen toggle changes only a className;
  `MoleculeViewer` keeps its tree position and is not remounted (the camera survives).
- **Sections, in order of use:** Selection & Measure (`AtomInspector`) · Edit (`EditPanel`) ·
  Fragments (the Add-Fragment palette — reagents / import / SMILES / library — **plus** `FragmentList`)
  · Constraints (`ConstraintPanel`) · History (`HistoryPanel`) · Actions (xTB pre-optimize). Each
  toggles **independently**; open-state is **session-only** (not persisted — a fresh screen starts
  viewer-first with just Fragments open so Add Fragment is discoverable). Each section shows its
  panel's **existing** empty state (e.g. `AtomInspector`/`HistoryPanel` render nothing until there's
  something to show); nothing new was invented — the panels only **moved**.

## The dock is pure layout (no model, no logic)

`EditorDock` holds no editor state and imports nothing under `scene.ts` / `oplog.ts` / `store.ts` /
`constraints.ts`. It takes an array of `{ id, label, short, glyph, body }` sections plus the
open-map and a toggle; `NewJobScreen` owns every panel's props and the open-state. The 2b-ux change
touched only `NewJobScreen.tsx`, `app.css`, and this new `EditorDock.tsx` — **the geometry model is
untouched** (that is the invariant for this unit).

## Resize — the one mechanism (WebKitGTK)

WebKitGTK breaks layout silently (the scar family: `min-width:auto` clipping — `debugging/011`;
`ResponsiveContainer` 0×0; `OffscreenCanvas` null). The 3Dmol canvas **must** be told to resize when
its container box changes, or it renders at the old size / 0×0 with no error. There is **one**
mechanism for this and the dock reuses it: `MoleculeViewer` runs a `ResizeObserver` on its container
that calls `viewer.resize()` on any box change. Collapsing/expanding a dock section changes the dock
width (and flips the split ratio), which changes the viewer's flex box, which fires the observer —
exactly the path fullscreen and the split-panel resize already use. **No per-toggle `viewer.resize()`
call exists** (that would be a second, ad-hoc mechanism). If a future layout change ever fails to
fire the observer under WebKitGTK, that is a new `debugging/` page, not a scattered fix.

## Feeding the viewer: geometry + AtomId table, one function (unit 2c1)

3Dmol is a **dumb renderer** (ADR-010 / ADR-011): it is handed one geometry and, alongside it, an
`AtomId↔viewer-index` table, and is never a source of truth. The contract that keeps that honest:

- **The table is built by the SAME function that forms the geometry.** `buildViewerFeed(scene)`
  (in `scene.ts`) returns `{ xyz, table }` from one pass over `allAtoms(scene)` — the model 3Dmol
  draws and the table picks resolve through come from one call, one atom sequence. The table is **not**
  a second piece of state that "also has to be updated"; it is a return value of the geometry builder.
  A construction where the table could drift from the geometry in even one code path is *wrong*, not
  "needs care" — the failure is silent and the worst kind (a click returns a *different* atom's id and
  everything downstream succeeds with the wrong atom). `MoleculeViewer` sets its `viewerTableRef` in
  the same effect run that calls `addModel`, and clears it on the non-scene paths — see
  [visualization.md](visualization.md) "Atom picking".
- **Picking returns an `AtomId`.** `onAtomPick(pick: AtomPick)` where `AtomPick = { atomId, viewerIndex }`.
  `viewerIndex` is 3Dmol's raw `atom.index` — **viewer space, diagnostics only**, never an app id.
- **The 2c1→2c2 seam.** `selection` / `measure` / `edit-plan` / `constraints` still key on the
  positional global index (they move to `AtomId` in 2c2). At the boundary sits **one** explicit, named
  adapter in `NewJobScreen.onAtomPick`: it converts the pick's `AtomId` back to a global index via
  `buildViewerAtomTable(scene).viewerIndexOf(...)` (the same pure table derivation, recomputed from the
  current scene — no stored copy to lag). It carries a `TODO(2c2)` and is deleted when the pipeline
  keys on `AtomId` directly (and `selectionSurvives` gains its removal dividend — see below).

## Reads from 3Dmol — the ADR-011 audit (unit 2c1)

The whole app imports `3dmol` in **one file** (`MoleculeViewer.tsx`) — the single renderer boundary.
Every place that pulls state *out* of 3Dmol was swept; only two exist, both conscious exceptions with
the coordinate-of-a-click read added by picking:

- **`viewer.pngURI()`** (`toPngBytes`, unit 3.16) — reads the rendered buffer to export a PNG.
  Sanctioned by ADR-011 verbatim ("the app requests it; the viewer produces it from what it drew"):
  it is *output from what was drawn*, not a truth-read.
- **`anim.model.selectedAtoms({})`** (frozen-topology animation, unit 3.14) — reads the atom objects of
  a model **the app itself built and holds** as the animation vehicle, to write the next frame's
  coordinates in place (and, in DEV only, to count drawable bonds — a diagnostic). Not 3Dmol as a
  source of geometry/selection/picking truth; the app owns this model deliberately. Documented in
  [visualization.md](visualization.md).
- **`atom.index` inside the pick callback** — the *identity of the clicked atom*, immediately resolved
  to an `AtomId` through the feed's table (ADR-011: "clicks are mapped back to AtomId through that
  table"). This is the one read the renderer boundary is *supposed* to have.

Not viewer reads (false positives worth recording so a future audit doesn't re-flag them):
`editor.getModel()` in `selection-panel.ts` is **Monaco**, not 3Dmol; `results.trajectory.frames` in
`ResultsCard` reads **our parsed results**. No consumer (`ModeAnimator`, `OrbitalPanel`,
`TrajectoryPlayer`, `MoleculesScreen`, `JobDetailScreen`, `NewJobScreen`) reads 3Dmol internals — they
feed it via props only.

## Where future panels go

- **2c2 — index-space labels.** When the pipeline moves onto `AtomId` (`selection`/`measure`/
  `edit-plan`), the space-labelled index readouts (ADR-010: never a bare index without naming its
  space) live in the **Selection & Measure** section — a dock section, per the principle above.
- **Phase 4.5 — reaction setup.** The reaction-center / scan-setup UI (ADR-007) is a **new dock
  section** (e.g. "Reaction"), not a new stack: it composes the existing selection/measure/constraint
  panels into a guided flow. Guided fragment placement (add a reagent at a distance/angle/dihedral)
  extends the **Fragments** section.

## Related

- `src/scene/EditorDock.tsx` — the sectioned dock (pure layout).
- `wiki/modules/scene.md` — the panels the dock hosts (store, HistoryPanel, EditPanel, FragmentList,
  ConstraintPanel) and the operation log they act on.
- `wiki/modules/visualization.md` — `MoleculeViewer` (the canvas, its `ResizeObserver`, picking).
- `wiki/debugging/011-*` — the WebKitGTK `min-width:auto` clip; the reason layout changes are treated
  as risk here.
