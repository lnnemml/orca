# Module: scene (`src/scene/`)

**Status:** current through **Phase 4.2 tail-1** (guided fragment placement) on top of Stage 3 (the
geometry-editor arc: the operation-log fold 2a–2d, rigid drag 3.1, vdW clash 3.2, rigid rotation
3.3/3.3b, the `adoptPreservesScene` fragment-merge guard). The Scene is the **source of truth for geometry on
New Job** — a Zustand store (`store.ts`) synced two-way with the Monaco buffer — and
carries the whole reaction-geometry workflow: multi-fragment build (Add-Fragment panel
+ `FragmentList`), electron-parity validation (`parity.ts`), conformer search (GOAT,
`ensemble.ts`), the geometry editor (atom picking `selection.ts`, d/θ/φ measurement
`measure.ts` with ASE conventions pinned to source, edit-mode planning `edit-plan.ts`
for inter- and intra-fragment edits, the reference-atom rule `maskRoleViolation` re-run
on both the inter-fragment and the resolved bond-graph split mask), the constraint block
over the input text (`constraints.ts`, `ConstraintPanel.tsx`, non-destructive rewrite),
and xTB pre-optimization (`replaceAllAtoms`). Scene layout persists as `jobs.scene_json`
(DB schema v4; the JSON payload is **v2** since unit 1b — per-atom `AtomId` +
`nextAtomId`, with v1 migrated on read) and restores on job iterate (`restoreScene`).
As of Phase 4.2 unit 1b every atom carries a stable **`AtomId`** (ADR-010 identity core,
in TS ahead of the Rust move — ADR-016). Per-unit history is in `wiki/log.md`.

## Responsibilities

The Scene / SceneFragment data model from
[ADR-008](../architecture/adr-008-scene-fragment-model.md): OrcaStudio's own
abstraction for a multi-molecule geometry (substrate + reagent in one coordinate
space, with known fragment boundaries). ORCA never sees a Scene — on export the
fragments merge into one flat `* xyz totalCharge multiplicity ... *` block.

The pure layer (`scene.ts`, `parity.ts`, `types.ts`) is **React-free** by design
(ADR-008 decision 10): merge / index-mapping / comparison as plain node-testable
functions, no imports from react / 3dmol / tauri. The reactive `store.ts` (added
2.5.0d-1) is the one React-facing file and stays a thin wrapper over that layer.

## Files

- `types.ts` — `RawAtom` / `SceneAtom` (raw + `AtomId`), `RawFragment` /
  `SceneFragment` (in-Scene subtype), `FragmentSource`, `Scene` (carries
  `nextAtomId`); `FRAGMENT_SOURCES` (the valid-source list, for deserialize
  validation).
- `ids.ts` — `AtomId` (branded), `makeAtomId`, `stampFreshIds` (mint), `carryIds`
  (positional identity carry). Pure, dependency-free (unit 1b).
- `scene.ts` — the merge / index / parse / serialize functions below, plus the
  ORCA-input ↔ Scene text I/O (`sceneFromOrcaInput`, `injectSceneIntoInput`).
- `scene-test-util.ts` — **test-only** `testScene` / `testAtom` constructors that
  mint ids the way production does (so tests never spell out ids and `id` is never
  made optional). Not imported by any production module.
- `parity.ts` — `checkElectronParity` (electron-parity validation, ADR-008 #8).
- `store.ts` — the Zustand scene store (React-facing; thin over the pure layer). Its one-step
  `previous`/`undoReset` is superseded by the operation log in unit 2b (not yet removed).
- `oplog.ts` — the **operation log** (Phase 4.2 Stage 2 unit 2a; ADR-017): pure types + pointer
  semantics (`append`/`undo`/`redo`/`goto`/`current`, `describe`, serialize), no
  store/viewer/Monaco/DB/Rust. See "The operation log" below.
- `HistoryPanel.tsx` — the read-only history panel (unit 2b; React): `describe()` list, click =
  pointer jump, Undo/Redo + hotkeys. Reads the store log; no state of its own.
- `placement.ts` — `placeFragment` (bounding-box separation for a new fragment).
- `fragment-library.ts` — `FRAGMENT_LIBRARY` (curated reagents, incl. the tail-2
  monatomic cations) + `libraryFragmentToScene`.
- `reagent-catalog.ts` — the **user** side of the reagent catalog (tail-2):
  `userReagentToFragment` (a saved `molecules` row → a scene fragment, charge carried
  into the total like a built-in) + `fragmentToXyz` (capture one fragment's geometry
  on save). Where the **curated↔user** distinction is made explicit (source `"library"`,
  never `"fragment-library"`; no `reference` contract). Pure / node-tested. See
  "Extensible reagent catalog" below.
- `FragmentList.tsx` — the fragment sidebar (React; reads the store, uses the
  shared `fragmentColor` palette).
- `restore.ts` — `restoreScene` (snapshot ↔ input reconciliation on job open) and
  `restoreSceneLog` (unit 2b: the log ↔ snapshot cross-check for New iteration —
  the log is rejected loudly if it diverges from `scene_json`).
- `carryForward.ts` — **New-iteration geometry provenance** (`debugging/021`, `debugging/022`). Pure,
  React/Tauri-free.
  - `resolveCarryForwardGeometry(job, results)` — the 021 half: carry the **converged output**
    (`results.final_geometry`), or an honest refusal (scan/NEB → per-point/image handoff; non-converged
    → not stationary; no result). `geometryMatchesFinal` — the bit-match guard (rejects the seed).
  - `iterationFrames(job, results)` — the **022 explicit frame picker** that supersedes the verdict
    classification: an optimization with ≥ 1 `results.trajectory.frames` → `{ frames, defaultIndex }`,
    **checked BEFORE `results.converged`** so a null-verdict post-GOAT Opt still gets a picker. The
    **default is the LAST frame** (optimized output), each `FrameChoice.geometry` sourced **directly**
    from `trajectory.frames[i]` (never `input_content`). The verdict drives the last frame's **label
    only** (`true`→"final (converged)", `false`→"…did not converge (not stationary)" *still selectable*,
    **`null`→"final frame (optimized output)"** — no false convergence claim). Refusals reuse 021's
    reasons (scan/NEB/no-result) + a `no-trajectory` kind for a single point. `frameProvenanceComment`
    → the `# geometry: frame <i> (<label>) of job <id>` header.
- `ensemble.ts` — GOAT conformer-ensemble parsing + input generation (2.5.1a),
  plus `isGoatInput` (2.5.2a — is this a conformer-search job?).
- `optts.ts` — `buildOptTSInput(sourceInput, seedGeometry, options?)`: the **source-agnostic**
  OptTS-refine create side (Phase 4.5 Stage E1a, ADR-020). A sibling of `reopt.ts` (documented in
  `conformer-reoptimization.md`) sharing its **charge-footgun discipline** — inherits `(charge, mult)`
  from `sourceInput`'s `* xyz` via `sceneFromOrcaInput` and **asserts them back out** of the emitted
  input — but for a **TS guess from ANY source** (a scan maximum today, a NEB climbing image in E3; the
  seed is a generic `TsGuessGeometry`, deliberately NOT `ScanGeometry`). Method + solvation **default to
  the source's** (verbatim, via `methodSolvationKeywords` in `reactions/compare.ts` — one `!`-line
  reader shared with the comparability guard, so re-emit and compare cannot drift); emits `! <method>
  <solvation> OptTS Freq TightSCF` + `%geom Calc_Hess true end`. REUSES `buildOrcaInput` — no
  order-bearing golden pair (Fork 2 of ADR-016; `wiki/orca/optts.md`), so the source's opt keyword /
  `Scan` / `Constraints` cannot leak (fresh build). Pure / node-tested. The scan entry point is
  `ScanProfilePanel`; the Rust create side is `create_optts_job` (`tauri-core.md`).
- `neb.ts` — `buildNebInput(state, reactantInput, reactantGeom, productGeom, options?)`: the NEB-TS
  create side (Phase 4.5 Stage E3a-1; **N2** moved creation into the Input Builder). Keeps the
  charge-footgun discipline of its `buildOptTSInput` sibling (inherit `(charge, mult)` from
  `reactantInput`, assert back out) but the **METHOD now comes from the builder's `BuilderState`**
  (family-aware — composite / DFT / DLPNO / xtb), NOT inherited from the reactant's `!` line: it emits
  the family-aware `buildOrcaInput` at `{ ...state, jobType: "NEB-TS", charge, multiplicity }`, so the
  method/basis/solvation the user picked drive the NEB level (this is what enables **NEB-on-xtb** from
  DFT-optimized endpoints). Splices a **multi-line `%neb` block** (`NEB_End_XYZFile "product.xyz"` /
  `NImages <n=8>`) — the measured, converging form (`wiki/orca/neb.md`), not the unverified single-line
  one (rule #10) — anchored on the family-independent `* xyz` block (never `%maxcore`, which a
  self-contained family could omit). Returns **both** the `.inp` AND `productXyz` (the product end
  image, reusing `finalGeometryXyz` — no second xyz builder). **THE SAME-ORDER GUARD** (the whole
  point): throws if reactant and product do not share atom order (element sequence AND count) — NEB
  interpolates image-k↔image-k atom-by-atom, so a mismatched pair silently fails; the builder refuses
  to emit one. Also exports `hasNebKeyword(content)` (any `!` line carries a NEB token — mirrors Rust
  `input_has_neb`), the gate `NewJobScreen.create()` uses to route to `create_neb_job`. Pure /
  vitest-tested. The Rust create side is `create_neb_job` (a two-file child; `tauri-core.md`); the
  setup UI is the builder's `NebBuilderSection` (was the retired `NebSetupPanel`).
- `selection.ts` — the geometry editor's atom pick list (2.5.2a; **AtomId-native
  2c2**): `toggleAtom(AtomId[], AtomId)`, `filterSelection` (the 2c2 dividend —
  keeps every id still in the scene), `describeAtom` (positional, for the
  constraint panel) + `describeAtomById`. Pure / node-tested, no React. The old
  positional guards `selectionSurvives` / `validateSelection` are **gone** — an
  `AtomId` gives "the same atom" an operational meaning, so there is nothing to
  remap or clear on a removal (see "Atom selection" below).
- `measure.ts` — geometry measurement off the pick list (2.5.2b): `distance`,
  `angle`, `dihedral`, `measureSelectionByIndex` (positional core) and
  `measureSelection(scene, AtomId[])` (resolves ids → indices, 2c2),
  `formatMeasurementValue`. Pure / node-tested, React-free. **ASE conventions
  pinned to source** (see below); the math is index-based and untouched.
