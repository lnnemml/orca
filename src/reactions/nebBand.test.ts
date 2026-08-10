import { describe, it, expect } from "vitest";

import type { NebResults, NebIteration } from "../types";
import { HARTREE_TO_KCAL } from "../scan/scanProfile";
import { iterationSeries, mepSeries, barrierSeries } from "./nebBand";

/**
 * A small NEB fixture in the shape the parser produces for the Menshutkin SN2:
 * per-iteration bands carry ABSOLUTE Eh energies (≈ −472.7); the MEP carries
 * RELATIVE energies (image 0 = 0). Three iterations, 4 images each — enough to
 * show the band relax toward the barrier while its reactant end drifts a little.
 */
const NEB: NebResults = {
  iterations: [
    {
      index: 0,
      // reactant end −472.7700; a shallow first-guess band.
      images: [
        { distance_angstrom: 0.0, energy_eh: -472.77 },
        { distance_angstrom: 1.0, energy_eh: -472.762 },
        { distance_angstrom: 2.0, energy_eh: -472.758 },
        { distance_angstrom: 3.0, energy_eh: -472.769 },
      ],
      barrier_eh: 0.012,
      climbing_image: null,
    },
    {
      index: 1,
      // reactant end drifted to −472.7710; barrier sharpening.
      images: [
        { distance_angstrom: 0.0, energy_eh: -472.771 },
        { distance_angstrom: 1.0, energy_eh: -472.76 },
        { distance_angstrom: 2.0, energy_eh: -472.7555 },
        { distance_angstrom: 3.0, energy_eh: -472.7695 },
      ],
      barrier_eh: 0.0145,
      climbing_image: 2,
    },
    {
      index: 2,
      // converged: barrier at image 2.
      images: [
        { distance_angstrom: 0.0, energy_eh: -472.7715 },
        { distance_angstrom: 1.0, energy_eh: -472.7585 },
        { distance_angstrom: 2.0, energy_eh: -472.756 },
        { distance_angstrom: 3.0, energy_eh: -472.77 },
      ],
      barrier_eh: 0.01555,
      climbing_image: 2,
    },
  ],
  mep: [
    { distance_angstrom: 0.0, energy_eh: 0.0 },
    { distance_angstrom: 1.0, energy_eh: 0.013 },
    { distance_angstrom: 2.0, energy_eh: 0.01555 },
    { distance_angstrom: 3.0, energy_eh: 0.0015 },
  ],
  final_barrier_eh: 0.01555,
  ts_geometry: { elements: ["N", "C"], xyz_angstrom: [[0, 0, 0], [2.353, 0, 0]] },
};

describe("iterationSeries — ΔE relative to the iteration's own image-0 (never absolute Eh)", () => {
  it("subtracts image-0: point 0 is exactly 0, others are (E−E0)·627.509", () => {
    const s = iterationSeries(NEB.iterations[2]);
    expect(s[0].deltaE_kcal).toBe(0); // the control: a raw-Eh plot would be ≈ −296,600, NOT 0
    const expected =
      (NEB.iterations[2].images[2].energy_eh - NEB.iterations[2].images[0].energy_eh) *
      HARTREE_TO_KCAL;
    expect(s[2].deltaE_kcal).toBeCloseTo(expected, 9);
    // …and it is a modest positive barrier (a raw-Eh y would be ≈ −472.756 · 627 ≈ −296,600).
    expect(s[2].deltaE_kcal).toBeGreaterThan(0);
    expect(s[2].deltaE_kcal).toBeLessThan(50);
    // distances pass through unchanged.
    expect(s.map((p) => p.distance)).toEqual([0, 1, 2, 3]);
  });

  it("NEGATIVE control — plotting absolute Eh does NOT overlay: two iterations with different reactant ends", () => {
    // What the code MUST do: relativized, both iterations start at 0 → they overlay.
    const rel0 = iterationSeries(NEB.iterations[0]);
    const rel1 = iterationSeries(NEB.iterations[1]);
    expect(rel0[0].deltaE_kcal).toBe(0);
    expect(rel1[0].deltaE_kcal).toBe(0);

    // The bite: a broken version that plotted absolute Eh·627 would give the two
    // iterations DIFFERENT y at image 0 (their reactant ends differ by 0.001 Eh),
    // so they would NOT overlay. Confirm that gap is real and large in absolute space.
    const abs0 = NEB.iterations[0].images[0].energy_eh * HARTREE_TO_KCAL;
    const abs1 = NEB.iterations[1].images[0].energy_eh * HARTREE_TO_KCAL;
    expect(Math.abs(abs0 - abs1)).toBeGreaterThan(0.5); // ≈ 0.001 Eh · 627 ≈ 0.63 kcal — a visible offset
    // …whereas relativized they are identical (overlay).
    expect(rel0[0].deltaE_kcal).toBe(rel1[0].deltaE_kcal);
  });

  it("empty image list → empty series (no NaN from a missing image-0)", () => {
    const empty: NebIteration = { index: 0, images: [], barrier_eh: 0, climbing_image: null };
    expect(iterationSeries(empty)).toEqual([]);
  });
});

describe("mepSeries — the converged MEP is already relative (image 0 = 0)", () => {
  it("point 0 is exactly 0 and energies are the interp values ·627.509", () => {
    const s = mepSeries(NEB);
    expect(s[0].deltaE_kcal).toBe(0);
    expect(s[2].deltaE_kcal).toBeCloseTo(0.01555 * HARTREE_TO_KCAL, 9);
    // the MEP barrier top is a modest positive number, not an absolute-Eh magnitude.
    expect(s[2].deltaE_kcal).toBeGreaterThan(0);
    expect(s[2].deltaE_kcal).toBeLessThan(50);
  });
});

describe("barrierSeries — the convergence curve, final == converged barrier", () => {
  it("keys by iteration index and converts barrier_eh → kcal/mol", () => {
    const s = barrierSeries(NEB);
    expect(s.map((p) => p.iteration)).toEqual([0, 1, 2]);
    expect(s[2].barrier_kcal).toBeCloseTo(0.01555 * HARTREE_TO_KCAL, 9);
  });

  it("the final value equals the converged NEB barrier (final_barrier_eh ·627.509)", () => {
    const s = barrierSeries(NEB);
    expect(s[s.length - 1].barrier_kcal).toBeCloseTo(
      (NEB.final_barrier_eh ?? NaN) * HARTREE_TO_KCAL,
      9,
    );
  });

  it("is monotone-ish toward the converged barrier (the band tightens, not loosens)", () => {
    const s = barrierSeries(NEB);
    expect(s[0].barrier_kcal).toBeLessThan(s[1].barrier_kcal);
    expect(s[1].barrier_kcal).toBeLessThan(s[2].barrier_kcal);
  });
});
