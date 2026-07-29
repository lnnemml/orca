# Module: Visualization

**Status:** 3Dmol.js component built (Phase 2.1); **multi-fragment rendering added (2.5.0c)** — one
model, index-range styling, shared `fragmentColor` palette. **Atom picking added (2.5.2a)** — mouse
click → `atom.index` → selection panel, WebKitGTK-confirmed. **Measurement labels/lines added
(2.5.2b)** — dashed bond lines + a d/θ/φ value label in the highlight effect, non-clickable.
Trajectories / orbitals / spectra not started.

## As built (Phase 2.1) — MoleculeViewer + xyz preview
`npm install 3dmol` (v2.5.5; ships its own TypeScript types under `build/types/`, so no custom
`.d.ts` was needed). Files in `src/viewer/`:

- **`MoleculeViewer.tsx`** — React component. Props `xyzData: string` (standard xyz) and optional
  `style`. One `createViewer()` per mount into a `useRef` div; a second effect re-renders on
  `xyzData` change (`removeAllModels` → `addModel(xyz,"xyz")` → `setStyle({},{stick:{},sphere:{scale:0.3}})`
  ball-and-stick → `zoomTo` → `render`). Mouse rotate/zoom/pan is 3Dmol's default — nothing added.
- **`3dmol-setup.ts`** — side-effect module (imported by `MoleculeViewer`), **critical for the
  WebKitGTK webview** — see decisions below and `debugging/002`.
- **`parse-xyz-from-input.ts`** — `extractXyzFromInput(input): string | null`. Scans for the
  opening `* xyz …` marker, collects `element x y z` lines until the closing `*`, returns standard
  xyz (`count\ncomment\ncoords`). Returns `null` for the `* xyzfile … file.xyz` form (external
  file, unreadable on the frontend) and when no coordinates are found. Tolerates blank/`#` lines.
  *(Deleted in 2.5.0d-1 — this extraction now lives in `src/scene/scene.ts` as `sceneFromOrcaInput`.)*

### Multi-fragment rendering (2.5.0c)
`MoleculeViewer` now takes an **optional `scene?: Scene`** alongside `xyzData?: string`; `scene`
takes precedence. At 2.5.0c both callers still passed `xyzData`; **since 2.5.0d-1 `NewJobScreen`
passes `scene`** (store-driven), while `MoleculesScreen` still passes `xyzData` (stored xyz
strings). With no props it renders an empty viewer. Approach (ADR-008 #2/#3):

- **One 3Dmol model, styled by atom index range** — not model-per-fragment. Coordinates come from
  `mergeToXyz(scene)`; `fragmentRanges(scene)` (start-inclusive, end-exclusive) gives each fragment's
  global index range. Base ball-and-stick (`{stick:{}, sphere:{scale:0.3}}`, CPK colours) is applied
  to all atoms, then fragments **1+** get an override `setStyle({index:[…]}, {stick:{color}, sphere:{scale:0.3,color}})`.
  Fragment **0 keeps CPK** — the substrate must not recolour when a reagent is added (this is a
  requirement, not a nicety; a single-fragment scene looks identical to the pre-2.5.0c render).
- **Palette** lives in `src/viewer/fragment-colors.ts` — `FRAGMENT_PALETTE`
  (teal/coral/gold/violet) + `fragmentColor(i)` (`undefined` for fragment 0, cycling palette for
  1+). One source of truth, shared with the 2.5.0d fragment sidebar so a fragment reads the same
  colour in both places.
- **`index` selector — confirmed behaviour.** 3Dmol's `AtomSelectionSpec.index` is typed
  `number | number[]`, and `setStyle({index:[3,4,5,6,7]}, …)` genuinely styles exactly those 0-based
  atoms. **Verified in the WebKitGTK 4.1 MiniBrowser** (technique from `debugging/002`), not just
  Chromium: a water (atoms 0–2) + BH₄⁻ (atoms 3–7) probe confirmed (a) context creation, (b) both
  fragments visible in different colours, (c) the coloured set was exactly `[3,4,5,6,7]` — read back
  from each atom's `.style.stick.color` — and (d) fragment 0 stayed CPK. **Picking (in the geometry
  editor, 2.5.2) keys off
  this same 0-based `atom.index`** (never `atom.serial`): pick index = merged-xyz line = Scene global
  index = ASE mask index.
