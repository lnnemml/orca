# Module: editor UI (the New Job workspace)

**Status:** Phase 4.2 tail-1 (guided fragment placement) on top of Stage 3 (operations over the
core — **COMPLETE**). The New Job screen (`src/screens/NewJobScreen.tsx`) is a
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
- **THE ONE RULE — the moving set (unified moving-set unit; `resolveMovingSet`, `scene/moving-set.ts`).**
  For any drag OR any single-side edit, the set of atoms that moves is decided by one pure rule:
  1. an explicit atom **selection** is present → move **exactly the selection** (the researcher chose the
     atoms; wins over the toggle);
  2. else the **"Move: Fragment | Selection"** rail toggle decides — **Fragment** = the whole fragment of
     the grabbed atom (rough placement, synchronous, no sidecar); **Selection** = the grabbed atom's
     **perceived connected component** (a broken-off / disconnected piece moves alone).

  `resolveMovingSet` is pure: the fragment atoms and the component are **injected** (perception has ONE
  home — the sidecar — ADR-010 correction ii). Fragment and Selection differ **only** when the fragment
  has disconnected pieces (a fully-bonded fragment's component IS its whole atom set → identical).
- **Move mode — rough placement by dragging (unit 3.1; Stage 3.x; unified moving-set unit).** A checkbox
  turns on rigid-body drag: grab any atom and drag in the plane of the screen (60fps, one Undo step —
  see `modules/visualization.md`). The moving set follows THE ONE RULE above (the drag reads it live on
  mousedown): **Fragment** resolves synchronously; **Selection** asks the sidecar
  `/geometry/connected-component` **once, not per frame** (so after a bond is **broken** the two pieces
  drag independently — break H–C in HCN → drag H → only H moves; drag C → C and N move together); an
  explicit selection moves exactly those atoms (even across fragments). The commit is one
  **`translate-atoms`** op carrying the moving set's AtomIds + total delta (count/order preserved via
  `translateAtomsInScene`, ADR-008; one Undo, ADR-010). **No stored connectivity** — perception is
  re-derived each drag, nothing is kept. If a `/geometry/connected-component` call fails the drag **falls
  back to a whole-fragment move and shows an honest dismissible banner** (never a silent wrong move). It
  is deliberately **coarse**: exact distances/angles/dihedrals come from the measure + constraint tools
  and the input editor. A drag on empty space still rotates the camera; a click still picks.
