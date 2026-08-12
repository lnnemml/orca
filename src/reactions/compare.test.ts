import { describe, it, expect } from "vitest";

import type { ScanProfileJson, NebResults } from "../types";
import { HARTREE_TO_KCAL } from "../units";
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
  referenceStoichiometryOk,
  absoluteBarrierCell,
  maxEnergyEh,
  isLocatedTsInput,
  deltaGDoubleDaggerKcal,
  locatedBarrierEKcal,
  deltaDeltaGKcal,
  nebMepCurve,
  normalizedScanCurve,
} from "./compare";

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

  // --- Opt-convergence presets are method-neutral (the r1-gate defect) ---------
  // A real scan uses `! … LooseOpt` for the floppy-complex convergence; the reference
  // jobs are `! … Opt`. Same electronic-structure method → the absolute barrier must be
  // allowed, not refused as "method differs".
  it("LooseOpt scan is comparable to Opt / TightOpt references (opt presets neutralized)", () => {
    const looseSig = methodSignature("! r2SCAN-3c SMD(DMF) TightSCF LooseOpt").display;
    // vs a plain-Opt reference, and vs a TightOpt reference — both the SAME method.
    expect(
      referenceComparable(
        ["! r2SCAN-3c SMD(DMF) TightSCF Opt", "! r2SCAN-3c SMD(DMF) TightSCF TightOpt PAL8"],
        looseSig,
      ),
    ).toEqual({ ok: true });
    // And the signatures themselves are equal across the whole opt-preset family.
    expect(methodSignature("! r2SCAN-3c LooseOpt").display).toBe(
      methodSignature("! r2SCAN-3c NormalOpt").display,
    );
    expect(methodSignature("! r2SCAN-3c LooseOpt").display).toBe(
      methodSignature("! r2SCAN-3c VeryTightOpt").display,
    );
  });

  it("does NOT over-broaden: a functional difference still refuses under a LooseOpt scan", () => {
    // The guard-against-over-broadening (r3 depends on it): LooseOpt is neutralized, but
    // B3LYP vs r2SCAN-3c is a genuine method difference and MUST still refuse.
    const looseSig = methodSignature("! r2SCAN-3c SMD(DMF) LooseOpt").display;
    const r = referenceComparable(["! B3LYP def2-SVP D4 SMD(DMF) Opt"], looseSig);
    expect(r.ok).toBe(false);
    if (!r.ok) expect(r.reason).toMatch(/reference method differs/);
  });
});

// --- Coordinate-block fixtures for the stoichiometry guard -------------------
// Methylamine (CH3NH2 = C1 N1 H5, 7 atoms), ethyl iodide (C2H5I = C2 H5 I, 8 atoms),
// and their 15-atom reacting complex (C3 H10 I N) as ONE `* xyz` block (the merged scan
// geometry). Coordinates are arbitrary — the parser reads element + charge only.
const METHYLAMINE =
  "! r2SCAN-3c SMD(DMF) Opt\n* xyz 0 1\nC 0 0 0\nN 1.47 0 0\nH -0.5 0.9 0\nH -0.5 -0.9 0\nH -0.5 0 0.9\nH 1.9 0.8 0\nH 1.9 -0.8 0\n*\n";
const ETI =
  "! r2SCAN-3c SMD(DMF) Opt\n* xyz 0 1\nC 0 0 0\nC 1.5 0 0\nI 3.6 0 0\nH -0.5 0.9 0\nH -0.5 -0.9 0\nH -0.5 0 0.9\nH 1.9 0.9 0\nH 1.9 -0.9 0\n*\n";
const COMPLEX =
  "! r2SCAN-3c SMD(DMF) LooseOpt\n* xyz 0 1\n" +
  "C 0 0 0\nN 1.47 0 0\nH -0.5 0.9 0\nH -0.5 -0.9 0\nH -0.5 0 0.9\nH 1.9 0.8 0\nH 1.9 -0.8 0\n" +
  "C 5 0 0\nC 6.5 0 0\nI 8.6 0 0\nH 4.5 0.9 0\nH 4.5 -0.9 0\nH 4.5 0 0.9\nH 6.9 0.9 0\nH 6.9 -0.9 0\n*\n";

