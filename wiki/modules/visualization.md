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
`onAtomPick?`, `selection?`, `maskHighlight?`, `clashHighlight?`, `axisHighlight?`, `ephemeralScene?`, `moveMode?`, `onFragmentDrag?`, `showAtomNumbers?`, `theme?`. With neither `scene`
nor `xyzData` it renders an empty viewer, no crash. `NewJobScreen` passes `scene`; `MoleculesScreen`
passes `xyzData` (stored xyz strings); the Job-detail conformer panel passes `xyzData`.

**Phase-3 results props** (details in [results-ui.md](results-ui.md)): `preserveCameraOnUpdate`
(trajectory / animation — redraw same-count frames without `zoomTo`); `bondTopologyReference` (freeze
bond topology from an equilibrium geometry and coordinate-update per frame — mode animation, unit
3.14); `orbitalCube` / `orbitalCubes` / `orbitalIsoValue` / `orbitalWireframe` (one `.cube` — or an
ARRAY of them, F2 — drawn as molecule + ± phase isosurfaces, solid or wireframe mesh (F2b), unit 3.15;
`orbitalCube` is the single-orbital alias);
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
- **Consumers key on `AtomId` (2c2):** the pick's `atomId` flows straight into an `AtomId[]`
  selection; `measure`/`planEdit` input/`constraintFromSelection` input all take ids. The 2c1→2c2
  adapter is gone. The ASE mask and the `%geom` constraint stay positional at their own emit seams
  (see `wiki/modules/scene.md`).

## Rigid-body drag — "Move mode" (unit 3.1; Stage 3.x; ADR-010 ephemeral layer)

