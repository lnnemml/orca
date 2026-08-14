import { describe, it, expect } from "vitest";

import { nearestIndex } from "./nearestIndex";

describe("nearestIndex — snap a click to the closest grid node", () => {
  it("an exact value → its own index", () => {
    expect(nearestIndex([1, 2, 3], 2)).toBe(1);
    expect(nearestIndex([1, 2, 3], 1)).toBe(0);
    expect(nearestIndex([1, 2, 3], 3)).toBe(2);
  });

  it("between two nodes → the CLOSER index (a naive floor/round-to-lower would go red)", () => {
    // 2.6 is closer to 3 (index 2) than to 2 (index 1); floor(2.6)=2 → value 2 → wrong index 1.
    expect(nearestIndex([1, 2, 3], 2.6)).toBe(2);
    // 2.4 is closer to 2 (index 1).
    expect(nearestIndex([1, 2, 3], 2.4)).toBe(1);
  });

  it("clamps: below the min → the min's index, above the max → the max's index (no out-of-range)", () => {
    expect(nearestIndex([1, 2, 3], 0)).toBe(0);
    expect(nearestIndex([1, 2, 3], -100)).toBe(0);
    expect(nearestIndex([1, 2, 3], 5)).toBe(2);
    expect(nearestIndex([1, 2, 3], 999)).toBe(2);
  });

  it("selects by VALUE, so a DESCENDING axis (the real coord1 3.446→1.50) snaps correctly", () => {
    const axis1 = [3.446, 3.0, 2.5, 2.0, 1.5];
    // A click near 1.5 → the LAST index (value-closest), not index 0 — no swap/index assumption.
    expect(nearestIndex(axis1, 1.52)).toBe(4);
    expect(nearestIndex(axis1, 3.4)).toBe(0);
    expect(nearestIndex(axis1, 2.4)).toBe(2);
  });

  it("ties resolve to the first index", () => {
    // 1.5 is equidistant from 1 (index 0) and 2 (index 1) → first.
    expect(nearestIndex([1, 2], 1.5)).toBe(0);
  });

  it("single-element → 0; empty → -1", () => {
    expect(nearestIndex([7], 100)).toBe(0);
    expect(nearestIndex([], 1)).toBe(-1);
  });
});
