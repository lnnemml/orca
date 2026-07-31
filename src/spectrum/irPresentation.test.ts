import { describe, it, expect } from "vitest";

import type { IrMode } from "./ir";
import {
  scaledModes,
  nearestMode,
  irTooltipModel,
  DEFAULT_SCALE,
} from "./irPresentation";

describe("scaledModes — a display transform, identity by default", () => {
  const active: IrMode[] = [
    { cm: 1000, kmMol: 5, index: 0 },
    { cm: 3000, kmMol: 40, index: 1 },
  ];

  it("returns the input unchanged at scale 1.0 (no-op is provable)", () => {
    expect(scaledModes(active, DEFAULT_SCALE)).toBe(active); // same reference
  });

  it("multiplies only the wavenumber; intensity and index survive", () => {
    const s = scaledModes(active, 0.96);
    expect(s.map((m) => m.cm)).toEqual([960, 2880]);
    expect(s.map((m) => m.kmMol)).toEqual([5, 40]); // intensity untouched
    expect(s.map((m) => m.index)).toEqual([0, 1]); // index → table row preserved
    expect(active.map((m) => m.cm)).toEqual([1000, 3000]); // input not mutated
  });
});

describe("nearestMode — closest by wavenumber, deterministic", () => {
  const modes: IrMode[] = [
    { cm: 110, kmMol: 1.2, index: 3 },
    { cm: 1752.7, kmMol: 632.8, index: 20 },
    { cm: 3714, kmMol: 146.8, index: 40 },
  ];

  it("picks the mode closest to the cursor", () => {
    expect(nearestMode(modes, 115)?.cm).toBe(110);
    expect(nearestMode(modes, 1750)?.cm).toBe(1752.7);
    expect(nearestMode(modes, 4000)?.cm).toBe(3714);
  });

  it("returns null on an empty list", () => {
    expect(nearestMode([], 500)).toBeNull();
  });

  it("resolves a tie to the first mode", () => {
    const tie: IrMode[] = [
      { cm: 100, kmMol: 1, index: 0 },
      { cm: 200, kmMol: 1, index: 1 },
    ];
    expect(nearestMode(tie, 150)?.index).toBe(0);
  });
});

describe("irTooltipModel — label and value come from ONE x (the unit-3.10 bug)", () => {
  // The exact shape of the screenshot bug: the cursor is at 115 cm⁻¹, there is a
  // weak mode nearby (110), and a strong O–H mode far away at 3714 cm⁻¹ (the one
  // whose height 9.350 leaked into the old tooltip). The model must anchor
  // EVERYTHING to 115 — the curve value passed in, and the NEAREST mode (110, not
  // 3714).
  const modes: IrMode[] = [
    { cm: 110, kmMol: 1.2, index: 3 },
    { cm: 1752.7, kmMol: 632.8, index: 20 },
    { cm: 3714, kmMol: 146.8, index: 40 },
  ];
  const CURSOR = 115;
  const CURVE_AT_CURSOR = 0.069; // the value the caller read on the curve at 115

  const model = irTooltipModel(CURSOR, CURVE_AT_CURSOR, modes);

  it("labels with the cursor wavenumber, verbatim", () => {
    expect(model.cm).toBe(CURSOR);
  });

  it("reports the curve value the caller read at that same x — nothing else", () => {
    expect(model.curve).toBe(CURVE_AT_CURSOR);
  });

  it("names the NEAREST mode (110), never the far O–H peak (3714)", () => {
    expect(model.nearest?.cm).toBe(110);
    expect(model.nearest?.kmMol).toBe(1.2);
    expect(model.nearest?.index).toBe(3); // maps to the right table row
    // the far mode's height (146.8 km/mol) must not appear anywhere in the model
    expect(model.nearest?.kmMol).not.toBe(146.8);
  });

  it("reports the distance to that mode so the UI can say 'nearest', not 'here'", () => {
    expect(model.nearest?.deltaCm).toBe(5); // |110 - 115|
  });

  it("degrades to no nearest mode on an empty spectrum, keeping cm and curve", () => {
    const empty = irTooltipModel(200, 0.5, []);
    expect(empty.cm).toBe(200);
    expect(empty.curve).toBe(0.5);
    expect(empty.nearest).toBeNull();
  });
});
