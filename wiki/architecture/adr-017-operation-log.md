# ADR-017: The operation log

**Status:** accepted · 2026-08-06
**Refines:** [ADR-010](adr-010-editor-identity-state.md) ("state is a fold over a log of typed
operations") · **Relates:** [ADR-016](adr-016-emit-input-ownership.md) (the identity core the ops
run over), [ADR-008](adr-008-scene-fragment-model.md) (the Scene the log holds)
**Stages:** ROADMAP Phase 4.2 Stage 2. This ADR is the **design**; unit **2a** lands the pure types
(`src/scene/oplog.ts`); the store on the log is **2b**.

## Context

ADR-010 fixed that **editor state is a fold over a log of typed operations** — undo/redo are a moving
pointer, and a new editor capability adds an *operation*, not a new piece of state. Stage 1 built the
identity core the operations act on (`AtomId` / `IndexMap`, ADR-016). Stage 2 builds the log itself.

Today the store (`src/scene/store.ts`) holds a **one-step** reset undo (`previous` + `undoReset`) —
enough for the Monaco-collapse case, but not a general history. This ADR designs the log that
replaces it. Four decisions, each with its rationale, because each would otherwise be relitigated.

## Decision 1 — Each entry MATERIALIZES its resultant snapshot (the fold is trivial, on purpose)

A log entry stores `{ op, scene }` where `scene` is the **Scene that resulted** from `op` — the full
materialized snapshot, not a recipe for recomputing it. `current(log)` is therefore just the snapshot
at the pointer; the "fold" is a lookup. That triviality is the decision, not a shortcut.

**The argument, stated so a future reader cannot "optimize" it away** (this paragraph stands verbatim
both here and as the header comment of `oplog.ts`):

> The dangerous optimization is to drop the snapshots, keep only the ops, and **replay** them to
> reconstruct state ("it's smaller, it's DRY"). DO NOT. The geometry ops (`replace-fragment-atoms`
> via set-internal / xtb / conformer, `replace-all-atoms`) are executed by **ASE in the Python
> sidecar**. A log that stores only ops and recomputes state makes the reconstructed history a
> **function of the installed ASE version** — a dependency bump would silently rewrite geometries
> computed months ago. The history of a scientific instrument must not change retroactively.

So: the snapshot is the **source of truth**, not a cache; the `Op` descriptor is **provenance** (a
lab-journal line), not a recipe; `current` reads a stored snapshot and never re-invokes a geometry
backend. The value of the log is exactly the lab journal — *what was done, in order, to reach this
geometry* — and `describe(op)` renders each step already at the type layer (decision below). Cost is
measured and small (§Measured sizes); materialization is affordable.

## Decision 2 — No WASM in Stage 2: the op **schema** is shared, apply-orchestration is TS

The op types are plain TypeScript in `src/scene/oplog.ts`. The `orcastudio-core` Rust crate is **not**
extended in Stage 2. Cross-language sharing, when Phase 4.5 needs it, is by **serde-JSON schema +
golden fixtures** (the same discipline that keeps the emit byte-identical —
[float-formatting-parity.md](float-formatting-parity.md)), not by compiling the log to WASM. Phase 4.5
(ReactionPath) reuses these `Op` types and **replays the materialized snapshots** — it does not
recompute them (decision 1).

**Correction recorded openly (do not rediscover this as a contradiction).** Unit 1c
([ADR-016](adr-016-emit-input-ownership.md)) justified making `orcastudio-core` a **separate** crate
partly by "Stage 2 compiles it to WASM for the renderer/interaction half." Decision 1 removes that
particular reason for *this* module: the log does not recompute, so it needs no WASM apply-engine. The
crate's separateness **still stands**, on the *other* stated ground — it is **std-only / `no_std`-
friendly with a controlled MSRV**, the precondition for the eventual renderer move (Stage 3), which is
independent of whether the op-log is WASM. The over-claim was the coupling "the crate is separate
*because* the op-log will be WASM"; the crate is separate because the core must stay std-only, and
that is untouched.

## Decision 3 — A new column `scene_log_json`; `jobs.scene_json` stays the v2 snapshot

