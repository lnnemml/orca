import { describe, it, expect } from "vitest";

import type { ScanProfileJson } from "../types";
import {
  deltaDeltaEKcal,
  intrinsicBarrierKcal,
  reactantSideMinEh,
  argMaxIndex,
  methodSignature,
  coordinateSignature,
  pathwaysComparable,
  absoluteBarrierKcal,
  referenceComparable,
  absoluteBarrierCell,
  maxEnergyEh,
} from "./compare";

const HARTREE_TO_KCAL = 627.5094740631;

/** Build a scan profile from (coordinate, act-Eh) pairs; scf mirrors act unless given. */
function scan(
  pts: [number, number][],
  opts: Partial<Pick<ScanProfileJson, "kind" | "atoms" | "coordinate_unit">> = {},
): ScanProfileJson {
  return {
    kind: opts.kind ?? "B",
    atoms: opts.atoms ?? [0, 1],
    coordinate_unit: opts.coordinate_unit ?? "Å",
    points: pts.map(([coordinate, e]) => ({
      coordinate,
      energy_act_eh: e,
      energy_scf_eh: e,
    })),
  };
}

// An EXOTHERMIC profile scanned reactant→product PAST the barrier into a lower product:
// reactant complex -100.05 (point 0), approx-TS -100.00 (mid), product -100.09 (last, the
// GLOBAL min). The forward intrinsic is E(max) − reactant-complex, NOT E(max) − product.
const SI = scan([
  [2.5, -100.05], // reactant-side min (encounter complex)
  [2.0, -100.02],
  [1.7, -100.0], // max (approx TS)
  [1.5, -100.09], // product — GLOBAL min, but the WRONG reference for the forward barrier
]);

describe("intrinsicBarrierKcal (C-intrinsic)", () => {
  it("HEADLINE (exothermic): forward barrier from the reactant-side min, NOT the global (product) min", () => {
    // max = -100.00, reactant-side min = -100.05 (point 0) → forward 0.05 Eh.
    // The OLD global-min behaviour would give -100.00 − (-100.09) = 0.09 Eh (the REVERSE
    // barrier) — this test pins the forward value so a regression to global-min is caught.
    expect(intrinsicBarrierKcal(SI)).toBeCloseTo(0.05 * HARTREE_TO_KCAL, 6);
    // Guard the primitives directly too: argmax is the mid TS point, and the reactant-side
    // min excludes the lower product.
    expect(argMaxIndex(SI)).toBe(2);
    expect(reactantSideMinEh(SI)).toBeCloseTo(-100.05, 10);
  });

  it("endothermic / monotonic-uphill: reactant-side min == global min (point 0) → fix is a no-op", () => {
    // Reactant complex is the global minimum; product sits ABOVE it. Old and new agree.
    const ENDO = scan([
      [2.5, -100.09], // reactant complex = global min
      [2.0, -100.04],
      [1.7, -100.0], // max (approx TS)
      [1.5, -100.02], // product, higher than the reactant
    ]);
    expect(reactantSideMinEh(ENDO)).toBeCloseTo(-100.09, 10);
    expect(intrinsicBarrierKcal(ENDO)).toBeCloseTo(0.09 * HARTREE_TO_KCAL, 6);
  });

  it("degenerate: max at the first point (monotonic downhill) → intrinsic 0", () => {
    const DOWNHILL = scan([
      [2.5, -100.0], // max IS point 0
      [2.0, -100.03],
      [1.5, -100.09],
    ]);
    expect(argMaxIndex(DOWNHILL)).toBe(0);
    expect(reactantSideMinEh(DOWNHILL)).toBeCloseTo(-100.0, 10);
    expect(intrinsicBarrierKcal(DOWNHILL)).toBeCloseTo(0, 9);
  });
});

