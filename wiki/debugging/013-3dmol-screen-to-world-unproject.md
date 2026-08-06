# 013 — 3Dmol screen→world unproject: which API, and is it accurate?

**Type:** probe / measurement (rule #10 — a fact about 3Dmol, recorded from a run, not the docs).
**Not a bug** — the mandatory de-risking probe before the rigid-body fragment drag (Phase 4.2
Stage 3, unit 3.1). The whole screen-plane drag strategy rests on turning a mouse pixel delta into a
world-space delta in the plane of the screen; if 3Dmol 2.5.5 had no reliable unproject we would have
stopped and switched to an axis-constrained scheme. It does. This records what it is and how accurate,
measured in the real running app.

## The question

Given the user drags the mouse by a pixel vector `(dxPx, dyPx)`, what world-space displacement moves
the grabbed atom so it *tracks the cursor* in the plane of the screen — at any camera orientation?

## The API (measured, not from docs)

3Dmol's `GLViewer` exposes two methods used together (`node_modules/3dmol/src/GLViewer.ts`):

- **`viewer.modelToScreen(coord | coord[])`** → `{x, y}` in **page pixels** (its `canvasOffset()` adds
  `getBoundingClientRect() + pageXOffset`). So a hit-test compares against `event.pageX/pageY`.
  Verified: an atom's projected point lands inside the canvas page-rect.
- **`viewer.screenOffsetToModel(dxPx, dyPx, modelz?)`** → a **world-space `Vector3` displacement**
  (a *delta*, not an absolute point). It unprojects the pixel offset at depth `modelz` and applies the
  view quaternion, so the returned vector lies in the tilted plane of the screen. This is the SAME
  function 3Dmol uses internally for its own pan (`translate`), so it is battle-tested.

## Accuracy (measured, round-trip: unproject then re-project)

Round-trip test: take an atom `P`, `S0 = modelToScreen(P)`; `wd = screenOffsetToModel(dx, dy, modelz)`;
`S1 = modelToScreen(P + wd)`; the tracking error is `S1 - S0 - (dx, dy)` in pixels.

| Camera | `modelz` | Tracking error |
|---|---|---|
| Frontal | default (scene centre, `rotationGroup.position.z`) | **~0.13 px** over 100 px (a flat −0.13%) |
| Rotated 45°y + 30°x | default | direction **exact** (no cross-axis bleed, ratios preserved), magnitude **+6%** — the grabbed atom leads the cursor |
| Frontal **and** rotated 45°y+30°x | **grabbed atom's depth** (below) | **0 px** across 3 atoms × 4 deltas |

The default-`modelz` +6% at a rotated camera is the **perspective depth effect**: the default unprojects
at the *scene-centre* depth, but the grabbed atom sits at a slightly different depth, and world-per-pixel
scale is depth-dependent in perspective. The delta's *direction* is always exact — only the magnitude of
which depth-plane sticks to the cursor is off.

### The fix — pass the grabbed atom's depth as `modelz` (pixel-exact)

The correct `modelz` is the grabbed atom's z **after `modelGroup.matrixWorld`** — which is literally the
first line of `modelToScreen`'s own chain (`t.applyMatrix4(this.modelGroup.matrixWorld)`), so this is
*reusing the documented projection input*, not invented camera math:

```js
modelz = new Vector3(atom.x, atom.y, atom.z).applyMatrix4(viewer.modelGroup.matrixWorld).z
```

Measured: `screenOffsetToModel(dx, dy, modelz)` tracks the grabbed atom to **0 px** at frontal and at
45°y+30°x. A wrong first guess (`t.z + rotationGroup.position.z`) blew up to −350% — the frame is
delicate, which is exactly why this is *measured*, not reasoned. `ratio_just_twz` (the atom's
`matrixWorld` z alone) came out `1.0000`.

## How it is used (unit 3.1)

`MoleculeViewer.grabbedAtomDepth(viewer, atom)` reaches `viewer.modelGroup.matrixWorld` and the
`Vector3` constructor via a **runtime probe** (neither is a typed export). If a 3Dmol upgrade moves
them it returns `undefined` and `screenOffsetToModel` falls back to the default `modelz` — degrading to
the ≤~6% tracking lag at a strongly rotated camera, still usable for a *coarse-position* affordance
(exact geometry comes from the measure/constraint tools, not the drag — division of labour). So the
internal-API coupling can only cost accuracy, never crash.

**Rendering vs projection.** These numbers were taken in the app's real viewer but driven from Chrome
(the dev server at :1420). The projection math is CPU-side (matrices) and identical across browsers;
the GPU/WebGL *render* path differs under WebKitGTK, so the 60fps question is a separate manual gate
(unit 3.1 m4) — measured at ~1.3 ms/frame for a 38-atom scene, ~12× under the 16.7 ms budget.

## See also

- `wiki/modules/visualization.md` — the ephemeral drag reuses the frozen-topology coordinate-update path.
- `wiki/modules/editor-ui.md` — Move mode; drag = coarse placement, precision via editor/constraints.
- ADR-010 — the ephemeral layer (60fps motion not logged; one op on release).
