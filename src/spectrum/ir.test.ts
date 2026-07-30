import { describe, it, expect } from "vitest";

import {
  classifyModes,
  lorentzian,
  autoGrid,
  spectrum,
  integrate,
  DEFAULT_FWHM_CM,
  type IrMode,
} from "./ir";

describe("classifyModes — split by measured fact, not a threshold", () => {
  // A saddle-like list: 6 exact-zero trans/rot, one imaginary (measured -33.66
  // on the real saddle), the rest real vibrations — plus deliberately tiny
  // values that a magnitude threshold would misclassify.
  const freq = [0, 0, 0, 0, 0, 0, -33.66, 0.5, 318.41, 3014.8];
  const ir = [0, 0, 0, 0, 0, 0, 12.0, 0.01, 0.0002, 51.97];

  it("excludes exactly-zero modes by === 0, counting them", () => {
    const { zeroCount } = classifyModes(freq, ir);
    expect(zeroCount).toBe(6);
  });

  it("splits imaginary by SIGN (kept separately), not by a cutoff", () => {
    const { imaginary } = classifyModes(freq, ir);
    expect(imaginary.map((m) => m.cm)).toEqual([-33.66]);
    // its km/mol intensity travels with it (a diagnosis, with its data)
    expect(imaginary[0].kmMol).toBe(12.0);
  });

  it("keeps a TINY positive frequency as a real vibration (sign, not magnitude)", () => {
    const { active } = classifyModes(freq, ir);
    expect(active.map((m) => m.cm)).toContain(0.5);
    // and a tiny NEGATIVE one would be imaginary, not dropped
    const { active: a2, imaginary: i2 } = classifyModes([-0.4], [1]);
    expect(a2).toHaveLength(0);
    expect(i2.map((m) => m.cm)).toEqual([-0.4]);
  });

  it("carries the ORIGINAL index so a peak maps back to a table row", () => {
    const { active } = classifyModes(freq, ir);
    const ch = active.find((m) => m.cm === 3014.8)!;
    expect(ch.index).toBe(9); // its position in the full list
  });
});

describe("lorentzian — area-normalized, FWHM is the full width at half max", () => {
  it("has unit area (∫ over a wide grid ≈ 1)", () => {
    const fwhm = 10;
    // integrate on a fine, wide grid around 0
    let area = 0;
    const step = 0.01;
    for (let x = -2000; x < 2000; x += step) area += lorentzian(x, 0, fwhm) * step;
    expect(area).toBeCloseTo(1, 2);
  });

  it("value at x₀±FWHM/2 is exactly half the peak value", () => {
    const fwhm = 8;
    const peak = lorentzian(1000, 1000, fwhm);
    const half = lorentzian(1000 + fwhm / 2, 1000, fwhm);
    expect(half).toBeCloseTo(peak / 2, 12);
  });

  it("peak value is 2/(π·FWHM)", () => {
    const fwhm = 10;
    expect(lorentzian(0, 0, fwhm)).toBeCloseTo(2 / (Math.PI * fwhm), 12);
  });
});

describe("spectrum — area under a peak equals its km/mol intensity", () => {
  it("integrates a single mode's curve back to its intensity", () => {
    const active: IrMode[] = [{ cm: 1500, kmMol: 42, index: 0 }];
    const fwhm = 6;
    // a wide, fine explicit grid so the full Lorentzian wings are captured
    const grid = { min: 0, max: 3000, step: 0.25 };
    const pts = spectrum(active, grid, fwhm);
    // the peak integral should recover 42 km/mol (a little is lost to the wings
    // beyond 0..3000, but for FWHM 6 that is tiny)
    expect(integrate(pts)).toBeCloseTo(42, 0);
  });

  it("total area of two modes equals the sum of intensities", () => {
    const active: IrMode[] = [
      { cm: 800, kmMol: 10, index: 0 },
      { cm: 2000, kmMol: 30, index: 1 },
    ];
    const grid = { min: 0, max: 3400, step: 0.25 };
    const pts = spectrum(active, grid, 8);
    expect(integrate(pts)).toBeCloseTo(40, 0);
  });

  it("is a superposition — sum of two single-mode curves equals the pair", () => {
    const grid = { min: 0, max: 3400, step: 2 };
    const a: IrMode = { cm: 800, kmMol: 10, index: 0 };
    const b: IrMode = { cm: 2000, kmMol: 30, index: 1 };
    const both = spectrum([a, b], grid, 8);
    const sa = spectrum([a], grid, 8);
    const sb = spectrum([b], grid, 8);
    for (let i = 0; i < both.length; i++) {
      expect(both[i].absorbance).toBeCloseTo(sa[i].absorbance + sb[i].absorbance, 12);
    }
  });
});

describe("autoGrid — range and step named explicitly", () => {
  const active: IrMode[] = [
    { cm: 318.41, kmMol: 0.0002, index: 0 },
    { cm: 3014.8, kmMol: 51.97, index: 1 },
  ];

  it("covers every active mode with padding, clamped at 0", () => {
    const g = autoGrid(active, DEFAULT_FWHM_CM);
    // pad = PAD_FWHM(8) * 10 = 80; lo 318.41 → floor(238.41) = 238 (> 0, no clamp)
    expect(g.min).toBe(238);
    // hi 3014.8 → ceil(3094.8) = 3095
    expect(g.max).toBe(3095);
    expect(g.min).toBeLessThan(318.41);
    expect(g.max).toBeGreaterThan(3014.8);
  });

  it("keeps step small relative to the FWHM so peaks are smooth", () => {
    const g = autoGrid(active, 10);
    expect(g.step).toBeLessThanOrEqual(10 / 4);
    expect(g.step).toBeGreaterThan(0);
  });

  it("never produces a negative-wavenumber left edge", () => {
    const near0: IrMode[] = [{ cm: 20, kmMol: 1, index: 0 }];
    const g = autoGrid(near0, 10);
    expect(g.min).toBe(0);
  });

  it("degrades safely on an empty active list", () => {
    const g = autoGrid([], 10);
    expect(g.max).toBeGreaterThan(g.min);
  });
});