- `edit-plan.ts` — edit-mode planner (2.5.2d): `planEdit(scene, AtomId[])` resolves
  the ids to current global indices ONCE, then the whole planner runs positionally
  (`EditPlan` is the ASE-mask emit seam — positional by design). Plus the pure apply
  helpers `applyResponseToScene` / `applyResponseIssue` and the shared reference-atom
  rule `maskRoleViolation` / `explainSplitViolation` (2.5.4a). Pure, no React/fetch.
- `guided-placement.ts` — **guided fragment placement** (Phase 4.2 tail-1): the pure
  planner `planGuidedPlacement` (a reagent atom + 1–3 substrate anchors + target d/θ/φ →
  a SEQUENCE of `set-internal` steps, each masking the reagent fragment) + `guidedStepOp`
  + the DI driver `runGuidedPlacement`. **Reuses `edit-plan.ts` (`planEdit` /
  `applyResponseToScene` / `applyResponseIssue`) — no new d/θ/φ math.** Pure /
  node-tested (`guided-placement.test.ts`, c1–c4). No React, no direct fetch (the
  sidecar call is injected). See "Guided placement" below.
- `GuidedPlacementPanel.tsx` — the guided panel (React; Fragments section): resolves the
  reagent atom + substrate anchors from the shared pick list, d/θ/φ fields, Preview
  (view-only) / Apply. A thin wrapper over `guided-placement.ts` + the exported
  `callSetInternal` — mirrors `EditPanel`.
- `constraints.ts` — ORCA `%geom Constraints` generate / parse / inject (2.5.4a):
  `Constraint` type (B/A/D/C), `ORCA_INDEX_BASE`/`toOrcaIndex`/`fromOrcaIndex`,
  `constraintsBlock`, `parseConstraintsBlock`, `injectConstraints`. Pure /
  node-tested, no React, no fetch. **Input text is the source of truth** — the
  panel is a view over the text; a `Constraint` stays **positional/textual** (its
  atoms are ORCA 0-based indices). `constraintFromSelection(scene, AtomId[], value?)`
  resolves the id selection to ORCA indices **at build time** (the ORCA-index emit
  seam, 2c2) — the id is never stored. Index base **0-based, settled by a real ORCA
  6.1.0 run** — `wiki/orca/constraints.md`.
- `geomBlock.ts` (Phase 4.5 Stage A1) — the **single `%geom` locator** shared by
  `injectConstraints` and `injectScan`: `scanTokens`, `locateGeom` (depth-tracking over
  the recognised sub-block set `constraints` + `scan`, returning each sub-block's char
  span), `leadingIndent`. Lifted out of `constraints.ts` so the two injectors compose into
  **one** `%geom` and cannot drift into emitting a second (which ORCA silently reduces to
  one).
