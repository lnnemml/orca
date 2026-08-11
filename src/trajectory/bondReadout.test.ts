import { describe, it, expect } from "vitest";

import { resultsBondLabel, mayerFor } from "./bondReadout";
import type { MayerBond } from "../types";

// A C–C pair 1.34 Å apart (a geometric double), plus a Mayer table that says the
// computed order is 1.85. The honest split: the label reports the Mayer number
// (authoritative), the geometric estimate is only the fallback.
const CC: [number, number, number][] = [
  [0, 0, 0],
  [1.34, 0, 0],
];

describe("resultsBondLabel — Mayer authoritative, geometric fallback", () => {
  it("shows the Mayer value (authoritative) when an entry exists for the pair", () => {
    const mayer: MayerBond[] = [{ i: 0, j: 1, order: 1.85 }];
    expect(resultsBondLabel(["C", "C"], CC, mayer, 0, 1)).toBe("Mayer 1.85 (authoritative)");
  });

  it("is order-independent in the pair lookup (j,i finds i,j)", () => {
    const mayer: MayerBond[] = [{ i: 0, j: 1, order: 0.68 }];
    expect(resultsBondLabel(["C", "I"], CC, mayer, 1, 0)).toBe("Mayer 0.68 (authoritative)");
  });

  it("NEGATIVE control — no Mayer entry → the GEOMETRIC estimate, clearly labelled", () => {
    // The bite: conflating the two would either fabricate a Mayer number or drop the
    // honest "(geometric estimate)" tag. Absent Mayer → the geometric line, tagged.
    const label = resultsBondLabel(["C", "C"], CC, null, 0, 1);
    expect(label).toBe("≈ double · 1.340 Å (geometric estimate)");
  });

  it("a partial TS bond with a Mayer entry reads the computed order, not 'not a bond'", () => {
    // The forming/breaking bond is long (2.3 Å → beyond geometric bonding range), but
    // the run COMPUTED an order for it — the authoritative value must still show.
    const far: [number, number, number][] = [
      [0, 0, 0],
      [2.3, 0, 0],
    ];
    const mayer: MayerBond[] = [{ i: 0, j: 1, order: 0.19 }];
    expect(resultsBondLabel(["C", "N"], far, mayer, 0, 1)).toBe("Mayer 0.19 (authoritative)");
    // Without the Mayer entry, a 2.3 Å pair is NOT a bond → no fabricated estimate.
    expect(resultsBondLabel(["C", "N"], far, null, 0, 1)).toBeNull();
  });

  it("returns null for the same atom", () => {
    expect(resultsBondLabel(["C", "C"], CC, null, 0, 0)).toBeNull();
  });
});

describe("mayerFor", () => {
  it("finds an unordered pair, returns undefined when absent", () => {
    const mayer: MayerBond[] = [{ i: 8, j: 9, order: 0.57 }];
    expect(mayerFor(mayer, 9, 8)?.order).toBe(0.57);
    expect(mayerFor(mayer, 0, 1)).toBeUndefined();
    expect(mayerFor(null, 0, 1)).toBeUndefined();
  });
});
