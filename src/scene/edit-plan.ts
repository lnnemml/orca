/**
 * Edit planner (2.5.2d). Pure / node-tested — no React, no fetch. Turns the pick
 * list into either a `ready` plan the edit UI + sidecar can act on, or an
 * `unavailable` explanation. The math is NOT duplicated: `op` and `current` come
 * straight from `measureSelection` (2.5.2b).
 *
 * ## Scope: inter-fragment edits only (this unit)
 *
 * The mask is a **whole fragment** — the fragment of the LAST-clicked atom. That
 * makes it the reagent-vs-substrate case ADR-007 is built around: click the
 * reagent atom last and the reagent moves. Editing an internal coordinate of one
 * molecule (rotating a torsion of the substrate itself) needs a bond-graph split
 * with ring detection to decide which atoms move — a separate unit (2.5.3). So an
 * intra-fragment selection is **explicitly rejected here with the reason**, not
 * silently applied to the whole fragment (which would translate the entire
 * molecule instead of a part of it).
 *
 * ## The reference-atom rule, mirrored from the server
 *
 * The sidecar enforces (422): the last atom of the chain must be IN the mask, all
 * preceding atoms must be OUT. We check the same thing here so the user learns
 * the rule from the UI, not from a 422 after clicking Apply. The server check
 * stays as the boundary guard; this is the friendly first line. Because the mask
 * is the last atom's fragment, "a reference atom fell into the mask" is exactly
 * the intra-fragment case — same test, one reason.
 */

import type { Scene } from "./types";
import { measureSelection } from "./measure";
import {
  atomCount,
  fragmentAtomIndices,
  fragmentRanges,
  locateAtom,
  parseAtomLines,
  replaceFragmentAtoms,
} from "./scene";

export type EditPlan =
  | {
      kind: "ready";
      op: "distance" | "angle" | "dihedral";
      indices: number[];
      mask: number[];
      current: number;
      unit: "Å" | "°";
      movingFragmentId: string;
    }
  | { kind: "unavailable"; reason: string };

const INTRA_FRAGMENT_REASON =
  "These atoms are in the same fragment. Editing an internal coordinate means " +
  "rotating part of a molecule about a bond, which needs a bond-graph split " +
  "(with ring detection) to decide which atoms move — that is the next step " +
  "(2.5.3). For now, pick atoms across two fragments so the mask is a whole " +
  "fragment.";

export function planEdit(scene: Scene, selection: number[]): EditPlan {
  if (selection.length < 2 || selection.length > 4) {
    return {
      kind: "unavailable",
      reason: "Pick 2, 3 or 4 atoms to set a distance, angle or dihedral.",
    };
  }

  // op + current value come from the ONE measurement implementation.
  const m = measureSelection(scene, selection);
  if (m.kind === "none") {
    return {
      kind: "unavailable",
      reason:
        "These atoms don't form a measurable coordinate (coincident atoms, a " +
        "zero vector, or a collinear dihedral).",
    };
  }

  // The mask is the fragment of the LAST-clicked atom — the atom that moves.
  const movingAtom = selection[selection.length - 1];
  const located = locateAtom(scene, movingAtom);
  if (!located) {
    return { kind: "unavailable", reason: "The selection is stale." };
  }
  const movingFragmentId = located.fragment.id;
  const mask = fragmentAtomIndices(scene, movingFragmentId);
  const maskSet = new Set(mask);

  // Reference-atom rule (mirror of the sidecar): every atom BEFORE the last must
  // be static (out of the mask). A reference atom inside the mask == the whole
  // selection (or the moving end of it) sits in one fragment → intra-fragment.
  const references = selection.slice(0, -1);
  if (references.some((r) => maskSet.has(r))) {
    return { kind: "unavailable", reason: INTRA_FRAGMENT_REASON };
  }

  return {
    kind: "ready",
    op: m.kind,
    indices: [...selection],
    mask,
    current: m.value,
    unit: m.unit,
    movingFragmentId,
  };
}

/** How many atoms a standard-xyz string declares on its first line. */
export function xyzAtomCount(xyz: string): number {
  return Number(xyz.trim().split("\n")[0]);
}

/**
 * Front-of-the-boundary check on a sidecar geometry response (2.5.2d) — pure, so
 * it's testable and shared by preview/apply. The server already ran its
 * post-conditions; this is our side's three-line guard before we mutate the
 * scene. Returns an error string, or `null` when the response is safe to apply:
 * - the response must not have moved any STATIC atom
 *   (`max_static_displacement < 1e-6`);
 * - the response must have the SAME atom count as the scene being edited.
 */
export function applyResponseIssue(
  scene: Scene,
  responseXyz: string,
  maxStaticDisplacement: number,
): string | null {
  if (maxStaticDisplacement >= 1e-6) {
    return (
      "The sidecar moved atoms outside the mask " +
      `(max ${maxStaticDisplacement.toExponential(2)} Å) — not applying.`
    );
  }
  const n = xyzAtomCount(responseXyz);
  if (n !== atomCount(scene)) {
    return `The response has ${n} atoms but the scene has ${atomCount(scene)} — not applying.`;
  }
  return null;
}

/**
 * Build the resulting Scene from a sidecar response xyz: take the moving
 * fragment's rows (by its global index range) and hand them to
 * `replaceFragmentAtoms`, which enforces atom count + element order (ADR-008).
 * Used for BOTH preview (view-only) and apply. Throws (never silently mis-slices)
 * if the xyz is unparseable or the fragment vanished.
 */
export function applyResponseToScene(
  scene: Scene,
  movingFragmentId: string,
  responseXyz: string,
): Scene {
  const lines = responseXyz.trim().split("\n");
  const atoms = parseAtomLines(lines.slice(2, 2 + xyzAtomCount(responseXyz)));
  if (!atoms) throw new Error("The sidecar returned no atoms.");
  const range = fragmentRanges(scene).find(
    (r) => r.fragmentId === movingFragmentId,
  );
  if (!range) throw new Error("The moving fragment is no longer in the scene.");
  return replaceFragmentAtoms(
    scene,
    movingFragmentId,
    atoms.slice(range.start, range.end),
  );
}