- `scan.ts` (Phase 4.5 Stage A1) — ORCA `%geom Scan` generate / parse / inject / guard:
  `ScanCoordinate` (kind B/A/D, 0-based atoms in the SAME space as `Constraint`,
  `start`/`end` with the `startText`/`endText` value_text-analogue, `npoints` ≥ 2),
  `scanBlock` (the 1-coordinate rendering is **byte-identical to Rust `emit_scan_block`** — the golden
  pair; the Rust twin is 1-coord golden-only, the frontend owns the 2D emit), `injectScan`
  (accepts **one coordinate OR a list (1..N)** — a list emits every coordinate inside the ONE `Scan`
  block, a **native N₁×N₂ relaxed grid** for a 2D PES; **composes into the one `%geom`** via `geomBlock`;
  insert/replace/remove the `Scan` sub-block as a sibling of `Constraints`, never a second `%geom`; a
  1-coordinate call is byte-identical to before), `parseScanBlock` (the 1-coordinate read) /
  `parseScanCoordinates` (**the N-aware superset — all coordinates in order, outer→inner; null on
  absent/comment/any-bad-line**) / `inspectScanBlock` (read-back for the absent/unrecognised distinction;
  still flags a >1-line block as `unrecognised`, which guards the single-coordinate add-path off a 2D
  scan), `scanOptIssue` (the loud Run-guard — a relaxed scan
  without a **measured** opt keyword is silently a single point; the set
  `{opt, optts, tightopt, verytightopt, looseopt}` is measured per rule #10, `wiki/orca/scan.md`),
  and (A2) `scanFromSelection(scene, AtomId[], range)` — build a coordinate from a 2/3/4-atom
  selection, resolving `AtomId → 0-based index` at build time (survives an index shift), mirroring
  `constraintFromSelection`. Pure / node-tested. `wiki/orca/scan.md`.
- `ScanPanel.tsx` (Phase 4.5 A2; 2D grid = Stage 4a) — the Scan dock section: a **view over the input
  text** (source = `parseScanCoordinates(content)`, edits = `injectScan`; the number fields keep only a
  transient keystroke draft, never scan state). Renders absent / parsed (coordinate 1 editable +
  **an optional 2nd coordinate** = a native N₁×N₂ grid) / unrecognised, surfaces `scanOptIssue` inline,
  and shows the **N₁×N₂ point count**. The 2nd coordinate is an atom-**pair** (`B`) entered in the panel
  (a `CoordEditor` with two 0-based `AtomIndexField`s); coordinate 1's add path is still `AtomInspector`
  "Scan this coordinate".
- `AtomInspector.tsx` — the atom panel on New Job (React; reads a selection held
  in `NewJobScreen` state, uses the shared `fragmentColor` palette).
- `EditPanel.tsx` — edit-mode UI in the Atom rail section (React): target field,
  Preview/Apply, and the direct `fetch` to the sidecar geometry endpoint.
- `RotatePanel.tsx` — "Rotate about axis" UI (unit 3.3), a **sibling of `EditPanel`**
  in the same Edit section but **pure TS** (no sidecar): two picked atoms = axis
  (P pivot, Q direction), numeric angle, live ephemeral preview, one op on Apply.
- `ConstraintPanel.tsx` — the constraint section of the geometry rail (React): a
  **view over the input text** (its only source is `parseConstraintsBlock(content)`),
  one row per constraint (type badge, atoms in our terms, set-vs-measured value, delete)
  plus the range and composition guards (2.5.4b); read-only on an `unrecognised` block
  (2.5.5).
- `vdw-radii.ts` — **cited** van der Waals radii (Å) for steric-clash detection
  (unit 3.2): Bondi 1964 (main group), Mantina 2009 (main-group gaps incl. **B**),
  Alvarez 2013 (transition metals incl. **Pd/Pt**). `vdwRadius(el)` returns
  `undefined` = **UNDETERMINED** for an uncovered element (rule #11 — skip + surface,
  never a guess or 0). A physical table, deliberately **separate** from
  `viewer/highlight.ts`'s 3Dmol-mirroring radii (see below). Pure / node-tested.
- `clash.ts` — `detectClashes(scene, k, activeConstraints): ClashReport` (unit 3.2):
  **inter-fragment** atom pairs closer than `k·(rᵢ+rⱼ)`. Reuses `measure.ts`
  `distance`; excludes pairs with an active **distance constraint** (intentional
  contacts) read via `constraints.ts` `fromOrcaIndex`; reports UNDETERMINED pairs
  apart from clashes. Plus `clashAtomIds` / `undeterminedElements` for the UI. Pure /
  node-tested. **A warning, not a block; clash state is derived over the Scene, `k` is
  app-owned** (never in the Scene). See the section below.
- `xtb-progress.ts` — `formatXtbProgress` (2.5.5-fix-2): renders the `xtb:progress`
  cycle count + an elapsed clock for the pre-optimize button. Pure / node-tested.
- `__fixtures__/butane.finalensemble.xyz` — a real (3-structure) slice of an ORCA
  6.1.0 GOAT run, the test oracle for `ensemble.test.ts`.
- `*.test.ts` (scene / parity / store / placement / fragment-library /
  add-fragment / restore / ensemble) — vitest; this module owns the bulk of the
  suite's tests.

## The index-space invariant (why this module exists)

One flat merged xyz means **one index space** end to end: a picked atom index
indexes the merged xyz, which indexes the ASE mask (ADR-008 decision 2). For that
to hold, **atom count and element order must never change** once a fragment
exists. Geometry operations *move* atoms; they never add, remove, or re-element
them. `replaceFragmentAtoms` enforces this — it throws on any count or
element-sequence change — so an ASE call or xTB round-trip can hand coordinates
back by position and every stored index stays valid.

## `AtomId` — stable atom identity (`ids.ts`, unit 1b; ADR-010 I1 / ADR-016)

A **positional** index is fine while nothing reorders, but the identity core
(ADR-010) wants an identity that is invariant under moves and independent of array
position. `ids.ts` adds it, in the TS Scene, ahead of the Rust core move (ADR-016
lands that in 1c–1e — 1b touches **no** Rust and does **not** rebrand the ~68
positional-index sites; it adds identity to the model only).

**The two atom shapes (`types.ts`).** A parser produces a **`RawAtom`**
(`{element,x,y,z}`, no identity); a **`SceneAtom`** is a `RawAtom` **plus** a
branded `AtomId` and exists only inside a Scene. `SceneFragment` is the in-Scene
subtype of the detached **`RawFragment`** (raw atoms) — so anything that accepts a
`RawFragment` also accepts a `SceneFragment`, but not the reverse. `AtomId` is a
branded `number` (compile-time phantom, erases to a plain integer at runtime).

**Allocation is pure.** The counter lives on the Scene (`nextAtomId`), never a
module global; `stampFreshIds(rawAtoms, start)` returns the id-bearing atoms and
the advanced counter. A fresh scene mints `0..n-1`; `addFragment` mints from
`scene.nextAtomId` for the joining atoms (a detached fragment has **no** ids — they
are minted only on entry). The counter is **monotonic**: `removeFragment` never
rolls it back and an id is never reused (uniqueness is required only *within one
Scene*, so `undoReset` restoring a previous scene wholesale is correct).

**The id-transfer rule — carried, never re-minted (the one that must not be
"optimized").** A geometry replacement (`replaceFragmentAtoms`, `replaceAllAtoms`)
takes **`RawAtom[]`** — the incoming atoms come from ASE / xtb / GOAT / parsed xyz
and **carry no id of their own** (the type has no `id` field, so there is
structurally nothing to mis-transfer). Identity is preserved by `carryIds`, which
takes the **old** atoms' ids **positionally** onto the new coordinates. This is
correct *because* the count + element-order invariant above is already enforced.
Minting fresh ids on a replace would silently void atom identity on every geometry
edit — nothing would crash, no coordinate test would go red. A negative-control
test asserts `id` is **identical (`===`) before and after `replaceAllAtoms`** and
was shown to go red when `carryIds` is swapped for a re-mint (log 1b). **A future
reader must not "simplify" `carryIds` into `stampFreshIds`: the input never carries
an id, and the whole point is to reuse the old one.** A rigid move
(`translateFragment`) is generic over Raw/Scene and preserves ids by construction.

**Identity boundary — the Monaco collapse (removed in unit 2d).** Before 2d, a
manual coordinate edit could win and collapse the Scene to one fragment with
**fresh** ids (identity continuity across arbitrarily hand-edited text is
undefined). Unit 2d **made the xyz block a read-only projection of the Scene** — a
hand-edit of the block is reverted, never adopted — so that path and its identity
boundary are gone. Geometry hand-editing now enters through two conscious doors
(Import xyz as fragment / Replace input); see the Scene ↔ Monaco sync section
below. The `collapse-from-text` op survives only as a **legacy** type for
deserializing pre-2d logs ([oplog.ts](../../src/scene/oplog.ts), ADR-017).

### `scene_json` v2 and the v1 migration

`serializeScene` writes **version 2**: each atom carries its `id`, and the scene
carries `nextAtomId`. `deserializeScene` reads v2 (validating that ids are unique
and every id `< nextAtomId`) and **migrates v1 in place** — it does **not** reject
it. A v1 snapshot (pre-1b: atoms without ids, no counter) has ids minted
`0..N-1` scene-wide in fragment order, `nextAtomId = N`. **Why migration is
mandatory, not optional:** every existing `jobs.scene_json` is v1; returning `null`
for those would make `restoreScene` treat each as a malformed snapshot
(`snapshotRejected = true`) and **silently collapse every multi-fragment job to a
single fragment on open**. A test drives a **real** v1 string (emitted by the
pre-1b code, `__fixtures__/scene-v1.json`, copied verbatim — not synthesized)
through `restoreScene` and asserts the two-fragment layout survives
(`snapshotRejected = false`); its negative control asserts a valid v1 does **not**
deserialize to `null`, and was shown to go red when migration is disabled (log 1b).

**No SQL migration is needed:** the version lives *inside* the JSON string, so an
old row is upgraded on read by `deserializeScene`; `jobs.scene_json` stays a plain
`TEXT` column and the DB schema is untouched.

## Functions (one-line contracts)

Merge / serialize:
- `mergeToAtomLines(scene): string[]` — canonical coordinate rows, fragment order.
- `mergeToXyz(scene, comment=""): string` — `count\ncomment\nrows\n`.
- `serializeScene(scene): string` — versioned JSON (`"version": 2`: per-atom `id` +
  `nextAtomId`). See "`AtomId`" above for the v1→v2 migration on read.
- `deserializeScene(json): Scene | null` — validates shape + version; **never
  throws** on user/DB data, returns `null` on anything unexpected.

Aggregates:
- `totalCharge(scene): number` — Σ fragment charges.
- `atomCount(scene): number`.
- `electronCount(scene): number` — Σ Z − totalCharge; **throws** on an unknown
  element (message names the symbol). Its parity constrains multiplicity parity.
- `atomicNumber(symbol): number` — **H–Rn (Z ≤ 86)**, case-insensitive; throws on
  unknown. Extended from H–Kr in 2.5.0d so Pd(46)/Pt(78) organometallics (the
  cross-couplings ADR-007 names) count electrons instead of silently declining.

Index space:
- `globalIndexOfAtom(scene, id): number | null` / `atomIdAtIndex(scene, gi): AtomId
  | null` — the **bijection AtomId ↔ global index** over `allAtoms` order (= merged
  xyz = viewer index). The resolver the 2c2 selection/measure/constraint pipeline
  keys on: a selection holds ids, these map to where the atom sits now (and back).
  Independent of the viewer's `ViewerAtomTable` on purpose — that names indices for
  3Dmol, these for the core.
- `globalIndex(scene, fragmentId, localIndex): number` — throws on unknown
  fragment / out-of-range local index.
- `fragmentAtomIndices(scene, fragmentId): number[]` — **IS the ASE `mask`** sent to the sidecar
  geometry kernel (`POST /geometry/set-internal`, 2.5.2c): the 0-based global indices allowed to
  move. The index space is the same on both sides of HTTP — index N here is index N in the sidecar's
  request/response xyz (ADR-008). See `wiki/modules/sidecar.md`.
- `locateAtom(scene, globalIndex): { fragment, localIndex } | null`.
- `compositionSignature(scene): string` — ordered `id:size` per fragment, joined;
  **coordinates excluded**. The one canonical way to ask "did the scene's
  composition change?" (atoms/fragments added or removed), the sibling of
  `xyzMatchesScene` ("did the coordinates change?"). **Moved here from
  `MoleculeViewer` in 2.5.2a** — there must not be a second copy. Two consumers,
  both keying the *same* question off it: the viewer re-`zoomTo`s only on a
  signature change (a coordinate-only edit must not move the camera), and
  `NewJobScreen`'s constraint composition-change warning (2.5.4b) fires on a
  signature change. **The pick list no longer keys off this** — since 2c2 it is
  pruned by AtomId (`filterSelection`), which needs no signature.
- `fragmentRanges(scene): { fragmentId, start, end }[]` — **start inclusive, end
  exclusive** (same convention as `OutputMatch` col_start/col_end, Phase 2.7).
  **First consumer (2.5.0c):** `MoleculeViewer` styles each fragment by its
  `[start, end)` index range via `setStyle({index:[…]}, …)`; the end-exclusive
  convention is what makes the water(0–2)+BH₄⁻(3–7) case colour atoms 3–7, not
  0–4 (WebKitGTK-confirmed — see `modules/visualization.md`).

Immutable mutators (each returns a new Scene, never mutates the input):
- `addFragment`, `removeFragment`, `renameFragment`, `setFragmentCharge`,
  `setMultiplicity`.
- `replaceFragmentAtoms(scene, fragmentId, atoms)` — enforces the index-space
  invariant above.
- `replaceAllAtoms(scene, atoms)` (2.5.5) — replace EVERY atom's coordinates from
  one flat list, sliced back to fragments by their `[start, end)` windows
  (`fragmentRanges`) and each slice run through `replaceFragmentAtoms` (so the
  count + element order invariant holds per fragment). Throws if the flat list's
  length ≠ `atomCount(scene)`. The apply path for a whole-scene optimizer (xtb
  pre-opt hands back a merged geometry, same order — `wiki/orca/xtb.md`).
- `translateFragment(fragment, dx, dy, dz)` — rigid-body shift of one fragment
  (same id / composition / internal geometry). Used by placement, and by
  `translateFragmentInScene` (kept for old-log deserialization).
- `translateAtomsInScene(scene, atomIds, dx, dy, dz)` — rigid-body shift of an
  **explicit set of atoms** (by stable id), everything else fixed. The Move-mode
  drag commit (Stage 3.x): the moving set is the dragged atom's perceived connected
  component, not the whole fragment, so after a bond break the pieces move
  independently. Count + order + id + element invariant **by construction** (every
  atom keeps its slot; only selected ids shift — the ADR-008 discipline inlined).
  A whole-fragment set equals `translateFragmentInScene` (backward-compat); a
  no-op (same reference) on an empty set or zero delta.
- `rotateFragment(fragment, axisDir, angleRad, pivot)` — rigid-body **rotation** of
  one fragment about the line through `pivot` with direction `axisDir` (Rodrigues'
  formula; `axisDir` normalized here, so a raw `Q − P` is fine — a zero-length axis
  throws rather than emitting `NaN`). Generic over Raw/Scene, the sibling of
  `translateFragment` (unit 3.3). **Rigid by construction:** every atom maps
  `p ↦ pivot + R·(p − pivot)` with the SAME `R`, so all internal pairwise distances
  are preserved and any point on the axis (`pivot`, and Q which lies on the line) is
  a fixed point (rule #9; c1/c2/c4 in `rotate.test.ts`).
- `rotationAxis(scene, P, Q)` — the axis two picked atoms define: `{ dir:
  normalize(Q−P), pivot: P }`, or **`null`** when an atom is absent or the two
  coincide (no direction). The **single degeneracy test** shared by the mutator
  (→ no-op) and the UI (→ Apply disabled), so they never disagree.
- `rotateFragmentInScene(scene, fragmentId, [P, Q], angleRad)` — rotate ONE fragment
  about the `[P, Q]` axis, every other fragment untouched, ids/order invariant. The
  scene-level mutator the store's `rotateFragment` commits; resolves `[P, Q]` **in the
  target scene** (present by construction — ADR-017). Returns the scene **unchanged
  (same reference)** on a degenerate axis / absent fragment, so a no-op appends no log
  entry. **Rigid TS, not the sidecar** — see the split below.

Parsing / reset detection:
- `parseAtomLines(lines): SceneAtom[] | null` — skips blanks and `#` comments;
  `null` when nothing parses.
- `sceneFromAtomLines(atomLines, opts): Scene | null` — single-fragment scene
  (the "editor" path). `opts.id` is accepted for determinism; defaults to
  `makeFragmentId()`.
- `sceneFromXyz(xyz, opts): Scene | null` — single-fragment scene from a
  **standard xyz string** (count/comment/rows), the shape the SMILES sidecar and
  library molecules use. Skips the count+comment lines then reuses
  `sceneFromAtomLines`.
- `sceneFromOrcaInput(content, opts): Scene | null` — the ORCA-input → Scene
  adapter: extracts the `* xyz charge mult ... *` block, taking the fragment
  charge and `scene.multiplicity` from the header. `null` for a `* xyzfile` block
  (external geometry) or no block.
- `injectSceneIntoInput(content, scene): string` — the inverse: replace/insert
  the coordinate block with the scene's merged canonical rows + `totalCharge` /
  `multiplicity` header, leaving the `!` line, `%` blocks and comments intact.
  The Scene → Monaco write of ADR-008 #6 (absorbed the old
  `viewer/inject-xyz-into-input.ts`).
- `xyzMatchesScene(scene, atomLines, tol=1e-6): boolean` — the reset-detection
  primitive (ADR-008 decision 6). Parses both sides and compares element symbols
  (case-insensitive) + coordinates within `tol`. **Float comparison, never
  string comparison** — formatting differs (`1.0` vs `1.00000000`). Different
  count / element sequence / `null` ⇒ `false`.
- `adoptPreservesScene(current, newContent): boolean` (`debugging/014`) — should a
  whole-buffer adopt **keep** the current (possibly multi-fragment) Scene instead of
  collapsing it to one text-parsed "Molecule" fragment? **True** iff a scene exists
  and `newContent`'s geometry matches it (reuses `xyzMatchesScene` /
  `mergeToAtomLines` — no second comparison); a different/absent geometry ⇒ **false**
  (a real re-adopt). The guard `adoptWholeInput` uses so **Generate Input** (new
  `!`/`%` over the same coords) doesn't destroy the fragment layout.