The **core contract is untouched**: `jobs.scene_json` remains the single **v2 Scene snapshot**
(`serializeScene`), which `restoreScene` reconciles against the input text (ADR-008 #5). The log is a
**new, optional** column `jobs.scene_log_json` holding the serialized `SceneLog` (log format **v1**;
see §Serialization). The schema migration that adds the column is **unit 2b, not 2a** — 2a is pure
types with no DB touch. "New iteration" restores the log (its last snapshot must agree with
`scene_json` / the input, the same reconciliation standard). A job with no log column (every existing
row, and any GOAT/xtb job) simply has no history — `scene_json` alone still opens it, exactly as
today.

## Decision 4 — Undo/redo is a pointer; append truncates the redo tail; **no length cap yet**

`SceneLog = { entries, pointer }`. `undo`/`redo` move the pointer; **`append` after an `undo`
truncates everything past the pointer** — the undone future is discarded, so the log never holds two
divergent futures (the quiet bug: a `redo` that resurrects a state the user did not expect).
Negative control (a) proves the truncation bites (§Verification).

**Pointer invariant.** `pointer === -1` means *before the first op / the empty scene* (`current →
null`); otherwise `0 ≤ pointer < entries.length`. The full machine-checkable form is **`-1 ≤ pointer <
len`, with `pointer === -1` exactly when the log is empty or has been fully undone**. The `-1`
sentinel extends the plain "`0 ≤ pointer < len`" so that **undo can reach the empty scene** — undoing
the first op returns a blank canvas, which the 2b store renders as `scene: null`. (`logInvariant` is
the checkable predicate; the invariant test drives an append/undo/redo walk.)

**No length cap in 2a — deliberately, and now with numbers.** A cap would be a silent truncation of
history (rule: `log()` what you drop). The measured sizes (below) show a typical session is a few tens
of KiB in one TEXT column, so a cap is **not needed** at realistic session lengths; the decision is
**deferred until a session is observed to need one**, not chosen blind.

### Measured sizes (rule #10 — numbers, not a guess)

Measured on a realistic reaction-build scene — a 33-atom substrate + a 5-atom reagent (38 atoms, the
case the edit planner targets) — over a 10-op session (restore, add reagent, conformer swap,
translate, three internal-coordinate edits, multiplicity, whole-scene xtb, rename):

| Quantity | Bytes |
|---|---|
| One scene snapshot (v2 JSON, `serializeScene`) | **2 904 B** |
| Whole log (v1 JSON, 10 entries) | **35 294 B** |
| Per entry (log ÷ entries) | **≈ 3 529 B** |
| Extrapolated 100-op session | **≈ 345 KiB** |

Per-entry (3 529 B) exceeds one bare snapshot (2 904 B) because the embedded scene is stored as its
`serializeScene` **string**, JSON-string-escaped inside the log (every `"` → `\"`) — the deliberate
cost of reusing the one Scene codec across the version boundary (§Serialization). Even a 1 000-op
session (~3.4 MiB in one column) is within SQLite's comfort; a cap is a future refinement, not a 2a
requirement.

## The op vocabulary — one variant per Scene mutator (checklist, so 2b finds no hole)

`Op` is a tagged union. Every geometry-carrying variant references atoms by **`AtomId`** (not a
positional index) — the log is AtomId-native ahead of the 2c2 pipeline move, so a materialized op
stays legible after a fragment is removed. The correspondence to the existing mutators:

| `scene.ts` mutator (or store act) | `Op` variant | `describe` example |
|---|---|---|
| `addFragment` | `add-fragment` | `Add fragment BH₄⁻ (borohydride)` |
| `removeFragment` | `remove-fragment` | `Remove fragment BH₄⁻` |
| `renameFragment` | `rename-fragment` | `Rename fragment Water → Solvent` |
| `setFragmentCharge` | `set-fragment-charge` | `Set charge of BH₄⁻ to -1` |
| `setMultiplicity` | `set-multiplicity` | `Set multiplicity to 3` |
| `translateFragment` | `translate-fragment` | `Move BH₄⁻ by (1.500, 0, -2) Å` |
| `replaceFragmentAtoms` | `replace-fragment-atoms` `{edit: via 'set-internal'\|'xtb'\|'conformer'}` | `Set dihedral 4-7-12-15 to 30°` |
| `replaceAllAtoms` | `replace-all-atoms` `{edit: via 'xtb'}` | `Pre-optimize all fragments (xtb)` |
| `collapseToSingleFragment` (store) | `collapse-from-text` | `Edit coordinates as text (3 fragments → 1)` |
| `restoreScene` / "New iteration" (store) | `restore-snapshot` | `Restore snapshot (New iteration) — 2 fragments, 12 atoms` |

`collapse-from-text` keeps the manual text-edit path **honest** in the journal (the block was
hand-edited); unit **2d** narrows it once the xyz block becomes a read-only projection.
`replaceFragmentAtoms`'s three producers are a sub-union (`FragmentGeometryVia`) because set-internal
(carrying `kind`/`atoms`/`target`/`unit`), an xtb pre-opt, and a conformer swap are genuinely
different acts that `describe` reads apart. `translateFragment`, though generic in `scene.ts`, appears
as one op (the ephemeral drag of Stage 3 commits **one** `translate-fragment` on mouseup — ADR-010).

## Serialization

`serializeLog(log): string` writes `{ version: 1, pointer, entries: [{ op, scene }] }`, where each
entry's `scene` is its **`serializeScene` string** (Scene format **v2**). The log format is versioned
**independently** of the Scene JSON, and the Scene codec is reused verbatim — including its v1→v2
migration on read, so a scene embedded in an old log still upgrades. `deserializeLog` validates shape,
version, the pointer invariant, and every embedded scene (via `deserializeScene`), and validates each
op's discriminant + the fields `describe` reads; it returns `null` on anything unexpected and **never
throws** on user/DB data (the `deserializeScene` contract).

## Verification (unit 2a)

- Full vitest suite green including the new `oplog.test.ts` (13 tests); `tsc` clean; no Rust touched.
- **Pointer invariants** — `logInvariant` across an append/undo/redo walk; `0 ≤ pointer < len` for a
  populated log; `-1` (empty scene) after a full undo.
- **Append truncates the redo tail** — an append after an undo drops the undone future.
- **undo/redo round-trip is identity** — walks back to `null` and forward to the **same frozen
  snapshot objects** (`===`, not just structural equality).
- **`describe()` on every variant** — the table above is the oracle.
- **Serialization round-trip** — `deserializeLog(serializeLog(x))` is structurally equal; rejects
  malformed / wrong-version / bad-pointer / corrupt-op input.
- **Negative controls, demonstrably red then green** (a gate whose bite is not shown is green for an
  unknown reason):
  - **(a) truncation** — breaking `append` to keep the redo tail turns "redo after append is
    impossible" **red** (got length 3, expected 2). Restored.
  - **(b) immutability** — an entry's snapshot is **deep-frozen**; a strict-mode write throws and the
    value survives. Neutering `deepFreezeScene` turns the freeze/write-throws test **red**. Restored.
    `Object.freeze` is a **real runtime guarantee in every environment** (not a dev-only flag): the
    freeze holds in production, and because every module here is an ES module (always strict mode) a
    write to a frozen field *throws* rather than silently no-ops — so (b) bites in prod too.

## Consequences

- **`oplog.ts` is pure and inert in 2a** — no store, viewer, Monaco, DB, or Rust touch. It is dead
  code until 2b wires the store onto it.
- **ROADMAP** — Phase 4.2 Stage 2 is rewritten into units **2a–2d** (2a = this + the pure types); the
  ephemeral drag layer moves to the first unit of Stage 3 (it is needed only for the drag).
- **`store.ts`'s one-step `previous`/`undoReset`** is superseded by the log in 2b (not yet removed).
- **Phase 4.5** reuses the `Op` types and replays materialized snapshots (decision 1/2).

## Amendment (unit 2b, 2026-08-06) — the store landed as designed; two small refinements

The four decisions above **stand unchanged** — the store implementation matches them, so no
decision is revised. Recorded here so a future reader isn't surprised by two details the design
didn't spell out:

1. **`scene` is derived, and there is no `setScene`.** The store holds the `SceneLog` and a `scene`
   field that is *only* ever `current(log)`. The two low-level doors are `commit(op, resultScene)`
   (append) and `installLog(log)` (lifecycle replace); every convenience mutator funnels through
   `commit`. This makes the "mutator bypasses the log" defect (the 2b main risk) impossible by
   construction — a store test asserts `scene === current(log)` after every action, proven-biting.
2. **`SnapshotSource` gained `text-adopt` and `library`** (beside `new-iteration`), and `oplog`
   gained `goto(log, pointer)` for the history-panel jump. Both are vocabulary/navigation additions,
   not decision changes: a whole-scene seed (a template/pasted block adopted from text, or a library
   molecule) is honestly a `restore-snapshot`, and `describe` names its origin. Persistence adds one
   mechanism the design named but didn't detail — the **log↔snapshot cross-check** on restore
   (`restoreSceneLog`): the persisted log is honoured only if its current snapshot equals the
   co-written `scene_json`; otherwise it is **rejected with a named reason and the snapshot wins**
   (decision 3's "core contract untouched," made operational). Negative controls (a)/(b)/(c) —
   bypass, cross-check, collapse↔undo loop — all demonstrably bite.

## References

- [ADR-010](adr-010-editor-identity-state.md) — "state is a fold over a log of typed operations"; the
  ephemeral layer that carries the drag; authority split (text = chemistry, Scene = geometry).
- [ADR-016](adr-016-emit-input-ownership.md) — the identity core (`AtomId`/`IndexMap`) the ops run
  over; the crate-separateness ground this ADR corrects (WASM → std-only/MSRV).
- [ADR-008](adr-008-scene-fragment-model.md) — the Scene the log materializes; `scene_json` v2.
- [float-formatting-parity.md](float-formatting-parity.md) — the serde-JSON + golden-fixture
  discipline decision 2 points at for future cross-language op sharing.
- `src/scene/oplog.ts`, `src/scene/oplog.test.ts` — the unit-2a implementation and its gates.
- `wiki/modules/scene.md` — the module page (op-log section: types ready, store in 2b).
