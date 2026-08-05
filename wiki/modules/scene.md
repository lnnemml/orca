# Module: scene (`src/scene/`)

**Status:** Phase 2.5 complete. The Scene is the **source of truth for geometry on
New Job** — a Zustand store (`store.ts`) synced two-way with the Monaco buffer — and
carries the whole reaction-geometry workflow: multi-fragment build (Add-Fragment panel
+ `FragmentList`), electron-parity validation (`parity.ts`), conformer search (GOAT,
`ensemble.ts`), the geometry editor (atom picking `selection.ts`, d/θ/φ measurement
`measure.ts` with ASE conventions pinned to source, edit-mode planning `edit-plan.ts`
for inter- and intra-fragment edits, the reference-atom rule `maskRoleViolation` re-run
on both the inter-fragment and the resolved bond-graph split mask), the constraint block
over the input text (`constraints.ts`, `ConstraintPanel.tsx`, non-destructive rewrite),
and xTB pre-optimization (`replaceAllAtoms`). Scene layout persists as `jobs.scene_json`
(schema v4) and restores on job iterate (`restoreScene`). Per-unit history is in
`wiki/log.md`.

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

- `types.ts` — `SceneAtom`, `FragmentSource`, `SceneFragment`, `Scene`;
  `FRAGMENT_SOURCES` (the valid-source list, for deserialize validation).
- `scene.ts` — the merge / index / parse / serialize functions below, plus the
  ORCA-input ↔ Scene text I/O (`sceneFromOrcaInput`, `injectSceneIntoInput`).
