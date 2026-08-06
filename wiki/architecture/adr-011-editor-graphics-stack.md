# ADR-011: Editor graphics stack

**Status:** proposed / **deferred** — gated on a spike · 2026-07-30
**Depends on:** [ADR-010](adr-010-editor-identity-state.md) (the identity/state model that must
land first)

The target graphics stack from the source proposal
([`proposals/editor-architecture-2026-07-30.md`](proposals/editor-architecture-2026-07-30.md),
§6–§7) is:

- **wgpu → WASM → WebGL2 / WebGPU inside the webview.** One Rust renderer crate
  (`orcastudio-render`) reading `&Scene` directly, compiled to WASM, drawing on a canvas in the
  React layout with DOM overlays on top.
- **Impostor spheres and cylinders** (ray–sphere / ray–cylinder in the fragment shader) for
  atoms and bonds.
- **GPU picking** — an `R32Uint` pass where each fragment writes its identity, returning an
  **`AtomId` directly**, with no intermediate viewer index space.

This is recorded as the intended direction, but the decision is **deferred**. It is not adopted
now, and no work starts against it until the spike below passes.

## Why deferred (stated plainly)

- **No phase-2.5 defect originated in 3Dmol's index space.** That space is aligned today
  precisely because the 3Dmol model is rebuilt from our merged xyz on every render (ADR-008).
  The expensive authority-claimants — the Monaco text, the Scene, the constraint block — are
  different, and they are removed by [ADR-010](adr-010-editor-identity-state.md) **without
  touching a single pixel**. The architectural win does not require the renderer.
- **The replacement cost is large and concrete.** Swapping 3Dmol means rewriting the halo, atom
  numbering, measurement labels, the mask glow, the contrast-invariant themes, and full-screen
  mode — five units of work — *plus* standing up a WASM toolchain, *plus* living with the
  WebGL2 lower bound (no storage buffers, no compute; §7.3 of the proposal), *plus* the
  WebKitGTK platform risk we have already been bitten by and documented
  ([debugging/002](../debugging/002-webkitgtk-3dmol-offscreencanvas.md),
  [debugging/003](../debugging/003-webkitgtk-select-styling.md)). The proposal itself lists
  WebGPU availability in WebKitGTK as an open question (§12.1).

Spending that cost to fix a defect class that ADR-010 already closes without it is the wrong
order of work.

## Gate — a spike with verifiable exit criteria

Before this ADR can move from *deferred* to *accepted*, a throwaway spike must pass **all** of
the following. Each is a checkable fact, not a "look at it later":

- [ ] A **wgpu triangle renders under `webkit2gtk-4.1`** via the existing MiniBrowser bench
      (the same technique used to diagnose [debugging/002](../debugging/002-webkitgtk-3dmol-offscreencanvas.md)).
- [ ] The **WebGL2 code path works** (not only WebGPU) — since WebGL2 is the mandatory lower
      bound for WebKitGTK.
- [ ] The **WASM bundle size is measured as a number** (wgpu + core), so the §12.2 open
      question is answered with data, not a guess.
- [ ] **GPU picking returns the correct identifier** under that same webview — a click resolves
      to the expected `AtomId`.

If any criterion fails, the fallback of a separate native wgpu window (proposal §11.5) is
reconsidered; the ADR-010 boundary (`orcastudio-render` reads `&Scene` and does not know where
its surface lives) keeps that door open.

## Until the gate passes

**3Dmol stays a dumb renderer.** It receives geometry and an `AtomId → viewer index` table from
the core and is **never** a source of truth — not for geometry, not for selection. Clicks are
mapped back to `AtomId` through that table. This is exactly ADR-010's migration Phase 2 shape,
and it is where the editor lives until the spike justifies moving to `orcastudio-render`.

**Realized in unit 2c1:** `buildViewerFeed(scene)` (`src/scene/scene.ts`) returns the geometry **and**
its `AtomId↔viewer-index` table from one pass, so the model 3Dmol draws and the table clicks resolve
through are one object; `onAtomPick` returns an `AtomId` (raw `atom.index` carried only as a diagnostic
`viewerIndex`); the reads-from-3Dmol audit found the boundary intact (`3dmol` imported in one file,
only the two sanctioned reads `pngURI` / the app-owned animation model). The consumers still key on a
positional index behind one named `2c1→2c2` adapter; moving them onto `AtomId` is 2c2. See
[modules/editor-ui.md](../modules/editor-ui.md) and [modules/visualization.md](../modules/visualization.md).

## References

- [`proposals/editor-architecture-2026-07-30.md`](proposals/editor-architecture-2026-07-30.md)
  — §6 (graphics stack), §7 (renderer spec), §11.5–§11.8 (rejected surface options), §12 (open
  questions). Source document; not edited.
- [ADR-010](adr-010-editor-identity-state.md) — the identity/state model that removes the
  authority-claimants without the renderer; the reason this decision can wait.
- [debugging/002](../debugging/002-webkitgtk-3dmol-offscreencanvas.md),
  [debugging/003](../debugging/003-webkitgtk-select-styling.md) — the documented WebKitGTK
  platform risk and the MiniBrowser bench the spike reuses.
