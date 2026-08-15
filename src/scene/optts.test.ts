import { describe, it, expect } from "vitest";

import { buildOptTSInput, type TsGuessGeometry } from "./optts";
import { DEFAULT_BUILDER_STATE, type MethodSlice } from "../input-builder/build-input";

/** A full method slice = the defaults; each test spreads the fields it overrides on top. */
const BASE_SLICE: MethodSlice = {
  methodFamily: DEFAULT_BUILDER_STATE.methodFamily,
  composite: DEFAULT_BUILDER_STATE.composite,
  functional: DEFAULT_BUILDER_STATE.functional,
  basis: DEFAULT_BUILDER_STATE.basis,
  dispersion: DEFAULT_BUILDER_STATE.dispersion,
  ri: DEFAULT_BUILDER_STATE.ri,
  xtbMethod: DEFAULT_BUILDER_STATE.xtbMethod,
  wavefunction: DEFAULT_BUILDER_STATE.wavefunction,
};

/** A tiny N···C seed (the SN2 forming bond), easy to eyeball. */
const SEED: TsGuessGeometry = {
  elements: ["N", "C"],
  xyz_angstrom: [
    [0, 0, 0],
    [0, 0, 2.353],
  ],
};

/** The `* xyz <c> <m>` header line of a built input. */
function xyzHeader(input: string): string {
  return input.split(/\r?\n/).find((l) => l.trim().startsWith("* xyz"))!.trim();
}
/** The `!` keyword line. */
function keywordLine(input: string): string {
  return input.split(/\r?\n/).find((l) => l.startsWith("!"))!;
}

// ── C-charge-inherited — THE footgun: (c,m) come from the source, never a 0 default ──
describe("buildOptTSInput — charge/multiplicity inheritance (THE footgun)", () => {
  it("carries an anion's `-1` from source to child (BH₄⁻ class), never `0 1`", () => {
    const source = "! r2SCAN-3c OptTS SMD(DMF)\n* xyz -1 1\nB 0 0 0\nH 1 1 1\n*\n";
    const child = buildOptTSInput(source, SEED);
    expect(xyzHeader(child)).toBe("* xyz -1 1");
    expect(child).not.toContain("* xyz 0 1");
  });

  it("carries a radical multiplicity `0 2` from source to child", () => {
    const source = "! r2SCAN-3c LooseOpt\n* xyz 0 2\nO 0 0 0\nH 1 0 0\n*\n";
    expect(xyzHeader(buildOptTSInput(source, SEED))).toBe("* xyz 0 2");
  });

  it("throws (never defaults to 0) when the source has no inline `* xyz` block", () => {
    // Bite: a version that defaulted charge to 0 would emit a `* xyz 0 1` child here.
    expect(() => buildOptTSInput("! r2SCAN-3c LooseOpt\n* xyzfile 0 1 geo.xyz\n", SEED)).toThrow(
      /coordinate block/i,
    );
    expect(() => buildOptTSInput("! r2SCAN-3c\n", SEED)).toThrow(/coordinate block/i);
  });
});

// ── C-optts-freq-calchess — the OptTS recipe; the source's opt/Scan MUST NOT leak ──
describe("buildOptTSInput — OptTS + Freq + TightSCF + Calc_Hess; no Scan/LooseOpt leak", () => {
  // A realistic scan source: LooseOpt + a %geom Scan block — both must be gone from the child.
  const scanSource =
    "! r2SCAN-3c LooseOpt SMD(DMF) TightSCF\n" +
    "%geom Scan B 0 1 = 3.000, 1.800, 12 end end\n" +
    "%pal nprocs 4 end\n%maxcore 2000\n\n* xyz 0 1\nN 0 0 0\nC 0 0 1.8\n*\n";

  it("emits `! … OptTS Freq … TightSCF` and a `%geom Calc_Hess true` block", () => {
    const child = buildOptTSInput(scanSource, SEED);
    const kw = keywordLine(child);
    expect(kw).toContain("OptTS");
    expect(kw).toContain("Freq");
    expect(kw).toContain("TightSCF");
    expect(child).toMatch(/%geom\s+Calc_Hess\s+true\s+end/i);
  });

  it("does NOT carry the source's opt keyword or Scan block (bite: append keeps LooseOpt)", () => {
    const child = buildOptTSInput(scanSource, SEED);
    // Bite: a naive "append OptTS to the source `!` line" would still contain LooseOpt; and a
    // "reuse the source verbatim" would still contain the Scan block. buildOrcaInput builds fresh.
    expect(child).not.toContain("LooseOpt");
    expect(child).not.toMatch(/%geom\s+Scan/i);
    expect(child).not.toContain("Scan B");
  });
});

