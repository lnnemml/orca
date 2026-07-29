# Module: scene (`src/scene/`)

**Status:** 2.5.0d-1 done — the Scene is now the **source of truth for geometry
on New Job**, via a Zustand store (`store.ts`) synced two-way with the Monaco
buffer. 2.5.0a pure core → 2.5.0b input-builder + parity → 2.5.0c multi-fragment
viewer → **2.5.0d-1 store + Scene↔Monaco sync + parser consolidation closed**.
**2.5.0d-2a** added the pure foundation (reagent library + placement); **2.5.0d-2b**
wired the UI — the Add-Fragment panel and the `FragmentList` sidebar — so
multi-fragment scenes became user-reachable. **2.5.0d-3** persists the layout
(`jobs.scene_json`, schema v4) and adds the **"New iteration"** action that reads
it, via the pure `restoreScene`. **Phase 2.5.0 (Scene/fragment foundation) is now
complete.** **2.5.1** (conformer search) is complete: 2.5.1a the GOAT primitive
(`ensemble.ts`), **2.5.1b** the UI — "Find conformers" on a fragment, the ensemble
panel on Job detail, and "Use this conformer" (two branches). The geometry editor
(**2.5.2**) is underway: **2.5.2a** atom picking + selection (`selection.ts`); **2.5.2b**
d/θ/φ measurement (`measure.ts`, ASE conventions pinned to source) + the selection-survival
rule (`selectionSurvives`). Next: **2.5.2c** — apply d/θ/φ through ASE, whose acceptance test
re-derives all three with `measure.ts` (already recorded).

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
- `AtomInspector.tsx` — the atom panel on New Job (React; reads a selection held
  in `NewJobScreen` state, uses the shared `fragmentColor` palette).
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
inform, we don't forbid.

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

## Consolidation — closed in 2.5.0d (ADR-008 delivered)

ADR-008 promised the `src/viewer/` coordinate parsers would fold into `src/scene/`;
2.5.0b narrowed it to `InputBuilderForm` only, and **2.5.0d closed it**. When
`NewJobScreen` moved onto the store + `sceneFromOrcaInput` / `injectSceneIntoInput`,
the duplicate ORCA-input parsers `parse-xyz-from-input.ts` (`extractXyzFromInput`)
and `inject-xyz-into-input.ts` (`injectXyzIntoInput`) were **deleted**, and
`parseChargeMult` was removed from `xyz-format.ts`. What remains in
`viewer/xyz-format.ts` — `xyzToAtomLines`, `atomLinesToXyz` — is standard-xyz-string
↔ atom-line **formatting**, not ORCA-input parsing, with live consumers
`import-file.ts` and `MoleculesScreen` (which manages library molecules as stored
xyz *strings*, not Scenes — deliberately not migrated). No duplication with this
module remains.

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

## Notes

- ADR mapping came through unchanged; nothing in 2.5.0a diverged from ADR-008.
- `sceneFromAtomLines` takes an optional `id` in `opts` so the "editor" path can
  be tested deterministically without stubbing `crypto.randomUUID()`.
- **Fragment colours** are not a Scene concern — they live in
  `src/viewer/fragment-colors.ts` (`FRAGMENT_PALETTE` + `fragmentColor(i)`), one
  source of truth shared by the 2.5.0c viewer and the 2.5.0d fragment sidebar so
  a fragment reads the same colour in the 3D view and the list. Fragment 0 keeps
  CPK element colours (the substrate must not recolour).