- `restoreScene(inputContent, sceneJson)` (`restore.ts`, pure) — reconcile a
  persisted snapshot with a job's input when opening/iterating it (ADR-008 #5
  amendment). Returns `{ scene, snapshotRejected }`. Four branches: **no coord
  block** → `{ null, false }`; **no snapshot** (pre-v4 / no scene) → single
  fragment from the text, `false` (not an anomaly); **malformed/wrong-version
  snapshot** → text fragment, `true`; **valid snapshot** →
  `xyzMatchesScene(snapshot, input geometry)` — match returns the *snapshot*
  (multi-fragment layout preserved), mismatch returns the text fragment, `true`.
  The `input_content` is authoritative; the snapshot only annotates it. `true`
  vs a `NULL`-snapshot `false` is the whole point of the flag — one draws a UI
  note, the other is silent.

Impure helper (isolated on purpose):
- `makeFragmentId(): string` — `crypto.randomUUID()`. The **only**
  non-deterministic function; every other function is deterministic so tests pass
  literal ids.

## Canonical xyz format (ADR-008 decision 4)

Per row: element symbol `padEnd(2)`, then each coordinate `toFixed(8)`
`padStart(14)`. Full xyz: `count\ncomment\nrows\n`. A non-finite coordinate is a
programming error → throw, never emit `NaN`. Determinism matters twice: golden
test diffs, and the float-tolerant comparison against the Monaco buffer.

## Electron parity (`parity.ts`, ADR-008 decision 8)

`checkElectronParity(scene): ParityIssue | null`. The electron count
(`electronCount` = Σ Z − totalCharge) fixes the **parity** of the allowed spin
multiplicity: even electrons ⇒ odd multiplicity (singlet/triplet/quintet), odd
electrons ⇒ even multiplicity (doublet/quartet/sextet). A mismatch returns a
`ParityIssue` with the electron count, the offending multiplicity, a
**smallest-first** list of valid multiplicities (`[1,3,5]` or `[2,4,6]`), and an
**explanatory** message (how many electrons, why that parity, what to use) — a
teaching moment, not a diagnostic "invalid multiplicity". The message names the
*nearest* valid multiplicity to the one entered (2.5.0d fix: for multiplicity 8
that is 7, not the smallest value 1 — `suggested` stays smallest-first, but the
prose points at the closest fix).

Scope of the check, deliberately narrow:
- It validates **arithmetic possibility only**, never physical plausibility.
  Whether a triplet is a sensible ground state for *this* molecule is the
  chemist's call; we only catch the provably-impossible class (the error ORCA
  reports cryptically ~30 s into a run).
- Returns `null` for an empty scene (nothing to validate) and for an element
  beyond the H–Rn table (can't count electrons → no parity opinion; it swallows
  the `electronCount` throw rather than crashing the caller).

**Why the UI warns, not blocks.** `InputBuilderForm` shows the issue inline but
still lets Generate proceed: the user may build a scene incrementally and pass
through a temporarily odd state, and ORCA itself rejects the truly impossible. We
inform, we don't forbid. **This human-path leniency is deliberately *not* extended
to an AI draft:** [ADR-014](../architecture/adr-014-ai-integration-boundary.md)'s
charge/multiplicity amendment makes an AI-drafted `.inp` *refuse* an invalid state
rather than warn — an AI emits a whole artifact in one shot and has no incremental
excuse, so it must be born valid. Same guard, different response, because the two
callers have different relationships to intermediate states.

## The store (`store.ts`) folds over the operation log (unit 2b) and Scene ↔ Monaco sync

`useSceneStore` (Zustand) holds `{ log, scene }`. Since unit 2b the
**`scene` is DERIVED**: `scene === current(log)`, always — that equality is the
store's core invariant and what makes the "mutator bypasses the log" defect
*impossible by construction* (ADR-017 / the 2b main risk). **There is no
`setScene`.** The only two ways the log changes are `commit(op, resultScene)`
(append one entry — the single door for a geometry op) and `installLog(log)`
(replace the whole log — a lifecycle event: seed / New iteration / clear), plus
the three pointer moves `undo` / `redo` / `jumpTo`. Every write to `scene` in the
store is `scene: current(log)` right after the log changed. Convenience mutators
(`addFragment`, `removeFragment`, `renameFragment`, `setMultiplicity`,
`replaceFragmentAtoms(via)`, `translateAtoms(id, atomIds, dx, dy, dz)` — the
rigid-body drag commit, Stage 3.x: `translateAtomsInScene` → a `translate-atoms`
op over the dragged atom's connected component, one op with the TOTAL delta on
mouseup, a no-op on a zero delta / empty set (`translateFragment(id, dx, dy, dz)`
→ `translate-fragment` remains for the whole-fragment path and old logs) — and
`rotateFragment(id, [P, Q], angleRad)` — the rigid **rotation** commit, unit 3.3:
`rotateFragmentInScene` → a `rotate-fragment` op, one op on Apply carrying the axis
atoms + final angle, a no-op on a zero angle or a degenerate axis) compute the result
from the pure `scene.ts` functions and funnel through `commit`; `seedScene(scene,
source)` is a
thin `installLog` of a fresh `restore-snapshot`-seeded log (or the empty log for a
`null` scene). Undo/redo are **deep** now (the whole log), superseding the old
one-step `previous`/`undoReset`. A store test asserts `scene === current(log)`
after **every** action, with a proven-biting negative control (control (a)).

**Reference stability is still a contract.** `MoleculeViewer` redraws on a new
`scene` reference (`useEffect([scene])`). `current(log)` returns the **same frozen
snapshot object** across undo/redo, and a no-op action returns state unchanged, so
navigating history does not churn the viewer. `store.test.ts` asserts repeated
reads are `===`.

