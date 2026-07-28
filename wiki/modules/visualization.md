# Module: Visualization

**Status:** 3Dmol.js component built (Phase 2.1); **multi-fragment rendering added (2.5.0c)** — one
model, index-range styling, shared `fragmentColor` palette. Trajectories / orbitals / spectra not
started.

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
