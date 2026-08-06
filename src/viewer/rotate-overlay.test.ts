import { describe, it, expect } from "vitest";

import {
  DEFAULT_ROTATE_OVERLAY,
  flipRotateOverlay,
  chooseRotateOverlay,
  type RotateOverlay,
} from "./rotate-overlay";

// ── Rotate-axis overlay toggle (unit 3.3b) — exactly ONE overlay per pick ─────
// The axis cylinder (unit 3.3) and the distance measurement drew on the SAME two
// atoms → two overlapping objects. A2: one overlay at a time, chosen by a toggle;
// the Å value always comes from the measure distance (single source, asserted in
// the viewer wiring — see grep gate). c1 = default/flip, c2 = the decision.

describe("rotate overlay toggle state (c1)", () => {
  it("defaults to axis (the panel opens for rotating)", () => {
    expect(DEFAULT_ROTATE_OVERLAY).toBe("axis");
  });

  it("flips both ways and is its own inverse", () => {
    expect(flipRotateOverlay("axis")).toBe("distance");
    expect(flipRotateOverlay("distance")).toBe("axis");
    const both: RotateOverlay[] = ["axis", "distance"];
    for (const o of both) expect(flipRotateOverlay(flipRotateOverlay(o))).toBe(o);
  });
});

describe("chooseRotateOverlay — exactly one overlay per pair (c2)", () => {
  it("outside Rotate (no axis) the measurement draws as before, in EITHER mode", () => {
    for (const o of ["axis", "distance"] as RotateOverlay[]) {
      expect(chooseRotateOverlay(false, o)).toEqual({ axis: false, measure: true });
    }
  });

  it("with an axis, 'axis' → cylinder only; 'distance' → measure only", () => {
    expect(chooseRotateOverlay(true, "axis")).toEqual({ axis: true, measure: false });
    expect(chooseRotateOverlay(true, "distance")).toEqual({ axis: false, measure: true });
  });

  it("NEVER draws both overlays for the pair (the invariant of the unit)", () => {
    for (const hasAxis of [true, false])
      for (const o of ["axis", "distance"] as RotateOverlay[]) {
        const r = chooseRotateOverlay(hasAxis, o);
        // Break (return {axis:true, measure:true} for the axis case) → this reddens.
        expect(r.axis && r.measure).toBe(false);
        // With an axis, exactly one is drawn; without, it's measure-only.
        if (hasAxis) expect(r.axis !== r.measure).toBe(true);
      }
  });
});
