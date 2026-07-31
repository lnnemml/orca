import { describe, it, expect } from "vitest";

import {
  parseXyzCoords,
  applyCoordsToAtoms,
  drawableBondCount,
} from "./frozenTopology";

/** A minimal fake of a 3Dmol atom for the draw-gate/coordinate tests. */
interface FakeAtom {
  x: number;
  y: number;
  z: number;
  index?: number;
  bonds: number[];
  bondOrder: number[];
}

/** Two atoms bonded to a central one (C bonded to H, O) — indices as a normal 3Dmol
 * parse assigns them. Symmetric bonds. */
function bondedTriple(withIndex: boolean): FakeAtom[] {
  const idx = (i: number) => (withIndex ? { index: i } : {});
  return [
    { x: 0, y: 0, z: 0, bonds: [1, 2], bondOrder: [1, 2], ...idx(0) }, // C
    { x: 1, y: 0, z: 0, bonds: [0], bondOrder: [1], ...idx(1) }, // H
    { x: 0, y: 1.2, z: 0, bonds: [0], bondOrder: [2], ...idx(2) }, // O
  ];
}

describe("parseXyzCoords — coordinates only, header lines skipped", () => {
  it("reads the N atom rows after the count + comment", () => {
    const xyz = "2\nframe\nC 0.000000 0.000000 0.000000\nH 1.090000 0.000000 0.000000";
    expect(parseXyzCoords(xyz)).toEqual([
      [0, 0, 0],
      [1.09, 0, 0],
    ]);
  });
  it("returns nothing on a malformed count line", () => {
    expect(parseXyzCoords("")).toEqual([]);
    expect(parseXyzCoords("notanumber\nx\nC 0 0 0")).toEqual([]);
  });
});

describe("applyCoordsToAtoms — moves atoms, PRESERVES topology (the crux)", () => {
  it("updates x/y/z in place and leaves bonds/bondOrder/index untouched", () => {
    const atoms = bondedTriple(true);
    const bondsBefore = atoms.map((a) => a.bonds); // same array references
    applyCoordsToAtoms(atoms, [
      [5, 5, 5],
      [6, 5, 5],
      [5, 6.2, 5],
    ]);
    expect(atoms[0]).toMatchObject({ x: 5, y: 5, z: 5 });
    expect(atoms[1]).toMatchObject({ x: 6, y: 5, z: 5 });
    // topology carried by bonds/index is exactly as before — the frozen invariant
    atoms.forEach((a, i) => {
      expect(a.bonds).toBe(bondsBefore[i]); // same reference, not rebuilt
      expect(a.index).toBe(i);
    });
    expect(atoms[0].bondOrder).toEqual([1, 2]);
  });

  it("is bounded by the shorter of atoms/coords (no throw on a mismatch)", () => {
    const atoms = bondedTriple(true);
    expect(() => applyCoordsToAtoms(atoms, [[9, 9, 9]])).not.toThrow();
    expect(atoms[0].x).toBe(9);
    expect(atoms[1].x).toBe(1); // untouched
  });
});

describe("drawableBondCount — mirrors 3Dmol's stick gate (the OUTPUT, not the input)", () => {
  it("counts each bond once when indices are assigned (a normal parse) → > 0", () => {
    // 3Dmol draws a bond from the lower index only: gate `atom.index < atom2.index`.
    // Two unique bonds here (C–H, C=O) → 2 drawable.
    expect(drawableBondCount(bondedTriple(true))).toBe(2);
  });

  it("is ZERO when atom.index is unset — the unit-3.13 regression, reproduced", () => {
    // assignBonds:false left index undefined; `undefined < undefined` is false for
    // every bond → nothing drawn, even though the bond arrays are fully populated.
    const atoms = bondedTriple(false);
    expect(atoms[0].bonds).toEqual([1, 2]); // the stored list looks fine…
    expect(drawableBondCount(atoms)).toBe(0); // …but 3Dmol draws no sticks
  });

  it("stays > 0 and constant across coordinate updates (topology frozen)", () => {
    const atoms = bondedTriple(true);
    const before = drawableBondCount(atoms);
    for (const coords of [
      [[0.1, 0, 0], [1.1, 0, 0], [0.1, 1.3, 0]] as [number, number, number][],
      [[-0.1, 0, 0], [0.9, 0, 0], [-0.1, 1.1, 0]] as [number, number, number][],
    ]) {
      applyCoordsToAtoms(atoms, coords);
      expect(drawableBondCount(atoms)).toBe(before); // never flickers to 0
    }
    expect(before).toBeGreaterThan(0);
  });
});
