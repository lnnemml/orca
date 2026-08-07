import { describe, it, expect } from "vitest";

import type { Scene } from "./types";
import type { RawAtom } from "./types";
import { testScene, idsFor, type RawFragment } from "./scene-test-util";
import {
  planGuidedPlacement,
  runGuidedPlacement,
  type GuidedStep,
  type GuidedPlan,
} from "./guided-placement";
import { atomCount, fragmentAtomIndices, mergeToXyz } from "./scene";
import { append, current, describe as describeOp, emptyLog, undo, type Op } from "./oplog";

// ── Fixtures ────────────────────────────────────────────────────────────────
// Substrate = 6 atoms (LARGER than the 5-atom reagent, the mission shape: a small
// nucleophile approaching a big substrate). A non-collinear zigzag so any 3/4
// picked atoms form a valid angle/dihedral. Global indices 0..5.
function substrate(id = "sub"): RawFragment {
  const els = ["C", "O", "C", "H", "H", "C"];
  const atoms: RawAtom[] = els.map((element, i) => ({
    element,
    x: i * 0.8,
    y: i % 2 === 0 ? 0.0 : 0.9,
    z: i % 3 === 0 ? 0.0 : 0.4,
  }));
  return { id, name: "Substrate", charge: 0, source: "editor", atoms };
}

// Reagent BH4⁻ (5 atoms) placed clear of the substrate. Boron is global index 6.
function borohydride(id = "bh4"): RawFragment {
  const d = 1.24 / Math.sqrt(3);
  const bx = 6.0;
  return {
    id,
    name: "BH4-",
    charge: -1,
    source: "fragment-library",
    atoms: [
      { element: "B", x: bx, y: 0.0, z: 0.0 },
      { element: "H", x: bx + d, y: d, z: d },
      { element: "H", x: bx - d, y: -d, z: d },
      { element: "H", x: bx - d, y: d, z: -d },
      { element: "H", x: bx + d, y: -d, z: -d },
    ],
  };
}

function scene(): Scene {
  return testScene([substrate(), borohydride()]);
}

const REAGENT_ID = "bh4";

/** Resolve the reagent atom (boron, global 6) and substrate anchors A,B,C
 * (global 0,1,2) as stable AtomIds for the current scene. */
function picks(s: Scene) {
  const [reagentAtom] = idsFor(s, 6); // boron
  const substrateRefs = idsFor(s, 0, 1, 2); // C, O, C — anchors A, B, C
  return { reagentAtom, substrateRefs };
}

function readyOrThrow(plan: GuidedPlan): Extract<GuidedPlan, { kind: "ready" }> {
  if (plan.kind !== "ready") throw new Error(`expected ready, got: ${JSON.stringify(plan)}`);
  return plan;
}

// ── c1 — ONLY the given coordinates are applied (θ/φ empty → skip, not 0) ──────
describe("(c1) only the given coordinates produce steps", () => {
  const s = scene();
  const { reagentAtom, substrateRefs } = picks(s);

  it("{d} → exactly one set-internal step (a distance)", () => {
    const plan = readyOrThrow(
      planGuidedPlacement(s, REAGENT_ID, reagentAtom, substrateRefs, {
        distance: 2.5,
        angle: null,
        dihedral: null,
      }),
    );
    expect(plan.steps.map((x) => x.op)).toEqual(["distance"]);
  });

  it("{d,θ} → two steps; {d,θ,φ} → three, in order", () => {
    const two = readyOrThrow(
      planGuidedPlacement(s, REAGENT_ID, reagentAtom, substrateRefs, {
        distance: 2.5,
        angle: 107,
        dihedral: null,
      }),
    );
    expect(two.steps.map((x) => x.op)).toEqual(["distance", "angle"]);

    const three = readyOrThrow(
      planGuidedPlacement(s, REAGENT_ID, reagentAtom, substrateRefs, {
        distance: 2.5,
        angle: 107,
        dihedral: 180,
      }),
    );
    expect(three.steps.map((x) => x.op)).toEqual(["distance", "angle", "dihedral"]);
  });

  // The BITE: `null` (empty field) is a SKIP, but a real `0` is a value. If a future
  // change coerced empty→0, the {d} case above would sprout an angle+dihedral step
  // and go red. This test proves the function distinguishes the two: a 0 target
  // DOES emit a step, so `null` skipping is a real decision, not a no-op.
  it("distinguishes null (skip) from 0 (a real target)", () => {
    const withZeros = readyOrThrow(
      planGuidedPlacement(s, REAGENT_ID, reagentAtom, substrateRefs, {
        distance: 2.5,
        angle: 0,
        dihedral: 0,
      }),
    );
    expect(withZeros.steps.map((x) => x.op)).toEqual(["distance", "angle", "dihedral"]);
  });
});

