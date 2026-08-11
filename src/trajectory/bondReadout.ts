//! Results bond-order readout — the honest split between the COMPUTED (Mayer)
//! order and the GEOMETRIC estimate. Pure, node-tested, React-free.
//!
//! The 3D lines a results viewer draws are a **geometric estimate** (nearest of the
//! single/double/triple covalent sums to the bond length — `bondOrderEstimate`, the
//! SAME basis the editor uses). This readout adds the **authoritative** number when
//! the run computed one: if `mayer_bond_orders` has an entry for the picked pair, we
//! show the Mayer value ("authoritative"); otherwise we fall back to the geometric
//! estimate ("geometric estimate"). The two are never conflated — a Mayer number is
//! labelled computed, the lines/estimate are labelled geometric.

import { bondOrderEstimate, bondingDistance } from "../scene/bond-edit";
import type { MayerBond } from "../types";

const ORDER_WORD: Record<number, string> = { 1: "single", 2: "double", 3: "triple" };

/** The Mayer entry for an unordered atom pair, or `undefined`. */
export function mayerFor(
  mayer: MayerBond[] | null | undefined,
  i: number,
  j: number,
): MayerBond | undefined {
  return mayer?.find((b) => (b.i === i && b.j === j) || (b.i === j && b.j === i));
}

/**
 * The bond-order readout line for the atom pair `(i, j)`, or `null` when there is
 * nothing honest to say (same atom, missing data, or a non-bonded through-space
 * contact with no Mayer entry). `xyz` is the CURRENTLY displayed frame's coordinates
 * (Å), in the same 0-based order as `elements` and the Mayer indices.
 *
 * - A **Mayer** entry for the pair → `"Mayer <order> (authoritative)"` — the computed
 *   order of the final structure (shown even for a partial/through-space TS bond,
 *   because a computed order for it is meaningful).
 * - Else, if the pair is within bonding range → `"≈ <word> · <d> Å (geometric
 *   estimate)"` from the displayed frame's geometry.
 * - Else `null` (not a bond, and no computed order to report).
 */
export function resultsBondLabel(
  elements: string[],
  xyz: [number, number, number][],
  mayer: MayerBond[] | null | undefined,
  i: number,
  j: number,
): string | null {
  if (i === j) return null;
  const ea = elements[i];
  const eb = elements[j];
  if (!ea || !eb) return null;

  // Authoritative first: a computed Mayer order wins over the geometric estimate.
  const m = mayerFor(mayer, i, j);
  if (m) return `Mayer ${m.order.toFixed(2)} (authoritative)`;

  // No computed order — the geometric estimate from THIS frame, only within bonding
  // range (a far through-space pair is not a bond, so we claim no order).
  const a = xyz[i];
  const b = xyz[j];
  if (!a || !b) return null;
  const d = Math.hypot(a[0] - b[0], a[1] - b[1], a[2] - b[2]);
  try {
    if (d > bondingDistance(ea, eb, 1) * 1.3) return null;
    const { order } = bondOrderEstimate(ea, eb, d);
    return `≈ ${ORDER_WORD[order]} · ${d.toFixed(3)} Å (geometric estimate)`;
  } catch {
    return null; // an element with no radius → no estimate, never a crash
  }
}
