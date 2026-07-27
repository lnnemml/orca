/**
 * Side-effect module: make 3Dmol.js render in the WebKitGTK webview Tauri uses
 * on Linux. Import once before the first `createViewer` call.
 *
 * WebKitGTK exposes `OffscreenCanvas` but returns `null` for a `webgl2` context
 * on it. 3Dmol's renderer prefers that offscreen path (see `initGL` in
 * node_modules/3dmol/build/3Dmol.js) and, getting a null context, dies with
 * `TypeError: null is not an object (evaluating 'this._gl.clearDepth')` — no
 * molecule renders. Plain in-DOM canvas WebGL works fine.
 *
 * Neutralising `OffscreenCanvas` forces 3Dmol onto its direct-canvas branch,
 * which works in every engine we target (verified in webkit2gtk-4.1 MiniBrowser
 * and Chromium). We only ever show single molecules, so the offscreen path's
 * grid optimisation is irrelevant here.
 *
 * See wiki/debugging/002-webkitgtk-3dmol-offscreencanvas.md.
 */
(window as unknown as { OffscreenCanvas?: unknown }).OffscreenCanvas = undefined;