describe("referenceStoichiometryOk (composition + charge balance)", () => {
  it("HEADLINE (the r2 defect): EtI-only reference does NOT sum to the complex → refuse", () => {
    const r = referenceStoichiometryOk(COMPLEX, [ETI]);
    expect(r.ok).toBe(false);
    // Names the imbalance (C2H5I reference vs C3H10IN complex).
    if (!r.ok) {
      expect(r.reason).toMatch(/C2H5I/);
      expect(r.reason).toMatch(/C3H10IN/);
    }
  });

  it("two references summing to the complex (methylamine + EtI = 15 atoms) → ok", () => {
    expect(referenceStoichiometryOk(COMPLEX, [METHYLAMINE, ETI])).toEqual({ ok: true });
  });

  it("a single reference that IS the whole complex → ok (the pre-reaction-complex mode)", () => {
    expect(referenceStoichiometryOk(COMPLEX, [COMPLEX])).toEqual({ ok: true });
  });

  it("charge imbalance → refuse (guards the ionic case) even when atoms balance", () => {
    // Same atoms as the complex, but the reference carries charge −1 vs the complex's 0.
    const anionComplex = COMPLEX.replace("* xyz 0 1", "* xyz -1 1");
    const r = referenceStoichiometryOk(COMPLEX, [anionComplex]);
    expect(r.ok).toBe(false);
    if (!r.ok) expect(r.reason).toMatch(/charge/);
  });

  it("unreadable complex (no coordinate block) → refuse (cannot verify)", () => {
    const r = referenceStoichiometryOk("! r2SCAN-3c Opt", [ETI]);
    expect(r.ok).toBe(false);
    if (!r.ok) expect(r.reason).toMatch(/cannot verify complex/);
  });
});

describe("absoluteBarrierCell (C-incomplete-no-number + honest-or-absent + stoichiometry)", () => {
  const pathSig = methodSignature("! r2SCAN-3c SMD(DMF) Opt").display;
  const refInputs = [METHYLAMINE, ETI]; // balanced against COMPLEX

  it("shows a number when the reference is present, complete, method-consistent AND balanced", () => {
    const cell = absoluteBarrierCell(-100.0, -100.2, refInputs, pathSig, 2, COMPLEX);
    expect("kcal" in cell).toBe(true);
    if ("kcal" in cell) expect(cell.kcal).toBeCloseTo(0.2 * HARTREE_TO_KCAL, 6);
  });

  it("HEADLINE (r2): a composition-mismatched reference (EtI only) is REFUSED, not the −60127 number", () => {
    // The −60127 kcal/mol garbage case: E(max) − E(EtI) ≈ −E(methylamine). Now a reason.
    const cell = absoluteBarrierCell(-100.0, -108.0, [ETI], pathSig, 1, COMPLEX);
    expect("kcal" in cell).toBe(false);
    if ("reason" in cell) {
      expect(cell.reason).toMatch(/do not sum to the reacting complex/);
      expect(cell.reason).toMatch(/C2H5I/);
    }
  });

  it("C-incomplete-no-number: energyEh === null → a reason, NO number (never treats null as 0)", () => {
    const cell = absoluteBarrierCell(-100.0, null, refInputs, pathSig, 2, COMPLEX);
    expect("kcal" in cell).toBe(false);
    if ("reason" in cell) expect(cell.reason).toMatch(/incomplete/);
  });

  it("no reference set → a reason (the C2b-1 'needs a reference' state), not a number", () => {
    const cell = absoluteBarrierCell(-100.0, null, [], pathSig, 0, COMPLEX);
    expect("kcal" in cell).toBe(false);
    if ("reason" in cell) expect(cell.reason).toMatch(/no reactant reference/);
  });

  it("method mismatch fires BEFORE the stoichiometry check (order preserved)", () => {
    const cell = absoluteBarrierCell(-100.0, -100.2, ["! B3LYP def2-SVP Opt"], pathSig, 1, COMPLEX);
    expect("kcal" in cell).toBe(false);
    if ("reason" in cell) expect(cell.reason).toMatch(/reference method differs/);
  });
});

