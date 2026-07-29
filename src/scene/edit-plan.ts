/**
 * Edit planner (2.5.2d, both-orientation fix 2.5.2d-2). Pure / node-tested — no
 * React, no fetch. Turns the pick list into either a `ready` plan the edit UI +
 * sidecar can act on, or an `unavailable` explanation. The math is NOT
 * duplicated: `op` and `current` come straight from `measureSelection` (2.5.2b).
 *
 * ## Click order is a DEFAULT, not a rule (2.5.2d-2)
 *
 * The mask is a **whole fragment** — the reagent-vs-substrate case ADR-007 is
 * built around. The original 2.5.2d took the fragment of the LAST-clicked atom
 * as the mover, full stop. That was wrong: a selection can be **read in either
 * direction**, and angle/dihedral are **invariant under chain reversal**
 * (`angle(i,v,j) == angle(j,v,i)`, `dihedral(i,j,k,l) == dihedral(l,k,j,i)`;
 * distance is symmetric — verified in ASE 3.29.0 and in `measure.test.ts`).
 * Reversing the chain doesn't change the value, only *which end moves*. So the
 * defect: B#33(BH₄⁻)→C#12(ibuprofen)→O#14(ibuprofen) was refused as
 * "same fragment" because the LAST atom's fragment (ibuprofen) held the
 * reference C#12 — when the very same angle read the other way (O#14–C#12–B#33)
 * moves BH₄⁻ with both references in ibuprofen. That's the nucleophile
 * attack-angle edit the editor exists for.
 *
 * `planEdit` therefore tries **both orientations**:
 *  - **candidate A** = chain as clicked (mover = last);
 *  - **candidate B** = reversed chain (mover = first).
 * Each must pass the reference-atom rule (mover in its own fragment's mask, no
 * reference in that mask). If only one passes, take it. If both pass (the usual
 * inter-fragment distance — either side can move), take **A** so click order
 * stays the default, and expose **B** as `alternative` for a "Move X instead"
 * toggle. If neither passes, refuse — with **two distinct reasons** (below).
 *
 * ## Two distinct refusals
 *  - **All atoms in one fragment** → genuinely intra-fragment: rotating a
 *    molecule's own torsion needs a bond-graph split with ring detection (2.5.3).
 *  - **Atoms across fragments but no orientation works** (a dihedral whose axis
 *    atoms straddle fragments, or an angle whose two ends share a fragment) → the
 *    rotation axis/vertex can't be held fixed; the message names the offending
 *    atom indices.
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

/** One valid way to run the edit: which fragment moves, the ASE chain (mover
 * last), and that fragment's mask. */
interface Orientation {
  movingFragmentId: string;
  indices: number[];
  mask: number[];
}

export type EditPlan =
  | {
      kind: "ready";
      op: "distance" | "angle" | "dihedral";
      indices: number[];
      mask: number[];
      current: number;
      unit: "Å" | "°";
      movingFragmentId: string;
      /** The chain was reversed vs click order so the reagent (first-clicked) moves. */
      reversed: boolean;
      /** The other valid orientation, if any — moves the opposite fragment. */
      alternative: Orientation | null;
    }
  | { kind: "unavailable"; reason: string };

const INTRA_FRAGMENT_REASON =
  "These atoms are in the same fragment. Editing an internal coordinate means " +
  "rotating part of a molecule about a bond, which needs a bond-graph split " +
  "(with ring detection) to decide which atoms move — that is the next step " +
  "(2.5.3). For now, pick atoms across two fragments so the mask is a whole " +
  "fragment.";

/** A single orientation's validity: mover = last atom of `chain`; every earlier
 * atom must be OUT of the mover's fragment mask. `null` if the mover is stale or
 * a reference atom sits in its mask. */
function orientationFor(scene: Scene, chain: number[]): Orientation | null {
  const mover = chain[chain.length - 1];
  const located = locateAtom(scene, mover);
  if (!located) return null;
  const mask = fragmentAtomIndices(scene, located.fragment.id);
  const maskSet = new Set(mask);
  if (chain.slice(0, -1).some((r) => maskSet.has(r))) return null;
  return { movingFragmentId: located.fragment.id, indices: [...chain], mask };
}

