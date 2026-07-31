import { describe, it, expect } from "vitest";

import {
  modeDisplacements,
  maxAtomicNorm,
  modeFrameCoords,
  modeFrameXyz,
  minInteratomicDistance,
  modeMinDistanceOverPeriod,
  atomicMasses,
  reducedMassAmu,
  zeroPointAmplitudeAngstrom,
  DEFAULT_AMPLITUDE_ANGSTROM,
  MIN_SAFE_DISTANCE_ANGSTROM,
} from "./mode";
import dex from "./__fixtures__/dexketoprofen-modes.json";

type Vec3 = [number, number, number];
const eq = dex.equilibrium_angstrom as Vec3[];
const nearestMode = (cm: number) =>
  dex.frequencies_cm.reduce(
    (best, f, i) => (Math.abs(f - cm) < Math.abs(dex.frequencies_cm[best] - cm) ? i : best),
    0,
  );
const K_CO = nearestMode(1752.7); // #84 — C=O acid stretch, LOCALIZED
const K_LOW = nearestMode(21.4); //  low — DELOCALIZED torsion over 33 atoms

/** Max atomic displacement (Å) of a frame vs equilibrium — the quantity `A` sets. */
function maxAtomicMove(coords: Vec3[]): number {
  let m = 0;
  coords.forEach((p, a) => {
    m = Math.max(m, Math.hypot(p[0] - eq[a][0], p[1] - eq[a][1], p[2] - eq[a][2]));
  });
  return m;
}

describe("modeDisplacements — extract the COLUMN, not the row (the seam)", () => {
  const n = 6;
  const M = Array.from({ length: n * n }, (_, i) => 10 * Math.floor(i / n) + (i % n));
  it("returns mode k as the column, per atom [x,y,z]", () => {
    expect(modeDisplacements(M, n, 1)).toEqual([
      [1, 11, 21],
      [31, 41, 51],
    ]);
  });
  it("throws on a bad matrix / non-3N dim / out-of-range index", () => {
    expect(() => modeDisplacements([1, 2, 3], 6, 0)).toThrow();
    expect(() => modeDisplacements(new Array(25).fill(0), 5, 0)).toThrow();
    expect(() => modeDisplacements(M, n, 6)).toThrow();
  });
});

describe("normalization is by ATOMIC norm, not by component (the √3 trap)", () => {
  it("maxAtomicNorm is the tri-vector norm, not the largest component", () => {
    // atom 0 moves on the body diagonal (norm √3, largest component 1); the naive
    // max-component would report 1.2 (atom 1) — wrong for choosing the busiest atom.
    expect(maxAtomicNorm([[1, 1, 1], [1.2, 0, 0]])).toBeCloseTo(Math.sqrt(3), 12);
  });

  it("the busiest atom moves EXACTLY A even when it moves diagonally", () => {
    // Only atom 0 moves, along [1,1,1] (norm √3). If we (wrongly) divided by the max
    // component (1), it would move A·√3 ≈ 0.866 at A=0.5 — off by √3. Correct: A.
    const A = 0.5;
    const coords = modeFrameCoords([[0, 0, 0], [9, 0, 0]], [[1, 1, 1], [0, 0, 0]], A, 0.25);
    const move0 = Math.hypot(coords[0][0], coords[0][1], coords[0][2]);
    expect(move0).toBeCloseTo(A, 12); // exactly A, not A·√3
  });
});

describe("modeFrameCoords — phase 0 = equilibrium; max atomic move == A", () => {
  it("phase 0 is EXACTLY the equilibrium (sin 0 = 0)", () => {
    const disp = modeDisplacements(dex.normal_modes, dex.n_modes, K_CO);
    const c = modeFrameCoords(eq, disp, DEFAULT_AMPLITUDE_ANGSTROM, 0);
    expect(maxAtomicMove(c)).toBe(0);
  });

  it("phase 0.5 is also equilibrium (sin π = 0)", () => {
    const disp = modeDisplacements(dex.normal_modes, dex.n_modes, K_CO);
    const c = modeFrameCoords(eq, disp, DEFAULT_AMPLITUDE_ANGSTROM, 0.5);
    expect(maxAtomicMove(c)).toBeLessThan(1e-12);
  });

  // THE proof the normalization is right: the busiest atom moves exactly A, whether
  // the mode is localized (#84 — norm on 2 atoms) or delocalized (low — norm spread
  // over ~100 components). Under the OLD `A·v` normalization these differed wildly.
  it.each([
    ["localized C=O #" + K_CO, K_CO],
    ["delocalized low #" + K_LOW, K_LOW],
  ])("max atomic displacement == A for a %s mode (both, to 1e-9)", (_label, k) => {
    const disp = modeDisplacements(dex.normal_modes, dex.n_modes, k);
    const A = DEFAULT_AMPLITUDE_ANGSTROM;
    const c = modeFrameCoords(eq, disp, A, 0.25); // sin = +1, the extreme
    expect(maxAtomicMove(c)).toBeCloseTo(A, 9);
  });

  it("throws on an atom-count mismatch (never a silent draw)", () => {
    expect(() => modeFrameCoords(eq, [[0, 0, 0]], 0.2, 0.25)).toThrow();
  });
});