// ── Located-TS barriers (Stage E1b): ΔG‡ / located ΔE‡ / ΔΔG‡ ────────────────────
describe("isLocatedTsInput (G-isLocatedTs)", () => {
  it("an OptTS input is a located TS; a plain Opt / scan input is NOT", () => {
    expect(isLocatedTsInput("! r2SCAN-3c OptTS Freq SMD(DMF) TightSCF")).toBe(true);
    // Bite: matching a bare "Opt" substring would misfire on a plain Opt job — assert OptTS
    // SPECIFICALLY. A geometry optimization and a relaxed scan are not transition states.
    expect(isLocatedTsInput("! r2SCAN-3c Opt Freq")).toBe(false);
    expect(isLocatedTsInput("! r2SCAN-3c LooseOpt SMD(DMF) TightSCF")).toBe(false);
    // Case-insensitive, and only on the `!` line (a stray "optts" in a comment/coords is ignored).
    expect(isLocatedTsInput("! R2SCAN-3C OPTTS FREQ")).toBe(true);
    expect(isLocatedTsInput("! r2SCAN-3c Opt\n* xyz 0 1\nC 0 0 0\n*")).toBe(false);
  });
});

describe("deltaGDoubleDaggerKcal / locatedBarrierEKcal (G-honest-absent)", () => {
  it("computes ΔG‡ = (G(TS) − ΣG(ref))·627.509 when both are present", () => {
    // Bimolecular: G(TS) − ΣG(reactants) is a POSITIVE barrier (association entropy already in G).
    expect(deltaGDoubleDaggerKcal(-100.0, -100.2)).toBeCloseTo(0.2 * HARTREE_TO_KCAL, 6);
    expect(locatedBarrierEKcal(-100.0, -100.25)).toBeCloseTo(0.25 * HARTREE_TO_KCAL, 6);
  });

  it("any null input → null (never treats a missing G as 0 — the fabricated-barrier bite)", () => {
    // A version that read null as 0 would emit e.g. -100·627.509 — a garbage barrier.
    expect(deltaGDoubleDaggerKcal(null, -100.2)).toBeNull();
    expect(deltaGDoubleDaggerKcal(-100.0, null)).toBeNull();
    expect(deltaGDoubleDaggerKcal(null, null)).toBeNull();
    expect(locatedBarrierEKcal(null, -100.2)).toBeNull();
    expect(locatedBarrierEKcal(-100.0, null)).toBeNull();
    expect(deltaDeltaGKcal(null, -100.0)).toBeNull();
    expect(deltaDeltaGKcal(-100.0, null)).toBeNull();
  });
});

describe("deltaDeltaGKcal (G-ddg-reference-free)", () => {
  it("ΔΔG‡ = G(TS_A) − G(TS_B), reading NO reference (the reactants cancel)", () => {
    const gA = -543.21;
    const gB = -543.19;
    // The value is PURELY the two TS Gibbs energies — the function has no reference parameter, so a
    // reactant sum cannot leak in (a version that subtracted one would double-count and differ).
    expect(deltaDeltaGKcal(gA, gB)).toBeCloseTo((gA - gB) * HARTREE_TO_KCAL, 6);
    // Antisymmetry + symmetry-zero (mirrors deltaDeltaEKcal).
    expect(deltaDeltaGKcal(gB, gA)).toBeCloseTo(-(gA - gB) * HARTREE_TO_KCAL, 6);
    expect(deltaDeltaGKcal(gA, gA)).toBe(0);
  });
});

/** A NEB result with a given MEP (distance, RELATIVE energy image0=0) + converged TS. */
function neb(mep: [number, number][], tsEnergyEh: number | null): NebResults {
  return {
    iterations: [],
    mep: mep.map(([distance_angstrom, energy_eh]) => ({ distance_angstrom, energy_eh })),
    final_barrier_eh: null,
    ts_geometry: { elements: ["C", "N", "H"], xyz_angstrom: [[0, 0, 0], [1, 0, 0], [2, 0, 0]] },
    ts_energy_eh: tsEnergyEh,
  };
}

