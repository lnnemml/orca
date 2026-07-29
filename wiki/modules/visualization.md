# Module: Visualization

**Status:** 3Dmol.js component built (Phase 2.1); **multi-fragment rendering added (2.5.0c)** — one
model, index-range styling, shared `fragmentColor` palette. **Atom picking added (2.5.2a)** — mouse
click → `atom.index` → selection panel, WebKitGTK-confirmed. **Measurement labels/lines added
(2.5.2b)** — dashed bond lines + a d/θ/φ value label in the highlight effect, non-clickable.
**Halo made proportional + atom numbering (2.5.2e-1)** — wireframe halo sized per element,
optional global-index labels. **Themes + fullscreen + measurement vertex marking (2.5.2e-2)** —
overlay colours move to the theme, background presets with a contrast invariant, remount-free
fullscreen, angle arc / dihedral axis. **Light-theme legibility (2.5.2e-3a)** — CPK element-colour
overrides + per-theme fragment palette (widened contrast invariant), element-selector `<select>`
fix, round swatches. **Colour distinctness + workbench rail (2.5.2e-3b)** — ADR-007 metals in the
CPK table, chartreuse halo (≥30° from every element hue), two-directional drift guards, one-panel
fullscreen rail. Trajectories / orbitals / spectra not started.

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

### Proportional selection halo (2.5.2e-1)
The manual check found the selection nearly invisible except on hydrogen. **Root cause was not
"radius too small" — it was a constant radius.** 3Dmol draws each atom as `sphere:{scale:0.3}`,
i.e. `vdwRadii[element] * 0.3` (`GLModel.getRadiusFromStyle`), so the drawn radius is
element-dependent: H 0.36 Å, O 0.456, N 0.465, **C 0.51**. The old halo was a *constant*
`HIGHLIGHT_RADIUS = 0.55`, so the visible shell outside the atom was 0.19 Å on H but **0.04 Å on
C** — visible only on hydrogen. A bigger constant is not a fix (huge on H, still thin on C).

- **Fix: `highlightRadius(element)` in `src/viewer/highlight.ts`** (pure, node-tested) =
  `vdwRadius(element) * 0.3 + 0.25`, floored at 0.5. The **constant 0.25 Å shell** is added on
  top of the *drawn* radius, so every element shows the same visible halo thickness — the whole
  point.
- **The vdW table is a documented DUPLICATE of 3Dmol's `GLModel.vdwRadii`** (transcribed
  verbatim, v2.5.5). We copy rather than `import { GLModel } from "3dmol"` because the 3dmol
  bundle needs `window`/`document` and **cannot load under the node test runner** (the suite has
  no jsdom, deliberately). Coverage matches 3Dmol's: H–Kr **plus Pd(46) and Pt(78)** — the
  cross-coupling metals ADR-007 names — and the rest of 3Dmol's list. Off-table elements fall
  back to **1.5 Å = 3Dmol's `defaultSphereRadius`**, so their halos still track the drawn sphere;
  never NaN or a silent zero. **Drift guard:** `vdwTableDrift(referenceTable)` (pure) lists any
  element where our copy disagrees; `MoleculeViewer` calls it once in dev with the live
  `GLModel.vdwRadii` and `console.warn`s — the active check the dup needs, run in the real webview
  where 3Dmol IS loaded. **Scope of the guard:** it compares the table's *values*, not 3Dmol's
  *lookup logic* — `getRadiusFromStyle` also does a second, case-normalised lookup for two-letter
  symbols (`"CL"`→`"Cl"`) before falling back. We don't reproduce that branch; our own
  `vdwRadius` normalises the symbol up front (via `normalizeElement`, same first-upper-rest-lower
  rule), so the value matches for any casing without copying the fallback dance.
