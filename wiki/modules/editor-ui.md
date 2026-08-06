# Module: editor UI (the New Job workspace)

**Status:** Phase 4.2 unit 3.3 (Stage 3 — operations over the core). The New Job screen (`src/screens/NewJobScreen.tsx`) is a
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
- **Text owns chemistry, the Scene owns geometry (ADR-010 authority split; enforced in unit 2d).**
  The editable surface of Monaco is the `!` line and `%` blocks — the chemistry. The `* xyz … *`
  coordinate block is a **read-only projection of the 3D Scene**: a hand-edit of it is reverted (with
  a notice pointing at the doors), never adopted, so there is one source of geometry (the viewer/Scene)
  and one source of chemistry (the text). The capability to hand-enter coordinates is preserved through
  two doors — **Paste xyz** (import as a fragment, the typical path) and **Replace input** (the
  **named escape**: unlock the whole buffer once to paste a different calculation, then Adopt it as a
  fresh scene). See `modules/scene.md` for the sync wiring; `modules/frontend.md` for the controls.
- **Move mode — rough placement by dragging (unit 3.1; a toggle in the Edit section).** A checkbox
  turns on rigid-body fragment drag: grab any atom of a fragment and drag to move the whole fragment in
  the plane of the screen (60fps, one Undo step — see `modules/visualization.md`). It is deliberately
  **coarse**: the drag sets *approximate* geometry; **exact** distances/angles/dihedrals come from the
  measure + constraint tools and the input editor. That division of labour is the point — the drag
  answers "roughly here", the editor answers "exactly this". A drag on empty space still rotates the
  camera; a click still picks; toggling Move off restores plain rotate/pick.
- **Steric-clash warning — a warning, never a block (unit 3.2).** After any geometry change, atoms of
  DIFFERENT fragments closer than `k·(rᵢ+rⱼ)` of their vdW sum are flagged: a warn banner ("N steric
  clashes — coarse placement…") and a **magenta danger glow** on the clashing atoms, visually apart
  from the selection halo and edit mask (its own hue AND form — no CPK element is magenta). It **does
  not block** Create/Run: a close contact at setup is expected, and the drag is coarse by design. The
  threshold `k` is a **labeled display-choice** in the Edit section — a slider captioned "vdW overlap
  threshold — heuristic, not a physical cutoff" (like the IR FWHM slider), app-owned, defaulting to
  ≈0.65 — surfaced, not a hidden constant. A pair carrying an active **distance constraint** is an
  intentional contact and is never flagged (the mission's reactive approach doesn't false-alarm); an
  element with no cited vdW radius gets a **quiet, separate** UNDETERMINED notice (skipped, not
  guessed). Chemistry + the physics of the threshold: `chemistry/vdw-steric.md`; mechanism:
  `modules/scene.md` / `visualization.md`.
- **Rotate about axis — rigid whole-fragment spin (unit 3.3; `RotatePanel`, a sibling of `EditPanel`
  in the Edit section).** Pick **two atoms** — P (first, the pivot, on the fragment to turn) and Q
  (second, the direction, typically the substrate contact atom) — and a **numeric angle**; the fragment
  spins rigidly about the P→Q **approach axis** in a live **ephemeral preview** (viewer-only, the Scene
  is untouched), committing **one** `rotate-fragment` op on **Apply** (Cancel drops the preview with zero
  ops). The axis is drawn as an extended cylinder through P→Q. **Numeric, not spin-drag** — a typed angle
  is **reproducible** (the journal reads "Rotate BH₄⁻ 30° about O→C") and serves the reaction-mechanism
  mission directly; spin-drag is deferred. A **swap** button flips which picked atom is the pivot. A
  degenerate axis (P ≡ Q) disables Apply with a reason. It is **pure TS, a sibling of — not routed
  through — the sidecar set-internal edit** (rigid transform vs internal-coordinate solve; see the split
  in `modules/scene.md`). Mechanism / preview path: `visualization.md`.
  - **Axis ⇄ Distance overlay toggle (unit 3.3b).** The axis cylinder and the measurement distance line
    were drawn on the same two atoms at once — two overlapping objects that read as one wrong line. A
    small segmented toggle now shows **exactly one** overlay for the pair: **Axis** (cylinder + the Å
    value on the axis midpoint) or **Distance** (the measurement line + label). The Å number is the same
    in both modes (single source: `measure` distance). App-owned state (`rotateOverlay` in `NewJobScreen`,
    like `clashK`), default Axis, reset to Axis when the pair changes or on Cancel; **the measurement
    tool outside Rotate is untouched**.
- **Sections, in order of use:** Selection & Measure (`AtomInspector`) · Edit (`EditPanel` + the Move-mode
  toggle + `RotatePanel`) · Fragments (the Add-Fragment palette — reagents / import / SMILES / **Paste xyz** / library —
  **plus** `FragmentList`) · Constraints (`ConstraintPanel`) · History (`HistoryPanel`) · Actions (xTB
  pre-optimize). Each
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
- **Picking → selection is `AtomId` end to end (2c2).** The 2c1→2c2 adapter is **gone**:
  `onAtomPick` feeds the pick's `AtomId` straight into an `AtomId[]` selection, and
  `selection` / `measure` / `planEdit` input / `constraintFromSelection` input / the viewer
  highlight all key on `AtomId`. Two things stay positional **at their own emit seam** (ADR-010:
  order matters in exactly one place — the emitter): the **ASE mask** (`EditPlan.indices`/`mask`/
  `cut`/`within`, `set-internal`, `rotatable-mask`) and the **`%geom` constraint** (a `Constraint`'s
  atoms are ORCA 0-based indices, frozen into the text). The `AtomId → positional index` conversion
  happens at **exactly those two seams** (`planEdit` resolves once on entry; `constraintFromSelection`
  resolves at build time), via `globalIndexOfAtom`.
- **The dividend.** Because the selection is ids, `filterSelection` keeps every picked atom still in
  the scene — removing an *unrelated* fragment no longer clears the selection (the positional guards
  `selectionSurvives`/`validateSelection` are removed). See `wiki/modules/scene.md`.

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

## Labelling the index space is a whole-UI rule (unit 2c2)

ADR-010 correction (iii) says: never show a **bare** index without naming its space (the fix for
"the user reports an index from the UI that doesn't match the logs" is *labelling the space*, not
*hiding the number* — OrcaStudio is a learning instrument). Unit 2c2 extends that from the coordinate
panel to **the whole UI**, now that different panels genuinely show different spaces:

- **`AtomInspector`** (Selection & Measure) — the primary readout keeps "global index M (both
  0-based)"; the multi-chip list labels each chip "global #N". Resolved from the picked `AtomId`, so
  the number is the atom's *current* global index, never a stale one.
- **`ConstraintPanel`** — atoms are labelled **ORCA 0-based index** (a header hint + the out-of-range
  note), because that is the number written into `%geom` and reported in the ORCA output — the one the
  user must be able to cross-reference against the log.
- **The 3D view** shows the 0-based **viewer index** on an atom (unchanged) — the same value as the
  global index, and the panels name which space they mean.

The principle: a panel names the space of every index it shows, and the *value* shown is the one that
space actually uses (global for the inspector, ORCA for constraints). Same number today, named homes,
so a future divergence can't reintroduce the "which index is this?" ambiguity.

## Where future panels go

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