// ── C-method-inherited — method+solvation default to the source's (comparable by construction) ──
describe("buildOptTSInput — method + solvation inherited from the source (comparability)", () => {
  it("r2SCAN-3c + SMD(DMF) in → same out (not a hardcoded default)", () => {
    const source = "! r2SCAN-3c LooseOpt SMD(DMF) TightSCF\n* xyz 0 1\nN 0 0 0\nC 0 0 1.8\n*\n";
    const kw = keywordLine(buildOptTSInput(source, SEED));
    expect(kw).toContain("r2SCAN-3c");
    expect(kw).toContain("SMD(DMF)");
  });

  it("a DIFFERENT source method+solvation is inherited, not overridden (bite: hardcoded r2SCAN-3c)", () => {
    // Bite: reopt.ts defaults to r2SCAN-3c; an OptTS engine that did the same would fail here.
    const source =
      "! B3LYP def2-TZVP def2/J RIJCOSX D4 SMD(water) Opt TightSCF\n* xyz 0 1\nN 0 0 0\nC 0 0 1.8\n*\n";
    const kw = keywordLine(buildOptTSInput(source, SEED));
    expect(kw).toContain("B3LYP");
    expect(kw).toContain("def2-TZVP");
    expect(kw).toContain("SMD(water)");
    expect(kw).not.toContain("r2SCAN-3c");
  });

  it("options.method / options.solvation override the inherited defaults", () => {
    const source = "! r2SCAN-3c SMD(DMF) LooseOpt\n* xyz 0 1\nN 0 0 0\nC 0 0 1.8\n*\n";
    const kw = keywordLine(
      buildOptTSInput(source, SEED, { method: "wB97X-3c", solvation: "SMD(acetonitrile)" }),
    );
    expect(kw).toContain("wB97X-3c");
    expect(kw).toContain("SMD(acetonitrile)");
    expect(kw).not.toContain("r2SCAN-3c");
    expect(kw).not.toContain("SMD(DMF)");
  });
});

// ── seed geometry preserved (count + order) ──────────────────────────────────────
describe("buildOptTSInput — the seed geometry is emitted exactly (count + order)", () => {
  it("emits exactly the seed's atoms, in its order", () => {
    const seed: TsGuessGeometry = {
      elements: ["C", "N", "I"],
      xyz_angstrom: [
        [0, 0, 0],
        [0, 0, 2.35],
        [0, 0, -2.59],
      ],
    };
    const source = "! r2SCAN-3c LooseOpt\n* xyz 0 1\nC 0 0 0\n*\n";
    const child = buildOptTSInput(source, seed);
    const rows = child.split(/\r?\n/).filter((l) => /^[A-Z][a-z]?\s/.test(l.trim()));
    expect(rows).toHaveLength(3);
    expect(rows.map((r) => r.trim().split(/\s+/)[0])).toEqual(["C", "N", "I"]);
  });

  it("throws on an elements/coordinate length mismatch (rule #9)", () => {
    const bad: TsGuessGeometry = { elements: ["N", "C", "H"], xyz_angstrom: [[0, 0, 0]] };
    const source = "! r2SCAN-3c\n* xyz 0 1\nN 0 0 0\n*\n";
    expect(() => buildOptTSInput(source, bad)).toThrow();
  });
});

