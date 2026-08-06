# debugging/014 — a 2-fragment scene silently merged into one "Molecule" fragment

**Symptom.** A substrate + reagent scene (e.g. Dexketoprofen + BH₄⁻, "2 fragments · 38 atoms")
silently collapsed into a **single fragment named "Molecule"**. Everything that keys on the
fragment layout broke at once: rigid **rotate** (unit 3.3) and **move** (3.1) grabbed the whole
merged blob, the inter-fragment **vdW clash** check (3.2) saw one fragment and could never flag a
contact, and per-fragment **charge**/**constraints** lost their boundaries.

**The clue that pinned it.** The merged fragment's name was **"Molecule"** — the *default* name
`sceneFromAtomLines` gives a fragment parsed from a coordinate block with no explicit name
(`opts.name ?? "Molecule"`, `scene.ts`). So something had **re-adopted the merged xyz from text as
one fragment**, overwriting the two-fragment structure. The History entry confirmed the op:
**"Adopt geometry from input text — 1 fragment, 8 atoms"** (a `restore-snapshot` with source
`text-adopt`).

## Measurement, not hypothesis (rule #10)

The prompt's leading hypothesis was **restore/New-iteration re-adopting from text**. That was
**measured false**: a persist→restore round-trip of a 2-fragment scene keeps both fragments —
`restoreScene`/`restoreSceneLog` honour the `scene_json` snapshot because `xyzMatchesScene(snapshot,
mergeToAtomLines(fromText))` is **true** (the input text is the injected projection of that very
scene, so the geometries match). Snapshot honoured → 2 fragments, `snapshotRejected: false`. Restore
was never the bug.

The culprit was found by a **live WebKitGTK repro** (`localhost:1420` in Chrome): build H₂O + BH₄⁻
(viewer shows CPK water + a **teal** BH₄ = 2 fragments, "2 fragments · 8 atoms"), then click
**Input Builder → Generate Input**. The BH₄ instantly recoloured from **teal → CPK green boron**
(fragment 0), and the DOM read **"1 fragment · 8 atoms · total charge −1"**, the fragment name input
**"Molecule"**. The measured action is **"Generate Input"**, not restore.

## Root cause

`handleGenerate` (Input Builder) and `pickTemplate` funnel through `adoptWholeInput(newContent)`,
which **unconditionally** did:

```ts
seedScene(sceneFromOrcaInput(newContent), "text-adopt");
```

`sceneFromOrcaInput` parses a coordinate block into exactly **one** fragment ("Molecule"). But
"Generate Input" rewrites only the `!`/`%` **keyword lines** — the coordinate block it emits is the
**current scene's own geometry**, merged. So the geometry hadn't changed at all; re-adopting it as a
single fragment threw away the substrate/reagent split for nothing. (The Input Builder is itself
fragment-blind — it derives its scene from `sceneFromOrcaInput(currentContent)`, one fragment — which
is why it showed "Σ of 1 fragment" even while the store held two.)

## Fix

Adopt should **preserve** the multi-fragment Scene when the adopted geometry matches it; re-seed only
on a genuinely different (or absent) geometry. A pure helper, reusing the same `xyzMatchesScene`
primitive that guards the live Scene↔Monaco sync (no second comparison):

```ts
// scene.ts
export function adoptPreservesScene(current: Scene | null, newContent: string): boolean {
  if (!current) return false;
  const parsed = sceneFromOrcaInput(newContent);
  return parsed !== null && xyzMatchesScene(current, mergeToAtomLines(parsed));
}
```

`adoptWholeInput` now guards on it — keep the Scene when the geometry matches (Generate Input over the
same coordinates), re-seed only otherwise (Replace input with a genuinely different molecule; a
template's placeholder geometry; no block → clear). `Replace input`'s own confirmed reset
(`adoptReplace`) is unchanged — it deliberately discards the lineage.

**Why this is the right seam.** It mirrors the content→Scene effect's existing guard
(`xyzMatchesScene(current, parsed) → keep`), so a text change that doesn't change geometry never
disturbs the Scene, whether it arrives by hand-edit (already handled) or by Generate/adopt (now
handled).

## Regression guard

`adopt.test.ts` — the seam a test would have caught this at (**not** the restore round-trip the
hypothesis suggested; restore was always correct):
- `adoptPreservesScene(twoFragScene, generatedSameGeometry)` → **true** (preserve);
- different geometry / `null` scene / no block → **false** (real re-adopt);
- store-level: seed 2 fragments → the guarded adopt keeps **2** (names `["Dexketoprofen", "BH4"]`);
  the **negative control** — a blind `text-adopt` of the same generated content — collapses to **1
  "Molecule"** (the measured bug). Forcing `adoptPreservesScene` to `false` (baseline) reddens both
  the seam and store assertions.

## Manual gate (WebKitGTK, live)

**m1 — confirmed:** after the fix, "Generate Input" on the H₂O + BH₄⁻ scene leaves **"2 fragments · 8
atoms"**, names **["H₂O", "BH₄⁻"]**, BH₄ still **teal** (not CPK) — the fragment layout survives.
m2/m3 (rotate turns only BH₄; clash sees inter-fragment pairs; per-fragment charge/constraints)
follow from the restored 2-fragment scene and their own unit tests.

## Related

- `wiki/modules/scene.md` — the adopt rule (`adoptPreservesScene`); `restore.ts` (persist is the
  source of truth on restore — measured correct here).
- Units 3.1 (move), 3.2 (clash), 3.3 (rotate) — all silently disabled by the merge; all rely on a
  correct fragment layout.
