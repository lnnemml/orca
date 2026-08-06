# Module: Visualization

**Status:** the 3Dmol.js viewer is built and drives the whole geometry editor — multi-fragment
rendering (one model, index-range styling), atom picking, a d/θ/φ measurement overlay, an
edit-mask glow, per-element proportional selection halos, optional atom numbering, four background
themes, and remount-free fullscreen with a workbench rail. It also now drives the **Phase-3 results
views** — trajectory playback, normal-mode animation, and orbital isosurfaces (all **done**; the
results-side detail lives in [results-ui.md](results-ui.md), this page keeps the viewer mechanics).

## Responsibilities & scope

`src/viewer/` — everything that draws a molecule in 3D and everything that annotates it (halos,
measurement lines/labels, atom numbers, the edit-mask glow). It does **not** own geometry logic
(that's `src/scene/`) or the sidebar/rail UI state (that's `NewJobScreen`). WebGL correctness in the
WebKitGTK webview is this module's hardest constraint (see Watchpoints).

### Files
- `MoleculeViewer.tsx` — the React 3Dmol component (props + effects below).
- `3dmol-setup.ts` — side-effect module (imported by `MoleculeViewer`) that neutralises
  `OffscreenCanvas` so 3Dmol renders in WebKitGTK. Critical — see `debugging/002`.
- `fragment-colors.ts` — `FRAGMENT_PALETTE` (teal/coral/gold/violet) + `fragmentColor(i)`
  (`undefined` for fragment 0, cycling palette for 1+). One source of truth, shared with
  `FragmentList`.
- `highlight.ts` — `highlightRadius(element)` + the vdW table dup + `vdwTableDrift` (halo sizing).
- `theme.ts` — the four `ViewerTheme` presets + all colour invariants (`contrastRatio`,
  `hueDistance`, CPK dup, drift guards).

## `MoleculeViewer` — props & effects

Props: `xyzData?: string` (standard xyz) | `scene?: Scene` (`scene` wins), plus optional `style`,
`onAtomPick?`, `selection?`, `maskHighlight?`, `showAtomNumbers?`, `theme?`. With neither `scene`
nor `xyzData` it renders an empty viewer, no crash. `NewJobScreen` passes `scene`; `MoleculesScreen`
passes `xyzData` (stored xyz strings); the Job-detail conformer panel passes `xyzData`.

