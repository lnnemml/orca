# ORCA `%geom Constraints` — syntax and the settled index base

The reference page for every piece of constraint code in OrcaStudio
(`src/scene/constraints.ts`) and for Phase 4.5's scan (`%geom Scan`, which shares
the atom-index convention). **Established against a real ORCA 6.1.0 run, not from
memory** — `gotchas.md` forbids trusting memory about ORCA behaviour, and this is
exactly the kind of fact where a confident guess is dangerous.

## The block

Constraints live in a `Constraints … end` sub-block inside `%geom`:

```
%geom
  Constraints
    {B 0 1 1.234 C}
    {A 0 1 2 C}
    {D 0 1 2 3 180.0 C}
    {C 4 C}
  end
end
```

The inline form `%geom Constraints … end end` parses identically (ORCA is
whitespace/newline-insensitive at block boundaries — both forms were run).
OrcaStudio emits the separate-line nested form (`constraintsBlock`) because it
composes cleanly with other `%geom` settings on injection.

### Constraint types

| Token | Coordinate | Atoms | Example |
|---|---|---|---|
| `B` | bond / distance | 2 | `{B 0 1 1.234 C}` |
| `A` | angle | 3 | `{A 0 1 2 109.5 C}` |
| `D` | dihedral | 4 | `{D 0 1 2 3 180.0 C}` |
| `C` | Cartesian (freeze the atom's position) | 1 | `{C 4 C}` |

- The **trailing `C`** is ORCA's *constrain* flag — it is what actually holds the
  coordinate. Without it the line is not a constraint. (For a Cartesian
  constraint the leading `C` is the *type* and the trailing `C` is the flag:
  `{C 4 C}`.)
- **With a value** (`{B 0 1 1.234 C}`) → the coordinate is driven to that value
  and frozen there. **Without a value** (`{B 0 1 C}`) → frozen at the current
  geometry. Using an explicit value is the unambiguous form and the one the
  experiment below relied on.

## Index base — **0-based** (ORCA 6.1.0, verified 2026-07-29)

**`%geom Constraints` atom indices are 0-based.** The first atom in the `* xyz`
block is atom **0**.

This was the open "Question C" from the first session. An off-by-one here does
not crash — it freezes the *wrong* coordinate on a calculation that finishes
`ORCA TERMINATED NORMALLY`, so it had to be settled by an experiment whose two
interpretations produce *visibly different geometry*, not just a different log
line.

### The experiment

Chloromethane, atoms ordered so a one-index shift changes the bond *type*
(`Cl, C, H, H, H`) — C–Cl ≈ 1.78 Å vs C–H ≈ 1.09 Å. The constraint uses an
**explicit value** (1.234 Å, distinct from both) so the frozen pair is unarguable:

```
! r2SCAN-3c Opt
%geom
  Constraints
    {B 1 2 1.234 C}
  end
end
* xyz 0 1
Cl  0.000000  0.000000  1.778000
C   0.000000  0.000000  0.000000
H   1.026719  0.000000 -0.363000
H  -0.513360 -0.889165 -0.363000
H  -0.513360  0.889165 -0.363000
*
```

Index pair `(1, 2)` selects **different physical atoms** under the two conventions:
- **0-based** → atoms 1,2 = **C, H** → a C–H bond frozen at 1.234 Å; C–Cl free.
- **1-based** → atoms 1,2 = **Cl, C** → the C–Cl bond frozen at 1.234 Å; C–H free.

**Result.** ORCA's own redundant-internal-coordinate table (the gold standard —
it prints how ORCA *interpreted* the atoms), with the constrained line flagged `C`:

```
    1. B(C   1,Cl  0)                  1.7780         0.337058
    2. B(H   2,C   1)                  1.2340         0.361425 C
    3. B(H   3,C   1)                  1.0890         0.361425
    4. B(H   4,C   1)                  1.0890         0.361425
```

ORCA labels **carbon = atom 1, chlorine = atom 0** — exactly the 0-based reading
of the `* xyz` order. The constrained (flag `C`) coordinate is `B(H 2, C 1)`, the
C–H bond. And the final optimized geometry confirms it physically:

```
C–H  (atoms 1–2) = 1.2340 Å   ← the constraint held
C–Cl (atoms 0–1) = 1.7996 Å   ← relaxed freely
```

Had ORCA been 1-based, C–Cl would have sat at 1.234 Å and every C–H near 1.09 Å.
It did the opposite → **0-based, unambiguously.**

### Out-of-range behaviour — no bounds check, hard crash

A probe on the same 5-atom molecule with `{C 5 C}` (index 5 is valid only under a
1-based reading; out of range 0..4 under 0-based) **segmentation-faulted** at
"Evaluating the coordinates" — ORCA reads past the atom array and dies. The
control `{C 4 C}` (in range 0..4) ran clean and printed `Will constrain atom 4`.

Two consequences for OrcaStudio:
1. **Overshoot by one → segfault, not a graceful error.** ORCA does *not*
   validate the constraint index. OrcaStudio must range-check indices before
   writing them (they come from the same 0-based merged-xyz space, so this is
   usually automatic — but a stale index after an edit must never reach a
   constraint line).
2. **Undershoot / in-range-but-wrong → silence.** Feeding 1-based indices that
   happen to stay ≤ N−1 would freeze the wrong atoms with *no* error at all —
   the "successful run, wrong chemistry" trap. This is the whole reason the base
   was measured, not assumed.

### Practical consequence — the app warns and blocks (2.5.4b)

Because the input text is the source of truth (constraints live in the text, not a
store) and Scene→Monaco rewrites **only** the coordinate block, the `%geom`
indices are **never rewritten when the scene changes**. Remove a fragment and a
constraint written against the old atom count is now either out of range or, worse,
silently pointing at a different atom. OrcaStudio therefore:

- **blocks Create / Create & Run** when any constraint index is out of range
  (`constraintIndexIssues`) — the one place a run is refused on input content,
  because the alternative is this segfault with no diagnostic;
- **warns on any composition change** while constraints exist, listing what each
  constraint names now (the in-range-but-wrong case, which no range check can
  catch) — but does **not** rewrite or remap the indices; "the same atom after a
  removal" has no operational definition (same call as `selectionSurvives`).

See `wiki/modules/frontend.md` (ConstraintPanel) and `wiki/modules/scene.md`.

## OrcaStudio's mapping

OrcaStudio's own atom indices are already 0-based (the merged-xyz / ASE-mask
space, ADR-008), so the conversion to a constraint-line index is the **identity**.
`constraints.ts` still routes every index through `toOrcaIndex` / `fromOrcaIndex`
(defined in terms of `ORCA_INDEX_BASE = 0`) so the code states the fact rather
than relying on the coincidence — if a future ORCA release changed the base, one
constant moves.

## See also

- `wiki/modules/scene.md` — `constraints.ts` and the "input text is the source of
  truth" decision.
- `wiki/orca/input-format.md` — the rest of the `%` block / `!` line anatomy.
- `wiki/orca/gotchas.md` — the memory-about-ORCA rule this page answers to.
- Phase 4.5 `%geom Scan` will reuse this 0-based convention.
