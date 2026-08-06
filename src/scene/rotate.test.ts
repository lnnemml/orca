import { describe, it, expect } from "vitest";

import { testScene, idsFor, type RawFragment } from "./scene-test-util";
import { rotateFragment, rotateFragmentInScene, rotationAxis } from "./scene";
import type { SceneFragment } from "./types";

// ── Rigid fragment rotation about a picked approach axis (Stage 3, unit 3.3) ──
// The pure Rodrigues core + its scene-level mutator. Post-condition (rule #9): a
// rotation is RIGID (all internal pairwise distances invariant), the pivot P and
// every point on the axis P→Q is a fixed point, other fragments are untouched, and
// count/order/AtomId are invariant. c1 RIGID, c2 RODRIGUES, c4 AXIS/PIVOT, c5
// DEGENERATE. Each negative control is noted where it bites (demonstrated red on
// break during the unit, then reverted — see wiki/log).

const pairwise = (atoms: { x: number; y: number; z: number }[]): number[] => {
  const ds: number[] = [];
  for (let i = 0; i < atoms.length; i++)
    for (let j = i + 1; j < atoms.length; j++) {
      const a = atoms[i];
      const b = atoms[j];
      ds.push(Math.hypot(a.x - b.x, a.y - b.y, a.z - b.z));
    }
  return ds;
};

/** Mover fragment (P = pivot at origin, M on the +z axis, A/B off-axis in the
 * xy-plane) + a substrate fragment carrying Q (the direction atom, +z from P) and
 * a spectator D. Global indices: P=0, M=1, A=2, B=3, Q=4, D=5. */
function twoFragmentScene() {
  const mover: RawFragment = {
    id: "mover",
    name: "reagent",
    charge: 0,
    source: "editor",
    atoms: [
      { element: "C", x: 0, y: 0, z: 0 }, // P — pivot, on axis
      { element: "H", x: 0, y: 0, z: 0.5 }, // M — on the P→Q (+z) axis
      { element: "H", x: 1, y: 0, z: 0 }, // A — off axis
      { element: "H", x: 0, y: 1, z: 0 }, // B — off axis
    ],
  };
  const sub: RawFragment = {
    id: "sub",
    name: "substrate",
    charge: 0,
    source: "editor",
    atoms: [
      { element: "O", x: 0, y: 0, z: 2 }, // Q — direction atom (+z from P), on axis
      { element: "N", x: 3, y: 3, z: 3 }, // D — spectator
    ],
  };
  const scene = testScene([mover, sub]);
  const [p, m, , , q] = idsFor(scene, 0, 1, 2, 3, 4);
  return { scene, p, q, m };
}

describe("rotateFragment — Rodrigues core (c2)", () => {
  const axis: [number, number, number] = [0, 0, 1];
  const pivot: [number, number, number] = [0, 0, 0];
  const frag: SceneFragment = testScene([
    {
      id: "f",
      name: "f",
      charge: 0,
      source: "editor",
      atoms: [
        { element: "C", x: 1, y: 0, z: 0 },
        { element: "H", x: 0, y: 1, z: 0 },
        { element: "H", x: 0, y: 0, z: 3 }, // on the +z axis → a fixed point
      ],
    },
  ]).fragments[0];

  it("rot(π/2) about +z matches the closed-form CCW rotation", () => {
    const r = rotateFragment(frag, axis, Math.PI / 2, pivot);
    expect(r.atoms[0]).toMatchObject({ x: expect.closeTo(0, 12), y: expect.closeTo(1, 12) });
    expect(r.atoms[1]).toMatchObject({ x: expect.closeTo(-1, 12), y: expect.closeTo(0, 12) });
    // A point ON the axis is invariant (c2: point-on-axis fixed).
    expect(r.atoms[2]).toMatchObject({
      x: expect.closeTo(0, 12),
      y: expect.closeTo(0, 12),
      z: expect.closeTo(3, 12),
    });
  });

  it("rot(0) and rot(2π) are the identity; rot(θ)∘rot(−θ) is the identity", () => {
    for (const theta of [0, 2 * Math.PI]) {
      const r = rotateFragment(frag, axis, theta, pivot);
      r.atoms.forEach((a, i) => {
        expect(a.x).toBeCloseTo(frag.atoms[i].x, 10);
        expect(a.y).toBeCloseTo(frag.atoms[i].y, 10);
        expect(a.z).toBeCloseTo(frag.atoms[i].z, 10);
      });
    }
    // Round-trip: a turn and its inverse cancel (breaks red on a wrong sign/formula).
    const back = rotateFragment(rotateFragment(frag, axis, 0.9, pivot), axis, -0.9, pivot);
    back.atoms.forEach((a, i) => {
      expect(a.x).toBeCloseTo(frag.atoms[i].x, 10);
      expect(a.y).toBeCloseTo(frag.atoms[i].y, 10);
      expect(a.z).toBeCloseTo(frag.atoms[i].z, 10);
    });
  });

  it("normalizes a non-unit axis (raw Q − P works)", () => {
    const unit = rotateFragment(frag, [0, 0, 1], 0.7, pivot);
    const raw = rotateFragment(frag, [0, 0, 5], 0.7, pivot); // 5× longer, same direction
    raw.atoms.forEach((a, i) => {
      expect(a.x).toBeCloseTo(unit.atoms[i].x, 12);
      expect(a.y).toBeCloseTo(unit.atoms[i].y, 12);
      expect(a.z).toBeCloseTo(unit.atoms[i].z, 12);
    });
  });

  it("throws on a degenerate (zero-length) axis rather than emitting NaN", () => {
    expect(() => rotateFragment(frag, [0, 0, 0], 0.5, pivot)).toThrow(/degenerate/);
  });
});