**`scene: null` — the three consumers (the −1 pointer's defined behaviour).** The
log's pointer is `-1` (empty / fully undone) exactly when `current(log)` is `null`.
Three consumers key off that: the **Scene↔Monaco sync** writes nothing to Monaco on
a `null` scene and clears the lineage (`seedScene(null, …)` → empty log) when the
coordinate block is deleted; the **input builder** reads charge/multiplicity only
when a scene exists; **`create_job` map-minting** takes its existing skip branch
(no scene → `{"skipped": …}`, unit 1e). Each is a test.

**History panel + New iteration (unit 2b).** `HistoryPanel.tsx` is a **read-only**
list of `describe()` lines — the current step highlighted, a click **jumps the
pointer** (the same undo/redo mechanism), plus Undo/Redo buttons and Ctrl/Cmd+Z /
Ctrl/Cmd+Shift+Z (skipped while a text field / Monaco has focus, so the editor
keeps its own undo). On **New iteration**, `restoreSceneLog` (`restore.ts`) restores
the persisted `scene_log_json` **only if its current snapshot equals the co-written
`scene_json`**; a mismatch **rejects the log with a named reason and honours the
snapshot** (`scene_json` is the map-minting contract, unit 1e — more authoritative
than history). A legacy job (no log) seeds a fresh log ("history begins here"). An
iteration boundary (`restore-snapshot`) is appended on top so history carries across
iterations. `restore.test.ts` covers all branches incl. the diverged-log control (b).

**Sync (ADR-008 #6), as wired in `NewJobScreen` — the coordinate block is a
READ-ONLY PROJECTION of the Scene (unit 2d; ADR-010 authority split: input text
owns chemistry `!`/`%`, the Scene owns geometry):**
- **Scene → Monaco (one-way generator):** on a scene change, `injectSceneIntoInput`
  writes the merged block; a guard skips the write when the text already matches
  (prevents the echo after a content→scene sync and never reformats a manual edit).
- **Monaco → Scene:** on the 500 ms debounce, using the **same** locator
  (`sceneFromOrcaInput`) and `xyzMatchesScene(scene, atomLines)` (**parsed floats,
  tol 1e-6 — never string compare**), four branches: **no scene yet** → a block
  typed/pasted into an empty editor **seeds** it (`seedScene(parsed, "text-adopt")`
  — keeps template/generated-input adoption alive); **block matches** → leave the
  scene (the user edited `!`/`%` keywords — the common, allowed path, they flow to
  the generated `.inp`); **block diverged or deleted, scene present** → the block is
  read-only, so **revert** it from the Scene (`injectSceneIntoInput`, keeping the
  `!`/`%` edits) and note the reverted edit. The pre-2d "diverged → the text wins →
  `collapseFromText`" branch is **gone**; a block hand-edit no longer touches
  geometry (there is no `setContent`-to-`collapse` path).
- **Two doors carry geometry hand-editing** (ROADMAP requires the capability
  survive the read-only block): **Import xyz as fragment** — paste xyz →
  `sceneFromXyz` → `addFragment` (a logged `add-fragment`; the typical path); and
  **Replace input** — a one-shot escape: confirm, unlock the whole buffer, paste a
  different calculation, **Adopt** it as a fresh `text-adopt` Scene (a new log; the
  old lineage is discarded). After adoption the block re-locks (read-only again).
  Whole-buffer replacers that already existed (template pick, builder Generate) go
  through `adoptWholeInput`, **guarded by `adoptPreservesScene`** (see below), so they
  aren't caught by the revert but also **don't blindly collapse a multi-fragment scene**.

**The adopt-preserve rule (`adoptPreservesScene`, `debugging/014`).** `adoptWholeInput`
(builder **Generate Input**, template pick) must NOT re-adopt the merged text as a
single "Molecule" fragment when the geometry didn't actually change — **"Generate
Input" rewrites only the `!`/`%` lines over the SAME coordinates**, and a blind
`text-adopt` silently merged substrate+reagent into one fragment (breaking
rotate/move/clash/per-fragment). `adoptPreservesScene(current, newContent)` returns
**true — keep the Scene** — exactly when a scene exists and the new content's geometry
matches it (same `xyzMatchesScene` primitive, no second comparison); a genuinely
different geometry (Replace input with another molecule), no current scene, or no
coordinate block returns **false — a real re-adopt**. This mirrors the Monaco→Scene
"block matches → keep" branch: a text change that doesn't change geometry never
disturbs the Scene, whether by hand-edit or by Generate/adopt. **On restore the
source of truth is the persisted `scene_json`, never a re-adopt from text** —
`restoreScene` was measured correct (it honours the snapshot via `xyzMatchesScene`);
the merge bug was the adopt path, not restore.

**Why no jsdom test for the revert loop.** The Monaco↔Scene *effect* is the manual
gate (unit 2d m1–m5, real WebKitGTK) — jsdom has no Monaco. The pure **decision**
(`store.test.ts`: revert on divergence, keep on a keyword edit, seed on an empty
scene) and the two doors' invariants (**c1** import builds an `add-fragment` op
preserving atom count+order; **c2** a Replace re-seed installs a fresh log with no
lineage leak) are vitest-covered, each with a proven-biting negative control.

**Regression guard on the round-trip (the subsystem's finest wire).** Adding a
fragment makes the scene multi-fragment → Scene→Monaco injects → ~500 ms later
Monaco→Scene re-parses and asks `xyzMatchesScene`. If ordering/formatting drift
made that FALSE, the block would **revert to the projection** right after the add
(the multi-fragment layout survives in the Scene, but the text would churn).
`add-fragment.test.ts` locks this: it drives the real inject → parse →
`xyzMatchesScene` path a real add produces and asserts the comparison stays TRUE
(so the effect leaves the block matching). It's a pure-function simulation, not a
rendered-component + fake-timers test, because the suite has no jsdom — and the
comparison is exactly where the bug would live.

## The operation log (`oplog.ts`, unit 2a; ADR-017) — the store folds over it (2b)

Editor state is a **fold over a log of typed operations** (ADR-010). `oplog.ts` is the **pure types
and pointer semantics** — it imports nothing from the store, viewer, Monaco, DB, or Rust. Unit 2b
wires the store onto it (see "The store folds over the operation log" above); `goto(log, pointer)`
is the history-panel jump, and `SnapshotSource` covers the three whole-scene seeds (`new-iteration`
/ `text-adopt` / `library`).

- **`Op`** — a tagged union with **one variant per Scene mutator** (`add-fragment`,
  `remove-fragment`, `rename-fragment`, `set-fragment-charge`, `set-multiplicity`,
  `translate-fragment`, `translate-atoms` `{atoms: AtomId[], delta}` — the drag's
  connected-component move (Stage 3.x), `rotate-fragment` `{axisAtoms: [P, Q], angleRad}`,
  `replace-fragment-atoms` `{edit: via 'set-internal'|'xtb'|'conformer'}`,
  `replace-all-atoms` `{edit: via 'xtb'}`) plus the store act `restore-snapshot`, and the
  **legacy** `collapse-from-text` (no post-2d path emits it — kept only to deserialize pre-2d
  logs). The mutator↔Op table is in ADR-017 (so 2b finds no hole). Geometry ops
  reference atoms by **`AtomId`**, not a positional index — the log is AtomId-native ahead of the
  2c2 pipeline move. **`rotate-fragment` stores the two axis ATOMS, not the derived vector** (unit
  3.3): the approach axis IS two atoms by definition (ADR-007), the journal line "about O→C" serves
  the teaching mission, and the resolve is safe — an op applies to its own snapshot, where P and Q
  are present by construction.
- **`describe(op): string`** — one human lab-journal line per variant, **AtomId-native** provenance
  (the id chain, e.g. "Set dihedral 4-7-12-15 to 30°", "Rotate BH₄⁻ 30° about 4→7"). Cheap and total.
  **`describeInScene(op, scene): string`** (2c2, Variant A) is a *presentation* over it for the
  history panel: for a `set-internal` op it renders the picked atoms — and for a `rotate-fragment` op
  the two axis atoms P→Q — by the **global index they occupy in the passed scene** (so the journal
  reads in the same 0-based space the rest of the UI is labelled with), delegating every other
  variant to `describe`. `HistoryPanel` calls it with the entry's **own** snapshot; both ops preserve
  atom count + order, so their atoms are always present there and the resolve always succeeds (no
  `[removed]` case arises).
- **`SceneLog {entries, pointer}`**, `LogEntry {op, scene}` — `append` (truncates the redo tail),
  `undo`/`redo`/`current`, `logInvariant`. Pointer invariant **`-1 ≤ pointer < len`**, `-1` = the
  empty scene (`current → null`), so undo can reach a blank canvas.
- **Serialization** — log format **v1**, versioned *independently* of the Scene JSON; each entry's
  scene is embedded as its `serializeScene` **string** (Scene format v2, migration reused).
  `deserializeLog` never throws, returns `null` on bad data (the `deserializeScene` contract).

**The one rule that must not be "optimized" (ADR-017 decision 1, the sibling of the `carryIds`
warning above).** Each entry **materializes the resultant snapshot** — the snapshot is the **source
of truth**, the `Op` is **provenance, not a recipe**. A future reader will be tempted to drop the
snapshots and *replay* the ops to reconstruct state ("smaller, DRY"). **Do not:** the geometry ops
run through ASE in the sidecar, so a replay makes history a **function of the installed ASE version**
— a dependency bump would silently rewrite geometries computed months ago, and a scientific
instrument's history must not change retroactively. The argument stands **verbatim** both in ADR-017
and as the header comment of `oplog.ts`, on purpose. Two negative controls demonstrably bite (log
2a): **(a)** breaking tail-truncation reddens "redo after append impossible"; **(b)** neutering the
deep-freeze reddens the immutability gate (`Object.freeze` is a real runtime guarantee in every
environment, and ES-module strict mode makes a write to a frozen field *throw*, so it bites in prod).
Sizes are **measured** (ADR-017): ~2.9 KB per 38-atom snapshot, ~3.5 KB per entry, ~345 KiB per
100-op session → **no length cap yet**, deferred with numbers.

## Boundary with `viewer/xyz-format.ts` (no duplicate parsers)

All ORCA-input ↔ Scene text I/O lives in this module (`sceneFromOrcaInput` /
`injectSceneIntoInput`). `viewer/xyz-format.ts` keeps only standard-xyz-string ↔
atom-line **formatting** (`xyzToAtomLines`, `atomLinesToXyz`), consumed by
`import-file.ts` and `MoleculesScreen` (which manages library molecules as stored
xyz *strings*, not Scenes — deliberately not Scene-backed). There is **no duplicate
parser**: the old viewer parsers `parse-xyz-from-input.ts`, `inject-xyz-into-input.ts`,
and `parseChargeMult` no longer exist — they were folded in here per ADR-008
(consolidation closed in `[2026-07-28] 2.5.0d-1`).

## Fragment placement (`placement.ts`, ADR-008 #7)

`placeFragment(scene, fragment, gap = 3.5)` translates a copy of `fragment` so it
sits clear of everything already in the scene. It separates the two axis-aligned
bounding boxes (AABBs) **along the axis where the scene is smallest** (ties → x):
for a substrate lying along x that means approaching from the side, not down the
chain (the naive centre-of-mass + fixed vector lands mid-chain for elongated
substrates). On the other two axes the fragment is centred over the scene so it
faces the object rather than sitting in a corner.

**The clearance is a guarantee, not a hope.** After placement every fragment atom
has its coordinate on the chosen axis ≥ (scene max + gap) while every scene atom is
≤ (scene max), so the difference on that one axis alone is ≥ gap for every
scene/fragment pair — hence Euclidean distance ≥ gap. No pairwise scan. Empty scene
→ the fragment is returned unmoved (it *is* the first fragment). The *orientation*
is deliberately crude and chemically meaningless; exact positioning (Bürgi-Dunitz)
is the geometry editor's job in 2.5.2. Placement is a pure translation, so a
fragment's internal geometry is untouched — `placement.test.ts` asserts both the
≥ gap separation (including a second fragment clearing the first) and that every
intra-fragment distance is preserved to 1e-9.

## Fragment library (`fragment-library.ts`, ADR-008 #9)

`FRAGMENT_LIBRARY` is a curated list of the reagents a reaction study starts from:
BH₄⁻, H⁻, OH⁻, CN⁻, Cl⁻, **Na⁺/Li⁺/K⁺ (+1), Mg²⁺ (+2)** (tail-2 counterions —
monatomic, closed-shell, empty `reference` like H⁻/Cl⁻), H₂O, NH₃, CH₃OH (the chemistry — why BH₄⁻ is tetrahedral,
why water is 104.5°, what a hydride nucleophile is — is in
[`chemistry/reagent-geometry.md`](../chemistry/reagent-geometry.md)). Each `LibraryFragment` carries `atoms`,
`charge`, a **non-empty `provenance`** (where the geometry came from), and a
`reference` of ideal internals (bonds/angles). `libraryFragmentToScene(f)`
instantiates it as a scene fragment with a fresh id, deep-copied atoms,
`source: "fragment-library"`, `sourceLabel = key`.

Geometries are built from **ideal symmetry + a named bond length/angle**, never
recalled loosely: H₂O / NH₃ from bond length + angle (bent C2v / pyramidal C3v
builders, experimental reference values); OH⁻ / Cl⁻ / H⁻ diatomic/monatomic. Three
lengths come from **ORCA r²SCAN-3c Opt** in isolated, cleaned-up job dirs (so the
provenance names a real source, not a memory or a circular doc reference): BH₄⁻
B–H 1.2368 Å (T_d), CN⁻ C≡N 1.1743 Å, and **CH₃OH** in full (the one polyatomic a
6-atom Z-matrix can't be hand-built safely — its optimised coordinates are
hardcoded).

**What the library tests actually verify — and what they don't.** `fragment-library.test.ts`
recomputes every declared `reference` bond/angle *from the coordinates* (1e-3 Å /
0.1°). For CH₃OH (hardcoded coords) this is a genuine independent cross-check:
coordinates vs declaration were written separately, so a transcription slip in
either fails the test. For the seven symmetry-built fragments the coordinates are
*generated from* the same reference values, so the test proves the **constructor is
correct** (a bad T_d / C2v / C3v formula makes the recomputed angle disagree) —
it does **not** prove the reference number itself is physically right. The guarantee
on the numbers is `provenance` + review, not the test. This is why the worst bug
class here (a wrong-but-converging bond length) is defended by naming the source,
not by a green suite. No runtime RDKit generation (MMFF lacks params for ions like
BH₄⁻).

## Extensible reagent catalog — curated + user (`reagent-catalog.ts`, Phase 4.2 tail-2)

The catalog has **two tiers, deliberately kept apart**:

- **Curated (built-in)** — `FRAGMENT_LIBRARY` above. Each entry carries a **`reference`
  internal-coordinate contract** the tests recompute from the coordinates, so a curated
  geometry can't silently ship wrong. `libraryFragmentToScene` instantiates it, `source:
  "fragment-library"`.
- **User** — a reagent the researcher saves, persisted as a **`molecules` row with a role
  flag** (`is_reagent`, schema **v12**; `wiki/modules/tauri-core.md`). **Reuses the molecules
  table** — the `charge` column was already there — rather than a new table. `create_reagent`
  / `list_reagents` are the role-split commands; `list_molecules` filters role 0, so the
  molecule library and its screen are untouched (existing rows are all role 0). A user reagent
  has **no `reference` contract** (user provenance — no verified geometry); `userReagentToFragment`
  gives it `source: "library"` (a saved library item), **never** `"fragment-library"`.

**The curated↔user distinction is a rule, not a display nicety (decision 2026-08-07).** A
built-in reagent's geometry is verified; a user reagent's is not. The palette shows them as two
visually distinct groups, and the two never merge by type (a `Molecule` has no `reference` field;
a `LibraryFragment` does). Mislabelling a user reagent as `"fragment-library"` would let the UI
imply a guarantee it can't make — `reagent-catalog.test.ts` (c4) guards against exactly that.

**Charge is mandatory at save (ADR-014, no silent footgun).** `create_reagent` takes `charge`
as a plain `i32` — never an `Option`, never a default. The save dialog refuses until a valid
integer charge is entered; multiplicity is **not** asked (electron parity + charge determine it,
and the Scene validates it — `parity.ts`). A user reagent's charge then **flows into the scene
total by the same path as a built-in's** (Σ fragment charges, ADR-008 #8) — no special case
(c1/c2). Geometry is captured on save via `fragmentToXyz` (a picked scene fragment) or a pasted
xyz block.

## GOAT conformer ensemble (`ensemble.ts`, 2.5.1a/b)

The primitive behind conformer search (ADR-007's mandatory first step, pulled up
from Phase 4.5 because SMILES fragments arrive as an arbitrary ETKDG conformer).
Pure; parser **written against a real ORCA 6.1.0 run**, not from memory — see
`wiki/orca/goat.md` for the observed file format and cost.

- `parseEnsemble(text): Conformer[] | null` — parse a `*.finalensemble.xyz`
  multi-frame xyz. `Conformer = { atoms, energy (Eh, `NaN` if the comment has no
  leading number), index }`. Energy is the leading token of the `"<energy>
  converged=true"` comment. Malformed/empty → `null`, never throws (same contract
  as `deserializeScene`). Observed ensembles are energy-sorted (global min first).
- `conformerMatchesFragment(fragment, conformer): boolean` — the exact composition
  check `replaceFragmentAtoms` enforces (count + element sequence, via the shared
  `normalizeElement`), but as a **predicate** so 2.5.1b can show a clear refusal
  instead of catching a throw. GOAT preserves atom order (**verified on the run**),
  so a fragment's own ensemble always matches — this guards against the wrong
  ensemble reaching the wrong fragment.
- `goatInputForFragment(fragment, mult=1): string` — a `! XTB GOAT` input for **one
  fragment in isolation**: charge is `fragment.charge` (not the scene's
  `totalCharge`), multiplicity 1 (all library fragments are closed-shell).
- `deltaEKcal(conformers): number[]` (2.5.1b) — ΔE of each conformer vs the lowest,
  in **kcal/mol** (`HARTREE_TO_KCAL_MOL`), the unit a chemist reads. `NaN` energies
  pass through (the UI shows a dash).
- `planConformerApply(storeScene, snapshotFragment, conformer): ConformerApply`
  (2.5.1b) — decides "Use this conformer" **purely** (so it's testable): `replace`
  in place if the snapshot's fragment id is still in the store scene; else `new`
  single-fragment scene; `refuse` (no throw) if `conformerMatchesFragment` fails.

- `isGoatInput(content): boolean` (2.5.2a) — is this a GOAT conformer-search
  job? Scans **only `!` keyword lines** (ignores `#` comments and the `* xyz`
  block), matches the `GOAT` token on word boundaries, case-insensitively. Drives
  the convergence panel's `variant` (a GOAT run's per-cycle bar is one inner
  optimisation of one candidate, not search progress — see
  `wiki/orca/goat.md` and `wiki/modules/frontend.md`).

`normalizeElement` is now **exported** from `scene.ts` so this predicate,
`replaceFragmentAtoms`, and `xyzMatchesScene` share one element normalisation.

### `scene_json` for a GOAT job (2.5.1b — one column, one meaning)

A GOAT job runs on **one fragment**, so its `input_content` holds only that
fragment's coordinates. Its `scene_json` is therefore a **single-fragment scene of
that same fragment** — the snapshot annotates its own single-fragment input, so
`restoreScene`'s `xyzMatchesScene(snapshot, input geometry)` holds and the snapshot
is honoured with **no special branch**, and the fragment's `id`/`name`/`charge`
survive a restart. **Not the whole scene:** that would fail `xyzMatchesScene`
against the single-fragment input and `restoreScene` would silently reject it —
loading a second, implicit meaning into one column. One column, one meaning
(`ensemble.test.ts` asserts this round-trip explicitly).

**Applying a conformer back — two branches** (both needed; `planConformerApply`):
the store is a singleton that survives the New Job → Job detail navigation, so when
the search finishes the original scene is usually still there → **`replace`** the
fragment (same id) in place, leaving the rest of the scene untouched, then navigate
to New Job with `keepScene` (skips the usual store reset). If the scene was cleared
(other session) → **`new`** single-fragment scene from the snapshot + the chosen
conformer's coordinates. Composition is checked first; a mismatch **refuses**
cleanly.

## Atom selection (`selection.ts`, 2.5.2a)

The geometry editor's pick list — pure, node-tested, React-free. Since **unit
2c2** a selection is an **ordered list of stable `AtomId`s**: a pick means "this
physical atom", not "whatever is at index N". The UI (`NewJobScreen`) holds it in
component state (**not** the scene store — the store stays a pure geometry
wrapper, ADR-008 #10) and drives every change through these functions.

- `MAX_SELECTION = 4` — a dihedral's four atoms.
- `toggleAtom(selection: AtomId[], id): AtomId[]` — one click, new array:
  already-selected → remove; new & under the cap → append (order kept); new &
  **at the cap → the selection becomes `[id]`**. The full-list rule is the
  decision: **not FIFO.** Silently evicting the oldest atom would leave the user
  measuring a set different from the atoms they see highlighted — a wrong-atom
  measurement with no visible cause. A hard reset to the just-clicked atom is
  unambiguous ("fifth click resets").
- `filterSelection(selection, scene): AtomId[]` — **the 2c2 dividend.** Drop only
  the ids **no longer in the scene** (their fragment was removed); keep the rest in
  click order. Returns the **same array reference** when nothing is dropped, so a
  coordinate-only edit is a no-op that doesn't churn React state. Because the pick
  list is ids, removing an **unrelated** fragment leaves the selection untouched —
  the thing the old positional guards could not do.
- `describeAtom(scene, globalIndex): AtomDescription | null` (positional — used by
  the constraint panel, which speaks ORCA indices) and `describeAtomById(scene,
  id)` (resolves the id, then the same body). Both `null` for absent, non-throwing.

### The selection survival rule — 2c2 replaced two guards with one

Before 2c2 the pick list was **positional global indices**, and a removal renumbers
every later atom, so "the same atom" had **no operational definition**: a kept
index would silently re-point at a different atom (remove water(0,1,2) from
water+BH₄⁻ with the boron at global 3 picked → global 3 is now a BH₄⁻ hydrogen). Two
guards handled that: `selectionSurvives` (a `compositionSignature` predicate that
kept a selection only on an unchanged signature or a pure append, else cleared it
whole) and `validateSelection` (a range-only second echelon). **Both are removed in
2c2.** An `AtomId` *is* the operational "the same atom", so `filterSelection` keeps
exactly the ids still present and clears nothing else — the boron stays selected
through the water removal, now correctly resolving to global 0. This is a
**conscious behaviour change** (the old clearing was *correct* for the positional
space; ROADMAP 2c2 records it as intentional), and it is the reason the move to
`AtomId` lives in Stage 2 rather than the identity-only Stage 1.

## Measurement (`measure.ts`, 2.5.2b)

Reads a list of **global indices positionally**: 2 atoms → `distance(i,j)`, 3 →
`angle(i, vertex, j)` with the **middle pick as the vertex**, 4 →
`dihedral(i,j,k,l)` along the chain (axis `j–k`). Two entry points (2c2):
`measureSelectionByIndex(scene, indices)` is the positional core (used by the
constraint panel, ORCA-index space), and `measureSelection(scene, AtomId[])`
resolves each id to its current global index and delegates — so a measurement
follows the atoms it named across a fragment removal. Both return a tagged
`Measurement` (`none | distance | angle | dihedral`), whose `atoms` field holds the
**resolved global indices** (what the viewer/inspector render against) and
`sameFragment` — inter-fragment distance is a future reaction coordinate (ADR-007)
and must read apart from internal geometry. **The math is index-based and
untouched**; only the addressing moved to ids.
Degenerate inputs (coincident atoms, zero vector, collinear inner triple for the
dihedral, out-of-range index) return **null, never NaN**; `measureSelection` maps
null → `none`.

### Coord-level primitives + `measureByCoords` (the results-viewer sibling, F1)

The ASE math lives in ONE place: `distanceCoords(p,q)`, `angleCoords(a,vertex,b)`,
`dihedralCoords(p0,p1,p2,p3)` take already-resolved `Vec3` points (now exported) and
carry the geometric degeneracy contract (zero vector / collinearity → null). The
Scene-based `distance/angle/dihedral(scene,…)` became **thin delegates** over them —
they keep only the *index-space* concerns (index→point resolution, and the
repeated-**index** `Set(...).size < N → null` rejection, which is an index concern, not
a geometric one). A behavior-preserving extraction: the 2.5.2b Scene tests stayed green
unchanged.

`measureByCoords(coords: Vec3[], picked: number[])` is the coord-array sibling of
`measureSelectionByIndex` — same positional rule (2→distance, 3→angle middle-vertex,
4→dihedral), same degenerate/out-of-range/0-1/≥5 → `none`, routing through the same
`*Coords` core. It is used by the results **TrajectoryPlayer** ([results-ui](results-ui.md),
F1), which holds a frame's raw coordinates (`Frame.xyz_angstrom`, frame/elements order —
the SAME 0-based order the picks are in) rather than a `Scene`. `sameFragment` is always
`true` (a parsed results geometry is one geometry — the field is inert here but the
`Measurement` shape is reused whole). Because both paths delegate to the same `*Coords`,
they can never diverge on the ASE conventions; `measure_by_coords_matches_scene_path`
deep-equals the two on the butane gauche fixture (the 60-side dihedral is the tripwire),
and `measure_by_coords_repeated_index_matches_scene_path` pins that `measureByCoords`
re-applies the repeated-index guard the `*Coords` primitives shed (its negative control:
drop that guard → red, while the different-index cross-check stays green).

### Conventions, pinned to ASE source (the 2.5.2c dependency)

These are not just a user readout: 2.5.2c's acceptance test applies a target
d/θ/φ through ASE, reads coordinates back, and **re-derives all three with this
module** to check them. A convention that diverged from ASE would fail a correct
core or — worse — pass a wrong one. So the conventions are fixed from the **real
ASE source** in the sidecar venv (`ase/geometry/geometry.py`, `ase/atoms.py`;
ASE checked 2026-07-29), not from memory:

- **Angle vertex = the middle index.** `Atoms.get_angle(a1, a2, a3)` is the angle
  between `a1-a2` and `a3-a2` (`atoms.py::get_angles`: `v12 = a1s - a2s`,
  `v32 = a3s - a2s`) — `a2` is the vertex. Our `(i, vertex, j)` matches
  positionally. Range `[0, 180]`.
- **Dihedral range `[0, 360)`, NOT `(-180, 180]`.** `geometry.py::get_dihedrals`
  computes `atan2` into `[-π, π]` then executes
  `dihedrals[dihedrals < 0.] += 2*pi` **before** converting to degrees — folding
  to `[0, 2π)`. We replicate that fold verbatim. Confirmed numerically against
  ASE on the butane fixture: anti = 179.998°, gauche = **67.523°** (the 60 side,
  not 300 — which is exactly what the `[0,360)` fold with vectors
  `v0=a1-a0, v1=a2-a1, v2=a3-a2` produces). `measure.test.ts` locks the gauche
  value; it is the tripwire if 2.5.2c's ASE call ever folds the other way.
- **Reversal invariance / reflection.** `dihedral(i,j,k,l) === dihedral(l,k,j,i)`
  (ASE-confirmed to full precision). A reflection through one axis (improper
  rotation) sends `φ → 360 − φ`; distance and angle are reflection-invariant.
- **Collinearity by cross-product norm, not angle.** The dihedral is undefined
  when an inner triple is planar; we threshold the **normalised cross-product
  magnitude** (== |sin θ|, scale-free, well-conditioned near 0/180°), not an
  `acos` angle.

`measure.test.ts` asserts invariants, not literals from our own constructor:
water H–O–H 104.52° / O–H 0.9572 Å and BH₄⁻ H–B–H 109.47° (from the
fragment-library source), the butane dihedrals above, the symmetries, a mirror
(`φ → 360 − φ`), and the load-bearing one — **rigid motion** (an explicit proper
rotation + translation of the whole scene leaves all three unchanged to 1e-9;
this catches a bug in the math, not in a single number).

## Constraints (`constraints.ts`, 2.5.4a; UI helpers + guards 2.5.4b) — input text is the source of truth

The pure generate / parse / inject layer for the ORCA `%geom Constraints` block.
**Decision (logged): the ORCA input *text* is the single source of truth for
constraints**, exactly as it is for the `!` keyword line and the geometry block.
The 2.5.4b UI panel is a *view over the text*, never a parallel store — a
second home for constraints would drift from the input the same way a parallel
Scene would drift from the coordinate block if `xyzMatchesScene` didn't force the
comparison. So every operation round-trips through the text, and the invariant
`parse(inject(x, cs)) === cs` is a test.

- `Constraint` = `distance` (B, 2 atoms) | `angle` (A, 3) | `dihedral` (D, 4) |
  `cartesian` (C, 1); `value?` present → freeze at that value, absent → freeze at
  current geometry. Atoms are in the 0-based merged-xyz / ASE-mask space (ADR-008).
- **Index base = 0**, settled by a real ORCA 6.1.0 run (`wiki/orca/constraints.md`),
  NOT memory. OrcaStudio's space is already 0-based, so `toOrcaIndex` /
  `fromOrcaIndex` are the identity — but every index is routed through them (in
  terms of `ORCA_INDEX_BASE`) so the code states the fact instead of relying on the
  coincidence.
- `injectConstraints` replaces an existing `Constraints` sub-block or inserts a new
  one **without disturbing sibling `%geom` settings** (maxiter, …) or the geometry:
  no `%geom` → a full block before the coordinate block; `%geom` present, no
  `Constraints` → sub-block inserted inside; `Constraints` present → replaced in
  place (never duplicated). Block location uses a depth-counting token scan so the
  inner `Constraints … end` and the outer `%geom … end` are told apart.
- `parseConstraintsBlock` returns `null` when there is no block *or* it's
  unrecognised, `[]` for a present-but-empty block, the constraints otherwise.
- `inspectConstraintsBlock` (2.5.5) is the richer view: **`absent` | `parsed{cs}` |
  `unrecognised{sample}`**. **We rewrite only what we fully recognised** —
  `injectConstraints` reformats the whole block, so a block holding a `#` comment
  or a token we can't parse must be **read-only** (else the next add/delete
  silently destroys it — the 2.5.4b data-loss bug). It finds the live block on
  comment-masked text (a commented-out block is `absent`) but reads the inner
  content from the original (a comment *inside* the block → `unrecognised`). The
  panel goes read-only and the Run/xtb/add paths are disabled on `unrecognised`.
