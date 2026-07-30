import { describe, it, expect } from "vitest";

import {
  frameToXyz,
  frameLabel,
  frameEnergyText,
  frameDeltaKcal,
  energySeries,
  elementsAgree,
  type Frame,
} from "./frame";

const F = (e: number | null, xyz: [number, number, number][]): Frame => ({
  energy_eh: e,
  xyz_angstrom: xyz,
});

describe("frameToXyz", () => {
  it("builds a standard xyz for one frame", () => {
    const xyz = frameToXyz(["C", "H"], F(-1, [[0, 0, 0], [0, 0, 1.09]]));
    expect(xyz).toBe("2\nframe\nC 0.000000 0.000000 0.000000\nH 0.000000 0.000000 1.090000");
  });

  it("throws on an element/coordinate count mismatch (no silent render)", () => {
    expect(() => frameToXyz(["C", "H"], F(null, [[0, 0, 0]]))).toThrow(/atoms/);
  });
});

describe("frameLabel — honest: optimization CYCLES, never 'scan step'", () => {
  it("is 1-based and says 'cycle'", () => {
    expect(frameLabel(0, 26)).toBe("cycle 1 / 26");
    expect(frameLabel(25, 26)).toBe("cycle 26 / 26");
    expect(frameLabel(0, 26)).not.toMatch(/scan/i);
    expect(frameLabel(0, 26)).not.toMatch(/step/i);
  });
});

describe("frame energy display", () => {
  it("formats an energy or a dash when absent", () => {
    expect(frameEnergyText(F(-79.7918, []))).toBe("-79.791800 Eh");
    expect(frameEnergyText(F(null, []))).toBe("—");
  });

  it("ΔE vs the first frame in kcal/mol, null if either energy is missing", () => {
    const frames = [F(-79.8, []), F(-79.81, [])];
    expect(frameDeltaKcal(frames, 1)).toBeCloseTo(-0.01 * 627.5094740631, 6);
    expect(frameDeltaKcal(frames, 0)).toBe(0);
    expect(frameDeltaKcal([F(null, []), F(-1, [])], 1)).toBeNull();
  });
});

describe("energySeries — chart data, only frames with an energy", () => {
  it("keeps 1-based cycle + original index, skips energyless frames", () => {
    const s = energySeries([F(-1, []), F(null, []), F(-1.5, [])]);
    expect(s).toEqual([
      { cycle: 1, energy: -1, index: 0 },
      { cycle: 3, energy: -1.5, index: 2 },
    ]);
  });
});

describe("elementsAgree — the UI-boundary identity check", () => {
  it("true only for same count AND same sequence (case-insensitive)", () => {
    expect(elementsAgree(["C", "H", "H"], ["c", "h", "h"])).toBe(true);
    expect(elementsAgree(["C", "H"], ["C", "H", "H"])).toBe(false); // count
    expect(elementsAgree(["C", "H", "O"], ["C", "O", "H"])).toBe(false); // order
  });
});