describe("deltaDeltaEKcal (C-symmetry-zero)", () => {
  it("is exactly 0 for two identical profiles (self vs self)", () => {
    expect(deltaDeltaEKcal(SI, SI)).toBe(0);
  });

  it("≈ 0 for a mirror profile with the same max energy (enantiomeric si/re)", () => {
    // A mirror image (coordinates reversed, energies permuted) with the SAME max Eh
    // and tiny numerical noise on the non-max points — the physics guarantees ΔΔE‡ ≈ 0.
    const RE = scan([
      [2.5, -100.0500001],
      [2.0, -100.0199998],
      [1.7, -100.0], // same max energy as SI
      [1.5, -100.0899999],
    ]);
    expect(Math.abs(deltaDeltaEKcal(SI, RE))).toBeLessThan(1e-3);
  });

  it("is non-zero and correctly signed when the maxima genuinely differ", () => {
    // RE's maximum is 0.01 Eh HIGHER than SI's → ΔΔE‡ = E(max_SI) − E(max_RE) < 0.
    const RE = scan([
      [2.5, -100.05],
      [2.0, -100.02],
      [1.7, -100.01], // max is -100.01 (higher energy is -100.00 vs -100.01 → SI max higher)
      [1.5, -100.09],
    ]);
    // SI max = -100.00, RE max = -100.01 → ΔΔE‡ = (-100.00) − (-100.01) = +0.01 Eh
    expect(deltaDeltaEKcal(SI, RE)).toBeCloseTo(0.01 * HARTREE_TO_KCAL, 6);
    // ...and antisymmetric under swap.
    expect(deltaDeltaEKcal(RE, SI)).toBeCloseTo(-0.01 * HARTREE_TO_KCAL, 6);
  });
});

describe("methodSignature", () => {
  it("keeps method identity, drops run-type / SCF-conv / print / PAL", () => {
    const sig = methodSignature("! B3LYP def2-SVP D3BJ TightSCF Opt PAL4 LargePrint");
    expect(sig.tokens).toEqual(["B3LYP", "D3BJ", "DEF2-SVP"]);
  });

  it("detects SMD from a %cpcm smd true block", () => {
    const input = "! B3LYP def2-SVP Opt\n%cpcm\n  smd true\n  SMDsolvent \"water\"\nend";
    expect(methodSignature(input).tokens).toContain("SMD");
  });

  it("is order-insensitive across the ! line", () => {
    expect(methodSignature("! def2-SVP B3LYP").display).toBe(
      methodSignature("! B3LYP def2-SVP").display,
    );
  });
});

describe("coordinateSignature", () => {
  it("captures kind + atoms (order) + unit", () => {
    expect(coordinateSignature(SI)).toBe("B 0-1 Å");
    expect(coordinateSignature(scan([[1, 0]], { atoms: [1, 0] }))).toBe("B 1-0 Å");
  });
});

describe("pathwaysComparable (C-guard-refuses)", () => {
  const inA = "! B3LYP def2-SVP D3BJ Opt";
  const scA = SI;

  it("returns ok when method AND coordinate match", () => {
    expect(pathwaysComparable(inA, "! B3LYP def2-SVP D3BJ Opt", scA, SI)).toEqual({ ok: true });
    // run-type / PAL differences do not break comparability
    expect(pathwaysComparable(inA, "! B3LYP def2-SVP D3BJ TightOpt PAL8", scA, SI).ok).toBe(true);
  });

  it("refuses with a reason on a method mismatch (different functional/basis)", () => {
    const r = pathwaysComparable(inA, "! PBE0 def2-TZVP D3BJ Opt", scA, SI);
    expect(r.ok).toBe(false);
    if (!r.ok) expect(r.reason).toMatch(/methods differ/);
  });

  it("refuses with a reason on a solvation mismatch (SMD vs gas)", () => {
    const gas = "! B3LYP def2-SVP D3BJ Opt";
    const smd = "! B3LYP def2-SVP D3BJ Opt\n%cpcm\n smd true\nend";
    const r = pathwaysComparable(gas, smd, scA, SI);
    expect(r.ok).toBe(false);
    if (!r.ok) expect(r.reason).toMatch(/methods differ/);
  });

  it("refuses with a reason on a different scan coordinate (checked first)", () => {
    const other = scan([[1, 0], [2, 0]], { atoms: [2, 5] });
    const r = pathwaysComparable(inA, inA, scA, other);
    expect(r.ok).toBe(false);
    if (!r.ok) expect(r.reason).toMatch(/different scan coordinate/);
  });
});

