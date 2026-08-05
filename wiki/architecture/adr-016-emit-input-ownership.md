# ADR-016: `emit_input` ownership — the Rust core

**Status:** accepted · 2026-08-05
**Refines:** [ADR-010](adr-010-editor-identity-state.md) (`emit_input`/`parse_output` paired with an
`IndexMap`) · **Relates:** [ADR-012](adr-012-output-parsing-ownership.md) (output parsing is Rust's),
[ADR-009](adr-009-process-orchestration.md) (process spawning is Rust's)
**Stages:** ROADMAP Phase 4.2 Stage 1 (units 1a–1e). This ADR is settled in unit **1a**; the code
lands across 1c–1e.

## Context

ADR-010 made `emit_input` and `parse_output` a **type-level pair**: `parse_output` cannot be called
without the `IndexMap` produced by the matching `emit_input`. But today the two halves live in
**different languages**:

- **`emit_input` is TypeScript** — `injectSceneIntoInput` (`src/scene/scene.ts`,
  `src/viewer/xyz-format.ts`), `%geom Constraints` (`src/scene/constraints.ts`), the
  `input-builder/` method/basis form.
- **`parse_output` is Rust** — the ADR-012 readers (`src-tauri/src/parse/{property,hess,xyz,mo}.rs`),
  which stream the unbounded log and size-cap the structured artifacts (rule #5). It cannot move to
  TS without discarding ADR-009/012.

A type invariant that must hold **across a language boundary** is not a type invariant — it is a
convention with a compiler on only one side. The pair can only be enforced by the compiler if both
halves are in the same language, and `parse_output` is immovable. Therefore **`emit_input` moves to
Rust.**

## Decision

### The identity core and the atom-order-bearing emit move into a Rust crate `orcastudio-core`

A new crate in the cargo workspace, per the source proposal §6.3
([`proposals/editor-architecture-2026-07-30.md`](proposals/editor-architecture-2026-07-30.md)):
`AtomId`, branded `OrcaIndex` / `AseIndex`, `IndexMap`, and the emit that **carries atom order**.
The crate is **separate** (not folded into `src-tauri`) on purpose: Stage 2 compiles it to **WASM**
for the renderer/interaction half, and a `no_std`-friendly core is the precondition for that
(proposal §6.2–6.3). Making it separate now is cheaper than extracting it later.

### Stage 1 moves ONLY the emit that carries atom order — nothing else

The move is deliberately **narrow**. What crosses into `orcastudio-core` in Stage 1:

- the **coordinate block** emit (Scene atom order → ORCA's implicit per-line atom index), and
- the **`%geom Constraints`** emit (explicit atom indices).

These are the two emit paths whose output **depends on atom order**, so they are the two that must
own the `IndexMap`. Everything below **stays in TypeScript**, and this is a decision, not an
oversight (see "What stays in TS").

### Measured scope of the seam (rough, not precise)

Five kinds of seam currently move a **bare integer** across a boundary:

1. **coordinate block** — Scene array position becomes ORCA's implicit atom index (`injectSceneIntoInput`);
2. **`%geom Constraints`** — explicit ORCA **0-based** atom indices, B/A/D/C (`constraints.ts`);
3. **`%geom Scan`** — same 0-based convention, **Phase 4.5**, not yet emitted (only in the manual index today);
4. **xtb `$constrain`** — **1-based** indices (opposite base to ORCA — `wiki/orca/xtb.md`), `constraints.ts`;
5. **sidecar `/geometry`** — positional arrays (`AseIndex`), request and response.

A grep over `index|indices|mask|atoms` in the editor's function signatures gives a **rough** site
estimate — ~68 sites across 7 files (edit-plan ~17, constraints ~13, scene ~11, EditPanel ~8,
measure ~7, NewJobScreen ~7, selection ~5). **This is an order-of-magnitude scoping number, not a
precise count** — a raw grep counts far more occurrences (tests, comments); the intent is to size
the work, not to promise a site list.

### Probe result and its consequence for `parse_output`

Unit 1a measured whether `! UseSym` reorders atoms in ORCA 6.1.0 output artifacts
([`wiki/orca/usesym-atom-order.md`](../orca/usesym-atom-order.md)). **Verdict, in the measured scope:
no observable reorder** — ORCA reorients and symmetrizes but preserves the input atom order in
`.out`, `.property.txt` `$Geometry`, `_trj.xyz`, final `.xyz`, and `.hess $atoms`, across
formaldehyde (C2v), methanol (Cs, SP **and** Opt+Freq — the run with distinguishable one-element
atoms crossing the motion artifacts), and water (C2v).

**Consequence — and the precise reason it is safe:** the `IndexMap` for a `UseSym` job is the
**identity** *in the measured scope*. But the architecture does **not** rest on that scope. What
makes identity a safe assumption **outside** the probe is the **post-condition** (rule #9): the
`parse_output` boundary re-runs the probe's own check — element-sequence equality plus the
rigid-motion fingerprint match — **on the real output of every job**, and a mismatch fails loudly
instead of silently animating a geometry that does not correspond to the Scene's `AtomId`s. The
post-condition is **not a defensive extra bolted onto an identity map**; it is the mechanism that
makes the identity map a legitimate assumption for the point groups the probe never ran and for a
future ORCA version that might start reordering. Identity is the *cheap path when the check passes*;
the check is what earns the right to take it. So `parse_output` builds an identity `IndexMap` by
default and **verifies it against the artifact** — it does not trust it. (Had the probe found a
reorder, this ADR would instead mandate a **permuted** map derived from the fingerprint match at the
boundary; it did not, so the default is identity-with-post-condition. The negative controls proved
the check can go red — `usesym-atom-order.md`.)

## What stays in TypeScript (a decision, named — not the residue of an unfinished move)

- **Chemical-semantics emit stays TS.** ADR-010 splits authority: the input **text owns chemical
  semantics** (method, basis, keyword blocks), the **Scene owns geometry**. The `input-builder/`
  method/basis form emits **no atom order**, so it has no `IndexMap` to own and no reason to move.
  Only the geometry-order-bearing emit crosses.
- **The Scene store, editor UI, and the geometry↔sidecar seam stay TS until Stage 2/3.** Per
  **ADR-010 correction (i)** (amended in this same unit — see that ADR): the geometry↔sidecar
  `IndexMap<AseIndex>` is owned by **whoever emits that order**, and **TS fetches `/geometry`
  directly — Rust never touches it** (verified: `grep -rn "/geometry" src-tauri/src/` is empty). So
  for that seam the owner is TS, and it stays TS until the Scene itself moves in Stage 2.
- **Bond perception stays where ADR-010 correction (ii) put it** — one implementation, in the
  sidecar, moved atomically or not at all. This ADR does not touch it.

Painting this move as "the editor moves to Rust" would be false. **Only the order-bearing emit
moves.** The pixels, the store, the forms, and the sidecar seam do not.

### Named tension, deferred to unit 1e (deliberately)

There are **two** emitters of the coordinate block, and they are not the same thing:

- a **display emit** — the Scene→Monaco projection (ROADMAP Stage 2 makes the xyz block a generated
  read-only projection of the Scene), which exists so the author can *read* the geometry; and
- an **authoritative emit** — the one that **mints the `IndexMap`** at `create_job`, whose order is
  the contract `parse_output` is later held to.

If both exist, which one is the source of truth, and does the display emit also mint a map (that is
then discarded) or borrow the authoritative one? Resolving this **now** would over-fit Stage 1 before
the Scene has moved. It is **consciously deferred to unit 1e** (the wiring unit: `create_job` mints
the map, xtb indices are branded at the serde boundary). Named here so 1e does not rediscover it as a
surprise.

## Consequences

- **ROADMAP** — Phase 4.2 Stage 1 is rewritten into units **1a–1e** (1a = this probe + ingest; 1b
  `AtomId` in the TS Scene; 1c the `orcastudio-core` crate; 1d parse pairing; 1e wiring). The
  Phase-4.5 UseSym open question points at the measured result. See ROADMAP.
- **ADR-010** gains an amendment to correction (i): the `IndexMap` is built by **the module that owns
  the emitted order** — Rust for the ORCA-input seam (this ADR), TS for the geometry↔sidecar seam
  (unchanged, until Stage 2). History is not rewritten.
- **No code in unit 1a** — this ADR records the decision; 1c–1e implement it.
- **`wiki/orca/usesym-atom-order.md`** is the measured basis for the `parse_output` post-condition.

## References

- [`proposals/editor-architecture-2026-07-30.md`](proposals/editor-architecture-2026-07-30.md) §6.3
  (crate structure), §6.2 (WASM precondition), migration Phase 1 (core move, no pixels).
- [ADR-010](adr-010-editor-identity-state.md) — the `emit_input`/`parse_output` pairing this ADR
  makes same-language; its correction (i) is amended alongside this ADR.
- [ADR-012](adr-012-output-parsing-ownership.md) — output parsing is Rust's (why `parse_output` is
  immovable).
- [`wiki/orca/usesym-atom-order.md`](../orca/usesym-atom-order.md) — the unit-1a probe; the
  post-condition's measured basis.
- [`wiki/orca/xtb.md`](../orca/xtb.md), [`wiki/orca/constraints.md`](../orca/constraints.md) — the
  1-based vs 0-based index bases the branded types keep from colliding.
