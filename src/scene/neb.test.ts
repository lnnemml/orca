import { describe, it, expect } from "vitest";

import { buildNebInput, hasNebKeyword, type NebGeometry } from "./neb";
import {
  DEFAULT_BUILDER_STATE,
  type BuilderState,
} from "../input-builder/build-input";

/** A builder state overlaid on the defaults (the NEB level the user picked). */
function state(overrides: Partial<BuilderState> = {}): BuilderState {
  return { ...DEFAULT_BUILDER_STATE, ...overrides };
}

/** A composite (r2SCAN-3c) NEB level with DMF/SMD, TightSCF. */
const COMPOSITE_DMF = state({
  methodFamily: "composite",
  composite: "r2SCAN-3c",
  solvationModel: "SMD",
  solvent: "DMF",
  scfConv: "TightSCF",
});
/** A GFN2-xTB NEB level — self-contained (no solvation/scfConv should survive). */
const XTB_LEVEL = state({ methodFamily: "xtb", xtbMethod: "XTB" });

// A reactant input (Menshutkin-shaped, 3-atom model): the reactant's OWN `!` line is a
// plain Opt — the NEB method must come from the BuilderState, not this line. charge 0 1.
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

describe("buildNebInput — reactant + product → NEB-TS input at the builder's level", () => {
  it("emits a NEB-TS input with the %neb block, at the builder's method + reactant charge", () => {
    const { inp, productXyz } = buildNebInput(
      COMPOSITE_DMF,
      REACTANT_INPUT,
      REACTANT_GEOM,
      PRODUCT_GEOM,
    );
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
    const { inp } = buildNebInput(COMPOSITE_DMF, REACTANT_INPUT, REACTANT_GEOM, PRODUCT_GEOM, {
      nImages: 12,
    });
    expect(inp).toContain("  NImages 12\n");
  });

  it("neb_line_uses_builder_method_not_reactant", () => {
    // BITE: the NEB level is the BUILDER's (here xtb), not the reactant's r2SCAN-3c. The
    // old inherit-from-reactant impl would emit r2SCAN-3c — the NEB-on-xtb point.
    const { inp } = buildNebInput(XTB_LEVEL, REACTANT_INPUT, REACTANT_GEOM, PRODUCT_GEOM);
    expect(bangLine(inp)).toContain("XTB");
    expect(bangLine(inp)).toContain("NEB-TS");
    expect(bangLine(inp)).not.toContain("r2SCAN-3c");
    // The %neb block must survive the xtb emit (splice anchored on `* xyz`, not %maxcore).
    expect(inp).toContain("%neb");
    expect(inp).toContain('NEB_End_XYZFile "product.xyz"');
    // ...and precede the coordinate block.
    expect(inp.indexOf("%neb")).toBeLessThan(inp.indexOf("* xyz"));
  });

  it("neb_xtb_line_has_no_solvation_or_scfconv", () => {
    // BITE: the N1a self-containment guarantee holds through the NEB path — even with
    // solvation + scfConv set in the state, an xtb NEB line carries neither.
    const xtbSolv = state({
      methodFamily: "xtb",
      xtbMethod: "XTB",
      solvationModel: "SMD",
      solvent: "water",
      scfConv: "TightSCF",
    });
    const { inp } = buildNebInput(xtbSolv, REACTANT_INPUT, REACTANT_GEOM, PRODUCT_GEOM);
    expect(bangLine(inp)).toContain("XTB NEB-TS");
    expect(bangLine(inp)).not.toContain("SMD(");
    expect(bangLine(inp)).not.toContain("TightSCF");
  });

  it("neb_charge_mult_from_reactant_not_builder", () => {
    // BITE: the footgun — (charge, mult) come from the reactant's `* xyz`, NEVER the
    // builder's charge field. A −1 reactant with the builder still at 0 emits `* xyz -1`.
    const anion = REACTANT_INPUT.replace("* xyz 0 1", "* xyz -1 1");
    const { inp } = buildNebInput(
      state({ charge: 0 }),
      anion,
      REACTANT_GEOM,
      PRODUCT_GEOM,
    );
    expect(xyzHeader(inp)).toBe("* xyz -1 1");
  });

  it("neb_same_order_guard_still_throws", () => {
    // BITE (regression): emitting anyway → NEB interpolates the wrong atoms and silently
    // fails. The guard is the whole point of the builder.
    const swapped: NebGeometry = {
      elements: ["N", "I", "C"], // C↔I swapped vs the reactant
      xyz_angstrom: PRODUCT_GEOM.xyz_angstrom,
    };
    expect(() => buildNebInput(COMPOSITE_DMF, REACTANT_INPUT, REACTANT_GEOM, swapped)).toThrow(
      /atom order/,
    );
  });

  it("THROWS on a different ATOM COUNT (a truncated product)", () => {
    const short: NebGeometry = {
      elements: ["N", "C"],
      xyz_angstrom: [
        [0, 0, 0],
        [1.5, 0, 0],
      ],
    };
    expect(() => buildNebInput(COMPOSITE_DMF, REACTANT_INPUT, REACTANT_GEOM, short)).toThrow();
  });

  it("THROWS when the reactant input has no inline * xyz block (never defaults charge to 0)", () => {
    const noXyz = "! r2SCAN-3c SMD(DMF) NEB-TS\n* xyzfile 0 1 reactant.xyz\n";
    expect(() => buildNebInput(COMPOSITE_DMF, noXyz, REACTANT_GEOM, PRODUCT_GEOM)).toThrow();
  });
});

describe("hasNebKeyword", () => {
  it("is true for a NEB `!` line (case-insensitive), false for a plain opt", () => {
    expect(hasNebKeyword("! r2SCAN-3c NEB-TS SMD(dmf) TightSCF\n* xyz 0 1\n*")).toBe(true);
    expect(hasNebKeyword("! xtb neb-ci\n* xyz 0 1\n*")).toBe(true);
    expect(hasNebKeyword("! r2SCAN-3c Opt\n* xyz 0 1\n*")).toBe(false);
  });
});
