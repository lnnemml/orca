import { describe, it, expect } from "vitest";

import { buildConnectivityChildren } from "./connectivity";
import type { Geometry } from "../spectrum/mode";

// A located-TS input (Menshutkin-shaped): method + solvation + OptTS Freq, charge 0 1.
const TS_INPUT = [
  "! r2SCAN-3c SMD(DMF) OptTS Freq TightSCF",
  "%geom Calc_Hess true end",
  "%maxcore 3000",
  "* xyz 0 1",
  "N 0.000000 0.0 0.0",
  "C 2.000000 0.0 0.0",
  "I 4.300000 0.0 0.0",
  "*",
].join("\n");

const TS_GEOMETRY: Geometry = {
  elements: ["N", "C", "I"],
  xyz_angstrom: [
    [0, 0, 0],
    [2.0, 0, 0],
    [4.3, 0, 0],
  ],
};
// Imaginary mode: the central C slides between N and I (the SN2 coordinate).
const IMAG_MODE = [0, 0, 0, 1, 0, 0, 0, 0, 0];

const xyzHeader = (input: string) =>
  input.split(/\r?\n/).find((l) => l.trim().startsWith("* xyz")) ?? "";
const bangLine = (input: string) =>
  input.split(/\r?\n/).find((l) => l.trim().startsWith("!")) ?? "";

describe("buildConnectivityChildren — two plain-Opt endpoints (Stage E2)", () => {
  it("emits TWO plain Opt children — Opt, never OptTS/Freq (Fork B)", () => {
    const { forwardInput, backwardInput } = buildConnectivityChildren(
      TS_INPUT,
      TS_GEOMETRY,
      IMAG_MODE,
      0.5,
    );
    for (const inp of [forwardInput, backwardInput]) {
      const bang = bangLine(inp).toLowerCase();
      expect(bang).toContain("opt");
      expect(bang).not.toContain("optts");
      expect(bang).not.toContain("freq");
    }
  });

  it("inherits method + solvation from the TS VERBATIM (comparability, not the r2SCAN-3c/no-solvent default)", () => {
    // BITE: buildReoptInput on its own defaults to r2SCAN-3c with NO solvation; a
    // connectivity child that dropped SMD(DMF) would relax on a different surface.
    const { forwardInput } = buildConnectivityChildren(TS_INPUT, TS_GEOMETRY, IMAG_MODE, 0.5);
    expect(bangLine(forwardInput)).toContain("r2SCAN-3c");
    expect(bangLine(forwardInput)).toContain("SMD(DMF)");
  });

  it("inherits charge/multiplicity (THE footgun) — 0 1 here, asserted by buildReoptInput", () => {
    const { forwardInput, backwardInput } = buildConnectivityChildren(
      TS_INPUT,
      TS_GEOMETRY,
      IMAG_MODE,
      0.5,
    );
    expect(xyzHeader(forwardInput)).toBe("* xyz 0 1");
    expect(xyzHeader(backwardInput)).toBe("* xyz 0 1");
  });

  it("a charged TS propagates its charge to both children (−1 1)", () => {
    const anion = TS_INPUT.replace("* xyz 0 1", "* xyz -1 1");
    const { forwardInput, backwardInput } = buildConnectivityChildren(
      anion,
      TS_GEOMETRY,
      IMAG_MODE,
      0.5,
    );
    expect(xyzHeader(forwardInput)).toBe("* xyz -1 1");
    expect(xyzHeader(backwardInput)).toBe("* xyz -1 1");
  });

  it("forward and backward inputs DIFFER (the app generates BOTH displaced geometries)", () => {
    const { forwardInput, backwardInput } = buildConnectivityChildren(
      TS_INPUT,
      TS_GEOMETRY,
      IMAG_MODE,
      0.5,
    );
    expect(forwardInput).not.toBe(backwardInput);
  });

  it("throws when the TS source has no inline * xyz block (never defaults charge to 0)", () => {
    const noXyz = "! r2SCAN-3c SMD(DMF) OptTS Freq\n* xyzfile 0 1 ts.xyz\n";
    expect(() =>
      buildConnectivityChildren(noXyz, TS_GEOMETRY, IMAG_MODE, 0.5),
    ).toThrow();
  });
});
