# Module: Visualization

**Status:** 3Dmol.js component built (Phase 2.1). Trajectories / orbitals / spectra not started.

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
