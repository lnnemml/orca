//! Formal-charge bookkeeping (geometric-editor completion) — pure, node-tested,
//! React-free.
//!
//! HONEST FRAME (stated in `wiki/modules/editor-ui.md`): ORCA reads geometry +
//! **total charge** + multiplicity + method. It does NOT read per-atom formal
//! charge. So per-atom formal charges are **bookkeeping/annotation** only — a way
//! for the chemist to reason about where charge sits — and their one hard
//! constraint is that they must **sum to the total charge** ORCA is actually given
//! (`totalCharge(scene) = Σ fragment.charge`, `scene.ts`). This module is that sum
//! and that consistency check; it stores nothing and knows nothing about ORCA.

/** Sum a set of per-atom formal charges. Total over an empty set is 0. */
export function formalChargeSum(charges: Iterable<number>): number {
  let sum = 0;
  for (const c of charges) sum += c;
  return sum;
}

/** The result of checking per-atom formal charges against the system total. */
export interface FormalChargeConsistency {
  /** Σ of the per-atom formal charges. */
  sum: number;
  /** The total charge ORCA is given (`totalCharge(scene)`). */
  total: number;
  /** Whether the bookkeeping is consistent — `sum === total`. */
  matches: boolean;
}

/**
 * Check per-atom formal charges against the system `total` (the charge ORCA
 * actually takes). `matches` is `sum === total` — the ONLY correctness constraint
 * on formal-charge bookkeeping. The UI shows the honest ✓ / mismatch from this;
 * a mismatch never blocks the run (ORCA still uses `total`), it just flags that the
 * per-atom annotation doesn't add up.
 */
export function formalChargeConsistency(
  charges: Iterable<number>,
  total: number,
): FormalChargeConsistency {
  const sum = formalChargeSum(charges);
  return { sum, total, matches: sum === total };
}