- **`zoomTo` only on composition change.** A ref holds a "composition signature" (`id:size` per
  fragment, joined — **not** coordinates). `zoomTo` fires only when that signature changes (atoms
  added/removed); a coordinate-only edit re-renders without moving the camera. This is essential for
  the 2.5.2 loop (type an angle → apply → look → adjust): a camera that re-zooms on every apply is
  unusable. The legacy `xyzData` path keeps its always-`zoomTo` behaviour.

Integrated on **NewJobScreen** as a split panel to the right of the editor
(`.editor-viewer-split`: editor `flex:2`, viewer `flex:1 min-width 260px`). The editor content is
parsed on a **500 ms debounce**; a valid block shows the molecule, otherwise a muted
"No coordinates in input" placeholder (the `MoleculeViewer` unmounts, releasing its WebGL context).

### Key decisions
- **Background** `#0d0f13` — matches the log console so the viewer sits in the dark theme.
- **Resize** via a `ResizeObserver` on the container calling `viewer.resize()` (the split panel
  changes size with the window). `automaticLayout`-style polling was avoided.
- **WebGL cleanup on unmount** — `viewer.clear()` + drop refs in the `useEffect` return. 3Dmol
  holds a WebGL context explicitly; the conditional render (mount/unmount on preview appear/
  disappear) exercises this every toggle.
- **Layout** — split-view over "viewer below editor": editing coordinates next to a live 3D
  preview is the natural pairing, and it doesn't steal the editor's height.
- **WebKitGTK compatibility (the documented watchpoint, now resolved)** — 3Dmol's default
  OffscreenCanvas render path yields a null WebGL context in WebKitGTK; `3dmol-setup.ts` forces
  the direct-canvas path. Full analysis + reusable MiniBrowser test technique in
  `debugging/002-webkitgtk-3dmol-offscreencanvas.md`.

### Atom picking (2.5.2a)
The first unit of the geometry editor: click an atom in the 3D view, and a selection
panel names it. Two **optional** props on `MoleculeViewer`:

- `onAtomPick?: (globalIndex: number) => void` — **its presence is the on-switch for
  clickability.** Absent → the viewer is display-only, byte-for-byte as before
  (`pickable = onAtomPick != null` gates the only `setClickable` call). Only
  `NewJobScreen` passes it; the Molecules screen (`xyzData` path) and the Job-detail
  conformer panel pass neither prop and are unchanged — an acceptance criterion, not
  a nicety.
- `selection?: number[]` — ordered global atom indices to highlight.

Mechanics:
- **`setClickable({}, true, cb)` is re-armed inside the model effect**, after every
  `removeAllModels`/`addModel` — the model (and the atom objects that carry the
  `clickable` flag) is rebuilt on each geometry change, so the flag must be reapplied
  or picking silently dies after the first edit.
