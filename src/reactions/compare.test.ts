import { describe, it, expect } from "vitest";

import type { ScanProfileJson } from "../types";
import {
  deltaDeltaEKcal,
  intrinsicBarrierKcal,
  methodSignature,
  coordinateSignature,
  pathwaysComparable,
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

// A profile with a genuine barrier: min at the ends is lower than a mid maximum.
const SI = scan([
  [2.5, -100.05],
  [2.0, -100.02],
  [1.7, -100.0], // max (approx TS)
  [1.5, -100.09], // min (product side)
]);

describe("intrinsicBarrierKcal (C-intrinsic)", () => {
  it("= (max − min)·627.509 on a known profile", () => {
    // max = -100.00, min = -100.09 → 0.09 Eh
    expect(intrinsicBarrierKcal(SI)).toBeCloseTo(0.09 * HARTREE_TO_KCAL, 6);
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
