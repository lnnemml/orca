import { describe, it, expect } from "vitest";
import { resolveClickedIndex } from "./clickIndex";

/** A minimal series with an x field (`coordinate`) and its own `index`, like the scan and
 *  trajectory series feed the resolver. */
const series = [
  { coordinate: 1.4, index: 0 },
  { coordinate: 1.6, index: 1 },
  { coordinate: 1.8, index: 2 },
  { coordinate: 2.0, index: 3 },
  { coordinate: 2.2, index: 4 },
  { coordinate: 2.4, index: 5 },
];
const getX = (d: { coordinate: number }) => d.coordinate;

describe("resolveClickedIndex — recharts v3 chart-click", () => {
  it("C-v3-string-index: activeTooltipIndex as a STRING resolves (the bug)", () => {
    // recharts v3 delivers the index as a string (TooltipIndex = string | null). The old
    // `typeof i === "number"` guard returned nothing here → every click dropped.
    expect(resolveClickedIndex({ activeTooltipIndex: "5" }, series)).toBe(5);
    // negative control: the old number-only logic would have missed this string.
    const oldGuard = (i: unknown) => (typeof i === "number" && i >= 0 ? i : null);
    expect(oldGuard("5")).toBeNull(); // proves the test bites the real bug
  });

  it("C-number-index: activeTooltipIndex as a NUMBER still works (v2 shape)", () => {
    expect(resolveClickedIndex({ activeTooltipIndex: 3 }, series)).toBe(3);
    expect(resolveClickedIndex({ activeTooltipIndex: 0 }, series)).toBe(0);
  });

  it("C-label-fallback: no index → match activeLabel (x value) via getX", () => {
    expect(resolveClickedIndex({ activeTooltipIndex: null, activeLabel: 2.4 }, series, getX)).toBe(5);
    expect(resolveClickedIndex({ activeTooltipIndex: null, activeLabel: 1.4 }, series, getX)).toBe(0);
    // a non-matching label → null (no false selection).
    expect(resolveClickedIndex({ activeTooltipIndex: null, activeLabel: 9.9 }, series, getX)).toBeNull();
    // …and without getX there is no fallback → null.
    expect(resolveClickedIndex({ activeTooltipIndex: null, activeLabel: 2.4 }, series)).toBeNull();
  });

  it("C-garbage: empty/garbage state → null, never throws", () => {
    expect(resolveClickedIndex({}, series)).toBeNull();
    expect(resolveClickedIndex({ activeTooltipIndex: "x" }, series)).toBeNull();
    expect(resolveClickedIndex({ activeTooltipIndex: "x" }, series, getX)).toBeNull();
    expect(resolveClickedIndex(null, series)).toBeNull();
    expect(resolveClickedIndex(undefined, series)).toBeNull();
    // out-of-range index → null (not a wild array access).
    expect(resolveClickedIndex({ activeTooltipIndex: "99" }, series)).toBeNull();
    expect(resolveClickedIndex({ activeTooltipIndex: -1 }, series)).toBeNull();
    // empty series → null.
    expect(resolveClickedIndex({ activeTooltipIndex: 0 }, [])).toBeNull();
  });
});
