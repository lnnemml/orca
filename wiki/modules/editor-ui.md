# Module: editor UI (the New Job workspace)

**Status:** Phase 4.2 unit 2b-ux. The New Job screen (`src/screens/NewJobScreen.tsx`) is a
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
