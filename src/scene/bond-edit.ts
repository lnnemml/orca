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
import { covalentRadius } from "./covalent-radii";
import { planEdit, type EditPlan } from "./edit-plan";

/** The bond-perception multiplier the sidecar uses ((rA+rB) × this = the
 * distance below which a bond is perceived; `wiki/modules/sidecar.md`, 2.5.3a).
 * Named here only to justify `breakDistance`'s ×2 clearing it — not re-derived. */
const PERCEPTION_MULTIPLIER = 1.2;

/** The bonding distance (Å) for a pair — the covalent-radius SUM (rA + rB), the
 * same basis perception uses, so a pair set here reads as a single bond. Element-
 * dependent: C–H ≈ 1.07, C–C ≈ 1.52, C–I ≈ 2.15 (not a fixed default). */
export function bondingDistance(elemA: string, elemB: string): number {
  return covalentRadius(elemA) + covalentRadius(elemB);
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
 * Plan a "form bond": set the picked pair to their bonding distance (rA + rB) so
 * perception draws the bond. DELEGATES the mask/orientation entirely to
 * `planEdit(scene, [a, b])` — a `needs-split` (intra-fragment) result is returned
 * unchanged, so the existing `/geometry/rotatable-mask` path still resolves it.
 */
export function planFormBond(scene: Scene, a: AtomId, b: AtomId): BondEditPlan {
  const [elemA, elemB] = elementsOf(scene, a, b);
  return { plan: planEdit(scene, [a, b]), target: bondingDistance(elemA, elemB) };
}

/**
 * Plan a "break bond": set the picked pair clearly past the perception window so
 * perception drops the bond. Same delegation as {@link planFormBond} — no new mask
 * logic, `needs-split` preserved.
 */
export function planBreakBond(scene: Scene, a: AtomId, b: AtomId): BondEditPlan {
  const [elemA, elemB] = elementsOf(scene, a, b);
  return { plan: planEdit(scene, [a, b]), target: breakDistance(elemA, elemB) };
}

/** Exposed for the test that proves ×2 clears the perception window; not used at
 * runtime (the runtime only needs the two distances above). */
export const _PERCEPTION_MULTIPLIER = PERCEPTION_MULTIPLIER;
