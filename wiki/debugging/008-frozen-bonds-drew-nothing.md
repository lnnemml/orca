# 008 — Frozen bond topology drew no bonds at all (unit 3.14)

## Symptom
After unit 3.13 froze the bond topology for mode animation (commit `2e54e49`), the
animation showed **atoms only — not a single bond/stick**. The atoms sat in the right
places and oscillated plausibly (the amplitude fix was correct); there were simply no
sticks. The author's screenshot confirmed it. Static structures (Molecules screen),
the trajectory player, and everything else still drew bonds normally — only the frozen
mode animation was blank of sticks.

## Root cause (measured from the 3Dmol bundle, not memory — rule #10)
Unit 3.13 froze topology by parsing each animation frame with `addModel(xyz, "xyz",
{assignBonds:false})` and then writing the equilibrium bonds onto the atoms by hand
(`applyFrozenBonds`). The bonds **did** reach the live atom objects (verified:
`selectedAtoms()` returns the model's real atoms, not copies, and we wrote them
symmetrically with a parallel `bondOrder`). So the input looked correct.

The failure is in **how 3Dmol draws** a stick. In `GLModel.drawBondSticks`
(`node_modules/3dmol/src/GLModel.ts`), each bond is drawn from the lower-indexed atom
only, to avoid drawing it twice:

```js
const j = atom.bonds[i];
const atom2 = atoms[j];
if (atom.index < atom2.index) { … draw the cylinder … }
```

The gate is **`atom.index < atom2.index`**. But `assignBonds:false` means the XYZ
parser never runs `assignBonds`, and `assignBonds` is the code that assigns
`atom.index = i` (the parser itself only sets `atom.serial`). So on every
`assignBonds:false` frame **`atom.index` is `undefined`**, and `undefined < undefined`
is `false` for *every* bond → **zero cylinders drawn**, regardless of a perfectly good
bond list. Spheres (which don't use that gate) still drew — hence "atoms, no bonds".

## Fix — the path where the problem cannot exist (commit of unit 3.14)
Instead of tearing down and rebuilding the model each frame (and then fighting 3Dmol's
perception with manual bonds), **build the model once and only update coordinates**:

- On the first frame, `viewer.addModel(equilibriumXyz, "xyz")` — a **normal** parse, so
  3Dmol perceives bonds *and* assigns `atom.index`, exactly as for any static molecule.
  Sticks draw because the index gate now holds.
- Each later frame updates only the atoms' `x/y/z` in place (`applyCoordsToAtoms` over
  `model.selectedAtoms({})`) and calls `model.setStyle({}, …)` — which sets the cached
  `molObj = null`, forcing `render()` to rebuild the geometry from the moved atoms with
  the **same** bonds and indices. Topology is frozen by construction: 3Dmol never gets
  a chance to re-perceive it, and there are no manual bond arrays.

This also corrects an earlier belief (unit 3.8 note) that in-place coordinate updates
require 3Dmol's `setFrame`/`animate` frame apparatus. They do not: mutating `x/y/z` and
nulling `molObj` via `setStyle` is enough, and frame ownership stays in the app
(`ModeAnimator` passes one frame's coords + the topology reference; the viewer draws —
ADR-011). No second bond-perception is introduced (ADR-010): the *only* perception is
3Dmol's normal one, run once on the equilibrium. The trajectory player is untouched —
it still re-perceives per frame on purpose, because along a path bonds really change.

The coordinate-update + draw-gate logic lives in the pure `src/viewer/frozenTopology.ts`.

## The lesson — the test checked the INPUT, not the OUTPUT
Unit 3.13 shipped green. Its test asserted "the **bonded set** is identical at phases
0 / 0.25 / 0.75" — computed from *our* coordinates and a distance cutoff. That is a test
of **what we fed in**, and it passes just as happily when nothing reaches the screen:
the set was stable, it just never got drawn. A blank render is invisible to an
input-side test.

So unit 3.14's test targets the **output**: `drawableBondCount` mirrors 3Dmol's exact
stick gate (`atom.index < atom2.index`) and asserts (a) it is **> 0** for a normally
parsed model, (b) it is **0 when `atom.index` is unset** — reproducing this very
regression — and (c) it stays > 0 and constant across coordinate updates. 3Dmol itself
needs WebGL and does not load in jsdom, so the *rendered* pixels still can't be checked
headless; that boundary is named, and a **DEV-mode assertion in `MoleculeViewer`** warns
in the real webview if a freshly built frozen model has 0 drawable bonds — the check the
3.13 test lacked, now on the real object.

## Files
- `src/viewer/MoleculeViewer.tsx` — the coordinate-update path + the DEV drawable-bonds warning.
- `src/viewer/frozenTopology.ts` / `.test.ts` — pure helpers + the output-side test.
- `wiki/modules/results-ui.md` — how the topology is held now.