// ── c2 — the mask is the REAGENT fragment (the reagent moves, substrate stays) ──
describe("(c2) every step masks the reagent fragment, not the substrate", () => {
  const s = scene();
  const { reagentAtom, substrateRefs } = picks(s);
  const reagentMask = fragmentAtomIndices(s, REAGENT_ID); // [6..10]
  const substrateMask = fragmentAtomIndices(s, "sub"); // [0..5]

  it("mask === reagent indices and movingFragmentId === reagent, for every step", () => {
    const plan = readyOrThrow(
      planGuidedPlacement(s, REAGENT_ID, reagentAtom, substrateRefs, {
        distance: 2.5,
        angle: 107,
        dihedral: 180,
      }),
    );
    expect(plan.movingFragmentId).toBe(REAGENT_ID);
    for (const step of plan.steps) {
      expect(step.mask).toEqual(reagentMask);
      // The BITE: the two masks are disjoint, so a resolve that picked the substrate
      // mask (the reagent would sit still while the substrate moved — the wrong
      // physics) would fail this equality.
      expect(step.mask).not.toEqual(substrateMask);
    }
    // Sanity: the masks really are different (the assertion above can bite).
    expect(reagentMask).not.toEqual(substrateMask);
  });
});

// ── c3 — a sequence of legible EXISTING ops; Undo unwinds one at a time ────────
describe("(c3) one legible set-internal op per coordinate, undo unwinds each", () => {
  const s = scene();
  const { reagentAtom, substrateRefs } = picks(s);

  // A fake sidecar that echoes the input geometry (moves nothing) — enough to drive
  // the op-sequence structure without a live server. Coordinates are irrelevant to
  // c3; the *shape* of the log is what's under test.
  const echoSetInternal = (sc: Scene, _step: GuidedStep) =>
    Promise.resolve({ xyz: mergeToXyz(sc), max_static_displacement: 0 });

  it("{d,θ,φ} → three replace-fragment-atoms(set-internal) ops, legibly described", async () => {
    const plan = readyOrThrow(
      planGuidedPlacement(s, REAGENT_ID, reagentAtom, substrateRefs, {
        distance: 2.5,
        angle: 107,
        dihedral: 180,
      }),
    );
    const result = await runGuidedPlacement(s, plan, "BH4-", echoSetInternal, {
      checkPostCondition: false,
    });

    // The BITE: three coordinates → THREE ops, never one bundled/opaque op.
    expect(result.ops).toHaveLength(3);
    for (const { op } of result.ops) {
      expect(op.type).toBe("replace-fragment-atoms");
      if (op.type === "replace-fragment-atoms") expect(op.edit.via).toBe("set-internal");
    }
    // Legible lab-journal lines (the existing `describe`, one per coordinate).
    const lines = result.ops.map(({ op }) => describeOp(op));
    expect(lines[0]).toMatch(/^Set distance /);
    expect(lines[1]).toMatch(/^Set angle /);
    expect(lines[2]).toMatch(/^Set dihedral /);
  });

  it("each op appends a log entry; undo unwinds them one by one", async () => {
    const plan = readyOrThrow(
      planGuidedPlacement(s, REAGENT_ID, reagentAtom, substrateRefs, {
        distance: 2.5,
        angle: 107,
        dihedral: 180,
      }),
    );
    const result = await runGuidedPlacement(s, plan, "BH4-", echoSetInternal, {
      checkPostCondition: false,
    });

    // Seed a log with the base scene (stands in for the committed add-fragment), then
    // append the guided ops with their snapshots — exactly what the store does.
    const seed: Op = {
      type: "restore-snapshot",
      source: "library",
      fragmentCount: s.fragments.length,
      atomCount: atomCount(s),
    };
    let log = append(emptyLog(), seed, s);
    for (const { op, scene: resultScene } of result.ops) log = append(log, op, resultScene);

    expect(log.entries).toHaveLength(4); // seed + d + θ + φ
    expect(log.pointer).toBe(3);

    // Three undos, one entry each — the pointer walks back step by step.
    log = undo(log);
    expect(log.pointer).toBe(2);
    log = undo(log);
    expect(log.pointer).toBe(1);
    log = undo(log);
    expect(log.pointer).toBe(0);
    expect(current(log)).toBe(s); // back at the seed (the roughly-placed reagent)
  });
});

// ── c4 — the Apply post-condition guard (rule #9) is enforced; Preview isn't ────
describe("(c4) Apply enforces the set-internal post-condition; Preview is view-only", () => {
  const s = scene();
  const { reagentAtom, substrateRefs } = picks(s);
  const plan = readyOrThrow(
    planGuidedPlacement(s, REAGENT_ID, reagentAtom, substrateRefs, {
      distance: 2.5,
      angle: null,
      dihedral: null,
    }),
  );

  // A response that moved a STATIC atom (max_static_displacement well past 1e-6).
  const movedStatic = (sc: Scene, _step: GuidedStep) =>
    Promise.resolve({ xyz: mergeToXyz(sc), max_static_displacement: 0.5 });

  it("Apply REFUSES a response that moved a static atom", async () => {
    await expect(
      runGuidedPlacement(s, plan, "BH4-", movedStatic, { checkPostCondition: true }),
    ).rejects.toThrow(/moved atoms outside the mask/i);
  });

  // The BITE: the same bad response under Preview (view-only) must NOT throw — so the
  // guard above is genuinely the Apply path, not something that always fires.
  it("Preview does NOT enforce it (view-only)", async () => {
    const result = await runGuidedPlacement(s, plan, "BH4-", movedStatic, {
      checkPostCondition: false,
    });
    expect(result.ops).toHaveLength(1);
  });
});
