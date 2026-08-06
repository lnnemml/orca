import { describe, it, expect } from "vitest";

import { VDW_RADII, VDW_SOURCE, vdwRadius } from "./vdw-radii";

describe("vdw-radii — cited coverage + UNDETERMINED", () => {
  it("every radius has a source and every source has a radius (no uncited number)", () => {
    expect(new Set(Object.keys(VDW_RADII))).toEqual(new Set(Object.keys(VDW_SOURCE)));
  });

  it("covers the mission elements incl. B (Mantina) and Pd/Pt (Alvarez)", () => {
    expect(vdwRadius("B")).toBeCloseTo(1.92, 6); // BH₄⁻ centre — Mantina 2009
    expect(VDW_SOURCE.B).toBe("Mantina2009");
    expect(vdwRadius("Pd")).toBeCloseTo(2.1, 6); // cross-coupling — Alvarez 2013
    expect(vdwRadius("Pt")).toBeCloseTo(2.13, 6);
    expect(VDW_SOURCE.Pd).toBe("Alvarez2013");
    expect(vdwRadius("C")).toBeCloseTo(1.7, 6); // Bondi 1964
    expect(VDW_SOURCE.C).toBe("Bondi1964");
  });

  it("is case-insensitive", () => {
    expect(vdwRadius("cl")).toBe(vdwRadius("Cl"));
  });

  it("returns undefined (UNDETERMINED, not 0) for an uncovered element", () => {
    expect(vdwRadius("W")).toBeUndefined(); // f-block / heavy TM outside our sources
    expect(vdwRadius("Nd")).toBeUndefined();
    expect(vdwRadius("Xx")).toBeUndefined();
  });
});