describe("rotateFragmentInScene — rigid, axis P→Q, pivot P (c1, c4)", () => {
  it("is rigid; P and on-axis atoms are fixed; other fragments untouched; ids/order invariant", () => {
    const { scene, p, q, m } = twoFragmentScene();
    const moverBefore = scene.fragments[0];
    const subBefore = scene.fragments[1];

    const out = rotateFragmentInScene(scene, "mover", [p, q], 1.1);
    const moverAfter = out.fragments[0];

    // c1 RIGID: internal pairwise distances unchanged (break: scale/non-rigid → red).
    pairwise(moverAfter.atoms).forEach((d, k) =>
      expect(d).toBeCloseTo(pairwise(moverBefore.atoms)[k], 10),
    );
    // c4 PIVOT P is fixed (break: pivot = centroid → P moves → red).
    expect(moverAfter.atoms[0]).toMatchObject({
      x: expect.closeTo(0, 12),
      y: expect.closeTo(0, 12),
      z: expect.closeTo(0, 12),
    });
    // c4 a point ON the P→Q axis (M) is fixed (break: wrong axis → M moves → red).
    expect(moverAfter.atoms[1]).toMatchObject({
      x: expect.closeTo(0, 12),
      y: expect.closeTo(0, 12),
      z: expect.closeTo(0.5, 12),
    });
    // Off-axis atoms actually moved (a real rotation happened, not a silent no-op).
    expect(Math.hypot(
      moverAfter.atoms[2].x - moverBefore.atoms[2].x,
      moverAfter.atoms[2].y - moverBefore.atoms[2].y,
      moverAfter.atoms[2].z - moverBefore.atoms[2].z,
    )).toBeGreaterThan(0.1);
    // count / order / AtomId invariant.
    expect(moverAfter.atoms).toHaveLength(moverBefore.atoms.length);
    moverAfter.atoms.forEach((a, i) => {
      expect(a.id).toBe(moverBefore.atoms[i].id);
      expect(a.element).toBe(moverBefore.atoms[i].element);
    });
    // Other fragment (which carries Q) is untouched — Q lies on the axis, and D is
    // in a fragment that does not rotate at all.
    expect(out.fragments[1]).toEqual(subBefore);
    // `m` names M for readability of the fixture; assert it resolves to the on-axis atom.
    expect(m).toBe(moverBefore.atoms[1].id);
  });
});

describe("rotationAxis + rotateFragmentInScene — degenerate axis (c5)", () => {
  it("rotationAxis is null for a coincident/same/absent axis; the mutator is a no-op (same ref)", () => {
    const { scene, p, q } = twoFragmentScene();
    // Same atom P==Q → no direction.
    expect(rotationAxis(scene, p, p)).toBeNull();
    // An absent id → no atom.
    const absent = idsFor(scene, 999)[0];
    expect(rotationAxis(scene, p, absent)).toBeNull();
    // A real axis resolves.
    expect(rotationAxis(scene, p, q)).not.toBeNull();

    // Degenerate → identity result, returned as the SAME reference (so the store
    // appends no log entry). Break (allow a degenerate axis) → NaN coords → red.
    expect(rotateFragmentInScene(scene, "mover", [p, p], 0.5)).toBe(scene);
    // Absent fragment → same-ref no-op too.
    expect(rotateFragmentInScene(scene, "ghost", [p, q], 0.5)).toBe(scene);
  });
});
