import { describe, it, expect } from "vitest";

import type { RawAtom, RawFragment, Scene, SceneAtom } from "./types";
import { testScene, idsFor, borohydrideAfterWaterRemoved } from "./scene-test-util";
import {
  distance,
  angle,
  dihedral,
  dihedralCoords,
  measureByCoords,
  measureSelection,
  measureSelectionByIndex,
  type Vec3,
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
  return testScene([libraryFragmentToScene(lf)], 1);
}

function fragmentOf(atoms: RawAtom[], id = "f"): RawFragment {
  return { id, name: id, charge: 0, source: "editor", atoms };
}

function sceneOf(...fragments: RawFragment[]): Scene {
  return testScene(fragments);
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
    id: a.id, // a rigid move preserves atom identity
    element: a.element,
    x: R[0][0] * a.x + R[0][1] * a.y + R[0][2] * a.z + t[0],
    y: R[1][0] * a.x + R[1][1] * a.y + R[1][2] * a.z + t[1],
    z: R[2][0] * a.x + R[2][1] * a.y + R[2][2] * a.z + t[2],
  });
  return {
    ...scene,
    fragments: scene.fragments.map((f) => ({ ...f, atoms: f.atoms.map(move) })),
  };
}

/** Reflect every atom through the x=0 plane (an improper rotation, det = −1). */
function mirrorX(scene: Scene): Scene {
  return {
    ...scene,
    fragments: scene.fragments.map((f) => ({
      ...f,
      atoms: f.atoms.map((a) => ({ ...a, x: -a.x })),
    })),
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
    expect(measureSelection(s, idsFor(s, 0))).toEqual({ kind: "none" });
  });

  it("2 atoms → distance, positional", () => {
    const s = libScene("water");
    const m = measureSelection(s, idsFor(s, 0, 1));
    expect(m.kind).toBe("distance");
    if (m.kind === "distance") {
      expect(m.value).toBeCloseTo(0.9572, 4);
      expect(m.unit).toBe("Å");
      expect(m.atoms).toEqual([0, 1]); // resolved global indices, for rendering
      expect(m.sameFragment).toBe(true);
    }
  });

  it("3 atoms → angle with the MIDDLE pick as the vertex", () => {
    const s = libScene("water");
    // Pick order H(1), O(0), H(2): the vertex is O (the second pick), NOT the
    // smallest index. 104.52°, not something else.
    const m = measureSelection(s, idsFor(s, 1, 0, 2));
    expect(m.kind).toBe("angle");
    if (m.kind === "angle") expect(m.value).toBeCloseTo(104.52, 2);
  });

  it("4 atoms → dihedral", () => {
    const s = butaneScene(0);
    const m = measureSelection(s, idsFor(s, 0, 1, 2, 3));
    expect(m.kind).toBe("dihedral");
    if (m.kind === "dihedral") expect(m.value).toBeCloseTo(179.998, 2);
  });

  it("flags an inter-fragment distance (a future reaction coordinate)", () => {
    // water (0,1,2) + Cl⁻ (3): a bond across fragments. The library places both
    // O and Cl at the origin, so offset the chloride to a real contact distance.
    const cl: RawFragment = {
      id: "cl",
      name: "Cl-",
      charge: -1,
      source: "fragment-library",
      atoms: [{ element: "Cl", x: 3, y: 0, z: 0 }],
    };
    const s = sceneOf(libScene("water").fragments[0], cl);
    const m = measureSelection(s, idsFor(s, 0, 3));
    expect(m.kind).toBe("distance");
    if (m.kind === "distance") expect(m.sameFragment).toBe(false);
    // ...while an intra-water distance is not inter-fragment.
    const intra = measureSelection(s, idsFor(s, 0, 1));
    if (intra.kind === "distance") expect(intra.sameFragment).toBe(true);
  });

  it("a degenerate pick collapses to none", () => {
    const s = libScene("water");
    expect(measureSelection(s, idsFor(s, 0, 0))).toEqual({ kind: "none" });
    expect(measureSelection(s, idsFor(s, 0, 1, 99))).toEqual({ kind: "none" });
  });

  // ── Negative control (b): the 2.5.2b bug is STRUCTURALLY dead ────────────────
  // On the divergent fixture (BH₄⁻ after water removed, so AtomId 3 = boron sits
  // at global 0), a measurement addressed by AtomId follows the boron; a
  // measurement addressed by the STALE global index (3) reads a hydrogen instead.
  it("addressed by AtomId, a measurement follows the physical atom past a removal", () => {
    const { scene: s, boronId } = borohydrideAfterWaterRemoved();
    const otherH = idsFor(s, 1)[0]; // some BH₄⁻ hydrogen (global 1 now)
    // AtomId path: boron is at global 0 → distance boron···H is a B–H bond (~1.2 Å).
    const byId = measureSelection(s, [boronId, otherH]);
    expect(byId.kind).toBe("distance");
    if (byId.kind === "distance") expect(byId.atoms[0]).toBe(0); // boron resolved to global 0
    // The break the id-space prevents: reading the OLD global index 3 (a hydrogen
    // now) instead of the boron's id. `measureSelectionByIndex` on [3,1] would
    // measure H···H, a different coordinate — proof the two spaces truly diverge.
    const byStaleIndex = measureSelectionByIndex(s, [3, 1]);
    expect(byStaleIndex.kind).toBe("distance");
    if (byStaleIndex.kind === "distance" && byId.kind === "distance") {
      expect(byStaleIndex.value).not.toBeCloseTo(byId.value, 3);
    }
  });
});