- In the 3Dmol callback `(atom, viewer, event, container)` we read **`atom.index`**,
  never `atom.serial` (ADR-008 #3): `index` == merged-xyz line == Scene global index
  == ASE mask index, one space end to end. `onAtomPick` is read through a **ref** so
  an inline handler from the parent doesn't rebuild the model every render.
- **Highlight = translucent `addSphere`, not `setStyle`.** A style override would
  clobber the per-fragment colours (index-range styling) applied on the same model and
  we'd have to restore them by hand; a sphere sits on top and leaves them intact. The
  highlight runs in a **separate effect** keyed on `[selection, scene]` that does
  `removeAllShapes()` + `removeAllLabels()` + re-add + `render()` — **no `zoomTo`, no
  model reload**, so a selection change never moves the camera.
- On a coordinate-only edit the model effect still re-renders (new `scene` ref) so the
  sphere follows the atom to its new position; the *selection* itself is left alone by
  `NewJobScreen` (it reconsiders it only on a `compositionSignature` change — 2.5.2b via
  `selectionSurvives`).

### Measurement labels & lines (2.5.2b)
Live in the **same** highlight effect (`[selection, scene]`), drawn by `drawMeasurement`
after the halo spheres, from `measureSelection` (pure, `scene/measure.ts`):
- a **dashed `addLine`** per bond of the pick chain (2 picks → 1 line, 3 → 2, 4 → 3) and
  one value **`addLabel`** (`formatMeasurementValue`) — anchored at the bond midpoint for a
  distance, the **vertex** for an angle, the **j–k axis midpoint** for a dihedral.
- **`removeAllShapes()` / `removeAllLabels()` run BEFORE the `!scene` early return** — the
  earlier code bailed on `!scene` first, so when the last fragment was removed the halos and
  labels lingered. Clear first, then bail.
- **Labels and lines are NOT made clickable.** 3Dmol shapes/labels default non-clickable and
  we never call `setClickable` on them, so a label lying over a selected atom cannot intercept
  the pick — a repeat click still toggles that atom off (the 2.5.2a picking path is
  untouched). This is a direct regression guard for the pick-through-label case.

## Structures & trajectories
3Dmol.js viewer component (**done**, above); multiframe xyz → trajectory playback with frame
slider (Phase 3).

## Orbitals / densities
`orca_plot` in batch (non-interactive) mode generates `.cube` from `.gbw`
→ 3Dmol.js volumetric isosurface (positive/negative lobes, adjustable isovalue).
Default grid 80–100; cubes cached in job dir; generated lazily on MO selection.

## Spectra
- IR: Lorentzian broadening over (freq, intensity) list; recharts; peak click →
  animate corresponding normal mode (displacement vectors from output).
- UV-Vis (Phase 6): Gaussian broadening over TD-DFT (energy, fosc).

## Watchpoints
- **Mouse-pick path in WebKitGTK — VERIFIED (2.5.2a), was the open risk.** Forcing the
  direct-canvas path (removing OffscreenCanvas, `debugging/002`) had never been checked
  against 3Dmol's *event/ray-cast* path — if clicks didn't reach the callback, or arrived
  with shifted coordinates, the whole picking UI was moot. Re-ran the `debugging/002`
  MiniBrowser technique (`webkit2gtk-4.1/MiniBrowser`, standalone HTML, real
  `node_modules/3dmol`, `OffscreenCanvas=undefined`): a 5-atom molecule, `setClickable({},
  true, cb)` pushing `atom.index`, then for each atom **project via `modelToScreen` and
  dispatch a real `mousedown` (on the canvas) + `mouseup` (on `document.body`)** at that
  page coordinate — 3Dmol's actual handler chain (`getX` reads `ev.pageX`;
  `closeEnoughForClick` tol 5; `handleClickSelection` ray-casts). **Result: clicking atoms
  0–4 at five distinct screen points [386,123]/[211,130]/[303,271]/[375,247]/[216,231]
  returned indices 0/1/2/3/4 exactly** — not always-0, not shifted. So `setClickable` +
  the event path + `atom.index` ray-casting all work under the direct-canvas path.
  (What this does *not* exercise: OS→WebKit hardware event delivery, which isn't
  WebKit-specific and isn't the risk; the concern was 3Dmol's coordinate math post-002.)
- WebKitGTK WebGL **context creation** — RESOLVED for single molecules via `3dmol-setup.ts`
  (`debugging/002`). Still validate WebGL **performance** with a ~100-atom molecule + cube in
  Phase 2/3 (the "Apple GPU"-masked renderer string in WebKitGTK is cosmetic; direct rendering is
  on an NVIDIA GPU on the dev box).
- Cube file parsing in JS: stream-parse, don't JSON-roundtrip through the sidecar.
- Atom identity for xyz models: use `atom.index` (0-based, stable — the model's atom-array
  position, i.e. xyz line order), **not** `atom.serial` (comes from PDB-like formats, may be
  1-based or absent). Picking, measurement, and ASE masks all key off `index`.
- Multi-fragment scenes render as **one 3Dmol model, styled by atom index range** — not one
  model per fragment. A single model keeps one index space end to end (pick index = merged-xyz
  index = ASE mask index); per-model indices reset to 0 and need an extra indirection layer.
  Per-fragment colouring is index-range styling on that one model. **Implemented in 2.5.0c** and the
  `{index:[…]}` selector is WebKitGTK-confirmed (see "Multi-fragment rendering" above). See ADR-008.
