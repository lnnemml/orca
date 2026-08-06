/**
 * The rigid-body fragment drag — PURE control logic (Phase 4.2 Stage 3, unit 3.1).
 *
 * This is the testable core of the drag, split out from the 3Dmol/DOM wiring in
 * `MoleculeViewer` exactly as `syncMonacoToScene` was split out in unit 2d: a
 * simulated sequence (begin, move×N, end) can be driven here with no jsdom to
 * prove the two contracts that matter (ADR-010 ephemeral layer):
 *
 *  • **One op on release.** `end` commits **exactly once**, with the TOTAL delta
 *    from the grab point — never one op per `move`. The total is computed from
 *    `endPx - startPx` (not by summing per-move steps), so it is the summed delta
 *    by construction.
 *  • **The Scene is untouched during the drag.** `move` only calls `showEphemeral`
 *    (a viewer-only overlay — the frozen-topology coordinate update); it never
 *    touches the store. Nothing is committed until `end`.
 *
 * The world delta is produced by the injected `unproject` (the viewer wires this
 * to 3Dmol's `screenOffsetToModel(dxPx, dyPx, modelz)`, measured pixel-exact at
 * the grabbed atom's depth — see `wiki/debugging/013`). Keeping `unproject`
 * injected is what lets the test run without a WebGL viewer.
 */

/** A world-space displacement in Å. */
export type WorldDelta = readonly [number, number, number];

/** Screen pixel position (page coordinates — the space `modelToScreen` returns). */
export type ScreenPx = readonly [number, number];

export interface DragHooks {
  /** Screen pixel delta → world delta at the grabbed atom's depth (the plane of the screen). */
  unproject: (dxPx: number, dyPx: number) => WorldDelta;
  /** Show the grabbed fragment shifted by this world delta in the VIEWER ONLY (no store write). */
  showEphemeral: (delta: WorldDelta) => void;
  /** Commit ONE `translate-fragment` op with the total delta — called once, on release. */
  commit: (fragmentId: string, delta: WorldDelta) => void;
  /** Drop the ephemeral overlay and restore the pickable Scene feed (after commit / on cancel). */
  restore: () => void;
}

export interface DragController {
  /** Grab a fragment at the pointer-down position. */
  begin(fragmentId: string, px: ScreenPx): void;
  /** Update the ephemeral position to follow the pointer (no commit). */
  move(px: ScreenPx): void;
  /** Release: commit ONE op with the total delta (if the fragment actually moved), then restore. */
  end(px: ScreenPx): void;
  /** Abandon the drag with no commit (e.g. the viewer unmounts, or Move mode turns off mid-drag). */
  cancel(): void;
  /** The fragment currently being dragged, or null. */
  readonly activeFragmentId: string | null;
}

/** A delta that is exactly zero on every axis (a click with no drag → no op). */
function isZero(d: WorldDelta): boolean {
  return d[0] === 0 && d[1] === 0 && d[2] === 0;
}

export function makeDragController(hooks: DragHooks): DragController {
  let fragmentId: string | null = null;
  let startPx: ScreenPx | null = null;
  let moved = false;

  const clear = () => {
    fragmentId = null;
    startPx = null;
    moved = false;
  };

  return {
    begin(id, px) {
      fragmentId = id;
      startPx = px;
      moved = false;
    },
    move(px) {
      if (startPx === null) return;
      moved = true;
      hooks.showEphemeral(hooks.unproject(px[0] - startPx[0], px[1] - startPx[1]));
    },
    end(px) {
      if (startPx === null || fragmentId === null) {
        clear();
        return;
      }
      // TOTAL delta from the grab point — one op, the summed motion (ADR-010).
      const delta = hooks.unproject(px[0] - startPx[0], px[1] - startPx[1]);
      const id = fragmentId;
      if (moved && !isZero(delta)) {
        // A real move: commit ONE op. The store update rebuilds the model at the
        // committed coordinates — which equal the ephemeral ones already on screen
        // — so we DON'T `restore` here (that would flash the pre-drag position for
        // a frame before the rebuild).
        hooks.commit(id, delta);
      } else {
        // A click with no drag, or a drag that netted zero → nothing committed, so
        // drop the (possibly nudged) ephemeral overlay back to the Scene geometry.
        hooks.restore();
      }
      clear();
    },
    cancel() {
      if (startPx !== null) hooks.restore();
      clear();
    },
    get activeFragmentId() {
      return fragmentId;
    },
  };
}