describe("absoluteBarrierKcal (C-absolute-barrier)", () => {
  it("= (max − ref)·627.509 on known inputs", () => {
    // E(max) = -100.00, E(ref) = -100.20 (separated reactants lower than the TS by 0.20 Eh).
    expect(absoluteBarrierKcal(-100.0, -100.2)).toBeCloseTo(0.2 * HARTREE_TO_KCAL, 6);
  });

  it("composes with maxEnergyEh on a real profile: barrier vs a summed reference", () => {
    // SI's max is -100.00 Eh; a two-job separated reference summing to -100.15 Eh.
    const refEh = -60.05 + -40.1; // = -100.15
    expect(absoluteBarrierKcal(maxEnergyEh(SI), refEh)).toBeCloseTo(0.15 * HARTREE_TO_KCAL, 6);
  });
});

describe("referenceComparable (C-ref-method-mismatch)", () => {
  const pathSig = methodSignature("! r2SCAN-3c Opt").display;

  it("ok when every reference job shares the pathway method", () => {
    // r2SCAN-3c substrate + r2SCAN-3c reagent, run-type/PAL differences ignored.
    expect(
      referenceComparable(["! r2SCAN-3c Opt", "! r2SCAN-3c Opt TightSCF PAL8"], pathSig),
    ).toEqual({ ok: true });
  });

  it("refuses with a reason when a reference job's method differs (B3LYP ref vs r2SCAN-3c scan)", () => {
    const r = referenceComparable(["! r2SCAN-3c Opt", "! B3LYP def2-SVP Opt"], pathSig);
    expect(r.ok).toBe(false);
    // The bite: a compute-anyway path would subtract mismatched energies and show a number.
    if (!r.ok) expect(r.reason).toMatch(/reference method differs/);
  });

  it("refuses on a solvation mismatch (SMD reference vs gas-phase pathway)", () => {
    const smdRef = "! r2SCAN-3c Opt\n%cpcm\n smd true\nend";
    const r = referenceComparable([smdRef], pathSig);
    expect(r.ok).toBe(false);
    if (!r.ok) expect(r.reason).toMatch(/reference method differs/);
  });

  it("an empty reference is vacuously ok (completeness is a separate check)", () => {
    expect(referenceComparable([], pathSig)).toEqual({ ok: true });
  });
});

describe("absoluteBarrierCell (C-incomplete-no-number + honest-or-absent)", () => {
  const pathSig = methodSignature("! r2SCAN-3c Opt").display;
  const refInputs = ["! r2SCAN-3c Opt", "! r2SCAN-3c Opt"];

  it("shows a number when the reference is present, complete AND method-consistent", () => {
    const cell = absoluteBarrierCell(-100.0, -100.2, refInputs, pathSig, 2);
    expect("kcal" in cell).toBe(true);
    if ("kcal" in cell) expect(cell.kcal).toBeCloseTo(0.2 * HARTREE_TO_KCAL, 6);
  });

  it("C-incomplete-no-number: energyEh === null → a reason, NO number (never treats null as 0)", () => {
    // The bite: a version doing absoluteBarrierKcal(max, refEnergyEh ?? 0) would return a
    // (wrong, ~ +62751 kcal/mol) number here; this asserts the cell carries a reason instead.
    const cell = absoluteBarrierCell(-100.0, null, refInputs, pathSig, 2);
    expect("kcal" in cell).toBe(false);
    if ("reason" in cell) expect(cell.reason).toMatch(/incomplete/);
  });

  it("no reference set → a reason (the C2b-1 'needs a reference' state), not a number", () => {
    const cell = absoluteBarrierCell(-100.0, null, [], pathSig, 0);
    expect("kcal" in cell).toBe(false);
    if ("reason" in cell) expect(cell.reason).toMatch(/no reactant reference/);
  });

  it("method mismatch → the guard reason, not a number (even when complete)", () => {
    const cell = absoluteBarrierCell(-100.0, -100.2, ["! B3LYP def2-SVP Opt"], pathSig, 1);
    expect("kcal" in cell).toBe(false);
    if ("reason" in cell) expect(cell.reason).toMatch(/reference method differs/);
  });
});