**Phase-3 results props** (details in [results-ui.md](results-ui.md)): `preserveCameraOnUpdate`
(trajectory / animation — redraw same-count frames without `zoomTo`); `bondTopologyReference` (freeze
bond topology from an equilibrium geometry and coordinate-update per frame — mode animation, unit
3.14); `orbitalCube` / `orbitalIsoValue` (a `.cube`'s molecule + ± phase isosurfaces, unit 3.15);
`representation` (`"stick"` | `"line"`, unit 3.16). The component is also a `forwardRef` exposing
`toPngBytes()` (3Dmol `pngURI()` readback, for PNG export). App-owned state throughout (ADR-011).

- **One `createViewer()` per mount** into a `useRef` div. A **model effect** rebuilds the model on a
  geometry change: base ball-and-stick `{stick:{}, sphere:{scale:0.3}}` on all atoms, then the
  per-fragment overrides (below). Mouse rotate/zoom/pan is 3Dmol's default.
- **Resize** via a `ResizeObserver` on the container calling `viewer.resize()` (not
  `automaticLayout` polling).
- **WebGL cleanup on unmount** — `viewer.clear()` + drop refs in the effect return; 3Dmol holds a
  WebGL context explicitly, and the conditional render (mount/unmount on preview appear/disappear)
  exercises this every toggle.
- **Layout** — on New Job the viewer is a split panel to the right of the editor
  (`.editor-viewer-split`: editor `flex:2`, viewer `flex:1 min-width 260px`), chosen over
  viewer-below-editor so editing coordinates sits next to a live 3D view without stealing editor
  height.

## Multi-fragment rendering (one model, index-range styling)

ADR-008 #2/#3: a Scene renders as **one 3Dmol model, styled by atom index range** — never
model-per-fragment (a single model keeps one index space end to end; per-model indices reset to 0
and would need an indirection layer). Geometry comes from **`buildViewerFeed(scene)`** (unit 2c1),
which returns the merged xyz **and** its `AtomId↔viewer-index` table from one pass over the same atoms
— the model 3Dmol draws and the table picks resolve through are the same object, not two states kept
in sync. `fragmentRanges(scene)` (start-inclusive, end-exclusive) gives each fragment's global range. Base CPK ball-and-stick on all
atoms, then fragments **1+** get `setStyle({index:[…]}, {stick:{color}, sphere:{scale:0.3,color}})`.
**Fragment 0 keeps CPK** — the substrate must not recolour when a reagent is added (a hard
requirement; a single-fragment scene looks identical to the plain render). The per-fragment colour
comes from the theme's palette (see Themes).

- **The `index` selector is confirmed in WebKitGTK**, not just Chromium: a water (0–2) + BH₄⁻ (3–7)
  probe in the `webkit2gtk-4.1` MiniBrowser (the `debugging/002` technique) styled exactly
  `[3,4,5,6,7]` (read back from each atom's `.style.stick.color`) — **not** `[0,1,2,3,4]`, confirming
  `fragmentRanges` end-exclusive has no off-by-one, and fragment 0 stayed CPK. 3Dmol's 0-based
  `atom.index` is the **viewer index** (= merged-xyz line); the feed's table maps it to the atom's
  stable `AtomId`, which is what leaves the viewer (2c1) — `atom.index` never escapes as an app id.
- **`zoomTo` only on composition change.** A ref holds a composition signature (`id:size` per
  fragment, **not** coordinates); `zoomTo` fires only when it changes (atoms added/removed). A
  coordinate-only edit re-renders without moving the camera — essential for the edit loop (type an
  angle → apply → look → adjust). The legacy `xyzData` path keeps its always-`zoomTo` behaviour.

## Atom picking (AtomId out, unit 2c1)

- **`onAtomPick?: (pick: AtomPick) => void` is the on-switch for clickability** — `pickable =
  onAtomPick != null` gates the only `setClickable` call; absent → display-only, byte-for-byte as
  before. Only `NewJobScreen` passes it. `AtomPick = { atomId, viewerIndex }`: `atomId` is the stable
  identity consumers key on; `viewerIndex` is the raw `atom.index`, **viewer space**, carried for
  diagnostics only — never used as an app id (that is the coupling 2c1 severs).
- **`setClickable({}, true, cb)` is re-armed inside the model effect**, after every
  `removeAllModels`/`addModel` — the atom objects that carry the `clickable` flag are rebuilt on each
  geometry change, so the flag must be reapplied or picking silently dies after the first edit. The
  table (`viewerTableRef`) is set in the **same effect run**, from the **same `buildViewerFeed`**, so
  it always names the geometry just drawn; the non-scene paths (xyz / orbital / animation) clear it,
  and it is nulled on unmount — a stale table can never resolve a pick.
- In the callback we read **`atom.index`** (never `atom.serial` — ADR-008 #3) and resolve it through
  the table: `table.atomIdAt(atom.index) → AtomId`. **Post-condition (domain rule #9):** if the table
  has no id for that drawn index, the callback emits **nothing** rather than a guessed id — an
  unresolvable click is dropped, never mapped to the wrong atom. `onAtomPick` is read through a **ref**
  so an inline parent handler doesn't rebuild the model every render.
- **The 2c1→2c2 seam:** `selection`/`measure`/`edit-plan`/`constraints` still key on the positional
  global index; `NewJobScreen` has one named adapter that converts the pick's `AtomId` back to a
  global index via `buildViewerAtomTable(scene).viewerIndexOf(...)` (TODO(2c2): deleted when the
  pipeline moves onto `AtomId`).

## The overlay effect (one owner of all shapes & labels)

Halos, measurement lines/labels, atom-number labels, and the edit-mask glow are all drawn in **one**
effect keyed on `[selection, scene, showAtomNumbers, theme]` — the **only** place that calls
`removeAllShapes()` / `removeAllLabels()` (a second owner would erase the first). It does clear →
re-add → `render()` with **no `zoomTo` and no model reload**, so a selection/number/theme change
never moves the camera. On a coordinate-only edit the model effect still re-renders (new `scene` ref)
so overlays follow atoms to their new positions.

- **Fed through the table (2c1).** The overlay builds `buildViewerAtomTable(scene)` and an `AtomId→atom`
  map, and resolves each halo/mask entry by *global index → AtomId → atom* rather than indexing the
  atom array directly. The **number** a label shows is the atom's viewer index **read from the table**
  (`table.viewerIndexOf(id)`), not the loop counter that merely coincides with it — the source is the
  table, not an accident. The value shown is unchanged (still the positional 0-based number; renaming
  the index *space* in the UI is 2c2). `drawMeasurement` is still positional — it is driven by
  `measure.ts`, a consumer frozen for 2c1 and moved to `AtomId` in 2c2.

- **Selection halo = translucent wireframe cage** (`addSphere`, not `setStyle` — a style override
  would clobber the per-fragment index-range colours). **Sized per element:** 3Dmol draws each atom
  as `sphere:{scale:0.3}` = `vdwRadii[element]*0.3` (`GLModel.getRadiusFromStyle`), so the drawn
  radius is element-dependent (H 0.36 Å, O 0.456, N 0.465, **C 0.51**). A *constant* halo radius
  (the old `0.55`) left a shell of 0.19 Å on H but **0.04 Å on C** — visible only on hydrogen.
  `highlightRadius(element)` (`highlight.ts`, pure) = `vdwRadius(element)*0.3 + 0.25`, floored at 0.5
  — a **constant 0.25 Å shell on top of the drawn radius**, so every element shows the same visible
  thickness (H 0.61 / O 0.706 / N 0.715 / C 0.76 Å). **Wireframe, not solid:** MiniBrowser
  screenshots showed a solid fill washing out over CPK red oxygen (hue clash) and thin over grey
  carbon; a wireframe cage reads on all elements. Colour and opacity come from the theme
  (`opacity 0.85`; the hue is chartreuse under the distinctness invariant — see Themes).
- **Measurement lines & labels** (`drawMeasurement`, from `scene/measure.ts`): a **dashed `addLine`**
  per bond of the pick chain (2 picks → 1 line, 3 → 2, 4 → 3) and one value **`addLabel`**
  (`formatMeasurementValue`) — anchored at the bond midpoint for a distance, the vertex for an angle,
  the j–k axis midpoint for a dihedral. **The vertex/axis is also marked geometrically** (never a
  second number — the "one number per atom = global index" rule holds): **angle** → two dashed rays
  + a solid **arc** at the vertex (`drawAngleArc`, slerped segments, radius ≤ the shorter arm, no-op
  when rays are (anti)parallel); **dihedral** → the j–k axis as a thick `addCylinder`
  (`AXIS_RADIUS = 0.05`), outer i–j / k–l bonds thin dashed.
- **`removeAllShapes()`/`removeAllLabels()` run BEFORE the `!scene` early return** — else halos and
  labels linger when the last fragment is removed. Clear first, then bail.
- **Shapes and labels are NOT clickable** — 3Dmol shapes/labels default non-clickable and we never
  `setClickable` them, so a label lying over a selected atom can't intercept the pick (repeat-click
  toggle-off is preserved). Verified in MiniBrowser: a "1" label placed over atom index 1, a real
  dispatched click still fired the callback with `atom.index === 1` (`PICKED-1`).

### Atom numbering
`showAtomNumbers?` (default **false**, so Molecules screen and the conformer panel are unchanged; a
`NewJobScreen` "Numbers" toggle drives it). **Only the GLOBAL 0-based index is ever shown in the 3D
view** — the local index stays in `AtomInspector` where the fragment gives it context (two numbers
on an atom is exactly the ambiguity the single index space removes). **Selected atoms are numbered
always**, even with the toggle off. `showAtomNumbers` is in the overlay effect's deps but **not** the
model effect's, so toggling redraws labels only — no model reload, no `zoomTo`.

### Edit-mask "will-move" glow
`maskHighlight?: number[]` — the atoms an edit would move (passed by `NewJobScreen` while `planEdit`
is `ready`). Drawn as a **solid translucent sphere** (`MASK_OPACITY = 0.22`, radius = the halo
radius + a small boost), drawn FIRST so the crisp selection cage sits on top where they overlap. The
halo says "I picked this" (a wireframe cage); the mask says "this will move" (a soft fill over the
moving fragment). **Distinctness — the corrected rule:** the ≥30°-hue invariant governs overlays that
mark **different** atoms (halo vs element colour, measurement vs element colour). The halo and the
mask **coexist on the SAME atom by construction** (the last-clicked atom is always in the mask), so
they are distinguished by **FORM** (cage vs fill) + lightness, and only **contrast against the
background** is required of them. The mask therefore reuses `theme.haloColor` — that is the rule, not
an exception (corrected in `[2026-07-29] 2.5.2d-1`; `theme.test.ts` splits the two overlay classes).

## Themes & colour (`theme.ts`)

Four `ViewerTheme` presets — `dark` (default, the pre-theme look exactly), `black`, `light`, `white`.
`MoleculeViewer` reads the background and every overlay colour from the theme. Background is set via
`setBackgroundColor(bg, 1)` in its own `[theme]` effect (no `addModel`, no `zoomTo`); the model
effect does **not** depend on `theme`, so a theme change never rebuilds the model. The theme is
persisted under `settings.viewer_theme`.

- **Presets, not a free colour picker — because the safety property is a testable contrast
  invariant.** `contrastRatio` (WCAG relative luminance, pure) lets `theme.test.ts` assert **3:1**
  against the background for three on-canvas colour families: the **overlay** (halo, label text,
  measurement line/text), the **fragment palette**, and the **CPK element colours**. A free picker
  could make any of them vanish and no test could catch an arbitrary runtime colour.
- **CPK element-colour overrides on light themes.** Fragment 0 is drawn in CPK element colours, and
  CPK hydrogen is **white** — on the white theme every hydrogen vanished. `ViewerTheme.elementColorOverrides`
  is a per-element map merged OVER 3Dmol's default colours: **empty on dark** (its look is untouched,
  byte-identical — with no overrides `cpkBaseStyle` returns the exact old `baseStyle()` object), and
  on light/white it covers the **13** elements that fail 3:1 against `#eceff3` (`H He B C N F Si P S
  Cl Fe Ba Au`), each the **same hue, darker** (H and C become greys), tuned to ≥3.2:1. Applied via
  `{ colorscheme: { prop: "elem", map: { ...elementColors.defaultColors, ...overrides } } }` (the
  merge is why partial overrides keep every other element's CPK colour).
  - Failing-contrast values (light `#eceff3` / white `#ffffff`): H 1.15/1.00, He 1.33/1.54, B
    1.19/1.37, C 1.45/1.67, N 2.41/2.78, F 1.94/2.24, Si 1.94/2.24, P 1.71/1.97, S 1.34/1.55, Cl
    1.19/1.37, Fe 1.71/1.97, Ba 1.71/1.97, Au 1.94/2.24 — all overridden to ≥3.2 vs `#eceff3`.
- **Fragment palette is a per-theme property** (`ViewerTheme.fragmentPalette`), read by
  `MoleculeViewer`; **`FragmentList` stays on the global `FRAGMENT_PALETTE`** (its swatches sit on
  the dark side panel, where the bright colours read). dark/black: `fragmentPalette ===
  FRAGMENT_PALETTE`; light/white: the same four **hues** at lower lightness
  (`#0f766e #e11d48 #a16207 #7c3aed`), each ≥3:1 on both light backgrounds. Named compromise: on a
  light theme the sidebar swatch (bright) and the viewer fragment (darker) are the same hue at
  different brightness — identity rides on hue, legibility on lightness; the alternative is an
  invisible fragment. `theme.test.ts` locks the light/white hue within **±15°** of its dark
  counterpart.
- **A distinctness invariant, not only contrast.** 3Dmol's default CPK is `elementColors.rasmol` (28
  elements, = `defaultColors`), which has **no Pd/Pt/Rh/Ru** — 3Dmol paints them
  `elementColors.defaultColor` **#ff1493** (deep pink), exactly ADR-007's cross-coupling metals, and
  the old pink halo sat `hueDistance` 1.05 from it (every metal looked permanently selected). Fixes:
  the metals are added to `CPK_ELEMENT_COLORS` from 3Dmol's own **`Jmol`** table (Pd #006985, Pt
  #d0d0e0, Rh #0a7d8c, Ru #248f8f, Ir #175487, Os #266696 — still library colours, source named per
  value; Pt near-white also gets a light-theme override); and `theme.test.ts` asserts, per theme,
  that the halo and measurement colour sit **≥30°** in hue (`hueDistance`, 0–180°) from every element
  colour (CPK *with* overrides), from `defaultColor` #ff1493, and from every fragment-palette colour.
  The **only** hue band clear of the whole wheel is **chartreuse, 74–90°** (CPK fills red 0, gold
  35–48, green 120, teal/cyan/blue metals 172–207, blue 240, purple 255–277, pink 328–351), so halo
  = **#adee2b/#5e8b04** (dark/light) and measurement = **#b1eb70/#519504**, both ≥3:1 on all four
  backgrounds.
- **Both duplicated element tables are guarded, two-directionally.** `CPK_ELEMENT_COLORS`
  (`elementColors.rasmol` transcribed verbatim + the Jmol metals) and the vdW table (`highlight.ts`,
  `GLModel.vdwRadii` transcribed v2.5.5, H–Kr + Pd/Pt, off-table → 1.5 Å = `defaultSphereRadius`) are
  documented dups because the 3dmol bundle needs `window`/`document` and can't load under the node
  test runner. `cpkColorDrift` / `vdwTableDrift` return `{ changed, missing }` and
  `MoleculeViewer` calls them in dev against the live tables (`console.warn`). **`missing`** —
  elements the live 3Dmol table HAS but our copy LACKS — is the direction a one-way guard is blind to
  by construction (it iterates our keys), and the direction that would have caught the missing metals.
  PDB uppercase aliases (`HE`, `LI`) are ignored; Jmol-sourced metals absent from `defaultColors` are
  not flagged as `changed`.

### Two 3Dmol colour-scheme facts (cost time, pinned)
- A custom `{prop:'elem', map}` colorscheme is honoured by the **"discrete property mapping" branch**
  of `getColorFromStyle` (`scheme.prop && scheme.map → map[atom[prop]]`). The *earlier* branch
  `scheme[atom[scheme.prop]]` is **always false** for the `{prop,map}` object shape (it looks up
  `scheme["Pd"]`, which the object doesn't have) — named schemes (strings like `"greenCarbon"`) take
  that first branch instead.
- 3Dmol's XYZ parser canonicalises the symbol as `elem[0].toUpperCase() + substring(1).toLowerCase()`
  — identical to our `normalizeElement`. So `atom.elem` is proper-case (`Cl`, `Pd`) and our map keys
  match with no case dance; the uppercase `rasmol` aliases (`HE`, `LI`, `NA`) are PDB-only and
  irrelevant (the drift guard skips them). Our `vdwRadius` also normalises up front, so it matches for
  any casing without copying 3Dmol's second case-normalised lookup.

## Fullscreen (no remount) + the workbench rail

Fullscreen changes **only a CSS class** — the `.viewer-column` (viewer panel + a `.viewer-rail`
holding AtomInspector + FragmentList, one DOM structure) goes `position: fixed; inset: 0`.
`MoleculeViewer` and the rail keep their React tree positions (the same `scene ?` branch renders
whether or not fullscreen), so React never remounts them. **This is deliberate:** a remount would
`viewer.clear()` + re-`createViewer` (context re-init is WebKitGTK's fragile spot, `debugging/002`)
and reset the camera via `zoomTo` exactly when the user enlarged to look closer.

- **Normal mode** = a flex column (viewer over rail — visually unchanged). **Fullscreen** = a flex
  row (viewer `flex:1`, rail `flex:0 0 320px` on the right, collapsible via `.viewer-rail-collapsed
  → display:none`, not persisted).
- **The rail is one shared instance** — never a duplicate `AtomInspector` (that would fork selection
  state); it's a section list so later units *add* a section, not reflow.
- **Remount witnesses** (dev): a module-level `viewerCreateCount` (`console.debug` on every
  `createViewer`) — a fullscreen toggle must not tick it. Proven in MiniBrowser: a `position:fixed`
  class toggle with a `getView()` snapshot reported `RO-fired=2 cameraSame=true maxDelta=0.00e+0`
  (the `ResizeObserver` fires, so `viewer.resize()` runs without an explicit call, and the camera view
  matrix is bit-identical); the rebuilt row layout was re-proven fresh by checking **canvas DOM node
  identity** (`sameCanvas=true cameraSame=true`) — a remount would replace the node and tear down the
  WebGL context.

## Watchpoints

- **Mouse-pick path in WebKitGTK — VERIFIED.** Forcing the direct-canvas path (removing
  OffscreenCanvas, `debugging/002`) had never been checked against 3Dmol's *event/ray-cast* path.
  The `debugging/002` MiniBrowser technique (`webkit2gtk-4.1/MiniBrowser`, standalone HTML, real
  `node_modules/3dmol`, `OffscreenCanvas=undefined`): a 5-atom molecule, `setClickable({}, true, cb)`
  pushing `atom.index`, then for each atom **project via `modelToScreen` and dispatch a real
  `mousedown` (on the canvas) + `mouseup` (on `document.body`)** at that page coordinate — 3Dmol's
  actual chain (`getX` reads `ev.pageX`; `closeEnoughForClick` tol 5; `handleClickSelection`
  ray-casts). Clicking atoms 0–4 at five distinct screen points
  [386,123]/[211,130]/[303,271]/[375,247]/[216,231] returned indices 0/1/2/3/4 exactly — not
  always-0, not shifted. (Does **not** exercise OS→WebKit hardware event delivery, which isn't
  WebKit-specific.)
- **WebKitGTK WebGL context creation** — RESOLVED for single molecules via `3dmol-setup.ts`
  (`debugging/002`, the OffscreenCanvas neutralisation). Still validate WebGL **performance** with a
  ~100-atom molecule + cube in Phase 2/3 (the "Apple GPU"-masked renderer string in WebKitGTK is
  cosmetic; direct rendering is on an NVIDIA GPU on the dev box).
- **recharts** `ResponsiveContainer` measures 0×0 in WebKitGTK — the convergence dashboard passes an
  explicit pixel width instead (see `modules/frontend.md`); same webview-mismeasurement class as the
  3Dmol OffscreenCanvas and `<select>` bugs.
- **Cube file parsing in JS:** stream-parse, don't JSON-roundtrip through the sidecar.
- **Atom identity:** use `atom.index` (0-based, stable — the model's atom-array position = xyz line
  order), **not** `atom.serial` (comes from PDB-like formats, may be 1-based or absent). Picking,
  measurement, and ASE masks all key off `index`.

## Structures & trajectories (unit 3.8 — done)
Trajectory playback is built — see [results-ui.md](results-ui.md). The viewer stays a **dumb
renderer**: it is fed **one frame** at a time; the current frame number and the play timer are
application state in `TrajectoryPlayer`, **not** 3Dmol's frame apparatus (ADR-011). The only viewer
change is the opt-in **`preserveCameraOnUpdate`** prop — an `xyzData` change that keeps the same atom
count redraws without `zoomTo` (the camera holds through playback); a count change still zooms.
Default false, so the Molecules/preview path is unchanged.

## Orbitals / densities (unit 3.15 — DONE)
`orca_plot` (driven over stdin, not the unusable `plot-inputfile` mode — measured, `orca/orca-plot.md`)
generates a `.cube` from `.gbw`; the viewer draws the molecule from the cube + two ± phase isosurfaces
(the sign is the wavefunction **phase, not charge**), with an isovalue slider. Default grid 80³
(measured ≈6.9 MB); cubes cached in the job dir, never in the DB; generated lazily on MO selection.
The `.cube` string is parsed once into a `VolumeData`, so an isovalue change redraws only the
surfaces. WebKitGTK renders the isosurface — gated via MiniBrowser (`debugging/002` technique).
Detail: [results-ui.md](results-ui.md). Density cubes / the MO-coefficient route are **not** done.

## Spectra (units 3.8–3.12 — IR + mode animation DONE)
IR: Lorentzian broadening over (freq, intensity) → recharts, in `src/spectrum/` — see
[results-ui.md](results-ui.md). Peak click selects the frequency-table row (and vice-versa), and
**animates the normal mode** (unit 3.12): the `.hess` frame was proven a pure translation of the
reference (Kabsch gate), so modes are added as-is; phase/amplitude/timer are app state, the viewer
gets one frame with a **frozen** bond topology. UV-Vis (Phase 6): Gaussian broadening over TD-DFT
(energy, fosc) — see the ROADMAP note.