describe("real dexketoprofen — bonds intact at the default amplitude (unit 3.13)", () => {
  it("mode #%i (C=O) keeps every interatomic distance ≥ 0.9 Å (was 0.63 at old A)", () => {
    const disp = modeDisplacements(dex.normal_modes, dex.n_modes, K_CO);
    const min = modeMinDistanceOverPeriod(eq, disp, DEFAULT_AMPLITUDE_ANGSTROM);
    expect(min).toBeGreaterThanOrEqual(0.9);
  });

  // The topology test: with a distance cutoff that separates bonds (<1.75 Å) from
  // non-bonds at equilibrium, the bonded SET is identical at phase 0, 0.25 and 0.75 —
  // the amplitude is small enough that no pair crosses. (This is a plain geometric
  // check, NOT a second bond-perception; production freezes 3Dmol's own perception in
  // the viewer. Measured separation at A=0.18: 0.10 Å for #84, 0.24 Å for the low mode.)
  const bondedSet = (coords: Vec3[], cutoff: number) => {
    const s = new Set<string>();
    for (let i = 0; i < coords.length; i++)
      for (let j = i + 1; j < coords.length; j++)
        if (minInteratomicDistance([coords[i], coords[j]]) < cutoff) s.add(`${i}-${j}`);
    return s;
  };
  it.each([["C=O #" + K_CO, K_CO], ["low #" + K_LOW, K_LOW]])(
    "topology at phase 0.25 / 0.75 equals equilibrium topology (%s)",
    (_label, k) => {
      const disp = modeDisplacements(dex.normal_modes, dex.n_modes, k);
      const A = DEFAULT_AMPLITUDE_ANGSTROM;
      const CUT = 1.75;
      const base = bondedSet(eq, CUT);
      for (const phase of [0.25, 0.75]) {
        expect(bondedSet(modeFrameCoords(eq, disp, A, phase), CUT)).toEqual(base);
      }
    },
  );
});

describe("physical (zero-point) amplitude — from VERIFIED masses (unit 3.13)", () => {
  it("the element→mass table matches the artifact's own mass column", () => {
    // ATOMIC_MASS_AMU (standard weights) must equal the .hess $atoms mass column,
    // element for element — the reason we can derive mass from the symbol.
    expect(atomicMasses(dex.elements)).toEqual(dex.masses_amu);
  });

  it("reduced mass and A0 for the C=O #%i mode (measured μ≈3.12 amu, A0≈0.055 Å)", () => {
    const disp = modeDisplacements(dex.normal_modes, dex.n_modes, K_CO);
    const masses = atomicMasses(dex.elements)!;
    expect(reducedMassAmu(disp, masses)).toBeCloseTo(3.12, 1);
    const a0 = zeroPointAmplitudeAngstrom(disp, masses, dex.frequencies_cm[K_CO]);
    expect(a0).not.toBeNull();
    expect(a0!).toBeCloseTo(0.055, 2); // real vibration ~0.055 Å; we draw ~0.18 for visibility
  });

  it("is null for an unknown element or a non-positive frequency", () => {
    expect(atomicMasses(["C", "Xx"])).toBeNull();
    const disp = modeDisplacements(dex.normal_modes, dex.n_modes, K_CO);
    const masses = atomicMasses(dex.elements)!;
    expect(zeroPointAmplitudeAngstrom(disp, masses, -33.6)).toBeNull(); // imaginary
  });
});

describe("modeFrameXyz + collapse guard", () => {
  it("builds a well-formed xyz, equilibrium at phase 0", () => {
    const xyz = modeFrameXyz(["C", "H"], [[0, 0, 0], [1.09, 0, 0]], [[0, 0, 0], [1, 0, 0]], 0.2, 0);
    const lines = xyz.split("\n");
    expect(lines[0]).toBe("2");
    expect(lines[3]).toBe("H 1.090000 0.000000 0.000000");
  });

  it("flags a mode+amplitude that overlaps atoms, passes a gentle one", () => {
    const eqPair: Vec3[] = [[0, 0, 0], [1, 0, 0]];
    // both atoms driven toward each other; A=0.5 max-move → they overlap
    const collapse = modeMinDistanceOverPeriod(eqPair, [[0.3, 0, 0], [-0.3, 0, 0]], 0.5);
    expect(collapse).toBeLessThan(MIN_SAFE_DISTANCE_ANGSTROM);
    const safe = modeMinDistanceOverPeriod(eqPair, [[0, 0.1, 0], [0, -0.1, 0]], 0.1);
    expect(safe).toBeGreaterThan(MIN_SAFE_DISTANCE_ANGSTROM);
  });
});