describe("nebMepCurve (N4 — normalized MEP shape)", () => {
  it("neb_mep_curve_normalizes_arc_to_0_1", () => {
    // BITE: arc length / last-point → x ∈ [0,1]; energy (already relative) → kcal/mol.
    const curve = nebMepCurve(neb([[0, 0], [0.5, 0.01], [1.2, 0.02], [2.0, 0.005]], -93.3246));
    expect(curve.map((p) => p.x)).toEqual([0, 0.25, 0.6, 1.0]);
    expect(curve[0].energyKcal).toBe(0); // image 0 is exactly 0 (already relative)
    expect(curve[2].energyKcal).toBeCloseTo(0.02 * HARTREE_TO_KCAL, 9);
  });

  it("neb_mep_curve_empty_for_degenerate_band", () => {
    // BITE: a 1-point (or zero-length) MEP, or a zero total arc, is [] — never a divide-by-
    // zero or a single dot posing as a path.
    expect(nebMepCurve(neb([[0, 0]], -93.0))).toEqual([]);
    expect(nebMepCurve(neb([], -93.0))).toEqual([]);
    expect(nebMepCurve(neb([[0, 0], [0, 0.01]], -93.0))).toEqual([]); // zero total arc
  });
});

describe("normalizedScanCurve (N4 — a scan on the same 0→1 axis, mixed overlay)", () => {
  it("maps coordinate min→max to 0→1 and energy relative to the reactant-side min", () => {
    // A 3-point scan: coord 2.5→1.5, max energy in the middle. x normalizes min→max.
    const s = scan([[2.5, -100.05], [2.0, -100.0], [1.5, -100.03]]);
    const curve = normalizedScanCurve(s);
    expect(curve.map((p) => p.x)).toEqual([1.0, 0.5, 0.0]); // (2.5−1.5)/1.0, (2.0−1.5)/1.0, 0
    // Zero = the reactant-side minimum (−100.05, the pre-barrier branch), so its ΔE is 0.
    const zero = reactantSideMinEh(s);
    expect(zero).toBe(-100.05);
    expect(curve[0].energyKcal).toBeCloseTo(0, 9);
    expect(curve[1].energyKcal).toBeCloseTo((-100.0 - -100.05) * HARTREE_TO_KCAL, 9);
  });

  it("degenerate scan (<2 points or all-equal coordinate) → []", () => {
    expect(normalizedScanCurve(scan([[1.5, -100]]))).toEqual([]);
    expect(normalizedScanCurve(scan([[1.5, -100], [1.5, -100.1]]))).toEqual([]); // zero span
  });
});

describe("N4 — a NEB estimate is guarded like a scan (honesty invariants)", () => {
  it("neb_estimate_refuses_gibbs_gives_electronic", () => {
    // The G1 estimate: eEh = the converged NEB-TS energy, gEh = null (no Freq). ΔG‡ is refused
    // for free (the null-guard), while the electronic ΔE‡ from the same eEh still stands.
    const tsEh = -93.3246;
    const refEh = -93.45; // Σ E(reactant refs) on the same scale
    // BITE: a version that read null-G as 0 would emit a garbage ΔG‡; it must be null.
    expect(deltaGDoubleDaggerKcal(null, -100.2)).toBeNull();
    // …and the electronic barrier from the SAME estimate energy is a real number.
    expect(locatedBarrierEKcal(tsEh, refEh)).toBeCloseTo((tsEh - refEh) * HARTREE_TO_KCAL, 9);
  });

  it("method_guard_flags_xtb_neb_vs_dft_scan", () => {
    // BITE: a xtb-NEB estimate must NOT be silently ΔΔ-compared against a DFT scan — the SAME
    // methodSignature guard that governs scan↔scan flags the cross-method mismatch.
    const xtbNeb = methodSignature("! XTB NEB-TS");
    const dftScan = methodSignature("! r2SCAN-3c def2-TZVP Opt");
    expect(xtbNeb.display).not.toBe(dftScan.display);
    // And it is comparable to another DFT pathway on the same method (the guard is symmetric).
    expect(methodSignature("! r2SCAN-3c def2-TZVP NEB-TS").display).toBe(dftScan.display);
  });
});
