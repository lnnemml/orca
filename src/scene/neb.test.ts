import { describe, it, expect } from "vitest";

import { buildNebInput, type NebGeometry } from "./neb";

// A reactant input (Menshutkin-shaped, 3-atom model): method + solvation, charge 0 1.
const REACTANT_INPUT = [
  "! r2SCAN-3c SMD(DMF) Opt TightSCF",
  "%maxcore 2000",
  "* xyz 0 1",
  "N 0.000000 0.0 0.0",
  "C 2.000000 0.0 0.0",
  "I 4.300000 0.0 0.0",
  "*",
].join("\n");

const REACTANT_GEOM: NebGeometry = {
  elements: ["N", "C", "I"],
  xyz_angstrom: [
    [0, 0, 0],
    [2.0, 0, 0],
    [4.3, 0, 0],
  ],
};
// Same atom order (N C I), different geometry (product basin).
const PRODUCT_GEOM: NebGeometry = {
  elements: ["N", "C", "I"],
  xyz_angstrom: [
    [0, 0, 0],
    [1.5, 0, 0],
    [5.6, 0, 0],
  ],
};

const bangLine = (inp: string) =>
  inp.split(/\r?\n/).find((l) => l.trim().startsWith("!")) ?? "";
const xyzHeader = (inp: string) =>
  inp.split(/\r?\n/).find((l) => l.trim().startsWith("* xyz")) ?? "";

describe("buildNebInput — reactant + product → NEB-TS input (Stage E3a-1)", () => {
  it("emits a NEB-TS input with the %neb block, inheriting method + charge", () => {
    const { inp, productXyz } = buildNebInput(REACTANT_INPUT, REACTANT_GEOM, PRODUCT_GEOM);
    expect(bangLine(inp)).toContain("NEB-TS");
    expect(bangLine(inp)).toContain("r2SCAN-3c");
    expect(bangLine(inp)).toContain("SMD(DMF)");
    // Multi-line %neb block (the measured, converging form).
    expect(inp).toContain('%neb\n  NEB_End_XYZFile "product.xyz"\n  NImages 8\nend');
    expect(xyzHeader(inp)).toBe("* xyz 0 1");
    // product.xyz: count line + comment + 3 atoms; carries the product C at 1.5.
    const pl = productXyz.split(/\r?\n/);
    expect(pl[0]).toBe("3");
    expect(pl[3]).toContain("C 1.5"); // count, comment, N, C(1.5), I
  });

  it("NImages is overridable", () => {
    const { inp } = buildNebInput(REACTANT_INPUT, REACTANT_GEOM, PRODUCT_GEOM, { nImages: 12 });
    expect(inp).toContain("  NImages 12\n");
  });

  it("inherits a charged reactant's charge (−1), never the 0-default (the footgun)", () => {
    const anion = REACTANT_INPUT.replace("* xyz 0 1", "* xyz -1 1");
    const { inp } = buildNebInput(anion, REACTANT_GEOM, PRODUCT_GEOM);
    expect(xyzHeader(inp)).toBe("* xyz -1 1");
  });

  it("inherits a DIFFERENT reactant method verbatim, not a default", () => {
    // BITE: a builder that hardcoded r2SCAN-3c would ignore this reactant's B3LYP.
    const b3lyp = "! B3LYP def2-SVP D3BJ Opt TightSCF\n%maxcore 2000\n* xyz 0 1\nN 0 0 0\nC 2 0 0\nI 4.3 0 0\n*\n";
    const { inp } = buildNebInput(b3lyp, REACTANT_GEOM, PRODUCT_GEOM);
    expect(bangLine(inp)).toContain("B3LYP");
    expect(bangLine(inp)).toContain("def2-SVP");
    expect(bangLine(inp)).not.toContain("r2SCAN-3c");
  });

  it("THROWS when reactant and product have DIFFERENT atom order (the guard is the point)", () => {
    // BITE: emitting anyway → NEB interpolates the wrong atoms and silently fails.
    const swapped: NebGeometry = {
      elements: ["N", "I", "C"], // C↔I swapped vs the reactant
      xyz_angstrom: PRODUCT_GEOM.xyz_angstrom,
    };
    expect(() => buildNebInput(REACTANT_INPUT, REACTANT_GEOM, swapped)).toThrow(/atom order/);
  });

  it("THROWS on a different ATOM COUNT (a truncated product)", () => {
    const short: NebGeometry = {
      elements: ["N", "C"],
      xyz_angstrom: [
        [0, 0, 0],
        [1.5, 0, 0],
      ],
    };
    expect(() => buildNebInput(REACTANT_INPUT, REACTANT_GEOM, short)).toThrow();
  });

  it("THROWS when the reactant input has no inline * xyz block (never defaults charge to 0)", () => {
    const noXyz = "! r2SCAN-3c SMD(DMF) NEB-TS\n* xyzfile 0 1 reactant.xyz\n";
    expect(() => buildNebInput(noXyz, REACTANT_GEOM, PRODUCT_GEOM)).toThrow();
  });
});
