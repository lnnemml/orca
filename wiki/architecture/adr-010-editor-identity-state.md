# ADR-010: Editor identity and state model

**Status:** accepted · 2026-07-30
**Supersedes:** —
**Refines:** [ADR-008](adr-008-scene-fragment-model.md) (per-layer index discipline → locality in one module)

The editor is the one place in the app where all three runtimes meet, and it is the place
where a single integer passes through four numbering conventions
(`pure TS → 3Dmol → HTTP → ASE → ORCA / xtb`). This ADR fixes the identity and state model
that removes that class of defect at the architecture level, not with local fixes.

**The full rationale and the list of rejected alternatives live in the source proposal:**
[`proposals/editor-architecture-2026-07-30.md`](proposals/editor-architecture-2026-07-30.md)
(author's design proposal, committed at `67c763c`). This ADR records what was **accepted** and
the **three corrections** made during review; it does not re-narrate the proposal's §11
(rejected alternatives) — read it there. The proposal is the input document; it is not edited,
and it is not itself a decision.

## Decision

The following are adopted as binding:

- **`AtomId`** — an opaque, stable atom identity assigned at creation and invariant until
  deletion. It is **not** a position in any array.
- **Branded `OrcaIndex` / `AseIndex`.** No bare integer crosses a runtime boundary; what
  crosses is either an `AtomId` or a space-tagged index. Mixing the branded types does not
  compile.
- **Atom order matters in exactly one place — the ORCA input generator.** The same module that
  emits the input hands back the inverse table used to parse the output.
- **`emit_input` / `parse_output` are paired with an `IndexMap`.** `parse_output` cannot be
  called without the mapping produced by the corresponding `emit_input` — this is a type-level
  invariant, not a convention.
- **State is a fold over a log of typed operations.** Undo/redo are free consequences; a new
  editor capability adds an *operation*, not a new piece of state.
- **An ephemeral layer carries the drag.** 60-fps drag motion is **not** written to the log;
  it is a transient overlay over the Scene, committed as **one** operation on `mouseup`.
- **Authority is split by kind of data:** the input text owns chemical semantics (method,
  basis, keyword blocks); the Scene owns geometry. There is no overlap.
- **The reaction product is derived from the reactant** by a sequence of operations, not drawn
  independently — so atom mapping (reactant→product) exists by construction, with no heuristic
  graph matching.

## Corrections made during review

Three points were changed relative to the proposal. Each is recorded with its reason, because
each would otherwise be relitigated.

**(i) The sidecar returns POSITIONAL arrays; Rust builds the `IndexMap` at the boundary.**
The proposal's boundary protocol (§4.2) requires every payload to carry `{id, z, xyz}` objects
and forbids bare `positions`. That rule is right for **our** boundaries, but it cannot be
imposed on the Python sidecar: cclib, RDKit and ASE know nothing about `AtomId` — they emit and
consume positional arrays, in file/library order. Forcing `AtomId` into the sidecar would
contradict [ADR-002](adr-002-python-sidecar.md) (the sidecar is stateless chemistry-of-files
logic) and would break the Phase 3 result-parsing plan, which is built on cclib's positional
output. **Resolution:** the sidecar stays positional. The `IndexMap<AseIndex>` is constructed
in Rust, at the moment the request is emitted and against the array the sidecar returns — the
same module owns the outgoing order and the incoming mapping, exactly as `emit_input` /
`parse_output` do for ORCA. The `AtomId`-carrying protocol of §4.2 governs the **core↔render**
and **core↔UI** boundaries (the ones we own end to end), not the core↔sidecar boundary. This
keeps ADR-010 and ADR-002 consistent instead of in conflict.

**(ii) Bond perception has exactly ONE implementation.** Today it lives in the sidecar
(`natural_cutoffs`, the explicit `1.2` multiplier, `within`, ring detection via "a cut that
does not disconnect the graph"). If it ever moves into `orcastudio-core`, the sidecar **loses**
its copy in the same change — it is not kept "just in case". Two implementations of the same
perception rule would repeat the history of the duplicated vdW and CPK tables that the phase-2.5
lint had to reconcile. One rule, one home, moved atomically or not at all.

**(iii) The proposal's index-visibility rule is REJECTED.** §4.3 says ORCA indices are *never*
shown to the user. OrcaStudio is a **learning instrument** (see `CLAUDE.md` mission): hiding
ORCA's own language works against that goal — the user should be able to see and learn the
numbering ORCA reports. **The rule in force instead:** never show a *bare* index without naming
its space — as the current UI already does with `local index 3 · global index 3 (both
0-based)`. The defect §4.3 targets (user reports an index from the UI that does not match the
logs) is removed by *labelling the space*, not by *hiding the number*.

## Relationship to ADR-008

ADR-010 **refines** [ADR-008](adr-008-scene-fragment-model.md); it does not contradict it, and
ADR-008 is not edited. ADR-008 established the Scene/SceneFragment model and kept one index
space end to end by rebuilding a single 3Dmol model from the merged xyz. That is per-layer
discipline: every boundary re-derives the mapping correctly by hand. ADR-010 takes the same
goal and makes it structural — the mapping becomes a typed artifact (`IndexMap`) owned by the
module that emits the order, so the discipline is enforced by the compiler in one place rather
than practised at each layer. Where ADR-008 says "one model, one index space," ADR-010 says
"one `AtomId`, one owner of order" — the second is the first, hardened.

## Empirical addendum: probes and post-conditions, not just types

The type invariants above are necessary but not sufficient, and the phase-2.5 record proves it:
**every defect found in phase 2.5 was caught by a probe or a post-condition, not by a type.**
The two ORCA/xtb index bases (0-based vs 1-based) disagreeing, the empty `--input` hanging xtb,
`mask` silently overriding `indices` — none of these are type errors; they are facts about
external programs that only a run reveals. Hence the two new domain rules recorded in
`CLAUDE.md` (#9 process-boundary post-conditions in our own terms; #10 no third-party behaviour
accepted from memory/docs, only from a logged run) are the **empirical complement** to this
ADR's type-level invariants. Types remove the off-by-one that never should have compiled; probes
and post-conditions remove the ones no type can see.

## References

- [`proposals/editor-architecture-2026-07-30.md`](proposals/editor-architecture-2026-07-30.md)
  — full rationale, invariants I1–I7, state model, migration plan, and §11 rejected
  alternatives. The source document for this ADR.
- [ADR-002](adr-002-python-sidecar.md) — the sidecar is stateless chemistry-of-files logic
  (correction (i)).
- [ADR-008](adr-008-scene-fragment-model.md) — the Scene/SceneFragment model this ADR refines.
- [ADR-009](adr-009-process-orchestration.md) — process spawning is Rust's, not the sidecar's
  (the boundary this ADR's `emit_input`/`parse_output` pairing sits behind).
- [ADR-011](adr-011-editor-graphics-stack.md) — the renderer decision, deferred; until it lands
  3Dmol is a dumb renderer fed geometry + an `AtomId → viewer index` table by the core.
- `ROADMAP.md` — Phase 4.2 (Geometry editor completion) stages this ADR; Phase 3's per-atom
  boundary seam item is the first place the `IndexMap` boundary is exercised.
