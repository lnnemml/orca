import { describe, it, expect } from "vitest";

import {
  ORBITAL_PHASE_PAIRS,
  MAX_ORBITALS,
  assignPairs,
  toggleOrbital,
  defaultSurfaceStyle,
} from "./orbitalPalette";

/** Pair 0 must be the viewer's single-orbital default (blue / red). The viewer's
 * `ORBITAL_POS_COLOR`/`ORBITAL_NEG_COLOR` are these exact hexes — kept in sync so a lone
 * frontier orbital is drawn byte-identically to before F2. */
const DEFAULT_POS = "#3b6fd4";
const DEFAULT_NEG = "#d43b3b";

describe("assign_pairs_by_selection_order", () => {
  it("maps [HOMO, LUMO] to pairs 0 then 1, in selection order", () => {
    const homo = 14;
    const assigned = assignPairs([homo, homo + 1]);
    expect(assigned).toHaveLength(2);
    expect(assigned[0].mo).toBe(homo);
    expect(assigned[1].mo).toBe(homo + 1);
    // First selected → pair 0; second → pair 1.
    expect(assigned[0]).toMatchObject(ORBITAL_PHASE_PAIRS[0]);
    expect(assigned[1]).toMatchObject(ORBITAL_PHASE_PAIRS[1]);
  });

  it("pair 0 is the blue/red single-orbital default (single orbital unchanged)", () => {
    expect(ORBITAL_PHASE_PAIRS[0]).toEqual({ posColor: DEFAULT_POS, negColor: DEFAULT_NEG });
    const [only] = assignPairs([7]);
    expect(only).toEqual({ mo: 7, posColor: DEFAULT_POS, negColor: DEFAULT_NEG });
  });

  it("assigns by POSITION, not by MO number", () => {
    // A higher MO selected first still takes pair 0.
    const assigned = assignPairs([20, 3]);
    expect(assigned[0]).toMatchObject({ mo: 20, ...ORBITAL_PHASE_PAIRS[0] });
    expect(assigned[1]).toMatchObject({ mo: 3, ...ORBITAL_PHASE_PAIRS[1] });
  });
});

describe("cap_blocks_beyond_max", () => {
  it("appends an absent MO under the cap, preserving order", () => {
    expect(toggleOrbital([5, 6], 7)).toEqual([5, 6, 7]);
  });

  it("a toggle-add past MAX_ORBITALS is a no-op", () => {
    const full = [0, 1, 2, 3];
    expect(full).toHaveLength(MAX_ORBITALS);
    const after = toggleOrbital(full, 4);
    expect(after).toEqual(full); // the 5th is refused
    expect(after).not.toContain(4);
  });

  it("toggling an already-selected MO removes it (even at the cap)", () => {
    expect(toggleOrbital([0, 1, 2, 3], 1)).toEqual([0, 2, 3]);
    expect(toggleOrbital([5, 6], 5)).toEqual([6]);
  });

  it("does not mutate the input array", () => {
    const input = [5, 6];
    toggleOrbital(input, 7);
    expect(input).toEqual([5, 6]);
  });
});

describe("default_surface_style_by_count", () => {
  it("defaults to solid for a single orbital (unchanged look)", () => {
    expect(defaultSurfaceStyle(1)).toBe("solid");
  });
  it("defaults to mesh for two or more (readable overlap — the F2b fix)", () => {
    expect(defaultSurfaceStyle(2)).toBe("mesh");
    expect(defaultSurfaceStyle(4)).toBe("mesh");
  });
  it("is solid for the degenerate empty/zero case", () => {
    expect(defaultSurfaceStyle(0)).toBe("solid");
  });
});

describe("pairs_reassign_on_removal", () => {
  it("removing the first of three re-colours the remaining two to pairs 0,1", () => {
    const start = [10, 11, 12];
    const before = assignPairs(start);
    expect(before.map((a) => a.mo)).toEqual([10, 11, 12]);

    const afterSel = toggleOrbital(start, 10); // remove the first
    expect(afterSel).toEqual([11, 12]);

    const after = assignPairs(afterSel);
    // The remaining two shift up to pairs 0 and 1 — the legend stays consistent.
    expect(after[0]).toMatchObject({ mo: 11, ...ORBITAL_PHASE_PAIRS[0] });
    expect(after[1]).toMatchObject({ mo: 12, ...ORBITAL_PHASE_PAIRS[1] });
  });
});