- `valueText` (2.5.5) preserves a user's exact numeric text (`90.0`, not `90`)
  through a rewrite — the parser sets it only when the text isn't the canonical
  rendering of the number, so canonical constraints round-trip clean.

### The two guards (2.5.4b) and the no-remap rule

The panel (`ConstraintPanel.tsx`, see `wiki/modules/frontend.md`) is a view over
the text; these two pure functions are the safety it must carry because ORCA
segfaults on a bad index rather than reporting it:

- `constraintIndexIssues(cs, atomCount): { constraint, badIndices }[]` — every
  constraint whose atom indices fall outside `[0, atomCount)`, with the offending
  indices (0-based). The panel marks those rows; `NewJobScreen` **blocks Create /
  Create & Run** on a non-empty result — the one place a run is refused on input
  *content*. Test: 38-atom scene, constraint on 37 → clean; drop a fragment
  (33 atoms) → 37 is in `badIndices`.
- `constraintFromSelection(selection, value?)` — build a constraint from an ordered
  2/3/4-atom pick (the same length→kind rule as `measureSelection`); `value`
  omitted → freeze as-is. `sameConstraint(a, b)` is the dedupe guard for a repeated
  "Constrain selection".

**No remap after a composition change — deliberately.** When a fragment is removed
the `%geom` indices are *not* rewritten (Scene→Monaco touches only the coordinate
block). We do **not** guess "the same atom" and re-point them: that has no
operational definition after a removal — **the exact call made for `selection` in
2.5.2a** (`selectionSurvives`, "a silent remap is worse than a lost click"). The
2.5.4b response is the same in spirit: don't remap, *warn* — via the existing
`compositionSignature` (no second notion of "composition changed"), listing what
each constraint names now so the user verifies by eye.

