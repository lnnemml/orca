import { describe, it, expect } from "vitest";

import type { Scene } from "./types";
import type { RawFragment } from "./scene-test-util";
import { testScene, idsFor } from "./scene-test-util";
import { planEdit } from "./edit-plan";
import {
  bondingDistance,
  breakDistance,
  planFormBond,
  planBreakBond,
  _PERCEPTION_MULTIPLIER,
} from "./bond-edit";

// Fragment A — a methylamine core: C(0), N(1), H(2). Fragment B — an ethyl-iodide
// core: C(3), I(4). Inter-fragment pair (N + C_B) is a movable distance; the C–I
// pair inside B is intra-fragment (a needs-split coordinate).
function amine(id = "meam"): RawFragment {
  return {
    id,
    name: "methylamine",
    charge: 0,
    source: "editor",
    atoms: [
      { element: "C", x: 0.0, y: 0.0, z: 0.0 },
      { element: "N", x: 1.47, y: 0.0, z: 0.0 },
      { element: "H", x: -0.6, y: 0.9, z: 0.0 },
    ],
  };
}
function ethylIodide(id = "eti"): RawFragment {
  return {
    id,
    name: "ethyl iodide",
    charge: 0,
    source: "editor",
    atoms: [
      { element: "C", x: 5.0, y: 0.0, z: 0.0 },
      { element: "I", x: 7.15, y: 0.0, z: 0.0 }, // ≈ C–I bonding distance apart
    ],
  };
}
function scene(): Scene {
  return testScene([amine(), ethylIodide()]);
}

describe("bondingDistance — the covalent-radius sum, element-dependent", () => {
  it("matches the Cordero sums (C–H, C–C, C–N, C–O, C–I ladder)", () => {
    expect(bondingDistance("C", "H")).toBeCloseTo(1.07, 2);
    expect(bondingDistance("C", "C")).toBeCloseTo(1.52, 2);
    expect(bondingDistance("C", "N")).toBeCloseTo(1.47, 2);
    expect(bondingDistance("C", "O")).toBeCloseTo(1.42, 2);
    expect(bondingDistance("C", "I")).toBeCloseTo(2.15, 2);
  });

  it("is case-insensitive (element symbols vary in casing across sources)", () => {
    expect(bondingDistance("c", "i")).toBeCloseTo(bondingDistance("C", "I"), 6);
  });

  it("NEGATIVE control — element-dependent, NOT a fixed 1.5 default", () => {
    // A broken impl that returned a constant 1.5 would make C–H and C–I equal.
    expect(bondingDistance("C", "H")).not.toBeCloseTo(bondingDistance("C", "I"), 1);
    // …and neither equals the fixed-default 1.5 that the bite would return.
    expect(bondingDistance("C", "H")).not.toBeCloseTo(1.5, 1);
    expect(bondingDistance("C", "I")).not.toBeCloseTo(1.5, 1);
  });

  it("throws on an unknown element (a missing radius is loud, never a guess)", () => {
    expect(() => bondingDistance("C", "Zz")).toThrow(/covalent radius/i);
  });
});

describe("breakDistance — clears the perception window for any pair", () => {
  it("is > (rA+rB) × perception multiplier, so the bond drops (C–I tested)", () => {
    const perceptionThreshold = bondingDistance("C", "I") * _PERCEPTION_MULTIPLIER;
    expect(breakDistance("C", "I")).toBeGreaterThan(perceptionThreshold);
  });

  it("is exactly twice the bonding distance", () => {
    expect(breakDistance("C", "I")).toBeCloseTo(bondingDistance("C", "I") * 2, 6);
    expect(breakDistance("N", "C")).toBeCloseTo(bondingDistance("N", "C") * 2, 6);
  });
});

describe("planFormBond / planBreakBond — delegate to planEdit with a computed target", () => {
  it("planFormBond: target == bondingDistance(N, C) and the plan is a ready distance edit", () => {
    const s = scene();
    const [n, cB] = idsFor(s, 1, 3); // N (fragment A) + C (fragment B) — inter-fragment
    const fb = planFormBond(s, n, cB);
    expect(fb.target).toBeCloseTo(bondingDistance("N", "C"), 6);
    expect(fb.plan.kind).toBe("ready");
    if (fb.plan.kind === "ready") expect(fb.plan.op).toBe("distance");
  });

  it("NEGATIVE control — form does NOT use breakDistance and does NOT ignore elements", () => {
    const s = scene();
    const [n, cB] = idsFor(s, 1, 3);
    const fb = planFormBond(s, n, cB);
    // The bite: a form that used breakDistance would set target ≈ 2× too far.
    expect(fb.target).not.toBeCloseTo(breakDistance("N", "C"), 2);
    // The bite: an element-blind form (fixed 1.5) would not track the N–C sum.
    const [cA, i] = idsFor(s, 0, 4); // C (fragment A) + I (fragment B) — different elements
    const fbCI = planFormBond(s, cA, i);
    expect(fbCI.target).not.toBeCloseTo(fb.target, 2); // C–I ≠ N–C target
  });

  it("planBreakBond: target == breakDistance(C, I)", () => {
    const s = scene();
    const [cB, i] = idsFor(s, 3, 4); // the C–I pair inside fragment B
    const bb = planBreakBond(s, cB, i);
    expect(bb.target).toBeCloseTo(breakDistance("C", "I"), 6);
  });

  it("delegation — the plan is EXACTLY planEdit's (mask/needs-split shape preserved)", () => {
    const s = scene();
    // inter-fragment (ready) pair
    const [n, cB] = idsFor(s, 1, 3);
    expect(planFormBond(s, n, cB).plan).toEqual(planEdit(s, [n, cB]));
    // intra-fragment (needs-split) pair — the C–I bond inside fragment B
    const [cB2, i] = idsFor(s, 3, 4);
    const bb = planBreakBond(s, cB2, i);
    expect(bb.plan).toEqual(planEdit(s, [cB2, i]));
    expect(bb.plan.kind).toBe("needs-split"); // proves the intra-fragment path is untouched
  });
});
