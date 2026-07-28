# Module: scene (`src/scene/`)

**Status:** 2.5.0b done — the pure core (2.5.0a) is now wired into ORCA-input
generation and electron-parity validation. `buildOrcaInput` accepts a Scene, and
`InputBuilderForm.tsx` derives its geometry through `src/scene/` (no longer
through the viewer parsers). Still ahead: 2.5.0c multi-fragment viewer, 2.5.0d
Zustand store + `jobs.scene_json` persistence (and the store is what makes the
form's read-only-charge / parity UI act on a *real* multi-fragment Scene rather
than the single fragment derived from the buffer). **2.5.0c** wired
`fragmentRanges` / `mergeToXyz` into `MoleculeViewer` for multi-fragment
rendering (viewer only; no store yet).

## Responsibilities

The Scene / SceneFragment data model from
[ADR-008](../architecture/adr-008-scene-fragment-model.md): OrcaStudio's own
abstraction for a multi-molecule geometry (substrate + reagent in one coordinate
space, with known fragment boundaries). ORCA never sees a Scene — on export the
fragments merge into one flat `* xyz totalCharge multiplicity ... *` block.

This module is **pure and React-free** by design (ADR-008 decision 10): the merge
/ index-mapping / comparison logic is plain node-testable functions; the reactive
store that wraps them arrives in 2.5.0d. No imports from react / 3dmol / tauri.

## Files

- `types.ts` — `SceneAtom`, `FragmentSource`, `SceneFragment`, `Scene`;
  `FRAGMENT_SOURCES` (the valid-source list, for deserialize validation).
- `scene.ts` — the merge / index / parse / serialize functions below.
- `parity.ts` — `checkElectronParity` (electron-parity validation, ADR-008 #8).
- `scene.test.ts` + `parity.test.ts` — vitest (46 tests total across the module).

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
- `atomicNumber(symbol): number` — H–Kr, case-insensitive; throws on unknown.

Index space:
- `globalIndex(scene, fragmentId, localIndex): number` — throws on unknown
  fragment / out-of-range local index.
- `fragmentAtomIndices(scene, fragmentId): number[]` — the future ASE mask.
- `locateAtom(scene, globalIndex): { fragment, localIndex } | null`.
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

Parsing / reset detection:
- `parseAtomLines(lines): SceneAtom[] | null` — skips blanks and `#` comments;
  `null` when nothing parses.
- `sceneFromAtomLines(atomLines, opts): Scene | null` — single-fragment scene
  (the "editor" path). `opts.id` is accepted for determinism; defaults to
  `makeFragmentId()`.
- `sceneFromOrcaInput(content, opts): Scene | null` — the ORCA-input → Scene
  adapter (2.5.0b): extracts the `* xyz charge mult ... *` block, taking the
  fragment charge and `scene.multiplicity` from the header. `null` for a
  `* xyzfile` block (external geometry) or no block. Used by `InputBuilderForm`.
- `xyzMatchesScene(scene, atomLines, tol=1e-6): boolean` — the reset-detection
  primitive (ADR-008 decision 6). Parses both sides and compares element symbols
  (case-insensitive) + coordinates within `tol`. **Float comparison, never
  string comparison** — formatting differs (`1.0` vs `1.00000000`). Different
  count / element sequence / `null` ⇒ `false`.

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
`ParityIssue` with the electron count, the offending multiplicity, a nearest-first
list of valid multiplicities (`[1,3,5]` or `[2,4,6]`), and an **explanatory**
message (how many electrons, why that parity, what to use) — a teaching moment,
not a diagnostic "invalid multiplicity".

Scope of the check, deliberately narrow:
- It validates **arithmetic possibility only**, never physical plausibility.
  Whether a triplet is a sensible ground state for *this* molecule is the
  chemist's call; we only catch the provably-impossible class (the error ORCA
  reports cryptically ~30 s into a run).
- Returns `null` for an empty scene (nothing to validate) and for an element
  outside the H–Kr table (can't count electrons → no parity opinion; it swallows
  the `electronCount` throw rather than crashing the caller).

**Why the UI warns, not blocks.** `InputBuilderForm` shows the issue inline but
still lets Generate proceed: the user may build a scene incrementally and pass
through a temporarily odd state, and ORCA itself rejects the truly impossible. We
inform, we don't forbid.

## Overlap — full consolidation deferred to 2.5.0d (narrowed from ADR-008)

ADR-008 said 2.5.0b would fully consolidate `src/viewer/xyz-format.ts`
(`xyzToAtomLines`, `atomLinesToXyz`, `parseChargeMult`) and
`src/viewer/parse-xyz-from-input.ts` (`extractXyzFromInput`) into `src/scene/`.
**2.5.0b narrowed that on purpose:** only `InputBuilderForm.tsx` was migrated onto
`src/scene/` (via `sceneFromOrcaInput`); `NewJobScreen.tsx` and
`MoleculesScreen.tsx` still use the viewer helpers. Reason — both screens are
rewritten in 2.5.0d (Add Fragment UI + Zustand), and consolidating their call
sites now would mean rewriting them twice. So `sceneFromOrcaInput` deliberately
duplicates a little of `extractXyzFromInput` / `parseChargeMult` in the interim;
**2.5.0d removes the viewer copies** once every screen is migrated — that is where
ADR-008's "full consolidation" actually lands. Nothing in `src/viewer/` was
deleted or changed in 2.5.0b. A comment in `scene.ts` names the overlap.

## Notes

- ADR mapping came through unchanged; nothing in 2.5.0a diverged from ADR-008.
- `sceneFromAtomLines` takes an optional `id` in `opts` so the "editor" path can
  be tested deterministically without stubbing `crypto.randomUUID()`.
- **Fragment colours** are not a Scene concern — they live in
  `src/viewer/fragment-colors.ts` (`FRAGMENT_PALETTE` + `fragmentColor(i)`), one
  source of truth shared by the 2.5.0c viewer and the 2.5.0d fragment sidebar so
  a fragment reads the same colour in the 3D view and the list. Fragment 0 keeps
  CPK element colours (the substrate must not recolour).