## Steric-clash detection (`clash.ts` + `vdw-radii.ts`, unit 3.2)

After any geometry change (a rigid drag release, an edit apply, a placement — all
mutate the Scene) the coarse positioning can overlap two fragments. `detectClashes(
scene, k, activeConstraints)` flags **inter-fragment** atom pairs closer than
`k·(rᵢ+rⱼ)` of their vdW sum, as a **warning, not a block** (the drag is coarse; a
close contact at setup is expected and refined in the editor). Four decisions, each
a rule:

1. **Inter-fragment only.** A fragment is rigid, so its internal geometry — its own
   bonds — can never self-clash; testing intra pairs would flag a fragment's own
   bonds (control c2).
2. **Reuse `measure.ts` `distance`** — one distance implementation.
3. **Excludes intentional contacts.** A pair carrying an active **distance
   constraint** is a deliberately forming bond (a Bürgi–Dunitz C···Nu approach) — NOT
   a clash, even inside the vdW sum. Read from the SAME `constraints.ts` parser
   (`fromOrcaIndex`); control **c4** is the mission gate, confirmed live (m4): at a
   real BD distance (C···B ≈ 2.8 Å) the default `k = 0.65` raises **zero** clashes
   even with no constraint (raw threshold `k·(r_C+r_B)` = 2.35 Å < 2.8 Å), and a
   distance constraint on the forming pair drops the flagged count while genuine
   peripheral clashes remain.
4. **UNDETERMINED, not guessed.** A pair touching an element with no cited vdW radius
   is **skipped and surfaced separately** (rule #11), never radius 0 (control c3).

**The two vdW tables are deliberately separate** (documented so a lint doesn't merge
them): `vdw-radii.ts` is the **physical, cited** (Bondi/Mantina/Alvarez) table with
UNDETERMINED semantics — for the *chemistry* of a clash; `viewer/highlight.ts`'s
`VDW_RADII` is a **verbatim mirror of 3Dmol's** radii (with a 1.5 Å fallback,
runtime drift-guarded) — for *sizing a halo to the sphere 3Dmol draws*. If 3Dmol
changed a radius, `highlight.ts` would follow it; `vdw-radii.ts` must not (it follows
the literature). Same numbers mostly, different masters.

