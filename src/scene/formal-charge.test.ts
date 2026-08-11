import { describe, it, expect } from "vitest";

import { formalChargeSum, formalChargeConsistency } from "./formal-charge";

describe("formalChargeSum", () => {
  it("sums a set of per-atom formal charges", () => {
    expect(formalChargeSum([1, -1, 0])).toBe(0);
    expect(formalChargeSum([1, 0, 0])).toBe(1);
    expect(formalChargeSum([-1, -1, 1])).toBe(-1);
  });

  it("is 0 over an empty set", () => {
    expect(formalChargeSum([])).toBe(0);
  });
});

describe("formalChargeConsistency — Σ formal must equal total", () => {
  it("matches when Σ formal == total (+1 on one atom, total +1)", () => {
    const c = formalChargeConsistency([1, 0, 0], 1);
    expect(c).toEqual({ sum: 1, total: 1, matches: true });
  });

  it("flags a mismatch when Σ formal ≠ total", () => {
    const c = formalChargeConsistency([1, 0, 0], 0);
    expect(c.sum).toBe(1);
    expect(c.total).toBe(0);
    expect(c.matches).toBe(false);
  });

  it("NEGATIVE control — a sum that dropped an atom would falsely 'match'", () => {
    // Real per-atom charges [+1, +1, -1] sum to +1 and the total is +1 → consistent.
    const charges = [1, 1, -1];
    expect(formalChargeConsistency(charges, 1).matches).toBe(true);
    // The bite: a buggy sum that ignored the last atom would give +2 ≠ +1 → the
    // check must go RED (not-match) on that wrong sum, proving it reads every atom.
    const droppedLast = charges.slice(0, -1); // [+1, +1] — an atom omitted
    expect(formalChargeConsistency(droppedLast, 1).matches).toBe(false);
  });
});