- **Edits & bonds ACROSS disconnected pieces of one fragment (`needs-component-move`).** A **distance**
  or **Form/Break bond** between two atoms of ONE fragment that sit in **different** connected components
  (the Diels-Alder case: a diene + a dienophile imported as one xyz = one fragment, two molecules) is
  **not** a torsion — there is no bond to cut, so it must not route to `needs-split` (which 422s "not
  bonded"). `NewJobScreen` resolves both picked atoms' components via `/geometry/connected-component` and
  injects them into `planEdit`, which then returns a **`needs-component-move`** plan carrying the two
  components. `EditPanel` translates the **smaller** component along the i→j axis to set the distance
  (or the bonding distance for Form bond) — a **pure rigid `translateAtoms`** move (count+order invariant;
  one Undo), **never** `set-internal`. **"Move the other piece instead"** swaps the components. A bonded
  intra pair still resolves as `needs-split` (unchanged); an inter-fragment pair still moves the smaller
  fragment (unchanged). The post-condition (rule #9) re-derives the resulting separation and refuses if
  it is not the target.
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
- **Bond display control — cations excluded by default + manual hide/show (unit bond-display-control;
  Edit section).** 3Dmol draws bonds by distance, so an s-block metal cation (Na⁺/K⁺/Mg²⁺…) coordinated
  to an O/N/aromatic-H gets a **spurious covalent stick**. Those are **hidden by default** (a "Show
  cation coordinate bonds" checkbox reveals them). A general escape hatch: **select two atoms → "Hide
  bond" / "Show bond"** toggles that specific bond, keyed by the **AtomId pair** so it survives
  drag/rotate/re-perception (a positional key would hide the wrong bond after an index shift). A "N
  bonds hidden · show all" reset appears when any are hidden. **DISPLAY-ONLY** — app-owned in
  `NewJobScreen`, never in the Scene; it changes no geometry, so Monaco's xyz block, the total charge,
  and the generated `.inp` are byte-identical (there is no bond list in an ORCA input —
  `wiki/orca/parse-sources.md`). Mechanism (filters the perception 3Dmol already did, no second pass):
  `modules/visualization.md`.
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
    like `clashK`), **default Distance** (`DEFAULT_ROTATE_OVERLAY` — on picking two atoms the researcher
    reads the separation first; the axis cylinder is one toggle away), reset to that default when the
    pair changes or on Cancel; **the measurement tool outside Rotate is untouched**.
- **Guided fragment placement — add a reagent at d/θ/φ in ONE flow (Phase 4.2 tail-1;
  `GuidedPlacementPanel`, in the Fragments section).** With **Guided placement** on, clicking a reagent
  adds it roughly (the same `placeFragment` + `add-fragment` op — unchanged) *and* opens the guided
  panel on that fragment. Pick the **reagent atom** (the one on the just-added fragment) + **1–3
  substrate anchors**, then enter **d** (required) / **θ** (optional, needs 2 anchors) / **φ** (optional,
  needs 3) — a field is disabled with a reason until enough anchors are picked; an **empty field is a
  SKIP, never a 0**. Preview is **view-only** (mirrors `EditPanel`); **Apply** commits one
  `replace-fragment-atoms` (`set-internal`) op **per given coordinate**, in Z-matrix order (d, then θ,
  then φ — each later edit rotates about an axis through anchor 1, preserving the earlier coordinate),
  so **Undo unwinds each step**. It **reuses the inter-fragment `set-internal` edit path verbatim**
  (`planEdit`/`applyResponseToScene`, mask = the reagent fragment) — no new d/θ/φ math. Guided state is
  **app-owned in `NewJobScreen`, NOT in the Scene**. Mechanism: `modules/scene.md` "Guided placement".
- **Sections, in order of use:** Selection & Measure (`AtomInspector`) · Edit (`EditPanel` + the Move-mode
  toggle + `RotatePanel`) · Fragments (the Add-Fragment palette — reagents / import / SMILES / **Paste xyz** / library —
  **plus** the guided `GuidedPlacementPanel` when a reagent is being placed, **plus** `FragmentList`) ·
  Constraints (`ConstraintPanel`) · **Scan** (`ScanPanel`, Phase 4.5 A2) · History (`HistoryPanel`) · Actions (xTB
  pre-optimize **+ the CREST/QCG microsolvation panel** `crest/CrestPanel.tsx`, Stage F F1c — both
  scene-operating helpers; the CREST panel runs a QCG grow from the scene and shows the grown cluster with
  an always-on charge-aware seed warning, `modules/crest-microsolvation.md`).
- **Scan (`ScanPanel`, Stage A2)** — sibling of Constraints, same **view-over-text** discipline
  (source = `inspectScanBlock(content)`, edits = `injectScan`; no scan state of its own). Renders
  absent / parsed (editable start/end/npoints + remove) / unrecognised, and surfaces the `! Opt`
  Run-guard (`scanOptIssue`) inline. The **add path** is in Selection & Measure: "Scan this
  {distance/angle/dihedral}" on a 2/3/4-atom pick (`AtomInspector` `onScan` → `scanFromSelection`,
  mirroring "Constrain selection"), keyed on `AtomId` so the coordinate survives an index shift. The
  Run-guard is wired into the SAME create/run gate as the constraint range-check — a scan with no
  measured opt keyword blocks Create & Run (`scanBlockMessage` in `NewJobScreen`; `wiki/orca/scan.md`). The Reagents palette shows **Built-in** (curated, reference-contract) and **My
  reagents** (user-saved, no contract) as visually distinct groups, plus a "+ Save" dialog that
  captures a fragment's or pasted geometry with a **required** charge (tail-2; `reagent-catalog.ts`,
  `modules/scene.md`). Each
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