**Surfacing (in `NewJobScreen`, not the Scene):** `k` is app-owned session state
(a labeled heuristic slider — `k ≈ 0.65`, "van der Waals overlap threshold —
heuristic, not a physical cutoff"), the clash report is a `useMemo` over
`(scene, k, content)` (stable reference so the viewer overlay only redraws when the
clash set changes), a warn banner shows the count, the clashing atoms get a distinct
**magenta danger glow** (`MoleculeViewer` `clashHighlight`, apart from the chartreuse
halo/mask in both hue and form), and UNDETERMINED elements get a **quiet, separate**
notice. Controls c1–c5 (found pair / no false positive / UNDETERMINED skip /
constraint exclusion / k monotonicity) each demonstrated red. See
`wiki/modules/editor-ui.md`, `visualization.md`, `chemistry/vdw-steric.md`.

## Edit planning (`edit-plan.ts`, 2.5.2d; both-orientation fix 2.5.2d-2; intra-fragment 2.5.3b; split-mask re-check 2.5.4a)

`planEdit(scene, selection)` turns the pick list into an `EditPlan` — a
three-way discriminated union:
- `{ kind: "ready"; op; indices; mask; current; unit; movingFragmentId; reversed;
  alternative }` — inter-fragment; the mask is a whole fragment, computed here;
- `{ kind: "needs-split"; op; indices; current; unit; cut; moving; within }` —
  intra-fragment torsion; the mask is a **bond-graph split** only the sidecar can
  do. `planEdit` stays **pure & synchronous**: it describes WHAT to ask, not the
  answer (**2.5.3b**);
- `{ kind: "unavailable"; reason }` — a genuine geometric refusal.

The math is **not duplicated** — `op` and `current` come straight from
`measureSelection`.

- **Click order is a DEFAULT, not a rule (2.5.2d-2).** The original unit took the
  LAST-clicked atom's fragment as the mover, full stop — which refused the real
  screenshot case `B#33(BH₄⁻)→C#12(ibuprofen)→O#14(ibuprofen)` as "same fragment"
  (the last atom's fragment, ibuprofen, held the reference C#12), even though the
  identical angle read the other way (`O#14–C#12–B#33`) moves BH₄⁻ with both
  references static. **The measured value is invariant under chain reversal** —
  `angle(i,v,j) == angle(j,v,i)`, `dihedral(i,j,k,l) == dihedral(l,k,j,i)`,
  distance symmetric (verified in ASE 3.29.0 and in `measure.test.ts` §f: e.g.
  angle 90.4615902578… and dihedral 171.5384757600… identical both ways).
  Reversal changes only *which end moves*, not the number — so click order can't
  decide whether the task is solvable.
- **Both orientations are tried.** Candidate A = chain as clicked (mover = last);
  candidate B = reversed (mover = first). Each must pass the reference-atom rule
  (mover in its fragment's mask, no reference in that mask). Only A valid → A
  (`reversed:false`); only B → B (`reversed:true`, `indices` = reversed chain);
  **both** valid (typical inter-fragment distance) → the **SMALLER fragment moves
  by default** (2.5.3b: `mask.length` decides; equal sizes → click order is the
  tie-breaker), and the other is exposed as `alternative` for the UI's "Move X
  instead" button (`swapToAlternative` flips them, `current`/`op`/`unit`
  unchanged). Rationale: moving BH₄⁻ (5 atoms) rather than ibuprofen (33) is
  almost always what the chemist means. The mask is always a **whole fragment** —
  the sidecar gets it as an explicit index list (the 2.5.0 decision).
- **Intra-fragment → `needs-split`, not a refusal (2.5.3b).** When ALL chain atoms
  are in one fragment, `planEdit` returns `needs-split` carrying the sidecar's
  **cut rule** — `distance(i,j)→cut (i,j)`, `angle(i,v,j)→cut (v,j)`,
  `dihedral(i,j,k,l)→cut (j,k)`; `moving` = the last chain atom — plus
  `within` = that fragment's atom indices (so perception can't fuse in a
  coordinated reagent; see sidecar `within`). The UI resolves it via
  `/geometry/rotatable-mask` and the returned mask drives BOTH the glow and
  `set-internal`. This was a *refusal* in 2.5.2d–3a; 2.5.3b turns it into a
  first-class edit.
- **The reference-atom rule is re-run on the RESOLVED split mask (2.5.4a).** The
  rule "the moving atom is IN the mask, every reference atom is OUT" was applied in
  `planEdit` for the inter-fragment case but **not** after the sidecar's bond-graph
  split returned — and the split doesn't know which atoms were references. A
  reference that lands on the moving side slipped through to a **422 at Apply**.
  Live repro (butane): `angle(3,1,2)` → `needs-split`, cut (1,2), moving 2;
  `/geometry/rotatable-mask` → `[2,3,9,10,11,12,13]`, which contains reference **3**
  (C#3 is bonded to the moving C#2, so it *is* on the rotatable side). Fix:
  `maskRoleViolation(mask, moving, references)` is now a **single pure function**
  used on BOTH paths — `orientationFor` (inter-fragment) and `NewJobScreen` after
  the mask resolves. On a violation the edit is refused **with an explanation in
  terms of the selection** (`explainSplitViolation`: "atom C#3 lies on the moving
  side of the C#1–C#2 bond — pick a reference atom on the static side"), **never**
  the sidecar's inter-fragment text (which reads wrong inside one molecule).
  `edit-plan.test.ts` locks the butane case.
- **The remaining refusal (2.5.2d-2).** When atoms span fragments but the pivot
  can't be held fixed:
  - **atoms across fragments but the pivot can't be held fixed** (a dihedral whose
    j–k axis atoms straddle fragments, or an angle whose two ends share a
    fragment) → a *different* reason that **names the offending atom indices**
    (`immovablePivotReason`: the references that would move with an endpoint
    whichever way the chain is read). The old code collapsed both into "same
    fragment" — the lie the screenshot exposed. The user learns the rule from the
    UI, not from a 422; the server check stays the boundary guard.
- **Apply helpers, pure and tested.** `applyResponseToScene(scene,
  movingFragmentId, responseXyz)` slices the moving fragment's rows out of the
  response xyz (by its `fragmentRanges` window) and hands them to
  `replaceFragmentAtoms` (which enforces count + element order — ADR-008).
  `applyResponseIssue(scene, responseXyz, maxStaticDisplacement)` is the
  front-of-the-boundary check before mutating: static atoms must not have moved
  (`< 1e-6`), and the response count must equal `atomCount(scene)` — else it
  returns a message and the edit is refused. `EditPanel` (React) does only the
  `fetch` and state; all decision logic is in this pure layer, so `edit-plan.test`
  covers it: the exact screenshot selection `[33,12,14]` → `reversed:true` moving
  BH₄⁻; the same selection reversed → same mask, `current` identical; the distance
  `alternative` + `swapToAlternative` mirror; the smaller-fragment default
  (independent of click order; ibuprofen+BH₄⁻ → BH₄⁻ moves in ANY order); the
  `needs-split` case carrying the right `cut`/`moving`/`within` per op (including
  a single-fragment scene); the immovable-pivot refusal naming culprits `#0`,
  `#3`; the slice; the boundary check.

## Guided placement (`guided-placement.ts` + `GuidedPlacementPanel`, Phase 4.2 tail-1)

Adding a reagent *at a target approach geometry* in ONE flow, by **composing existing
ops** — it invents no geometry. The reagent is added roughly first (`placeFragment` +
an `add-fragment` op, unchanged); guided placement then drives it to the target d/θ/φ.

- **One flow = a sequence of the EXISTING `set-internal` op.** `planGuidedPlacement(scene,
  reagentFragmentId, reagentAtom, substrateRefs, targets)` returns a `GuidedStep[]`, one
  per **given** coordinate. Each step is resolved through **`planEdit`** (the mask, the
  reference-atom rule, the both-orientation search — all already tested in `edit-plan.ts`),
  forcing the orientation whose moving fragment IS the reagent (`swapToAlternative` when
  `planEdit` defaulted to the smaller fragment). So the **mask is the reagent fragment** —
  exactly the inter-fragment case 2.5.2d. There is **no new d/θ/φ math here**.
- **Only GIVEN coordinates apply (invariant 2).** d is required (`> 0`); θ/φ are emitted
  ONLY when their target is non-null AND enough substrate anchors were picked. **An empty
  field is a SKIP, never a 0** — `guided-placement.test.ts` (c1) proves the null-vs-0
  distinction bites.
- **Z-matrix nesting — why d → θ → φ, in that order.** With reagent atom R and anchors
  A, B, C: d = distance(R, A) → chain `[A, R]`; θ = angle(R, A, B) → `[B, A, R]` (vertex A);
  φ = dihedral(R, A, B, C) → `[C, B, A, R]` (axis A–B). `planEdit`'s mover is the LAST atom,
  so R is last in every chain. Each later edit rotates the reagent about an axis **through
  A**, so it **preserves** the earlier coordinate (the distance to A, then the angle at A) —
  the standard internal-coordinate construction; the sidecar's sequential-apply acceptance
  test (`test_sequential_burgi_dunitz_acceptance`) is the numeric proof the composition holds.
- **A legible op per coordinate; Undo unwinds each (invariant 1).** `guidedStepOp` builds one
  `replace-fragment-atoms` (`via: "set-internal"`) op per step — the SAME provenance shape
  `EditPanel` writes, so the history reads "Set distance …", "Set angle …", "Set dihedral …".
  **NOT one bundled, opaque op** (c3 bites a bundle). The `add-fragment` op is already
  committed at rough placement, so the log reads `add-fragment` + one `set-internal` per
  coordinate; `NewJobScreen`'s `applyGuided` commits them in order.
- **Preview view-only; Apply enforces the post-condition.** `runGuidedPlacement` threads the
  scene through each step, the sidecar call INJECTED (so tests drive it with a fake server).
  Apply runs `applyResponseIssue` after every step (rule #9 — a moved static atom / wrong count
  is REFUSED); Preview skips it (mirrors `EditPanel`). c4 proves the guard is the Apply path.
- **Guided state is app-owned, NOT in the Scene.** `guidedReagent` / `guidedMode` live in
  `NewJobScreen` (`store.ts` is untouched — no guided field). The panel reuses the shared
  `selection` pick list, splitting it by fragment membership (the one atom on the reagent
  fragment is R; the rest are substrate anchors in click order). See `modules/editor-ui.md`.

## Rigid transforms are TS; internal-coordinate edits are the sidecar (the Stage-3 split)

A hard boundary, so a future reader doesn't reach for the wrong tool:

- **Rigid whole-fragment transforms — pure TS in `scene.ts`.** `translateFragment` (unit 3.1) and
  `rotateFragment` (unit 3.3) move a fragment as one body; they change no bond, no internal
  coordinate, so there is nothing for ASE to compute. They are Rodrigues/vector arithmetic, run
  synchronously in the browser (which is what lets the rotation preview update live per slider tick,
  no HTTP). Post-condition (rule #9): the same internal pairwise distances go in and out.
- **Intra-fragment internal-coordinate edits — the ASE sidecar.** Setting a distance/angle/dihedral
  (`set-internal`, 2.5.2–2.5.3) or a torsion about a bond needs a **bond-graph split** and a
  coordinate solve only ASE can do — `POST /geometry/set-internal`, `/geometry/rotatable-mask`
  (see `wiki/modules/sidecar.md`). These emit `replace-fragment-atoms`, not `rotate-fragment`.

The two look similar in the UI (both are pick + value + preview + apply in the Edit section) but are
**different operations on different math**. Do NOT route rigid rotation through the sidecar, and do
NOT reimplement the bond-graph torsion in TS.

## Notes

- ADR mapping came through unchanged; nothing in 2.5.0a diverged from ADR-008.
- `sceneFromAtomLines` takes an optional `id` in `opts` so the "editor" path can
  be tested deterministically without stubbing `crypto.randomUUID()`.
- **Fragment colours** are not a Scene concern — they live in
  `src/viewer/fragment-colors.ts` (`FRAGMENT_PALETTE` + `fragmentColor(i)`), one
  source of truth shared by the 2.5.0c viewer and the 2.5.0d fragment sidebar so
  a fragment reads the same colour in the 3D view and the list. Fragment 0 keeps
  CPK element colours (the substrate must not recolour).
