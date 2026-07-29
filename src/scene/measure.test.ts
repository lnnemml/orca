import { describe, it, expect } from "vitest";

import type { Scene, SceneAtom, SceneFragment } from "./types";
import {
  distance,
  angle,
  dihedral,
  measureSelection,
} from "./measure";
import {
  FRAGMENT_LIBRARY,
  libraryFragmentToScene,
} from "./fragment-library";
import { parseEnsemble, type Conformer } from "./ensemble";

// A real (3-structure) GOAT ensemble — the same fixture ensemble.test uses.
import ENSEMBLE from "./__fixtures__/butane.finalensemble.xyz?raw";

// ── Helpers ──────────────────────────────────────────────────────────────────

function libScene(key: string): Scene {
  const lf = FRAGMENT_LIBRARY.find((f) => f.key === key);
  if (!lf) throw new Error(`no library fragment ${key}`);
  return { fragments: [libraryFragmentToScene(lf)], multiplicity: 1 };
}

function fragmentOf(atoms: SceneAtom[], id = "f"): SceneFragment {
  return { id, name: id, charge: 0, source: "editor", atoms };
}

function sceneOf(...fragments: SceneFragment[]): Scene {
  return { fragments, multiplicity: 1 };
}

/** The butane conformers of the fixture, as single-fragment scenes. Conformer 0
 * is anti (C-C-C-C ≈ 180°), conformer 1 is gauche (≈ 67.5°). */
function butaneScene(conformerIndex: number): Scene {
  const conformers = parseEnsemble(ENSEMBLE) as Conformer[];
  return sceneOf(fragmentOf(conformers[conformerIndex].atoms.map((a) => ({ ...a }))));
}

/** Rigid rotation (Rz·Ry·Rx, fixed angles) + translation of every atom in a
 * scene — an explicit proper-rotation matrix (det = +1), built by hand so the
 * test is deterministic. */
function rigidMove(scene: Scene): Scene {
  const ax = 0.3,
    ay = -0.7,
    az = 1.1;
  const [cx, sx] = [Math.cos(ax), Math.sin(ax)];
  const [cy, sy] = [Math.cos(ay), Math.sin(ay)];
  const [cz, sz] = [Math.cos(az), Math.sin(az)];
  // R = Rz * Ry * Rx
  const R = [
    [cz * cy, cz * sy * sx - sz * cx, cz * sy * cx + sz * sx],
    [sz * cy, sz * sy * sx + cz * cx, sz * sy * cx - cz * sx],
    [-sy, cy * sx, cy * cx],
  ];
  const t = [1.234, -5.6, 7.89];
  const move = (a: SceneAtom): SceneAtom => ({
    element: a.element,
    x: R[0][0] * a.x + R[0][1] * a.y + R[0][2] * a.z + t[0],
    y: R[1][0] * a.x + R[1][1] * a.y + R[1][2] * a.z + t[1],
    z: R[2][0] * a.x + R[2][1] * a.y + R[2][2] * a.z + t[2],
  });
  return {
    fragments: scene.fragments.map((f) => ({ ...f, atoms: f.atoms.map(move) })),
    multiplicity: scene.multiplicity,
  };
}

/** Reflect every atom through the x=0 plane (an improper rotation, det = −1). */
function mirrorX(scene: Scene): Scene {
  return {
    fragments: scene.fragments.map((f) => ({
      ...f,
      atoms: f.atoms.map((a) => ({ ...a, x: -a.x })),
    })),
    multiplicity: scene.multiplicity,
  };
}

// ── Chemistry: library molecules recomputed from their own coordinates ────────

describe("angle / distance on library molecules", () => {
  it("water: H–O–H ≈ 104.5° and O–H ≈ 0.9572 Å (fragment-library source)", () => {
    // bent("O","H",...) → O is atom 0, the two H are 1 and 2.
    const s = libScene("water");
    const hoh = angle(s, 1, 0, 2)!; // vertex = O (middle pick)
    const oh = distance(s, 0, 1)!;
    expect(hoh).toBeCloseTo(104.52, 2);
    expect(oh).toBeCloseTo(0.9572, 4);
  });

  it("BH4⁻: H–B–H ≈ 109.47° for every hydrogen pair", () => {
    // tetrahedral("B","H",...) → B is atom 0, the four H are 1..4.
    const s = libScene("bh4-");
    for (const [i, j] of [
      [1, 2],
      [1, 3],
      [1, 4],
      [2, 3],
      [2, 4],
      [3, 4],
    ]) {
      expect(angle(s, i, 0, j)!).toBeCloseTo(109.47, 1);
    }
  });
});

// ── Dihedral convention (ASE [0, 360), verified on real butane) ───────────────