- `parity.ts` — `checkElectronParity` (electron-parity validation, ADR-008 #8).
- `store.ts` — the Zustand scene store (React-facing; thin over the pure layer).
- `placement.ts` — `placeFragment` (bounding-box separation for a new fragment).
- `fragment-library.ts` — `FRAGMENT_LIBRARY` (curated reagents) +
  `libraryFragmentToScene`.
- `FragmentList.tsx` — the fragment sidebar (React; reads the store, uses the
  shared `fragmentColor` palette).
- `restore.ts` — `restoreScene` (snapshot ↔ input reconciliation on job open).
- `ensemble.ts` — GOAT conformer-ensemble parsing + input generation (2.5.1a),
  plus `isGoatInput` (2.5.2a — is this a conformer-search job?).
- `selection.ts` — the geometry editor's atom pick list (2.5.2a): `toggleAtom`,
  `validateSelection`, `describeAtom`, plus `selectionSurvives` (2.5.2b — the
  composition-signature survival rule). Pure / node-tested, no React.
- `measure.ts` — geometry measurement off the pick list (2.5.2b): `distance`,
  `angle`, `dihedral`, `measureSelection`, `formatMeasurementValue`. Pure /
  node-tested, React-free. **ASE conventions pinned to source** (see below).
- `edit-plan.ts` — edit-mode planner (2.5.2d): `planEdit` (pick list → `ready` |
  `needs-split` | `unavailable`), plus the pure apply helpers `applyResponseToScene` and
  `applyResponseIssue`, and the shared reference-atom rule `maskRoleViolation` /
  `explainSplitViolation` (2.5.4a). Pure / node-tested, no React, no fetch.
- `constraints.ts` — ORCA `%geom Constraints` generate / parse / inject (2.5.4a):
  `Constraint` type (B/A/D/C), `ORCA_INDEX_BASE`/`toOrcaIndex`/`fromOrcaIndex`,
  `constraintsBlock`, `parseConstraintsBlock`, `injectConstraints`. Pure /
  node-tested, no React, no fetch. **Input text is the source of truth** (see
  below); the 2.5.4b panel will be a view over the text. Index base **0-based,
  settled by a real ORCA 6.1.0 run** — `wiki/orca/constraints.md`.
- `AtomInspector.tsx` — the atom panel on New Job (React; reads a selection held
  in `NewJobScreen` state, uses the shared `fragmentColor` palette).
- `EditPanel.tsx` — edit-mode UI in the Atom rail section (React): target field,
  Preview/Apply, and the direct `fetch` to the sidecar geometry endpoint.
- `ConstraintPanel.tsx` — the constraint section of the geometry rail (React): a
  **view over the input text** (its only source is `parseConstraintsBlock(content)`),
  one row per constraint (type badge, atoms in our terms, set-vs-measured value, delete)
  plus the range and composition guards (2.5.4b); read-only on an `unrecognised` block
  (2.5.5).
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

## Functions (one-line contracts)

Merge / serialize:
- `mergeToAtomLines(scene): string[]` — canonical coordinate rows, fragment order.
- `mergeToXyz(scene, comment=""): string` — `count\ncomment\nrows\n`.
- `serializeScene(scene): string` — versioned JSON (`"version": 1`).
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
  `NewJobScreen` reconsiders the pick list only on a signature change — passing
  the before/after signatures to `selectionSurvives` (2.5.2b).
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
  (same id / composition / internal geometry). Used by placement and, later, the
  geometry editor (2.5.2).

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

## The store (`store.ts`) and Scene ↔ Monaco sync

`useSceneStore` (Zustand) holds `{ scene, previous, resetNotice }` and actions
that are **thin wrappers over the pure functions** — no geometry logic lives in
the store, so the pure layer stays node-testable. `collapseToSingleFragment`,
`undoReset`, `dismissResetNotice` are the only store-specific pieces (the pure
layer has no place for undo bookkeeping).

**Reference stability is a contract, not an optimisation.** `MoleculeViewer`
redraws on a new `scene` reference (`useEffect([scene])`). Selectors return the
stored object directly (`useSceneStore((s) => s.scene)`); actions that would be
no-ops return the current state unchanged. So an unchanged scene keeps its
identity and the viewer does not `removeAllModels`/`addModel` on every keystroke.
`store.test.ts` asserts repeated reads are `===`.

**Sync (ADR-008 #6), as wired in `NewJobScreen`:**
- **Scene → Monaco:** on a scene change, `injectSceneIntoInput` writes the merged
  block; a guard skips the write when the text already matches (prevents the echo
  after a content→scene sync and never reformats a manual edit).
- **Monaco → Scene:** on the 500 ms debounce, `xyzMatchesScene(scene, atomLines)`
  (**parsed floats, tol 1e-6 — never string compare**) decides: match → leave the
  scene (the user edited keywords, the common silent path); diverged → the text
  wins, `collapseToSingleFragment`; block gone → `setScene(null)`; no scene yet
  but a block appeared (template / generated input) → adopt it.
- **Reset notice + Undo:** collapse stashes `previous`; the notice ("N fragments
  merged into one" + Undo, rendered by `NewJobScreen`, reachable from 2.5.0d-2b)
  shows **only when >1 fragment was lost** — a single-fragment collapse is
  geometrically a no-op, so it stays silent (else the user would see a warning on
  every hand-edit of a water molecule). Undo restores `previous`, which re-injects
  its coordinates.

**Regression guard on the round-trip (the subsystem's finest wire).** Adding a
fragment makes the scene multi-fragment → Scene→Monaco injects → ~500 ms later
Monaco→Scene re-parses and asks `xyzMatchesScene`. If ordering/formatting drift
made that FALSE, the scene would **silently collapse back to one fragment half a
second after the add** — no error, just "the sidebar blinked and the fragments
merged". `add-fragment.test.ts` locks this: it drives the real inject → parse →
`xyzMatchesScene` path a real add produces and asserts the comparison stays TRUE
(so the effect leaves the scene at two fragments). It's a pure-function
simulation, not a rendered-component + fake-timers test, because the suite has no
jsdom — and the comparison is exactly where the bug would live.

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
BH₄⁻, H⁻, OH⁻, CN⁻, Cl⁻, H₂O, NH₃, CH₃OH (the chemistry — why BH₄⁻ is tetrahedral,
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

The geometry editor's pick list — pure, node-tested, React-free. A selection is
an **ordered list of global atom indices** (the merged-xyz / ASE-mask space);
2.5.2b reads it positionally as (a,b) distance / (a,vertex,b) angle / 4-atom
dihedral chain. The UI (`NewJobScreen`) holds it in component state (**not** the
scene store — the store stays a pure geometry wrapper, ADR-008 #10) and drives
every change through these functions.

- `MAX_SELECTION = 4` — a dihedral's four atoms.
- `toggleAtom(selection, index): number[]` — one click, new array:
  already-selected → remove; new & under the cap → append (order kept); new &
  **at the cap → the selection becomes `[index]`**. The full-list rule is the
  decision: **not FIFO.** Silently evicting the oldest atom would leave the user
  measuring a set different from the atoms they see highlighted — a wrong-atom
  measurement with no visible cause. A hard reset to the just-clicked atom is
  unambiguous ("fifth click resets").
- `validateSelection(selection, scene): number[]` — drop indices **out of range**
  (a fragment removed, scene cleared). Returns the **same array reference** when
  nothing is dropped, so a no-op doesn't churn React state. **Range only:** it
  *survives an index shift* — a picked index that is still in range but now points
  at a different atom passes through unchanged. It is the **second echelon**, not
  the primary removal guard (see `selectionSurvives`).
- `selectionSurvives(prevSignature, nextSignature): boolean` (2.5.2b) — does a
  selection survive a composition change, working on the two
  `compositionSignature` strings alone (never sees the scene)? **true** iff the
  signature is **unchanged** or a **pure append** (`next` starts with
  `prev + "|"`); **false** on a removal, a recomposition, or a cleared/appeared
  scene. The trailing `"|"` forces a whole-field match so `"a:3"` can't
  append-match `"a:30|b:2"`. This is the **primary** guard `NewJobScreen` keys
  the pick list off (see the survival rule below).
- `describeAtom(scene, globalIndex): AtomDescription | null` — a thin wrapper over
  `locateAtom` (no own fragment walk); adds `fragmentIndex` (the palette key) and
  the atom's coordinates. `null` for out-of-range (same non-throwing contract).

### The selection survival rule (2.5.2b)

`addFragment` **always appends** the new fragment last, so an append leaves every
existing atom's global index unchanged — a selection of the older atoms survives
it. Any **other** composition change (a fragment removed, its atom count changed)
shifts indices, and after a removal "the same atom" has **no operational
definition**: a silent remap (index N now means a different atom) is worse than a
lost click. So the rule is a clean binary:

- signature unchanged (a coordinate-only edit) or pure append → **keep** the
  selection;
- anything else → **clear it outright**, no remap.

`NewJobScreen` asks `selectionSurvives(prev, next)` on every signature change;
`!survives → setSelection([])`. `validateSelection` stays a defensive second
echelon (mainly the append path and `scene → null`). **Why the split matters:**
`validateSelection` is range-only, so it *survives an index shift* — remove
water(0,1,2) from water+BH₄⁻ with the boron (global 3) picked and global 3 is
still in range but now addresses a BH₄⁻ hydrogen. Range validation keeps `[3]`
silently pointing at the wrong atom; `selectionSurvives` (signature `wat:3|bh4:5`
→ `bh4:5`, not an append) returns false and clears it. In 2.5.2d that index
becomes an ASE mask, so a silent shift would mask the wrong atom.

## Measurement (`measure.ts`, 2.5.2b)

Reads the pick list **positionally**: 2 atoms → `distance(i,j)`, 3 → `angle(i,
vertex, j)` with the **middle pick as the vertex**, 4 → `dihedral(i,j,k,l)` along
the chain (axis `j–k`). `measureSelection(scene, selection)` returns a tagged
`Measurement` (`none | distance | angle | dihedral`), each carrying `atoms` (the
picks in click order) and `sameFragment` — inter-fragment distance is a future
reaction coordinate (ADR-007) and must read apart from internal geometry.
Degenerate inputs (coincident atoms, zero vector, collinear inner triple for the
dihedral, out-of-range index) return **null, never NaN**; `measureSelection` maps
null → `none`.

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

## Notes

- ADR mapping came through unchanged; nothing in 2.5.0a diverged from ADR-008.
- `sceneFromAtomLines` takes an optional `id` in `opts` so the "editor" path can
  be tested deterministically without stubbing `crypto.randomUUID()`.
- **Fragment colours** are not a Scene concern — they live in
  `src/viewer/fragment-colors.ts` (`FRAGMENT_PALETTE` + `fragmentColor(i)`), one
  source of truth shared by the 2.5.0c viewer and the 2.5.0d fragment sidebar so
  a fragment reads the same colour in the 3D view and the list. Fragment 0 keeps
  CPK element colours (the substrate must not recolour).