// ── measureByCoords — the results-viewer coord path (F1) ──────────────────────
// The TrajectoryPlayer measures a frame's raw coordinates (Frame.xyz_angstrom, in
// frame/elements order) rather than a Scene. These pin that the coord path is
// bit-identical to the Scene path on the SAME geometry (the ASE-convention seam),
// and that xyz_angstrom's 0-based order needs no re-mapping.

/** The Vec3 coords of a single-fragment scene, in global-index order. */
function coordsFromScene(scene: Scene): Vec3[] {
  return scene.fragments.flatMap((f) => f.atoms.map((a): Vec3 => [a.x, a.y, a.z]));
}

describe("measureByCoords (results-viewer coord path)", () => {
  // MAIN RISK bite: the coord path must produce bit-identical Measurements to the
  // Scene path on the same geometry, for 2/3/4 picks — incl. the butane gauche
  // dihedral (the 60-side value, so the [0,360) fold is pinned through the coord
  // path). Flipping the fold sign in dihedralCoords makes the 4-pick case go red.
  it("measure_by_coords_matches_scene_path", () => {
    const scene = butaneScene(1); // gauche — the 4 carbons are atoms 0,1,2,3
    const coords = coordsFromScene(scene);

    for (const picked of [
      [0, 3], // distance
      [0, 1, 2], // angle (middle vertex)
      [0, 1, 2, 3], // dihedral (chain)
    ]) {
      expect(measureByCoords(coords, picked)).toEqual(
        measureSelectionByIndex(scene, picked),
      );
    }

    // Pin the butane gauche dihedral through the COORD path: 67.523° (the 60 side),
    // NOT 300 — exactly what the ASE [0,360) fold produces.
    const dih = measureByCoords(coords, [0, 1, 2, 3]);
    expect(dih.kind).toBe("dihedral");
    if (dih.kind === "dihedral") {
      expect(dih.value).toBeCloseTo(67.523, 2);
      expect(dih.value).toBeGreaterThan(0);
      expect(dih.value).toBeLessThan(180);
    }
  });

  // Closes the parity hole on the MAIN RISK seam: *Coords no longer carry the
  // repeated-INDEX guard (it lives in the Scene wrapper), and measureByCoords calls
  // *Coords directly — so measureByCoords must re-apply that guard to stay bit-
  // identical to the Scene path on a REPEATED pick. angleCoords(a,v,a) is 0° (not
  // null), dihedralCoords with a repeat is a number (not null); both Scene paths are
  // none via Set().size < N. Negative control: drop measureByCoords's repeated-index
  // guard → this bite goes red (0° / a number instead of none) while the different-
  // index cross-check above STAYS green — proving this bite covers exactly that seam.
  it("measure_by_coords_repeated_index_matches_scene_path", () => {
    const scene = butaneScene(1);
    const coords = coordsFromScene(scene);
    // angle with i == j (a repeated index): Scene → none (Set size 2 < 3).
    expect(measureByCoords(coords, [0, 1, 0])).toEqual(
      measureSelectionByIndex(scene, [0, 1, 0]),
    );
    expect(measureByCoords(coords, [0, 1, 0])).toEqual({ kind: "none" });
    // dihedral with a repeated index: Scene → none (Set size 3 < 4).
    expect(measureByCoords(coords, [0, 1, 1, 2])).toEqual(
      measureSelectionByIndex(scene, [0, 1, 1, 2]),
    );
    expect(measureByCoords(coords, [0, 1, 1, 2])).toEqual({ kind: "none" });
  });

  it("measure_by_coords_angle_vertex_is_middle", () => {
    // A right-angle L: a=(1,0,0), vertex=(0,0,0), b=(0,1,0) → 90° at the middle pick.
    const L: Vec3[] = [
      [1, 0, 0],
      [0, 0, 0],
      [0, 1, 0],
    ];
    const m = measureByCoords(L, [0, 1, 2]);
    expect(m.kind).toBe("angle");
    if (m.kind === "angle") expect(m.value).toBeCloseTo(90, 6);

    // Permutation putting a DIFFERENT atom (coords[0]) in the middle → a different
    // value (45°): the vertex is positional, not the smallest index.
    const perm = measureByCoords(L, [1, 0, 2]);
    expect(perm.kind).toBe("angle");
    if (perm.kind === "angle") {
      expect(perm.value).toBeCloseTo(45, 6);
      expect(perm.value).not.toBeCloseTo(90, 3);
    }
  });

  it("measure_by_coords_dihedral_reversal_invariant", () => {
    // A deterministic generic geometry (no symmetry to accidentally satisfy it).
    const p: Vec3[] = [
      [0.13, 0.21, -0.34],
      [1.42, 0.02, 0.51],
      [2.11, 1.33, -0.22],
      [3.05, 0.71, 1.14],
    ];
    const fwd = dihedralCoords(p[0], p[1], p[2], p[3]);
    const rev = dihedralCoords(p[3], p[2], p[1], p[0]);
    expect(fwd).not.toBeNull();
    expect(fwd!).toBeCloseTo(rev!, 10);
  });

  it("measure_by_coords_degenerate_is_none", () => {
    // Coincident pair → none.
    const dup: Vec3[] = [
      [1, 1, 1],
      [1, 1, 1],
      [2, 2, 2],
    ];
    expect(measureByCoords(dup, [0, 1])).toEqual({ kind: "none" });

    // Collinear dihedral → none (every inner angle planar).
    const line: Vec3[] = [
      [0, 0, 0],
      [1, 0, 0],
      [2, 0, 0],
      [3, 0, 0],
      [4, 0, 0], // a 5th point, for the ≥5 case below
    ];
    expect(measureByCoords(line, [0, 1, 2, 3])).toEqual({ kind: "none" });

    // Out-of-range index → none.
    expect(measureByCoords(line, [0, 99])).toEqual({ kind: "none" });

    // 1 atom and ≥5 atoms → none.
    expect(measureByCoords(line, [0])).toEqual({ kind: "none" });
    expect(measureByCoords(line, [0, 1, 2, 3, 4])).toEqual({ kind: "none" });
  });
});