describe("dihedral (ASE convention, butane ensemble)", () => {
  it("anti conformer: C-C-C-C ≈ 180°", () => {
    const s = butaneScene(0); // the 4 carbons are atoms 0,1,2,3
    expect(dihedral(s, 0, 1, 2, 3)!).toBeCloseTo(179.998, 2);
  });

  it("gauche conformer: C-C-C-C ≈ 67.5° — the 60 side, NOT 300 (documents [0,360) fold)", () => {
    // ASE get_dihedral(0,1,2,3) on this frame is 67.523°: the [0,360) fold with
    // v0=a1-a0, v1=a2-a1, v2=a3-a2 lands gauche on the 60 side. If 2.5.2c's ASE
    // call ever produced 300 instead, this assertion breaks — the tripwire.
    const s = butaneScene(1);
    const phi = dihedral(s, 0, 1, 2, 3)!;
    expect(phi).toBeCloseTo(67.523, 2);
    expect(phi).toBeGreaterThan(0);
    expect(phi).toBeLessThan(180);
  });

  it("stays in [0, 360)", () => {
    for (const idx of [0, 1, 2]) {
      const phi = dihedral(butaneScene(idx), 0, 1, 2, 3)!;
      expect(phi).toBeGreaterThanOrEqual(0);
      expect(phi).toBeLessThan(360);
    }
  });
});

// ── Symmetries ────────────────────────────────────────────────────────────────

describe("symmetries", () => {
  const s = butaneScene(1); // gauche — a generic, non-degenerate geometry

  it("distance is symmetric", () => {
    expect(distance(s, 0, 3)!).toBeCloseTo(distance(s, 3, 0)!, 12);
  });

  it("angle(a,v,b) === angle(b,v,a)", () => {
    expect(angle(s, 0, 1, 2)!).toBeCloseTo(angle(s, 2, 1, 0)!, 12);
  });

  it("dihedral(i,j,k,l) === dihedral(l,k,j,i) — reversal invariant", () => {
    // Reversing the four points gives the SAME dihedral (ASE-confirmed:
    // get_dihedral(0,1,2,3) == get_dihedral(3,2,1,0) to full precision).
    expect(dihedral(s, 0, 1, 2, 3)!).toBeCloseTo(dihedral(s, 3, 2, 1, 0)!, 10);
  });
});

// ── (f) chain-reversal invariance — the fact 2.5.2d-2's edit planner rests on ──
// planEdit reads a selection in BOTH directions and reverses it so the reagent
// (first-clicked) can be the moving end. That is only sound because the measured
// value is identical either way: angle(i,v,j) == angle(j,v,i) and
// dihedral(i,j,k,l) == dihedral(l,k,j,i). Pinned here on a deterministic generic
// geometry (no symmetry to accidentally satisfy it).
describe("chain-reversal invariance (basis of the both-orientation edit plan)", () => {
  const s = sceneOf(
    fragmentOf([
      { element: "C", x: 0.13, y: 0.21, z: -0.34 },
      { element: "N", x: 1.42, y: 0.02, z: 0.51 },
      { element: "O", x: 2.11, y: 1.33, z: -0.22 },
      { element: "C", x: 3.05, y: 0.71, z: 1.14 },
    ]),
  );

  it("angle(i,v,j) === angle(j,v,i) to 1e-9", () => {
    const fwd = angle(s, 0, 1, 2)!;
    const rev = angle(s, 2, 1, 0)!;
    expect(fwd).toBeCloseTo(rev, 9);
    // (for the report: a real number, not just "equal")
    expect(fwd).toBeGreaterThan(0);
  });

  it("dihedral(i,j,k,l) === dihedral(l,k,j,i) to 1e-9", () => {
    const fwd = dihedral(s, 0, 1, 2, 3)!;
    const rev = dihedral(s, 3, 2, 1, 0)!;
    expect(fwd).toBeCloseTo(rev, 9);
    expect(fwd).toBeGreaterThanOrEqual(0);
    expect(fwd).toBeLessThan(360);
  });

  it("distance is symmetric to 1e-12", () => {
    expect(distance(s, 0, 3)!).toBeCloseTo(distance(s, 3, 0)!, 12);
  });
});

// ── Reflection: distance/angle invariant, dihedral → 360 − φ ──────────────────

describe("mirror reflection", () => {
  const s = butaneScene(1);
  const m = mirrorX(s);

  it("leaves distances and angles unchanged", () => {
    expect(distance(m, 0, 3)!).toBeCloseTo(distance(s, 0, 3)!, 10);
    expect(angle(m, 0, 1, 2)!).toBeCloseTo(angle(s, 0, 1, 2)!, 10);
  });

  it("sends the dihedral φ → 360 − φ (handedness flips)", () => {
    const phi = dihedral(s, 0, 1, 2, 3)!; // 67.523
    const mirrored = dihedral(m, 0, 1, 2, 3)!; // 292.477
    expect(mirrored).toBeCloseTo(360 - phi, 8);
  });
});

// ── Rigid-motion invariance — the load-bearing test ───────────────────────────