## Form / break bond — a set-distance PRESET, not a new engine (Stage E3b)

The Edit section (`EditPanel`) grows two buttons — **Form bond** / **Break bond** — shown only for a
**two-atom distance** edit (`active.op === "distance"`). They are the researcher's way to **derive a
product geometry from a reactant** (the concerted-reaction case NEB needs, `wiki/orca/neb.md`).

The load-bearing insight (recorded so it isn't re-litigated): **form/break is NOT a new geometric
primitive.** It is exactly `planEdit(op="distance")` with a **computed target** —
`planFormBond`/`planBreakBond` (`src/scene/bond-edit.ts`) delegate the mask entirely to `planEdit`
(the `active` plan already IS that, resolved upstream in `NewJobScreen`, including any `needs-split`
bond-graph split from `/geometry/rotatable-mask`, 2.5.3b, **or** a `needs-component-move` when the two
atoms are in different disconnected pieces of one fragment — the Diels-Alder form-bond, which then
routes through the pure `translateAtoms` component move; `bond-edit` also takes the injected `components`
so its own returned plan is honest) and only **compute the distance**:

- **Form** = the covalent-radius sum `rA + rB` (`covalent-radii.ts`, Cordero 2008) — the same basis
  distance-based perception uses, so the pair lands **inside** the perception window (sidecar `×1.2`)
  and a bond is drawn.
- **Break** = `(rA + rB) × 2` — comfortably **past** the perception window, so the bond drops for any
  pair; the later `Opt` relaxes the fragments into the product basin.

The button pre-fills the target field and drives the **same** preview→apply path as a typed
set-distance edit (`callSetInternal` → preview view-only; **Apply → `applyResponseToScene` →
`replaceFragmentAtoms`**, one Undo). Only `.target` is taken from the planner (the distance is
orientation-invariant, so it survives a "Move X instead" swap). A refused intra-fragment mask
(reference-atom rule / ring) surfaces the **existing** honest message — no new refusal path.

**Why this closes the product-derivation → NEB loop (the point of the unit).** Every form/break edit
goes through `replaceFragmentAtoms`, which enforces the **atom count + order invariant** (ADR-008). So
a product derived by form/break has the **same atom order as the reactant** — the NEB setup's same-order
assert (E3a-1) accepts `(reactant job, derived product job)` with no refusal, and **nothing new is
stored**: no connectivity, no bond edge-set, no reactant→product lineage column. Perception follows the
distance; the order invariant is the only guard needed.

## Geometric bond order + formal-charge bookkeeping (geometric-editor completion)

**The honest frame — state it before anything else.** ORCA reads **geometry + total charge +
multiplicity + method**. It does **not** read bond order, and it does **not** read per-atom charge.
Everything in this section is either (a) a *geometric* target/estimate — a "double bond" is just a
**shorter set-distance**, and the method infers the order from that geometry — or (b) *bookkeeping*
that must sum to the total charge ORCA is actually given. Nothing here is a new physical input. The UI
copy says so at every touchpoint; the **Mayer bond order in the results context** (parsed from a
finished calculation — `parse/mayer.rs`, shown authoritatively in the results viewer,
`wiki/modules/visualization.md`) is the authoritative order — this editor never claims to have it, it
only estimates from geometry.

- **Set bond order (Form single / double / triple).** `EditPanel`'s Form affordance is now three
  buttons; each calls `planFormBond(scene, a, b, order)` and drives the **same** preview→apply path.
  Order only changes the **target distance**: `bondingDistance(elemA, elemB, order)` sums single
  (Cordero) or double/triple (**Pyykkö & Atsumi 2009**) covalent radii — C–C 1.52 / C=C 1.34 / C≡C
  1.20 Å. It is the **one** covalent-radii table (`covalent-radii.ts`) extended with `order`, and an
  element with no double/triple radius throws loudly (rule #11) into the existing error banner. Break
  is unchanged (order is meaningless when clearing a bond). **Perception is untouched — still
  Cordero-single × 1.2**; order affects only the form target and the display.
- **Bond-order analyzer (honest label).** Selecting a two-atom **distance** shows, under the measure
  readout, `≈ double · 1.34 Å (geometric estimate)` — `bondOrderEstimate` picks the **nearest** of the
  single/double/triple sums to the measured length (`AtomInspector.analyzeBondOrder`). It is shown
  **only within bonding range** (≤ single sum × 1.3), so a through-space contact (a forming/breaking
  ~2.2 Å distance) is **not** labelled an order. Always tagged "(geometric estimate)"; the tooltip
  points at the Mayer-in-results follow-up. The estimate is discrete — a delocalised/aromatic ~1.40 Å
  reads as the nearest integer ("double"); fractional orders await Mayer.
- **Double/triple DISPLAY (2/3 lines).** `applyGeometricBondOrders` (`bond-display.ts`) overwrites each
  drawn bond's 3Dmol `bondOrder` from the current geometry (via `bondOrderEstimate`), so the stick pass
  draws 1/2/3 parallel cylinders. **DISPLAY-ONLY, nothing stored** — re-derived from geometry every time
  the model is (re)built, exactly like perception (and like it, it mutates the throwaway 3Dmol array,
  never the Scene). Runs right after `applyBondFilter`; an element with no radius stays a single line
  (never a crash).
- **Formal charge (per-atom bookkeeping).** `AtomInspector` gains a `+ / −` control on the last-picked
  atom; the value is a **display annotation keyed by AtomId** (owned by `NewJobScreen` like
  `hiddenBonds`, **not** in the Scene), shown as a `+1`/`−1` badge on the atom (a viewer label) and in
  the chip list. A **Σ-formal-vs-total indicator** (`formalChargeConsistency`, `formal-charge.ts`)
  reads `Σ formal = total ✓` or an honest mismatch that states *ORCA still uses the total*. The sum is
  over the atoms **currently** in the scene (a stale entry for a removed atom isn't counted).
  `totalCharge(scene) = Σ fragment.charge` (`scene.ts`) is the charge ORCA is given; formal charges
  never change it — a mismatch is a bookkeeping flag, never a run blocker.

## New Job header — the group picker (unit 2a)

Beside the Job-title field the form carries an explicit **Group** picker (`<GroupSelect>`) that sets
the created job's destination group — default = the active sidebar group, overridable to any group or
ungrouped. It composes `move_job` (no editor/scene coupling); the mechanism lives in
[`modules/groups-ui.md`](groups-ui.md).

## Where future panels go

- **Phase 4.5 — reaction setup.** The reaction-center / scan-setup UI (ADR-007) is a **new dock
  section** (e.g. "Reaction"), not a new stack: it composes the existing selection/measure/constraint
  panels into a guided flow. (Guided fragment placement — add a reagent at a distance/angle/dihedral —
  is **built** as of Phase 4.2 tail-1; it extends the **Fragments** section, see above.)

## Related

- `src/scene/EditorDock.tsx` — the sectioned dock (pure layout).
- `wiki/modules/scene.md` — the panels the dock hosts (store, HistoryPanel, EditPanel, FragmentList,
  ConstraintPanel) and the operation log they act on.
- `wiki/modules/visualization.md` — `MoleculeViewer` (the canvas, its `ResizeObserver`, picking).
- `wiki/debugging/011-*` — the WebKitGTK `min-width:auto` clip; the reason layout changes are treated
  as risk here.
