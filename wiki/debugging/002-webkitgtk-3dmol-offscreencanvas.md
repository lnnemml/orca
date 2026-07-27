# 002-webkitgtk-3dmol-offscreencanvas.md — 3Dmol.js renders nothing in the Tauri (WebKitGTK) webview

**Date:** 2026-07-27 · **Area:** frontend
**Symptom:** The `MoleculeViewer` (3Dmol.js) works in Chromium (`vite dev`) but in the
WebKitGTK webview Tauri uses on Linux `createViewer(...)` throws
`TypeError: null is not an object (evaluating 'this._gl.clearDepth')` and no molecule is drawn.
Generic WebGL is otherwise fine in the same engine (a plain
`canvas.getContext('webgl')` succeeds).

**Root cause:** 3Dmol's `WebGLRenderer.initGL()` (see `node_modules/3dmol/build/3Dmol.js`)
prefers an **OffscreenCanvas + `webgl2`** path and only falls back to an in-DOM canvas when
`OffscreenCanvas` is absent (or the viewer is a grid):

```js
if (OffscreenCanvas && !(isGrid)) {
  _gl_singleton = new OffscreenCanvas(w, h).getContext("webgl2", {...}); // ← null in WebKitGTK
  this._gl = _gl_singleton;                                              // ← null
  this._bitmap = canvas.getContext("bitmaprenderer", {...});
} else {
  this._gl = canvas.getContext("webgl2"|"experimental-webgl"|"webgl", {...}); // works everywhere
}
```

WebKitGTK (2.52 / `libwebkit2gtk-4.1`) **exposes `OffscreenCanvas`** but returns **`null`** when
asked for a `webgl2` context on it (its offscreen WebGL support is incomplete — 3Dmol's own
comment even flags Safari/Firefox, i.e. WebKit, as unreliable for the offscreen `transferFromImageBitmap`
trick). So `_gl_singleton`/`this._gl` end up null and the next GL call (`clearDepth`) throws.
The in-DOM canvas path (`else` branch) works in WebKit — it's just never taken because
`OffscreenCanvas` is truthy.

**Fix:** neutralise `OffscreenCanvas` before the first `createViewer` so 3Dmol takes its
direct-canvas branch. `src/viewer/3dmol-setup.ts` (a side-effect module imported by
`MoleculeViewer.tsx`, mirroring `editor/monaco-setup.ts`):

```ts
(window as unknown as { OffscreenCanvas?: unknown }).OffscreenCanvas = undefined;
```

Assign `undefined` (do **not** `delete` — a bare `OffscreenCanvas` reference inside 3Dmol's
`if (OffscreenCanvas && …)` would then throw `ReferenceError`, get swallowed by its `try/catch`,
and leave `_gl` null again). The direct-canvas path works in every engine we target, and our
viewer only shows single molecules, so the offscreen grid optimisation is irrelevant — safe to
disable unconditionally. (commit: Phase 2.1)

**How it was verified (reusable technique):** the Tauri GUI can't be driven headlessly here
(no `xdotool`; same limitation noted in Phase 0/1). But Tauri's webview **is**
`libwebkit2gtk-4.1`, and the distro ships its `MiniBrowser` at
`/usr/lib/x86_64-linux-gnu/webkit2gtk-4.1/MiniBrowser`. Loading a standalone probe HTML (the real
`node_modules/3dmol` build + the exact `createViewer/addModel/setStyle/render` calls) in
MiniBrowser reproduces the failure **in the identical engine**, and confirms the fix renders the
H₂ dumbbell. Window title / `gnome-screenshot -w` give a headless pass/fail signal. Use this to
test any WebKit-specific rendering without the full Tauri app.

**Lesson / rule:** feature-detecting a browser API by presence (`if (OffscreenCanvas)`) is not
enough in WebKitGTK — the object can exist while a specific context type on it returns null.
When a JS lib works in Chromium but not the Tauri webview, suspect a WebKit-incomplete API on an
otherwise-present global, and test in `webkit2gtk-4.1/MiniBrowser` before blaming the integration.
Recorded as a watchpoint in `wiki/modules/visualization.md`.
