import { describe, it, expect } from "vitest";

import {
  buildOrcaInput,
  buildKeywordLine,
  DEFAULT_BUILDER_STATE,
  type BuilderState,
} from "./build-input";

/** A builder state overlaid on the defaults. */
function state(overrides: Partial<BuilderState> = {}): BuilderState {
  return { ...DEFAULT_BUILDER_STATE, ...overrides };
}

describe("buildKeywordLine", () => {
  it("emits only the composite method — no basis, dispersion, or RI", () => {
    const line = buildKeywordLine(
      state({
        useComposite: true,
        composite: "r2SCAN-3c",
        // These would be wrong to emit for a 3c method:
        basis: "def2-TZVP",
        dispersion: "D4",
        ri: "RIJCOSX",
        jobType: "Opt Freq",
        scfConv: "TightSCF",
      }),
    );
    expect(line).toBe("r2SCAN-3c Opt Freq TightSCF");
    expect(line).not.toContain("def2-TZVP");
    expect(line).not.toContain("D4");
    expect(line).not.toContain("RIJCOSX");
  });

  it("adds def2/J for RIJCOSX with a def2 basis", () => {
    const line = buildKeywordLine(
      state({
        useComposite: false,
        functional: "B3LYP",
        basis: "def2-TZVP",
        ri: "RIJCOSX",
        dispersion: "D4",
        jobType: "Opt Freq",
        scfConv: "TightSCF",
      }),
    );
    expect(line).toBe("B3LYP def2-TZVP def2/J RIJCOSX D4 Opt Freq TightSCF");
  });

  it("adds def2/JK for RI-JK", () => {
    const line = buildKeywordLine(
      state({
        useComposite: false,
        functional: "B3LYP",
        basis: "def2-TZVP",
        ri: "RI-JK",
        dispersion: "",
        jobType: "",
        scfConv: "",
      }),
    );
    expect(line).toContain("def2/JK");
    expect(line).not.toContain("def2/J ");
  });

  it("does not double-count dispersion for a functional with built-in D4", () => {
    const line = buildKeywordLine(
      state({
        useComposite: false,
        functional: "wB97X-D4",
        basis: "def2-TZVP",
        ri: "",
        dispersion: "D4",
        jobType: "",
        scfConv: "",
      }),
    );
    // The functional name (which contains "D4") appears exactly once...
    expect(line.match(/wB97X-D4/g)).toHaveLength(1);
    // ...and no standalone D4 dispersion token is appended after the basis.
    expect(line.split(/\s+/)).not.toContain("D4");
  });

  it("wraps solvent in the model: CPCM(water)", () => {
    const line = buildKeywordLine(
      state({ solvationModel: "CPCM", solvent: "water" }),
    );
    expect(line).toContain("CPCM(water)");
  });

  it("emits no solvation keyword in gas phase", () => {
    const line = buildKeywordLine(state({ solvationModel: "", solvent: "water" }));
    expect(line).not.toContain("CPCM");
    expect(line).not.toContain("SMD");
  });
});

describe("buildOrcaInput", () => {
  it("writes %maxcore without end and %pal with end", () => {
    const out = buildOrcaInput(state({ nprocs: 8, maxcore: 3000 }), null);
    expect(out).toContain("%pal nprocs 8 end");
    expect(out).toContain("%maxcore 3000");
    expect(out).not.toContain("%maxcore 3000 end");
  });

  it("preserves the coordinate block verbatim", () => {
    const atoms = [
      "O   0.000000   0.000000   0.117790",
      "H   0.000000   0.755453  -0.471161",
      "H   0.000000  -0.755453  -0.471161",
    ].join("\n");
    const out = buildOrcaInput(state({ charge: 0, multiplicity: 1 }), atoms);
    expect(out).toContain(atoms);
    expect(out).toContain("* xyz 0 1");
    expect(out.trimEnd().endsWith("*")).toBe(true);
  });

  it("emits a placeholder comment when there are no coordinates", () => {
    const out = buildOrcaInput(state(), null);
    expect(out).toContain("# paste coordinates here");
  });

  it("uses the form's charge and multiplicity in the geometry header", () => {
    const out = buildOrcaInput(state({ charge: -1, multiplicity: 2 }), "Cl 0 0 0");
    expect(out).toContain("* xyz -1 2");
  });
});
