//! Form / break bond (Stage E3b) — pure, node-tested, React-free.
//!
//! **Form/break is NOT a new engine.** It is `planEdit(op="distance")` with a
//! COMPUTED target: form = set the picked pair to a bonding distance so
//! distance-based perception (`covalent-radii.ts` basis, the sidecar's 1.2×
//! window) sees a bond; break = set it clearly past that window so perception
//! drops it. The mask (what moves), the reference-atom rule, and — critically —
//! `replaceFragmentAtoms`'s count+order invariant (ADR-008) ALL come from the
//! existing edit path unchanged. So a product geometry DERIVED by form/break keeps
//! the reactant's atom identity (index + order) by construction — which is exactly
//! the NEB precondition (`wiki/orca/neb.md`): NEB(reactant, derived product) is
//! accepted with no same-order refusal. Nothing new is stored: no connectivity, no
//! bond edge-set, no reactant→product lineage — perception follows the distance.

import type { Scene } from "./types";
import type { AtomId } from "./ids";
import { describeAtomById } from "./selection";
import { covalentRadius, type BondOrder } from "./covalent-radii";
import { planEdit, type EditPlan } from "./edit-plan";

/** The bond-perception multiplier the sidecar uses ((rA+rB) × this = the
 * distance below which a bond is perceived; `wiki/modules/sidecar.md`, 2.5.3a).
 * Named here only to justify `breakDistance`'s ×2 clearing it — not re-derived. */
const PERCEPTION_MULTIPLIER = 1.2;

/** The bonding distance (Å) for a pair at bond `order` — the order-appropriate
 * covalent-radius SUM (rA + rB). Order 1 (default) = Cordero single, the same basis
 * perception uses, so the pair reads as a bond; order 2/3 = the Pyykkö double/triple
 * sums, i.e. a SHORTER geometric target that the method then infers as a multiple
 * bond (ORCA reads geometry, not bond order — the honest frame). Element-dependent:
 * C–C ≈ 1.52, C=C ≈ 1.34, C≡C ≈ 1.20 (not a fixed default). Throws if either element
 * lacks a radius at that order (rule #11). */
export function bondingDistance(
  elemA: string,
  elemB: string,
  order: BondOrder = 1,
): number {
  return covalentRadius(elemA, order) + covalentRadius(elemB, order);
}

/**
 * A GEOMETRIC bond-order estimate: of the single/double/triple covalent sums for
 * the pair, the one NEAREST to `distance` (min |d − sum|). Purely from geometry +
 * radii — an estimate, never the computed (Mayer) order, which the editor does not
 * have (that is a results-context follow-up). Orders whose radius is undefined for
 * either element are simply not candidates (a metal with only a single radius →
 * always `1`). Throws only if the pair has NO defined order at all (both single
 * radii missing → an unknown element, rule #11). `refLength` is the reference sum
 * of the chosen order (for the honest "≈ double · 1.34 Å" label).
 */
export function bondOrderEstimate(
  elemA: string,
  elemB: string,
  distance: number,
): { order: BondOrder; refLength: number } {
  const candidates: { order: BondOrder; sum: number }[] = [];
  for (const order of [1, 2, 3] as const) {
    let sum: number;
    try {
      sum = bondingDistance(elemA, elemB, order);
    } catch {
      continue; // this order is not defined for one of the elements — skip it
    }
    candidates.push({ order, sum });
  }
  if (candidates.length === 0) {
    // Not even a single-bond radius → the element is genuinely unknown. Re-invoke
    // so the loud, element-naming error from `covalentRadius` surfaces (rule #11).
    covalentRadius(elemA);
    covalentRadius(elemB);
    throw new Error("bondOrderEstimate: no covalent radius for the pair");
  }
  let best = candidates[0];
  for (const c of candidates) {
    if (Math.abs(distance - c.sum) < Math.abs(distance - best.sum)) best = c;
  }
  return { order: best.order, refLength: best.sum };
}

/** A "broken" distance (Å) — the covalent sum × 2, comfortably past the perception
 * window ((rA+rB) × 1.2), so the bond drops for ANY pair regardless of elements.
 * A screening separation: the later Opt relaxes the fragments into the product
 * basin (the Menshutkin C–I relaxes to ≈ 4.12 Å from a 4.30 Å seed). */
export function breakDistance(elemA: string, elemB: string): number {
  return bondingDistance(elemA, elemB) * 2;
}

/** A form/break request: the `planEdit` result (the mask + reference-atom rule +
 * needs-split shape, unchanged) plus the COMPUTED target distance the edit path
 * should set the pair to. The apply path uses `target` exactly where a typed
 * set-distance value would go — same preview → `replaceFragmentAtoms` → one Undo. */
export interface BondEditPlan {
  plan: EditPlan;
  /** Target separation (Å): `bondingDistance` for form, `breakDistance` for break. */
  target: number;
  /** The bond order the target expresses (form: 1/2/3; break: always 1 — order is
   * meaningless when clearing a bond). For the honest form label, not a stored fact. */
  order: BondOrder;
}

/** Resolve the two picked atoms' element symbols, or throw if either id is stale.
 * The Form/Break buttons are enabled only with exactly two resolvable picks, so a
 * miss here is an invariant violation (a bug), not a user state — fail loud. */
function elementsOf(scene: Scene, a: AtomId, b: AtomId): [string, string] {
  const da = describeAtomById(scene, a);
  const db = describeAtomById(scene, b);
  if (!da || !db) {
    throw new Error("form/break bond: one of the picked atoms is no longer in the scene");
  }
  return [da.element, db.element];
}

/**
 * Plan a "form bond" at bond `order` (default single): set the picked pair to the
 * order-appropriate bonding distance (rA + rB) so perception draws the bond and the
 * method infers the order from the shorter geometry. DELEGATES the mask/orientation
 * entirely to `planEdit(scene, [a, b])` — a `needs-split` (intra-fragment) result is
 * returned unchanged, so the existing `/geometry/rotatable-mask` path still resolves
 * it. Only `target` changes with `order`; the mask does not.
 */
export function planFormBond(
  scene: Scene,
  a: AtomId,
  b: AtomId,
  order: BondOrder = 1,
): BondEditPlan {
  const [elemA, elemB] = elementsOf(scene, a, b);
  return { plan: planEdit(scene, [a, b]), target: bondingDistance(elemA, elemB, order), order };
}

/**
 * Plan a "break bond": set the picked pair clearly past the perception window so
 * perception drops the bond. Same delegation as {@link planFormBond} — no new mask
 * logic, `needs-split` preserved.
 */
export function planBreakBond(scene: Scene, a: AtomId, b: AtomId): BondEditPlan {
  const [elemA, elemB] = elementsOf(scene, a, b);
  return { plan: planEdit(scene, [a, b]), target: breakDistance(elemA, elemB), order: 1 };
}

/** Exposed for the test that proves ×2 clears the perception window; not used at
 * runtime (the runtime only needs the two distances above). */
export const _PERCEPTION_MULTIPLIER = PERCEPTION_MULTIPLIER;
