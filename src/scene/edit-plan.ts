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
import type { AtomId } from "./ids";
import { measureSelectionByIndex } from "./measure";
import { describeAtom } from "./selection";
import {
  atomCount,
  atomIdAtIndex,
  fragmentAtomIndices,
  fragmentRanges,
  globalIndexOfAtom,
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
  | {
      // 2.5.3b: all atoms are in ONE fragment (an intra-fragment torsion). The
      // mask is not a whole fragment but the rotatable side of a bond — the UI
      // must ASK the sidecar (`/geometry/rotatable-mask`). planEdit stays pure &
      // synchronous: it describes WHAT to ask (cut/moving/within), it doesn't ask.
      kind: "needs-split";
      op: "distance" | "angle" | "dihedral";
      indices: number[];
      current: number;
      unit: "Å" | "°";
      /** The bond to break (global indices), per the sidecar's cut rule. */
      cut: [number, number];
      /** The chain's last atom — the side that moves. */
      moving: number;
      /** Restrict perception to this fragment's atoms (keeps a metal–ligand
       * contact from fusing two fragments — see sidecar `within`). */
      within: number[];
    }
  | {
      // Unified moving-set unit: a DISTANCE whose two atoms are in the SAME
      // fragment but DIFFERENT perceived connected components. This is NOT a
      // torsion — there is no bond between them to cut, so it must NOT route to
      // `needs-split` (which would 422 "not bonded" at the sidecar — the
      // Diels-Alder "diene + dienophile in one xyz" bug). Instead one component is
      // translated toward the other to set the distance. planEdit stays pure: the
      // components are INJECTED (bond perception has one home, the sidecar —
      // ADR-010 correction ii), and both moving sets are carried so the UI can
      // offer "move the other component instead".
      kind: "needs-component-move";
      op: "distance";
      /** The two picked global indices `[i, j]`. */
      indices: number[];
      current: number;
      unit: "Å" | "°";
      /** The moving component (global indices) — the SMALLER one, moved by default. */
      moving: number[];
      /** The other component (global indices) — the "move the other instead" set. */
      other: number[];
    }
  | { kind: "unavailable"; reason: string };

/**
 * Injected perceived connectivity for the same-fragment DISTANCE branch: a map from
 * a global atom index to its connected component (global indices), as resolved by
 * the sidecar `/geometry/connected-component`. planEdit does NOT compute this — bond
 * perception has one home (ADR-010 correction ii), so the caller injects it (the
 * same discipline `resolveMovingSet` follows). Absent ⇒ a same-fragment distance
 * stays a `needs-split` torsion, exactly as before (backward compatible).
 */
export type ComponentLookup = ReadonlyMap<number, readonly number[]>;

/** Do two index sets share no atom? (Same fragment, different components ⇒ disjoint.) */
function disjoint(a: readonly number[], b: readonly number[]): boolean {
  const setB = new Set(b);
  return a.every((x) => !setB.has(x));
}

/**
 * The reference-atom rule that makes a rotation mask valid, extracted so ONE
 * function guards BOTH code paths that produce a mask:
 *  - the inter-fragment orientation (`orientationFor` — mask = a whole fragment);
 *  - the intra-fragment bond-graph split, once the sidecar has resolved it
 *    (`NewJobScreen`, after `/geometry/rotatable-mask` — mask = the rotatable side
 *    of a bond).
 *
 * A valid mask holds the MOVING atom and NONE of the reference atoms — the other
 * selected atoms that DEFINE the coordinate and must stay on the STATIC side.
 * Returns `null` when the rule holds, else the offending indices.
 *
 * The 2.5.3b hole this closes: the rule was applied in `planEdit` for the
 * inter-fragment case but never re-run after the sidecar mask arrived. So an
 * intra-fragment split that put a reference atom on the moving side slipped
 * through to a 422 at Apply — the butane case `angle(3,1,2)` → cut (1,2),
 * mask `[2,3,9,10,11,12,13]` ∋ reference `3`. The split can't know which atoms
 * were the references, so the caller must re-check.
 */
export interface MaskRoleViolation {
  /** Reference atoms wrongly INSIDE the mask (on the moving side). */
  referencesOnMovingSide: number[];
  /** The moving atom is missing from the mask (shouldn't happen; guarded anyway). */
  moverOffMovingSide: boolean;
}

export function maskRoleViolation(
  mask: number[],
  moving: number,
  references: number[],
): MaskRoleViolation | null {
  const inMask = new Set(mask);
  const referencesOnMovingSide = references.filter((r) => inMask.has(r));
  const moverOffMovingSide = !inMask.has(moving);
  if (referencesOnMovingSide.length === 0 && !moverOffMovingSide) return null;
  return { referencesOnMovingSide, moverOffMovingSide };
}

/**
 * Explain a post-split mask violation in terms of the user's SELECTION — never
 * the sidecar's own message. `/geometry/rotatable-mask`'s wording is written for
 * the inter-fragment case and reads wrong inside a single molecule; here we name
 * the reference atom(s) that landed on the moving side and the bond the torsion
 * turns about ("atom C#3 lies on the moving side of the C#1–C#2 bond — pick a
 * reference atom on the static side").
 */
export function explainSplitViolation(
  scene: Scene,
  cut: [number, number],
  moving: number,
  violation: MaskRoleViolation,
): string {
  const label = (i: number) => {
    const d = describeAtom(scene, i);
    return d ? `${d.element}#${i}` : `#${i}`;
  };
  const bond = `${label(cut[0])}–${label(cut[1])}`;
  const refs = violation.referencesOnMovingSide;
  if (refs.length > 0) {
    const names = refs.map(label).join(", ");
    const [subject, verb] = refs.length === 1 ? ["atom", "lies"] : ["atoms", "lie"];
    const object = refs.length === 1 ? "a reference atom" : "reference atoms";
    return (
      `${subject} ${names} ${verb} on the moving side of the ${bond} bond — ` +
      `pick ${object} on the static side.`
    );
  }
  return (
    `the atom you're moving (${label(moving)}) isn't on the rotatable side of the ` +
    `${bond} bond — pick a different coordinate.`
  );
}

/** A single orientation's validity: mover = last atom of `chain`; every earlier
 * atom must be OUT of the mover's fragment mask. `null` if the mover is stale or
 * a reference atom sits in its mask (the `maskRoleViolation` rule — shared with
 * the intra-fragment split path). */
function orientationFor(scene: Scene, chain: number[]): Orientation | null {
  const mover = chain[chain.length - 1];
  const located = locateAtom(scene, mover);
  if (!located) return null;
  const mask = fragmentAtomIndices(scene, located.fragment.id);
  if (maskRoleViolation(mask, mover, chain.slice(0, -1))) return null;
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

export function planEdit(
  scene: Scene,
  atomIds: AtomId[],
  components?: ComponentLookup,
): EditPlan {
  if (atomIds.length < 2 || atomIds.length > 4) {
    return {
      kind: "unavailable",
      reason: "Pick 2, 3 or 4 atoms to set a distance, angle or dihedral.",
    };
  }

  // Resolve the stable AtomId selection to the CURRENT positional global indices
  // ONCE, at the boundary (unit 2c2): the whole planner below — masks, cut/within,
  // both orientations, the reference-atom rule — is positional, because its output
  // (`EditPlan.indices`/`mask`/`cut`) is the ASE-mask emit seam, which is positional
  // by design (ADR-010 correction i). An id that no longer resolves → -1, read as
  // out-of-range by `measureSelectionByIndex` → `unavailable`.
  const selection = atomIds.map((id) => globalIndexOfAtom(scene, id) ?? -1);

  // op + current value come from the ONE measurement implementation. `current`
  // is orientation-invariant (see the module note), so it's computed once.
  const m = measureSelectionByIndex(scene, selection);
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

  if (a && b) {
    // Both sides are movable (typical inter-fragment distance). Default to moving
    // the SMALLER fragment (2.5.3b fix): the geometry is identical either way
    // (ORCA is indifferent to absolute coordinates), but moving 33 substrate
    // atoms to place a 5-atom reagent jumps the whole view. Tie → click order (A).
    const chooseA = a.mask.length <= b.mask.length;
    const primary = chooseA ? a : b;
    const alternative = chooseA ? b : a;
    return { kind: "ready", ...base, ...primary, reversed: !chooseA, alternative };
  }
  if (a) return { kind: "ready", ...base, ...a, reversed: false, alternative: null };
  if (b) return { kind: "ready", ...base, ...b, reversed: true, alternative: null };

  // Neither orientation works with WHOLE fragments.
  if (allInOneFragment(scene, selection)) {
    // Unified moving-set unit: a DISTANCE whose two atoms sit in DIFFERENT
    // connected components of the same fragment is NOT a torsion — move one
    // component toward the other (no bond to cut). Requires injected connectivity;
    // without it (or when the two atoms share a component — a genuine
    // bond/torsion) we fall through to `needs-split` unchanged.
    if (m.kind === "distance" && components) {
      const [i, j] = selection;
      const ci = components.get(i);
      const cj = components.get(j);
      if (ci && cj && disjoint(ci, cj)) {
        // Move the SMALLER component (mirrors the inter-fragment "move the smaller
        // fragment" default); tie → the last-clicked atom's component (j).
        const movingIsJ = cj.length <= ci.length;
        return {
          kind: "needs-component-move",
          op: "distance",
          indices: [i, j],
          current: m.value,
          unit: m.unit,
          moving: movingIsJ ? [...cj] : [...ci],
          other: movingIsJ ? [...ci] : [...cj],
        };
      }
    }
    // Intra-fragment torsion → the mask is a bond-graph split the sidecar must
    // compute. Describe the request; the UI makes the call (see the module note).
    const fragmentId = locateAtom(scene, selection[0])!.fragment.id;
    const { cut, moving } = cutAndMoving(m.kind, selection);
    return {
      kind: "needs-split",
      op: m.kind,
      indices: [...selection],
      current: m.value,
      unit: m.unit,
      cut,
      moving,
      within: fragmentAtomIndices(scene, fragmentId),
    };
  }
  // Atoms across fragments but the pivot can't be held fixed — not a bond issue.
  return {
    kind: "unavailable",
    reason: immovablePivotReason(scene, selection, m.kind === "dihedral" ? "dihedral" : "angle"),
  };
}

/**
 * Which bond the motion turns about, and which end moves — the rule the sidecar
 * split uses (`wiki/modules/sidecar.md`): `distance(i,j)` → cut (i,j), move j;
 * `angle(i,v,j)` → cut (v,j), move j; `dihedral(i,j,k,l)` → cut (j,k), move l.
 */
function cutAndMoving(
  op: "distance" | "angle" | "dihedral",
  selection: number[],
): { cut: [number, number]; moving: number } {
  if (op === "distance") {
    return { cut: [selection[0], selection[1]], moving: selection[1] };
  }
  if (op === "angle") {
    return { cut: [selection[1], selection[2]], moving: selection[2] };
  }
  return { cut: [selection[1], selection[2]], moving: selection[3] }; // dihedral
}

/**
 * Flip a ready plan to its alternative orientation (the "Move X instead"
 * action) — the alternative becomes the mover and the current mover becomes the
 * alternative. `op`/`current`/`unit` are unchanged (orientation-invariant).
 * No-op if the plan has no alternative.
 */
export function swapToAlternative(plan: EditPlan): EditPlan {
  // For a component move, "move the other instead" simply swaps the two components.
  if (plan.kind === "needs-component-move") {
    return { ...plan, moving: plan.other, other: plan.moving };
  }
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

// ── needs-component-move resolution (unified moving-set unit) ─────────────────
// A `needs-component-move` plan sets a DISTANCE between two disconnected pieces of
// ONE fragment by RIGIDLY TRANSLATING the moving component along the i→j axis — the
// same net effect as ASE `set_distance` on that mask, but computed purely here and
// committed through `translateAtoms` (count+order invariant, ADR-008; one Undo,
// ADR-010) — never a bespoke mover. No sidecar round-trip for the move itself; the
// component was already perceived by `/geometry/connected-component` upstream.

type Vec3 = [number, number, number];

/** A picked atom's position, or null if the global index has no atom. */
function positionOf(scene: Scene, globalIdx: number): Vec3 | null {
  const loc = locateAtom(scene, globalIdx);
  if (!loc) return null;
  const a = loc.fragment.atoms[loc.localIndex];
  return [a.x, a.y, a.z];
}

/**
 * The rigid translation (Å) that moves an atom at `movingPos` along the
 * `refPos → movingPos` axis so the pair's separation becomes `target`. Applied to
 * the WHOLE moving component (a rigid shift), it sets `|i − j| = target` exactly
 * (the resulting distance is `target` by construction — a linear move along the
 * axis). Coincident points have no axis → zero translation (guarded upstream: the
 * plan is only built for a MEASURABLE distance). Pure.
 */
export function axisTranslation(movingPos: Vec3, refPos: Vec3, target: number): Vec3 {
  const dx = movingPos[0] - refPos[0];
  const dy = movingPos[1] - refPos[1];
  const dz = movingPos[2] - refPos[2];
  const d = Math.hypot(dx, dy, dz);
  if (d === 0) return [0, 0, 0];
  const s = (target - d) / d;
  return [dx * s, dy * s, dz * s];
}

export type NeedsComponentMove = Extract<EditPlan, { kind: "needs-component-move" }>;

/** The concrete component-move: the moving atoms (stable AtomIds → `translateAtoms`),
 * their fragment, the two picked positions (mover / reference), and the current
 * separation. `null` if any picked/moving atom no longer resolves. */
export interface ResolvedComponentMove {
  movingAtomIds: AtomId[];
  movingFragmentId: string;
  movingPos: Vec3;
  refPos: Vec3;
  current: number;
}

/**
 * Resolve a `needs-component-move` plan against the CURRENT scene. The mover is the
 * picked atom that lies in `plan.moving` (the smaller component by default, or the
 * other after a "move the other instead" swap); the reference is the other picked
 * atom. Returns the moving component as AtomIds so the caller commits ONE
 * `translate-atoms` op. Pure / testable.
 */
export function resolveComponentMove(
  scene: Scene,
  plan: NeedsComponentMove,
): ResolvedComponentMove | null {
  const [i, j] = plan.indices;
  const movingSet = new Set(plan.moving);
  const movingPicked = movingSet.has(i) ? i : j;
  const refPicked = movingPicked === i ? j : i;
  const movingPos = positionOf(scene, movingPicked);
  const refPos = positionOf(scene, refPicked);
  const movingFragmentId = locateAtom(scene, movingPicked)?.fragment.id;
  if (!movingPos || !refPos || !movingFragmentId) return null;
  const movingAtomIds: AtomId[] = [];
  for (const gi of plan.moving) {
    const id = atomIdAtIndex(scene, gi);
    if (id === null) return null; // a moving atom vanished — refuse rather than mis-move
    movingAtomIds.push(id);
  }
  return { movingAtomIds, movingFragmentId, movingPos, refPos, current: plan.current };
}

/** The separation between the two picked atoms in `scene` — for the after-apply
 * post-condition (rule #9: verify the move in OUR terms, not the mover's). `null`
 * if either atom is gone. */
export function pickedDistance(scene: Scene, indices: number[]): number | null {
  const a = positionOf(scene, indices[0]);
  const b = positionOf(scene, indices[1]);
  if (!a || !b) return null;
  return Math.hypot(a[0] - b[0], a[1] - b[1], a[2] - b[2]);
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
