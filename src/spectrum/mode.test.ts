import { describe, it, expect } from "vitest";

import {
  modeDisplacements,
  modeFrameCoords,
  modeFrameXyz,
  minInteratomicDistance,
  modeMinDistanceOverPeriod,
  DEFAULT_AMPLITUDE,
  MIN_SAFE_DISTANCE_ANGSTROM,
} from "./mode";

type Vec3 = [number, number, number];

describe("modeDisplacements — extract the COLUMN, not the row (the seam)", () => {
  // A 2-atom (nModes = 6) row-major matrix whose column 1 is deliberately different
  // from row 1 — so a row/column mix-up is caught. Entry (r,c) = 10*r + c.
  const n = 6;
  const M = Array.from({ length: n * n }, (_, i) => 10 * Math.floor(i / n) + (i % n));

  it("returns mode k as the column, per atom [x,y,z]", () => {
    // column 1 is entries (r,1) = 10*r + 1 for r = 0..5 → [1,11,21,31,41,51]
    const disp = modeDisplacements(M, n, 1);
    expect(disp).toEqual([
      [1, 11, 21], // atom 0: rows 0,1,2 of column 1
      [31, 41, 51], // atom 1: rows 3,4,5 of column 1
    ]);
  });

  it("length is N = 3N/3 atoms (the mode vector is 3N components)", () => {
    const disp = modeDisplacements(M, n, 0);
    expect(disp).toHaveLength(2); // N atoms
    expect(3 * disp.length).toBe(n); // 3N == nModes
  });

  it("throws on a matrix that is not n², a non-3N dim, or an out-of-range index", () => {
    expect(() => modeDisplacements([1, 2, 3], 6, 0)).toThrow(); // wrong length
    expect(() => modeDisplacements(new Array(25).fill(0), 5, 0)).toThrow(); // 5 not 3N
    expect(() => modeDisplacements(M, n, 6)).toThrow(); // index == n
    expect(() => modeDisplacements(M, n, -1)).toThrow();
  });
});

describe("modeFrameCoords — x(t) = x_eq + A·sin(2π·phase)·v", () => {
  const eq: Vec3[] = [
    [0, 0, 0],
    [1, 0, 0],
  ];
  const disp: Vec3[] = [
    [0, 0, 0],
    [0.5, 0, 0],
  ];

  it("phase 0 is EXACTLY the equilibrium (post-condition: max Δ < 1e-9)", () => {
    const c = modeFrameCoords(eq, disp, DEFAULT_AMPLITUDE, 0);
    let maxD = 0;
    c.forEach((p, a) => p.forEach((v, k) => (maxD = Math.max(maxD, Math.abs(v - eq[a][k])))));
    expect(maxD).toBeLessThan(1e-9);
    expect(maxD).toBe(0); // sin(0) is exactly 0 → exact equality
  });

  it("phase 0.5 is also the equilibrium (sin π = 0)", () => {
    const c = modeFrameCoords(eq, disp, DEFAULT_AMPLITUDE, 0.5);
    expect(Math.abs(c[1][0] - 1)).toBeLessThan(1e-9);
  });

  it("phase 0.25 / 0.75 are the +A / −A extremes", () => {
    const plus = modeFrameCoords(eq, disp, 2, 0.25); // sin = +1
    const minus = modeFrameCoords(eq, disp, 2, 0.75); // sin = −1
    expect(plus[1][0]).toBeCloseTo(1 + 2 * 0.5, 12); // 1 + A·v = 2.0
    expect(minus[1][0]).toBeCloseTo(1 - 2 * 0.5, 12); // 1 − A·v = 0.0
  });

  it("throws on an atom-count mismatch (never a silent draw)", () => {
    expect(() => modeFrameCoords(eq, [[0, 0, 0]], 2, 0.25)).toThrow();
  });
});

describe("modeFrameXyz — one frame, standard xyz, count-checked", () => {
  it("builds a well-formed xyz and equals equilibrium at phase 0", () => {
    const eq: Vec3[] = [
      [0, 0, 0],
      [1.09, 0, 0],
    ];
    const disp: Vec3[] = [
      [0, 0, 0],
      [1, 0, 0],
    ];
    const xyz = modeFrameXyz(["C", "H"], eq, disp, 2, 0);
    const lines = xyz.split("\n");
    expect(lines[0]).toBe("2");
    expect(lines[2]).toBe("C 0.000000 0.000000 0.000000");
    expect(lines[3]).toBe("H 1.090000 0.000000 0.000000"); // equilibrium at phase 0
  });

  it("throws when the element order length disagrees with the geometry", () => {
    expect(() => modeFrameXyz(["C"], [[0, 0, 0], [1, 0, 0]], [[0, 0, 0], [0, 0, 0]], 2, 0)).toThrow();
  });
});

describe("collapse guard — detect when the amplitude drives atoms together", () => {
  it("minInteratomicDistance finds the closest pair", () => {
    const c: Vec3[] = [
      [0, 0, 0],
      [1, 0, 0],
      [1.2, 0, 0],
    ];
    expect(minInteratomicDistance(c)).toBeCloseTo(0.2, 12);
  });

  it("flags a mode+amplitude that overlaps atoms, passes a gentle one", () => {
    // Two atoms 1.0 Å apart; the mode drives them toward each other along x. At A=2
    // the +A/−A extreme moves each 2·0.3 = 0.6 Å → they can reach 1.0 − 1.2 < 0.
    const eq: Vec3[] = [
      [0, 0, 0],
      [1, 0, 0],
    ];
    const towards: Vec3[] = [
      [0.3, 0, 0],
      [-0.3, 0, 0],
    ];
    const collapse = modeMinDistanceOverPeriod(eq, towards, DEFAULT_AMPLITUDE);
    expect(collapse).toBeLessThan(MIN_SAFE_DISTANCE_ANGSTROM); // guard would warn

    // A small transverse wag never brings them close — stays well above the floor.
    const gentle: Vec3[] = [
      [0, 0.1, 0],
      [0, -0.1, 0],
    ];
    const safe = modeMinDistanceOverPeriod(eq, gentle, 0.5);
    expect(safe).toBeGreaterThan(MIN_SAFE_DISTANCE_ANGSTROM);
  });

  it("is symmetric across the period — samples both the + and − extreme", () => {
    // A mode that only collapses at the −A extreme must still be caught.
    const eq: Vec3[] = [
      [0, 0, 0],
      [1, 0, 0],
    ];
    const asym: Vec3[] = [
      [0, 0, 0],
      [0.6, 0, 0], // atom 1 moves +x at +A (apart), −x at −A (together, through 0)
    ];
    expect(modeMinDistanceOverPeriod(eq, asym, DEFAULT_AMPLITUDE)).toBeLessThan(
      MIN_SAFE_DISTANCE_ANGSTROM,
    );
  });
});