/** Do all selected atoms belong to one fragment (the genuine intra-fragment case)? */
function allInOneFragment(scene: Scene, selection: number[]): boolean {
  const first = locateAtom(scene, selection[0])?.fragment.id;
  if (first == null) return false;
  return selection.every((i) => locateAtom(scene, i)?.fragment.id === first);
}

/** Reason for a multi-fragment selection that no orientation can satisfy: names
 * the reference atoms that would move with an endpoint whichever way the chain
 * is read (the atoms straddling the rotation axis / vertex). */
function immovablePivotReason(
  scene: Scene,
  selection: number[],
  op: "angle" | "dihedral",
): string {
  const offending = new Set<number>();
  for (const chain of [selection, [...selection].reverse()]) {
    const mover = chain[chain.length - 1];
    const located = locateAtom(scene, mover);
    if (!located) continue;
    const maskSet = new Set(fragmentAtomIndices(scene, located.fragment.id));
    for (const r of chain.slice(0, -1)) if (maskSet.has(r)) offending.add(r);
  }
  const list = [...offending].sort((a, b) => a - b).map((i) => `#${i}`).join(", ");
  const pivot = op === "dihedral" ? "both axis atoms" : "the vertex";
  return (
    `No orientation of this chain keeps the rotation ${op === "dihedral" ? "axis" : "vertex"} ` +
    `fixed: atoms ${list} would move with an endpoint whichever end you pick. For a valid ` +
    `edit, ${pivot} — and every atom except one endpoint — must lie in a single STATIC ` +
    `fragment. Re-pick so only one endpoint is in a different fragment.`
  );
}

export function planEdit(scene: Scene, selection: number[]): EditPlan {
  if (selection.length < 2 || selection.length > 4) {
    return {
      kind: "unavailable",
      reason: "Pick 2, 3 or 4 atoms to set a distance, angle or dihedral.",
    };
  }

  // op + current value come from the ONE measurement implementation. `current`
  // is orientation-invariant (see the module note), so it's computed once.
  const m = measureSelection(scene, selection);
  if (m.kind === "none") {
    return {
      kind: "unavailable",
      reason:
        "These atoms don't form a measurable coordinate (coincident atoms, a " +
        "zero vector, or a collinear dihedral).",
    };
  }

  const base = { op: m.kind, current: m.value, unit: m.unit } as const;
  const a = orientationFor(scene, selection); // mover = last-clicked
  const b = orientationFor(scene, [...selection].reverse()); // mover = first-clicked

  if (a) {
    // Click order stays the default; expose B (if valid) as the alternative.
    return { kind: "ready", ...base, ...a, reversed: false, alternative: b };
  }
  if (b) {
    return { kind: "ready", ...base, ...b, reversed: true, alternative: null };
  }

  // Neither orientation works → refuse with the right reason.
  if (allInOneFragment(scene, selection)) {
    return { kind: "unavailable", reason: INTRA_FRAGMENT_REASON };
  }
  return {
    kind: "unavailable",
    reason: immovablePivotReason(scene, selection, m.kind === "dihedral" ? "dihedral" : "angle"),
  };
}

/**
 * Flip a ready plan to its alternative orientation (the "Move X instead"
 * action) — the alternative becomes the mover and the current mover becomes the
 * alternative. `op`/`current`/`unit` are unchanged (orientation-invariant).
 * No-op if the plan has no alternative.
 */
export function swapToAlternative(plan: EditPlan): EditPlan {
  if (plan.kind !== "ready" || !plan.alternative) return plan;
  const alt = plan.alternative;
  return {
    ...plan,
    movingFragmentId: alt.movingFragmentId,
    indices: alt.indices,
    mask: alt.mask,
    reversed: !plan.reversed,
    alternative: {
      movingFragmentId: plan.movingFragmentId,
      indices: plan.indices,
      mask: plan.mask,
    },
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