When `moveMode` is on (scene path, pickable, with `onFragmentDrag` wired), a mouse-drag that STARTS on
an atom moves a **moving set** decided by THE ONE RULE (`resolveMovingSet`, `scene/moving-set.ts`; see
`editor-ui.md`) rigidly **in the plane of the screen at 60fps**. The rule, read live on mousedown from
the `moveGranularity` prop (the **"Move: Fragment | Selection"** rail toggle) and the current `selection`:
an explicit **selection** moves exactly those atoms (even across fragments); else **Fragment** moves the
whole grabbed fragment **synchronously** (no sidecar); else **Selection** moves the grabbed atom's
**perceived connected component**, resolved **once on mousedown** (never per frame) by asking the sidecar
`/geometry/connected-component` for the dragged fragment's own xyz + the grabbed atom's fragment-local
index. The moving set is held as a `Set` of **viewer indices** (`resolveMovingSet` → `globalIndexOfAtom`);
everything else stays pinned at its pre-drag coords. So under the Selection toggle, after a bond is
**broken** the two pieces drag independently; a **fully-bonded fragment's component is the whole
fragment**, so an intact molecule drags exactly as before. The entire drag is a **viewer-only ephemeral
overlay** — the Scene/store is untouched until release, when exactly ONE **`translate-atoms`** op is
committed with the moving set's AtomIds + total delta (ADR-010: 60fps motion is not logged; one op, one
Undo). Fragment placement and an explicit selection resolve synchronously; **only** the Selection-toggle
component path hits the sidecar, so **until that async resolve returns** (or if it fails) the moving set
defaults to the whole fragment — a `settled` flag stops a late resolve touching a finished drag, and a
**failed** call falls back to a whole-fragment move **plus an honest banner** (`onFragmentDragFallback`),
never a silent wrong move. The pure accumulate/commit logic is `src/viewer/fragment-drag.ts` (`makeDragController`),
unit-tested without jsdom (the same split as `syncMonacoToScene` in 2d) — its contract is unchanged
(the component/moving-set logic lives in the viewer's hook closures, not the controller); the
3Dmol/mouse wiring is a separate effect in `MoleculeViewer`, keyed `[moveMode, pickable, scene, theme]`.

- **Camera suppression.** 3Dmol binds `mousedown` on its canvas (`glDOM`). The Move effect binds
  `mousedown` on the **container in the CAPTURE phase**, so an atom-grab runs first and
  `stopPropagation()`s — 3Dmol's canvas mousedown never fires, and its rotate (gated on state that
  mousedown sets) is suppressed for the drag. A drag on **empty space** is not stopped → 3Dmol rotates
  as usual; a **click** (movement under `DRAG_THRESHOLD_PX`) picks the grabbed atom (we intercepted
  3Dmol's own click, so the pick is re-emitted through `viewerTableRef`). `mousemove`/`mouseup` are on
  `window` for the drag's life so it survives the cursor leaving the canvas; a mid-drag Move-off /
  unmount cancels cleanly via a tracked cleanup ref.
- **screen→world, pixel-exact.** `screenOffsetToModel(dxPx, dyPx, modelz)` turns the pixel delta into a
  world delta in the screen plane; `modelz = grabbedAtomDepth(atom)` (the atom's `modelGroup.matrixWorld`
  z) makes the grabbed atom track the cursor to **0 px** at any camera orientation (measured,
  `debugging/013`). The internal-API reach falls back to the default depth (≤~6% lag) if a 3Dmol upgrade
  moves it — accuracy only, never a crash.
- **Ephemeral coordinate-update reuses the frozen-topology path (unit 3.14).** `showEphemeral` mutates
  the live model-atom `.x/.y/.z` of the **moving-set atoms** (= pre-drag coords + world delta; the
  pinned atoms are re-written to their `orig` each frame so a narrowing resolve snaps them back) then
  `applySceneStyle` (which `setStyle`-nulls the cached geometry so sticks redraw at the new coords) +
  `render()` — **no `addModel`, no bond re-perception, no `zoomTo`**, exactly like the mode-animation
  frame update. `applySceneStyle` is the styling extracted from the model effect so a drag frame keeps
  the per-fragment palette (no CPK flash). On release the store commits → the model effect rebuilds from
  the new Scene at the same coordinates (no flick); on a click/zero-drag the controller `restore`s the
  fragment to its pre-drag coords.
- **The Scene is the source of geometry; the overlay never writes it (rule #9 post-condition, unit 3.1
  c1/c3).** During the drag the store snapshot is `===` its pre-drag value; on commit `translateFragment`
  shifts every mover atom by the same delta (internal pairwise distances invariant, count/order/AtomId
  invariant), other fragments untouched.

## Rigid fragment rotation preview — "Rotate about axis" (unit 3.3; ADR-010 ephemeral layer)

The live preview of a rigid whole-fragment rotation reuses the SAME frozen-topology coordinate-update
path as the Move-mode drag and mode animation — driven not by mouse events but by a **prop**, because
the angle comes from a slider in `RotatePanel`, outside the viewer.

- **`ephemeralScene?: Scene | null`** — the rotated preview scene (`rotateFragmentInScene` over the
  committed scene; same composition, one fragment turned), computed in `NewJobScreen`. A dedicated
  effect (`[ephemeralScene, scene, theme, cubes]`) sets the live model atoms' `.x/.y/.z` from it
  and `applySceneStyle` re-draws the sticks at the new coords — **no `addModel`, no bond re-perception,
  no `zoomTo`**. Re-perception is deliberately avoided: a model rebuild would re-guess bonds every
  slider tick and **flicker an inter-fragment stick** in and out exactly in the reactive-approach setup
  this feature is for. On `null` (Cancel/Apply) the committed coords are written back (a ref tracks
  whether an overlay is held, so an unrelated re-render doesn't churn a restore); on Apply the committed
  `scene` changes and the model effect rebuilds at the final coords (no flick). The Scene/store is
  untouched throughout (ADR-010) — this is why the model effect's `scene` dep does NOT include
  `ephemeralScene`, so turning the angle re-runs only this cheap effect.
- **`axisHighlight?: [AtomId, AtomId] | null`** — the two picked atoms of the active rotation, drawn in
  the overlay effect as an **extended axis cylinder** through P→Q (a touch thicker than the dihedral
  axis, extended ~0.7 Å past each atom so it reads as an axis, not a P–Q bond). **Coloured with
  `theme.axisColor` — a distinct azure accent, NOT the green `haloColor` (unit 3.3b-fix).** Borrowing the
  halo/measurement green made the axis rod visually indistinguishable from the green measurement line, so
  the Axis⇄Distance toggle changed nothing perceptible — a **manual-gate finding** (the toggle logic was
  right; the colour wasn't). The azure is chosen for maximum hue distance from every other overlay — the
  chartreuse halo / green measurement line (~85°), the magenta clash glow (~330°), the off-table Pd/Pt
  pink, and each fragment-palette hue — `theme.test.ts` locks a whole-family gap from the greens and a
  clear gap from the rest. Drawn from the **committed** coords, which is correct throughout the preview:
  **both endpoints are fixed points** of the rotation (P is the pivot; Q lies on the axis line), so
  neither moves as the angle turns — and the selection halos on P and Q stay correct for the same reason.
- **`rotateOverlay?: "axis" | "distance"` — exactly ONE overlay for the pair (unit 3.3b).** The axis
  cylinder (3.3) and the measurement distance line drew on the **same two atoms** → two overlapping
  greenish objects of different length, read as "the line is wrong". Now a toggle picks one: **`"axis"`**
  draws the cylinder **plus the Å label on the axis midpoint** and **suppresses the measurement line**;
  **`"distance"`** draws the measurement line + label and **suppresses the cylinder**. The choice is a
  pure function, `chooseRotateOverlay(hasAxis, overlay) → {axis, measure}` (`viewer/rotate-overlay.ts`,
  unit-tested apart from the jsdom-less viewer — the post-condition `axis && measure` is **never** both
  true). **The Å number is ALWAYS the `measure` distance** — `rotationAxisValueLabel` reuses
  `measureSelection`/`formatMeasurementValue`, the SAME source `drawMeasurement` uses (via the shared
  `drawValueLabel`), so length reads identically in both modes; no second computation. When
  `axisHighlight` is null (outside Rotate) the plan is `{axis:false, measure:true}` — the measurement is
  **untouched**.
  - **The `RotatePanel` wiring that drives `axisHighlight`/`ephemeralScene` must not churn state (unit
    3.3b-fix, same manual gate).** `rotationAxis` returns a fresh object; feeding it into an effect's deps
    made the effect `setRotateAxis`/`setRotateEphemeral` **every render** → an infinite update loop
    ("Maximum update depth exceeded", 92/min in the console) AND — because `NewJobScreen`'s `[rotateAxis]`
    reset snaps the overlay back to `"axis"` whenever `rotateAxis`'s identity changes — a toggle that
    could never leave Axis. Fix: **memoize** `axis` (keyed on `scene`/P/Q) and **split** the panel's one
    effect into two — `onAxis([P,Q])` keyed only on the pair (so it fires on a pair change, not an angle
    tick), `onEphemeral` keyed on the angle + memoized axis. The console is clean and the toggle sticks;
    verified live. (jsdom-less viewer + panel → the regression guard is the manual gate: no "Maximum
    update depth" after selecting a pair.)

## Bond display filter — cations excluded + manual hide/show (unit bond-display-control)

3Dmol perceives bonds from interatomic **distances**. That is right for covalent molecules but wrong
for two cases the filter removes **from the drawing only** (`src/viewer/bond-display.ts`, pure /
node-tested, **3Dmol-free** like `frozenTopology.ts`):

- **Cation coordinate bonds — excluded by default.** An **s-block metal cation** (Na⁺/K⁺/Mg²⁺/… —
  `CATION_ELEMENTS` = alkali group 1 + alkaline-earth group 2) *coordinates* an O/N/π face
  electrostatically; it does not bond covalently, so the perceived stick is spurious (Na⁺ next to a
  carbonyl O or an aromatic H). The list is **not** "anything with a + charge" and **not** the
  transition metals: `H` (H⁺ is a proton; every C–H/O–H is real), `N` (NH₄⁺ is covalent), and Pd/Pt/Fe…
  (organometallic M–L bonds are **real**, ADR-007) all bond normally. A `showCationBonds` toggle draws
  them anyway for who wants to see the contact.
- **Manual hide/show — the general escape hatch.** The user can hide/show any specific bond (e.g. a
  forming C···Nu contact in a compressed TS guess 3Dmol draws as a bond). **Keyed by the AtomId PAIR**
  (`bondKey`), never a viewer index — so a hide survives re-perception, geometry edits, and drag/rotate
  (a positional key would hide the *wrong* bond after an index shift — the 2.5.2b defect class; the c2
  negative control bites exactly this). App-owned in `NewJobScreen` (`hiddenBonds: Set<BondKey>`), NOT
  in the Scene.

**DISPLAY-ONLY, and it filters the perception 3Dmol already did — never a second perception.** After
`addModel` (the ONLY place bonds are perceived — the scene path and the frozen-topology build),
`applyBondFilter` removes the rejected bonds **in place** on the live atoms (`filterDrawnBonds` splices
`bonds`/`bondOrder`, the same live-atom mutation `frozenTopology` uses), before `applySceneStyle`. So:
- the **geometry is untouched** — `buildViewerFeed(scene).xyz`, the Scene, and the ORCA input (which is
  coordinates + charge — there is **no** bond list, `wiki/orca/parse-sources.md`) are byte-identical
  whatever is hidden; Generate/Run don't change (c3, and the m4 manual gate);
- the **sidecar's mask perception is separate** (it has its own `within`) — this filter is purely the
  viewer's;
- the **ephemeral drag/rotate paths reuse the filtered model without re-perceiving**, so a hidden bond
  stays hidden across an animation (c4 — orthogonal to `frozenTopology`; no double perception, no lost
  bonds beyond the excluded ones). The scene-path resolver is the feed's `ViewerAtomTable`
  (`atomIdAt`); the mode-animation build passes `() => undefined` (no table → only the element-based
  cation rule applies, manual hides inert there).

The model effect gains `hiddenBonds`/`showCationBonds` in its deps (a toggle re-perceives + re-filters;
the zoom guard keeps the camera since the composition signature is unchanged). `hiddenBonds` defaults to
a module-level empty set so an unspecified prop doesn't churn the effect.

**Geometric bond order — 2/3 parallel sticks (geometric-editor completion).** Right after
`applyBondFilter`, `applyGeometricBondOrders` (`bond-display.ts`) overwrites each surviving bond's 3Dmol
`bondOrder` with `bondOrderEstimate(elemA, elemB, distance).order` — the nearest of the single/double/
triple covalent sums to the CURRENT interatomic distance — so the stick pass (no `singleBonds` set)
draws 1/2/3 cylinders. **DISPLAY-ONLY, nothing stored:** the order is a function of geometry recomputed
every model (re)build, exactly like perception; it never enters the Scene or the ORCA input (ORCA reads
geometry + total charge, not bond order — the honest frame, `editor-ui.md`). Cheap: one pass, each
undirected bond set once from its lower-index end; an element with no double/triple radius (H, metals)
stays a single line, never a throw. **Formal-charge labels** ride the overlay effect that draws atom
numbers/halos: a `+1`/`−1` label on any atom in the `formalCharges` map (display bookkeeping keyed by
AtomId, not in the Scene) — the effect gains `formalCharges` in its deps.

**The same order in the RESULTS/trajectory viewer (Mayer-in-results).** `applyGeometricBondOrders` is
now ALSO called on the two non-scene render paths — the **frozen-topology animation** (per frame, after
the coordinate update) and the **plain xyz** path — reusing the SAME call the scene path makes (no second
impl). So a results/trajectory 3D view draws 1/2/3 lines **re-derived from each frame's geometry**
(butadiene → two C=C); nothing is stored. Picking on these paths (no Scene/AtomId table) is armed via a
separate `onXyzAtomPick(index)` prop that emits the **raw 0-based viewer index** (== the frame /
`final_geometry` index, the identity `mayer_bond_orders` keys on). `TrajectoryPlayer` holds a 2-atom pick
and shows the honest split (`bondReadout.ts`, pure + tested): a **Mayer** entry for the pair →
`"Mayer <order> (authoritative)"` (the COMPUTED order of the final structure, `parse/mayer.rs`), else the
geometric `"≈ <word> · <d> Å (geometric estimate)"` from the shown frame. The two are **never
conflated** — the lines are the geometric estimate, the Mayer number is the computed order; a partial TS
bond (long, geometrically "not a bond") still shows its computed Mayer value. No highlight overlay is
drawn on the xyz paths (the picked atoms are named in the readout instead).

## The overlay effect (one owner of all shapes & labels)

Halos, measurement lines/labels, atom-number labels, and the edit-mask glow are all drawn in **one**
effect keyed on `[selection, scene, showAtomNumbers, theme, …, xyzData, xyzSelection]` — the **only**
place that calls `removeAllShapes()` / `removeAllLabels()` (a second owner would erase the first). It
does clear → re-add → `render()` with **no `zoomTo` and no model reload**, so a selection/number/theme
change never moves the camera. On a coordinate-only edit the model effect still re-renders (new `scene`
ref) so overlays follow atoms to their new positions.

> **Label note — the "F1/F2" collision.** This module documents two 2026-08-2x viewer works whose
> unit labels reuse letters that ALSO name an unrelated, ROADMAP-load-bearing family. Decode:
> **this session's** `F1`/`F1c` = on-molecule **geometry measurement** (distance/angle/dihedral on a
> trajectory frame, drawn on the molecule); `F2`/`F2b` = **orbital overlap** (simultaneous MOs +
> wireframe mesh). These are **distinct from the CREST Stage F microsolvation `F1a–F2`** (grow-parse →
> ORCA re-opt handoff), which keeps its labels in ROADMAP. Older `log.md` entries are append-only and
> keep their original `F1`/`F2` titles — this note is the bridge that decodes them.

**Position-based drawing primitives (F1c) — the effect serves BOTH the Scene path and the xyz path.**
The two drawing primitives take **already-resolved points**, not a Scene, so the editor (`selection:
AtomId[]`) and the results/trajectory viewer (`xyzSelection: number[]`) share one drawing core:
- `drawSelectionHalos(viewer, points: {x,y,z,element}[], theme)` — the wireframe-cage loop.
- `drawMeasurementFromPoints(viewer, m: Measurement, pts: Pt[], theme)` — the distance line / angle arc
  / dihedral axis + value label, everything below the `measure` call. (`Pt = {x,y,z}`; `SceneAtom` is
  structurally a `Pt`, so the Scene callers are unchanged — a behavior-preserving extraction.)
`drawMeasurement(viewer, scene, selection, theme)` is now thin: `measureSelection` → resolve `m.atoms`
to scene rows (same stale-index bail) → `drawMeasurementFromPoints`. The **xyz branch** (inside the
`!scene` path, below) builds `coords: Vec3[]` from the loaded model's atoms (`viewer.selectedAtoms({})`
in `atom.index` order — the SAME 0-based index `onXyzAtomPick` emits and `xyzSelection` holds, so no
re-mapping), runs `measureByCoords(coords, xyzSelection)`, then calls the SAME two primitives. Because
`xyzData` + `xyzSelection` are in the deps, the xyz overlay **redraws per frame** — halos + line + label
follow the atoms during trajectory playback (the model effect, declared first, rebuilds the frame's
model before this effect reads it).

- **Fed through the table (2c1/2c2).** The overlay builds `buildViewerAtomTable(scene)` and an
  `AtomId→atom` map. Since 2c2 the **`selection` prop is `AtomId[]`**, so a halo resolves an id
  **directly** to its atom via the map — no positional round-trip. The **mask** (`maskHighlight`)
  stays a positional global index (it is `EditPlan.mask`, the ASE emit seam) and resolves *index →
  AtomId → atom* through the table. The **number** a label shows is the atom's viewer index read from
  the table (`table.viewerIndexOf(id)`), not a loop counter. `drawMeasurement` now takes the `AtomId[]`
  selection (`measureSelection` resolves it); its `m.atoms` are the resolved global indices it renders
  against.

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
  labels linger when the last fragment is removed. Clear first, then bail. **The `!scene` path is no
  longer a bare bail:** when `xyzData` + a non-empty `xyzSelection` are present it draws the xyz overlay
  (halos + measurement via the shared primitives, off `measureByCoords`) before `render()`; otherwise it
  renders empty and returns. `if (cubes.length > 0) return` stays FIRST — the orbital effect owns its shapes.
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

### Steric-clash "danger glow" (unit 3.2)
`clashHighlight?: AtomId[]` — atoms in an inter-fragment vdW clash (from
`scene/clash.ts`, passed by `NewJobScreen`). Drawn as a **solid magenta sphere**
(`CLASH_COLOR = "#ff2d95"`, `CLASH_OPACITY = 0.4`, radius = halo + a larger boost),
before the selection cage. **Distinct from the halo AND the mask in BOTH hue and
form:** the halo/mask share the theme's chartreuse `haloColor` (distinguished from
each other only by form, since they coexist on one atom); the clash marks a
**different** set of atoms and a different *kind* of thing (a warning, not a
selection), so it takes a hue no CPK element uses (magenta) — deliberately NOT the
chartreuse, because a semantic colour that collided with an element once already bit
(the Pd/Pt-vs-halo case). A theme-independent constant, like the orbital-phase
colours. It resolves an `AtomId` straight to its atom (like `selection`); a warning
marker only — nothing here blocks Run/Apply. The banner dot in `NewJobScreen` uses
the same magenta so the two read as one signal.

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

## Orbitals / densities (unit 3.15 — DONE; F2 — simultaneous MOs DONE)
`orca_plot` (driven over stdin, not the unusable `plot-inputfile` mode — measured, `orca/orca-plot.md`)
generates a `.cube` from `.gbw`; the viewer draws the molecule from the cube + two ± phase isosurfaces
(the sign is the wavefunction **phase, not charge**), with an isovalue slider. Default grid 80³
(measured ≈6.9 MB); cubes cached in the job dir, never in the DB; generated lazily on MO selection.
Each `.cube` string is parsed once into a `VolumeData`, so an isovalue change redraws only the
surfaces. WebKitGTK renders the isosurface — gated via MiniBrowser (`debugging/002` technique).
Detail: [results-ui.md](results-ui.md). Density cubes / the MO-coefficient route are **not** done.

**F2 — several MOs at once (HOMO–LUMO overlap for FMO reading).** The viewer takes an
ARRAY prop `orbitalCubes: Array<{ cube; posColor; negColor }>` — N cubes, each with its own
± phase colour pair. Normalization (memoized `cubes`): `orbitalCubes` wins when both are
passed; else the `orbitalCube` (string) prop is an ALIAS normalized to a **1-element list
with the default blue/red pair** → the single-orbital path is byte-identical (one cube,
`#3b6fd4`/`#d43b3b`, opacity 0.85, same shape order). Key mechanics preserved from the
single-orbital design:
- **One model, built once** from `cubes[0]` (the simultaneous MOs of one job share
  geometry). The model effect keys on the string primitive `modelCube = cubes[0].cube`, so a
  colour/selection change on a sibling never rebuilds the model — only a change of the shared
  geometry does. `zoomTo` still gates on the `orbital:${atomCount}` signature (camera holds
  across MO switches).
- **`volDataRef` is a `Map<cubeText, VolumeData>`** — each cube parsed once, reused across
  isovalue changes AND siblings; entries not in the current set are pruned each draw (no
  `VolumeData` leak between selections).
- **The isosurface effect draws 2N surfaces** and stays the **single owner** of the ±
  surfaces: `isoShapesRef` holds the full 2N set and `removeShape`s exactly that set on each
  change — **never `removeAllShapes`** (which would wipe the selection-halo path). Solid
  opacity via `orbitalIsoOpacity(n)`: `n === 1 → 0.85` (`ORBITAL_ISO_OPACITY`), `n >= 2 → 0.55`
  (`ORBITAL_MULTI_ISO_OPACITY`, live-tunable).
- **F2b — wireframe mesh for overlap legibility.** Translucent SOLID surfaces depth-sort
  poorly in WebGL, so the front orbital occluded the back one (live gate). Prop
  **`orbitalWireframe?: boolean`** (in the effect deps): when true both `addIsosurface` specs
  get `wireframe: true` + `linewidth: ORBITAL_WIRE_LINEWIDTH (1.5)` and opacity
  `ORBITAL_WIRE_OPACITY (0.85)` — a mesh doesn't occlude, so it can be near-opaque and you see
  THROUGH it to the other orbital. When false the surfaces stay solid at `orbitalIsoOpacity(n)`.
  **Probe (Rule #10, verified on the installed build):** `IsoSurfaceSpec extends ShapeSpec`,
  and `ShapeSpec.wireframe?: boolean` is consumed by `GLShape.ts` (`shape.wireframe = …`,
  `if (this.wireframe)`, passed to the THREE material as `wireframe`/`wireframeLinewidth`);
  `linewidth` is a best-effort hint (WebGL clamps line width to ~1px on most drivers — the
  type def warns), so legibility comes from the mesh itself, not the stroke. The app defaults
  it on for ≥2 orbitals and off for one (single orbital unchanged — solid 0.85); see
  [results-ui.md](results-ui.md).
- **Seam (identity stability):** the isosurface effect deps are `[cubes, orbitalIsoValue]`
  and `cubes` is memoized on the raw props, so the parent (`OrbitalPanel`) MUST hold
  `orbitalCubes` in state with a stable reference (set once per fetch) — otherwise a stray
  re-render (isovalue drag, representation toggle) would re-parse/redraw 2N surfaces every
  frame. Content-hashing is not an option (each cube ≈6.9 MB); reference stability is the
  guard.
- The three orbital-mode guards (model-build, overlay-bail, ephemeral-model-bail) key on
  `cubes.length > 0` (was `orbitalCube?.trim()`).

## Spectra (units 3.8–3.12 — IR + mode animation DONE)
IR: Lorentzian broadening over (freq, intensity) → recharts, in `src/spectrum/` — see
[results-ui.md](results-ui.md). Peak click selects the frequency-table row (and vice-versa), and
**animates the normal mode** (unit 3.12): the `.hess` frame was proven a pure translation of the
reference (Kabsch gate), so modes are added as-is; phase/amplitude/timer are app state, the viewer
gets one frame with a **frozen** bond topology. UV-Vis (Phase 6): Gaussian broadening over TD-DFT
(energy, fosc) — see the ROADMAP note.