- **Wireframe, not a solid translucent sphere.** MiniBrowser screenshots (the `debugging/002`
  technique, H·C·N·O in one frame) showed the solid magenta halo washing out over **CPK red
  oxygen** (hue clash) and thin over **grey carbon**, while a wireframe cage reads on all four.
  Colour `#ff2d95` (saturated magenta) — **not `#ffffff`** (that is CPK hydrogen and would vanish
  on the light background e-2 adds), and distinct from the amber measurement colour and the
  fragment palette. `opacity 0.85`.

### Atom numbering (2.5.2e-1)
Optional prop `showAtomNumbers?: boolean` (default **false**, so Molecules screen and the
Job-detail conformer panel are unchanged). A `NewJobScreen` "Numbers" toggle drives it.
- **Only the GLOBAL 0-based index is ever shown in the 3D view.** The local index stays in
  `AtomInspector`, where the fragment gives it context — two numbers on an atom would reintroduce
  exactly the ambiguity the single end-to-end index space exists to remove.
- **Selected atoms are numbered ALWAYS**, even with the toggle off, so a pick is legible the
  instant it happens.
- **One effect owns all shapes AND labels.** Halos, measurement lines/labels, and number labels
  are drawn in the **same** `[selection, scene, showAtomNumbers]` overlay effect — the only place
  that calls `removeAllShapes`/`removeAllLabels`. A second effect calling those would erase this
  one's work. `showAtomNumbers` is in this effect's deps but **not** the model effect's, so
  toggling Numbers redraws labels only — no `addModel`, no `zoomTo`, no camera move.
- **Number labels are non-clickable** — same rule and mechanism as the measurement labels.
  **Verified empirically (MiniBrowser):** a probe placed the "1" label over atom index 1, armed
  `setClickable`, and dispatched a real click at that atom (project via `modelToScreen`, then
  `mousedown` on canvas + `mouseup` on body — the 2.5.2a event technique). The callback fired with
  `atom.index === 1` (window title `PICKED-1`): the label did not intercept the pick.

### Themes — overlay colours belong to the theme (2.5.2e-2)
`src/viewer/theme.ts` (pure, node-tested) defines four `ViewerTheme` presets (`dark`, `black`,
`light`, `white`); `MoleculeViewer` takes a `theme` prop (default `dark`).
- **Overlay colours are the theme's, not the module's.** 2.5.2e-1 hard-coded `NUMBER_BG =
  "#0d0f13"`, `HALO_COLOR = "#ff2d95"`, the amber measurement colour — all tuned for a near-black
  background. On a light background those become dark label rectangles and low-contrast marks, so
  the whole overlay palette (halo, label text/bg, measurement line/text) now travels with the
  theme. **`dark` reproduces the pre-2.5.2e-2 look exactly**, so it's a no-op default.
- **Background via `setBackgroundColor(bg, 1)` in its own `[theme]` effect** — no `addModel`, no
  `zoomTo`. The overlay effect gains `theme` in its deps so halos/labels/measurement recolour with
  it. The model effect does NOT depend on `theme`, so a theme change never rebuilds the model.
- **The contrast invariant is why we use PRESETS, not a free colour picker.** `contrastRatio`
  (WCAG relative luminance, pure) lets `theme.test.ts` assert **3:1** against the background for
  **three** on-canvas colour families (widened in 2.5.2e-3a): the **overlay** (halo, label text,
  measurement line/text), the **fragment palette**, and the **CPK element colours**. A free
  background picker could make any of them vanish and no test could catch an arbitrary runtime
  colour; presets keep the guarantee testable.

### CPK element-colour overrides on light themes (2.5.2e-3a)
The e-2 test measured only `FRAGMENT_PALETTE` (fragments 1+) — but **fragment 0 is drawn in CPK
element colours, and CPK hydrogen is white.** On the white theme every hydrogen vanished (the BH₄⁻
screenshot — one molecule, one fragment, all CPK, the commonest case). The test passed honestly; it
measured the wrong thing. The fix widens the invariant, it doesn't rewrite it.
- **`ViewerTheme.elementColorOverrides`** — a per-element map merged OVER 3Dmol's default element
  colours. **Empty for the dark themes** (their look is untouched, not a pixel). For light/white it
  covers the **13 elements** whose default colour fails 3:1 against `#eceff3` (the harder of the two
  light backgrounds): `H He B C N F Si P S Cl Fe Ba Au`. Each override is the **same hue, darker**
  (H and C become greys); tuned to ≥3.2:1 vs `#eceff3`, which also clears white.