// ── C-method-override — a <MethodPicker> methodState overrides the inherited method ──
// The XTB-scan → XTB-OptTS pain this unit fixes: a refine can now run at a chosen level.
describe("buildOptTSInput — methodState override (family model, not a flattened string)", () => {
  const xtbScan = "! XTB LooseOpt\n%pal nprocs 4 end\n%maxcore 2000\n\n* xyz 0 1\nN 0 0 0\nC 0 0 1.8\n*\n";

  // BITE 1 — inherit-default is byte-identical: an absent/empty/undefined methodState must
  // produce EXACTLY today's output. A regression in the default path fails this three ways.
  it("inherit_default_is_byte_identical — no methodState → unchanged output", () => {
    const source = "! r2SCAN-3c LooseOpt SMD(DMF) TightSCF\n* xyz 0 1\nN 0 0 0\nC 0 0 1.8\n*\n";
    const baseline = buildOptTSInput(source, SEED);
    // The three "no override" spellings must all equal the baseline, byte for byte.
    expect(buildOptTSInput(source, SEED, {})).toBe(baseline);
    expect(buildOptTSInput(source, SEED, { methodState: undefined })).toBe(baseline);
    // And the baseline itself is pinned — the composite slot carries method + solvation verbatim.
    expect(baseline).toMatchInlineSnapshot(`
      "! r2SCAN-3c SMD(DMF) OptTS Freq TightSCF
      %pal nprocs 4 end
      %maxcore 2000
      %geom Calc_Hess true end

      * xyz 0 1
      N     0.00000000    0.00000000    0.00000000
      C     0.00000000    0.00000000    2.35300000
      *
      "
    `);
  });

  // BITE 2 — a composite override (r2SCAN-3c) emits a self-contained `!` line: no basis, no RI.
  // Fixes XTB-scan → the child runs r2SCAN-3c, not XTB.
  it("composite_override — r2SCAN-3c replaces XTB, no basis/RI leaks in", () => {
    const kw = keywordLine(
      buildOptTSInput(xtbScan, SEED, { methodState: { ...BASE_SLICE, methodFamily: "composite", composite: "r2SCAN-3c" } }),
    );
    expect(kw).toContain("r2SCAN-3c");
    expect(kw).toContain("OptTS");
    expect(kw).not.toContain("XTB");
    expect(kw).not.toContain("def2");
    expect(kw).not.toContain("RIJCOSX");
  });

  // BITE 3 — a DFT override pairs the RI aux via buildOrcaInput's dft branch. A string-flatten
  // impl (jamming "B3LYP def2-TZVP" into the composite slot) emits NO aux → this goes red.
  it("dft_override_pairs_ri_aux — functional+basis carries the paired def2/J aux + RIJCOSX + D4", () => {
    const kw = keywordLine(
      buildOptTSInput(xtbScan, SEED, {
        methodState: { ...BASE_SLICE, methodFamily: "dft", functional: "B3LYP", basis: "def2-TZVP", ri: "RIJCOSX", dispersion: "D4" },
      }),
    );
    expect(kw).toContain("B3LYP");
    expect(kw).toContain("def2-TZVP");
    expect(kw).toContain("def2/J"); // ← the paired Coulomb aux; a flattened string would drop it
    expect(kw).toContain("RIJCOSX");
    expect(kw).toContain("D4");
    expect(kw).not.toContain("XTB");
  });

  // Solvation still DEFAULTS to the source's under an override (comparability), carried per-family.
  it("inherits the source solvation under a dft override (emitted for dft, per-family)", () => {
    const source = "! r2SCAN-3c LooseOpt SMD(DMF) TightSCF\n* xyz 0 1\nN 0 0 0\nC 0 0 1.8\n*\n";
    const kw = keywordLine(
      buildOptTSInput(source, SEED, {
        methodState: { ...BASE_SLICE, methodFamily: "dft", functional: "B3LYP", basis: "def2-TZVP" },
      }),
    );
    expect(kw).toContain("SMD(DMF)");
    expect(kw).not.toContain("r2SCAN-3c");
  });

  // BITE 4 — the override still inherits (charge, mult) from the source AND the post-condition
  // still bites: an anion's -1 survives, and a length-mismatch seed throws under an override too.
  it("charge_mult_and_postcondition_unchanged — (c,m) inherited under an override", () => {
    const source = "! r2SCAN-3c SMD(DMF) LooseOpt\n* xyz -1 1\nB 0 0 0\nH 1 1 1\n*\n";
    const child = buildOptTSInput(source, SEED, {
      methodState: { ...BASE_SLICE, methodFamily: "dft", functional: "B3LYP", basis: "def2-TZVP" },
    });
    expect(xyzHeader(child)).toBe("* xyz -1 1");
    // The seed-preservation post-condition (rule #9) still fires under an override.
    const bad: TsGuessGeometry = { elements: ["N", "C", "H"], xyz_angstrom: [[0, 0, 0]] };
    expect(() =>
      buildOptTSInput(source, bad, { methodState: { ...BASE_SLICE, methodFamily: "dft" } }),
    ).toThrow();
  });
});
