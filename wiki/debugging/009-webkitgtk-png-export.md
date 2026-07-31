# 009 — WebKitGTK PNG export: what works (unit-3.16 gate)

Not a bug — a **capability measurement** before building the PNG-export UI, in the same
register as [002](002-webkitgtk-3dmol-offscreencanvas.md) (WebKitGTK's incomplete WebGL).
Two independent PNG paths, both unknown under `webkit2gtk-4.1`; both tested with the 002
**MiniBrowser** technique (a standalone probe in the identical engine Tauri uses, the
result encoded in the window title). Rule #10 — measured, not assumed.

## Path 1 — charts (recharts SVG → 2D-canvas → PNG): **WORKS**
The export path for the recharts plots: serialize the `<svg>`, load it as an `Image` via a
`data:image/svg+xml` URL, `drawImage` it onto a 2D `<canvas>`, then `canvas.toDataURL("image/png")`.
Probe result: **`SVG_OK 6237`** — a real `data:image/png;base64,…`, 6237 bytes, no exception,
no canvas-taint `SecurityError`. So 2D-canvas + `toDataURL` are sound in WebKitGTK (unlike the
webgl2-on-OffscreenCanvas path of 002).

**Caveat for the app (not a WebKit limit):** recharts writes themed colours as
`stroke="var(--muted)"`. A serialized standalone SVG has no `:root` to resolve those, so the
export **resolves `var(--…)` to computed values** (a string replace over the serialized SVG +
an injected `<style>` of the computed custom properties) and forces a white background before
rasterizing. Measured/handled in `src/export/png.ts`.

## Path 2 — 3D scene (3Dmol WebGL → `pngURI()` readback): **WORKS**
`viewer.pngURI()` reads the WebGL drawing buffer back into a PNG data URL. This is a *readback*
from the GL context, which can fail where rendering succeeds — so it needed its own probe.
Result (water, direct-canvas fix from 002): **`PNG_OK 17388`** — a real PNG, 17388 bytes, no
exception. The readback works in WebKitGTK. (A first "stuck at title" run was a **probe bug** —
a double-escaped `\n` broke the inline script's JS — not a WebKit failure; corrected, then OK.)

## Verdict: BOTH paths PASS
All three PNG exports are in scope: the IR spectrum and the energy-per-cycle charts (Path 1),
and a 3D-scene snapshot (Path 2). Nothing is faked or dropped. The author still confirms in the
actual Tauri app (theme/layout differ); the engine-level capability is confirmed here.

## Reusable notes
- `viewer.pngURI()` needs a rendered frame first; call it after `render()` (a `setTimeout(…, ~1s)`
  in the probe to be safe — in the app it is called on a user click, well after the first paint).
- Kill MiniBrowser probes by **PID** (`setsid … & ; kill -- -$PID`), never `pkill -f MiniBrowser`
  — the probe shell's own argv contains "MiniBrowser", so `pkill -f` kills the shell (exit 144,
  output vanishes). Learned here.