- **Applied via 3Dmol `colorscheme`.** `MoleculeViewer`'s base style becomes
  `{ colorscheme: { prop: "elem", map } }` where `map = { ...elementColors.defaultColors,
  ...theme.elementColorOverrides }` (the merge is why partial overrides keep every other element's
  CPK colour). Confirmed against the bundle — 3Dmol's default CPK scheme is `elementColors.rasmol`
  (aliased by `defaultColors`), and a `{prop,map}` colorscheme is honoured. With **no** overrides
  the function returns the exact old `baseStyle()` object, so the dark path is byte-identical.
  **MiniBrowser-verified:** BH₄⁻ on white renders all four H grey and B dark-green, legible.
- **The CPK table is a documented DUPLICATE**, same as the vdW table: `CPK_ELEMENT_COLORS` in
  `theme.ts` is `elementColors.rasmol` transcribed verbatim (the 3dmol bundle can't load under the
  node test runner), **plus the ADR-007 metals from 3Dmol's `Jmol` table** (see below).
  `cpkColorDrift(reference)` (pure) guards it; `MoleculeViewer` calls it in dev with the live
  `elementColors.defaultColors`. The contrast overrides are computed against these values, so a
  3Dmol change must be noticed.

### Element-colour distinctness + the ADR-007 metals (2.5.2e-3b)
A review finding, verified on the bundle: 3Dmol's `rasmol` (28 elements, = `defaultColors`) has
**no Pd/Pt/Rh/Ru**, so 3Dmol painted them `elementColors.defaultColor` **#ff1493** (deep pink) —
`hueDistance` **1.05** from the old halo `#ff2d95`, so every metal atom looked permanently selected.
These are exactly ADR-007's cross-coupling metals. Two fixes:
- **Metals added to `CPK_ELEMENT_COLORS` from 3Dmol's own `Jmol` table** (Pd #006985, Pt #d0d0e0,
  Rh #0a7d8c, Ru #248f8f, Ir #175487, Os #266696) — still the library's dictionary, no invented
  colours; each value's source (`rasmol`/`Jmol`) is named in a comment. Pt (near-white) is the one
  that also fails the light-theme contrast floor, so it gets an override like the other elements.