describe("rigid-motion invariance (rotation + translation)", () => {
  // The most important test: a proper rotation + translation of the WHOLE scene
  // must leave all three quantities identical to 1e-9. This catches a bug in the
  // math, not in a single number — any frame-dependent error surfaces here.
  const s = butaneScene(1);
  const moved = rigidMove(s);

  it("distance, angle, and dihedral are all invariant to 1e-9", () => {
    expect(distance(moved, 0, 3)!).toBeCloseTo(distance(s, 0, 3)!, 9);
    expect(angle(moved, 0, 1, 2)!).toBeCloseTo(angle(s, 0, 1, 2)!, 9);
    expect(dihedral(moved, 0, 1, 2, 3)!).toBeCloseTo(
      dihedral(s, 0, 1, 2, 3)!,
      9,
    );
  });
});

// ── Degenerate cases → null, never NaN ────────────────────────────────────────

describe("degenerate inputs return null, not NaN", () => {
  const s = libScene("water");

  it("distance of an atom with itself → null", () => {
    expect(distance(s, 0, 0)).toBeNull();
  });

  it("distance between coincident atoms (zero vector) → null", () => {
    const dup = sceneOf(
      fragmentOf([
        { element: "H", x: 1, y: 1, z: 1 },
        { element: "H", x: 1, y: 1, z: 1 },
      ]),
    );
    expect(distance(dup, 0, 1)).toBeNull();
  });

  it("angle with a repeated index → null", () => {
    expect(angle(s, 0, 0, 1)).toBeNull();
    expect(angle(s, 1, 0, 1)).toBeNull();
  });

  it("dihedral of a collinear inner triple → null (cross-product threshold)", () => {
    // Four points on the x-axis: every inner angle is planar → undefined.
    const line = sceneOf(
      fragmentOf([
        { element: "C", x: 0, y: 0, z: 0 },
        { element: "C", x: 1, y: 0, z: 0 },
        { element: "C", x: 2, y: 0, z: 0 },
        { element: "C", x: 3, y: 0, z: 0 },
      ]),
    );
    expect(dihedral(line, 0, 1, 2, 3)).toBeNull();
  });

  it("out-of-range indices → null (no throw)", () => {
    expect(distance(s, 0, 99)).toBeNull();
    expect(angle(s, 0, 1, 99)).toBeNull();
    expect(dihedral(s, 0, 1, 2, 99)).toBeNull();
    expect(distance(s, -1, 0)).toBeNull();
  });
});

// ── measureSelection ──────────────────────────────────────────────────────────

describe("measureSelection", () => {
  it("0 or 1 atoms → none", () => {
    const s = libScene("water");
    expect(measureSelection(s, [])).toEqual({ kind: "none" });
    expect(measureSelection(s, [0])).toEqual({ kind: "none" });
  });

  it("2 atoms → distance, positional", () => {
    const s = libScene("water");
    const m = measureSelection(s, [0, 1]);
    expect(m.kind).toBe("distance");
    if (m.kind === "distance") {
      expect(m.value).toBeCloseTo(0.9572, 4);
      expect(m.unit).toBe("Å");
      expect(m.atoms).toEqual([0, 1]);
      expect(m.sameFragment).toBe(true);
    }
  });

  it("3 atoms → angle with the MIDDLE pick as the vertex", () => {
    const s = libScene("water");
    // Pick order H(1), O(0), H(2): the vertex is O (the second pick), NOT the
    // smallest index. 104.52°, not something else.
    const m = measureSelection(s, [1, 0, 2]);
    expect(m.kind).toBe("angle");
    if (m.kind === "angle") expect(m.value).toBeCloseTo(104.52, 2);
  });

  it("4 atoms → dihedral", () => {
    const m = measureSelection(butaneScene(0), [0, 1, 2, 3]);
    expect(m.kind).toBe("dihedral");
    if (m.kind === "dihedral") expect(m.value).toBeCloseTo(179.998, 2);
  });

  it("flags an inter-fragment distance (a future reaction coordinate)", () => {
    // water (0,1,2) + Cl⁻ (3): a bond across fragments. The library places both
    // O and Cl at the origin, so offset the chloride to a real contact distance.
    const cl: SceneFragment = {
      id: "cl",
      name: "Cl-",
      charge: -1,
      source: "fragment-library",
      atoms: [{ element: "Cl", x: 3, y: 0, z: 0 }],
    };
    const s = sceneOf(libScene("water").fragments[0], cl);
    const m = measureSelection(s, [0, 3]);
    expect(m.kind).toBe("distance");
    if (m.kind === "distance") expect(m.sameFragment).toBe(false);
    // ...while an intra-water distance is not inter-fragment.
    const intra = measureSelection(s, [0, 1]);
    if (intra.kind === "distance") expect(intra.sameFragment).toBe(true);
  });

  it("a degenerate pick collapses to none", () => {
    const s = libScene("water");
    expect(measureSelection(s, [0, 0])).toEqual({ kind: "none" });
    expect(measureSelection(s, [0, 1, 99])).toEqual({ kind: "none" });
  });
});