- **A distinctness invariant, not just contrast** (`hueDistance`, 0–180°). `theme.test.ts` asserts
  that in **every** theme the halo and the measurement colour sit **≥30°** in hue from every element
  colour (CPK *with* the theme's overrides), from `defaultColor` #ff1493, and from every
  fragment-palette colour. The old pink halo fails this; the search showed the **only** hue band
  clear of the entire CPK/palette wheel is **chartreuse, 74–90°** (CPK fills red 0, gold 35–48,
  green 120, teal/cyan/blue metals 172–207, blue 240, purple 255–277, pink 328–351). So halo and
  measurement moved there — **halo `#adee2b`/`#5e8b04`** (dark/light), **measurement
  `#b1eb70`/`#519504`** — still ≥3:1 on all four backgrounds. This is the same doctrine as contrast:
  the test states the requirement, the constant satisfies it. (This is the one e-3b change to the
  dark theme; its background, CPK and palette are untouched.)
- **Two-directional drift guards.** `cpkColorDrift` and `vdwTableDrift` now return
  `{ changed, missing }`. `missing` — elements the live 3Dmol table HAS but our copy LACKS — is the
  direction the one-way guards were blind to *by construction* (they iterated our keys), and the
  direction that would have caught the missing metals. PDB uppercase aliases (`HE`, `LI`) are
  ignored; Jmol-sourced metals absent from `defaultColors` are not flagged as `changed`.

### Two 3Dmol facts that cost time (2.5.2e-3b)
- **A custom `{prop:'elem', map}` colorscheme is honoured by the "discrete property mapping" branch
  of `getColorFromStyle`** (`scheme.prop && scheme.map` → `map[atom[prop]]`). The *earlier* branch
  `scheme[atom[scheme.prop]]` is **always false** for the `{prop,map}` object shape (it looks up
  `scheme["Pd"]`, but our object only has `prop`/`map` keys) — so don't expect that branch to fire;
  the discrete-mapping branch below it is the one that colours atoms. Named schemes (strings like
  `"greenCarbon"`) take the first branch instead.
- **3Dmol's XYZ parser canonicalises the element symbol** as
  `elem[0].toUpperCase() + elem.substring(1).toLowerCase()` — identical to our `normalizeElement`.
  So `atom.elem` is always proper-case (`Cl`, `Pd`) and our proper-case map keys match without a
  case dance. The uppercase aliases (`HE`, `LI`, `NA`) in `rasmol` exist for PDB-style inputs and
  are irrelevant to us — we neither copy nor need them (and the drift guard skips them).
- **`theme.test.ts` invariants:** dark/black overrides are empty; light/white override **exactly**
  the computed failing set (no gaps, no redundant entries); every shipped override clears 3:1; each
  overridden element genuinely failed first; hue stays recognisable (≤25° from CPK, greys aside).

### Fragment palette as a theme property (2.5.2e-3a)
The four fragment colours are now `ViewerTheme.fragmentPalette`, and `MoleculeViewer` reads the
palette from the theme (not the global constant). **`FragmentList` stays on the global
`FRAGMENT_PALETTE`** — its swatches sit on the app's dark side panel, where the bright colours read.
- dark/black: `fragmentPalette === FRAGMENT_PALETTE` (unchanged);
- light/white: the same four **hues** at lower lightness (`#0f766e #e11d48 #a16207 #7c3aed`), each
  ≥3:1 on both light backgrounds. **Named compromise:** on a light theme the sidebar swatch and the
  viewer fragment are the *same hue at different brightness* — identity rides on hue, legibility on
  lightness. The alternative (leave the palette bright) is an invisible fragment, which is worse.
- **`theme.test.ts`:** every theme's four palette colours clear 3:1 vs its background; the
  light/white hue stays within **±15°** of its dark counterpart (the invariant that guards "same
  colour, darker" — not a literal). The global `FRAGMENT_PALETTE` constant is unchanged.

### Fullscreen — no remount, camera survives (2.5.2e-2)
A `.viewer-panel-fullscreen` class puts the viewer container `position: fixed; inset: 0`. **Only
the container's className changes** — `MoleculeViewer` keeps its position in the React tree (the
same `scene ?` branch renders it whether or not fullscreen), so React never remounts it. This is
deliberate: a remount would `viewer.clear()` + re-`createViewer`, and **context re-init is the
fragile spot in WebKitGTK** (`debugging/002`), plus the camera would reset via `zoomTo` exactly
when the user enlarged the view to look closer. No Fullscreen API, no conditional JSX with
`MoleculeViewer` in two different tree positions.
- **Remount witness:** a module-level `viewerCreateCount`, `console.debug`'d in dev on every
  `createViewer`. A fullscreen toggle must not tick it.
- **Resize:** the existing `ResizeObserver` on the inner container fires on the size change and
  calls `viewer.resize()` — no explicit call needed. **Verified in MiniBrowser** (the
  `debugging/002` technique): a probe with a `getView()` snapshot, then a `position: fixed` class
  toggle, reported window title `RO-fired=2 cameraSame=true maxDelta=0.00e+0` — the observer fired
  and the camera view matrix was **bit-identical** before and after (the `getView` array is the
  concrete proof, not "looks the same").

### Fullscreen workbench rail — one panel, two modes (2.5.2e-3b)
The fixed class moved from `.viewer-panel` to `.viewer-column` (renamed conceptually to the
workbench): the column now holds **both** the viewer panel AND the geometry rail (AtomInspector +
FragmentList) in **one DOM structure** for both modes. Normal: a flex **column** (viewer over rail
— the old right column, visually unchanged). Fullscreen: `position: fixed`, a flex **row** (viewer
`flex:1`, rail `flex:0 0 320px` on the right). **Only classNames change**, so — critically — the
layout rebuild does NOT move `MoleculeViewer` or the rail in the tree, and neither remounts.
- **The rail is a single instance shared by both modes** — never duplicated. Two `AtomInspector`s
  would be two copies of selection state and double maintenance of every future section (edit mode,
  constraints, xTB); the rail is a section list designed so those units *add* a section, not
  reflow. Collapsible in fullscreen (`.viewer-rail-collapsed` → `display:none`) for a clean canvas;
  the collapse state is not persisted.
- **No-remount re-proven AFTER the layout rebuild** (not inherited from e-2 — a layout change is
  exactly what could silently reintroduce a remount). A fresh MiniBrowser probe drove the new
  operation — a flex container toggled to `position: fixed; flex-direction: row` with a sibling
  rail becoming the right column — and checked the **canvas DOM node identity** before/after (a
  React remount would replace the node, tearing down the WebGL context). Title:
  `sameCanvas=true cameraSame=true RO=2 maxD=0.0e+0` — same canvas node, `getView` bit-identical,
  ResizeObserver fired. The in-app witness stays `viewerCreateCount` (dev log; a fullscreen toggle
  must not tick it).

### Measurement vertex marking (2.5.2e-2)
All halos are identical, so the pick that is the angle vertex / dihedral axis wasn't visible.
`drawMeasurement` now marks it **geometrically** (never a second number — the "one number per atom
= global index" rule from e-1 holds; click order is shown by geometry, not digits):
- **angle:** two dashed rays + a solid **arc** at the vertex (`drawAngleArc` — a fan of short line
  segments slerped along the great circle from one ray to the other, radius scaled to the shorter
  arm so it never overshoots a bond; no-op when the rays are (anti)parallel);
- **dihedral:** the **j–k axis** is a thick solid `addCylinder` (radius `AXIS_RADIUS = 0.05`), the
  outer i–j / k–l bonds stay thin dashed lines — the rotation axis reads at a glance.

### Edit-mask "will-move" glow (2.5.2d)
`MoleculeViewer` takes a `maskHighlight?: number[]` — the atoms an edit would move. They're drawn
as a **solid translucent sphere** (`MASK_OPACITY = 0.22`, radius = the halo radius + a small boost),
drawn FIRST so the crisp selection cage sits on top where they overlap. The halo says "I picked
this" (a wireframe cage on 2–4 atoms); the mask says "this will move" (a soft fill over the whole
fragment). `NewJobScreen` passes the mask only while `planEdit` is `ready`.
- **Distinctness — the mask shares the halo hue by necessity, distinguished by FORM.** The
  distinctness invariant wants the mask ≥30° in hue from every element colour, the palette,
  `defaultColor` #ff1493, **and the halo**. It is provably impossible to clear all of those at once:
  the ONLY hue band ≥30° from the whole CPK/palette wheel is chartreuse (74–90°) — and that band IS
  the halo's. `theme.test.ts` locks this (`no hue clears elements/palette/default AND the halo`,
  per theme). So the mask **reuses `theme.haloColor`** and is set apart by **form** (solid glow vs
  wireframe cage) and by coverage (whole fragment vs the picked atoms), not by hue. This is a
  reported, deliberate exception to the "distinct hue" rule, surfaced to the architect rather than
  faked.

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
