import { describe, it, expect } from "vitest";

import {
  buildOrcaInput,
  buildKeywordLine,
  DEFAULT_BUILDER_STATE,
  type BuilderState,
} from "./build-input";
import type { Scene } from "../scene/types";
import { testScene } from "../scene/scene-test-util";

/** A builder state overlaid on the defaults. */
function state(overrides: Partial<BuilderState> = {}): BuilderState {
  return { ...DEFAULT_BUILDER_STATE, ...overrides };
}

/** A single-fragment scene with the given charge/multiplicity. */
function scene(charge = 0, multiplicity = 1): Scene {
  return testScene(
    [
      {
        id: "f",
        name: "mol",
        charge,
        source: "editor",
        atoms: [
          { element: "O", x: 0, y: 0, z: 0.11779 },
          { element: "H", x: 0, y: 0.755453, z: -0.471161 },
          { element: "H", x: 0, y: -0.755453, z: -0.471161 },
        ],
      },
    ],
    multiplicity,
  );
}

describe("buildKeywordLine", () => {
  it("emits only the composite method — no basis, dispersion, or RI", () => {
    const line = buildKeywordLine(
      state({
        methodFamily: "composite",
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
        methodFamily: "dft",
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
        methodFamily: "dft",
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
        methodFamily: "dft",
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

  // GFN2-xTB is self-contained: method + job type ONLY. Even with basis,
  // solvation and SCFConv set in the state, none may leak onto the `!` line.
  // BITE: a shared-tail impl (basis block or solvation/SCFConv tail running
  // for xtb) would emit `XTB def2-TZVP SMD(water) Opt TightSCF` and fail here.
  it("xtb_line_is_method_and_jobtype_only", () => {
    const line = buildKeywordLine(
      state({
        methodFamily: "xtb",
        xtbMethod: "XTB",
        jobType: "Opt",
        solvationModel: "SMD",
        solvent: "water",
        scfConv: "TightSCF",
        basis: "def2-TZVP",
        ri: "RIJCOSX",
        dispersion: "D4",
      }),
    );
    expect(line).toBe("XTB Opt");
    expect(line).not.toContain("def2-TZVP");
    expect(line).not.toContain("SMD");
    expect(line).not.toContain("TightSCF");
    expect(line).not.toContain("RIJCOSX");
    expect(line).not.toContain("D4");
  });

  // A non-def2 basis (Dunning/Pople) under RI has no def2 fit set — the aux is
  // ORCA's general AutoAux. BITE: the old auxBasisFor emitted no aux at all for
  // cc-pVTZ (it only matched def2*), so this line would be `... cc-pVTZ RIJCOSX`.
  it("non_def2_basis_with_ri_emits_autoaux", () => {
    const line = buildKeywordLine(
      state({
        methodFamily: "dft",
        functional: "B3LYP",
        basis: "cc-pVTZ",
        ri: "RIJCOSX",
        dispersion: "",
        jobType: "",
        scfConv: "",
      }),
    );
    expect(line).toContain("cc-pVTZ AutoAux RIJCOSX");
  });

  // Regression: def2 bases still pair with the tuned def2/J — an over-eager
  // AutoAux switch that fired for every basis would break this.
  it("def2_basis_with_ri_still_emits_def2J", () => {
    const line = buildKeywordLine(
      state({
        methodFamily: "dft",
        functional: "B3LYP",
        basis: "def2-TZVP",
        ri: "RIJCOSX",
        dispersion: "",
        jobType: "",
        scfConv: "",
      }),
    );
    expect(line).toContain("def2/J");
    expect(line).not.toContain("AutoAux");
  });

  // Regression: a composite method stays self-contained under the new branch —
  // no basis/aux/RI/dispersion tokens ever join a 3c line.
  it("composite_still_self_contained", () => {
    const line = buildKeywordLine(
      state({
        methodFamily: "composite",
        composite: "r2SCAN-3c",
        basis: "def2-TZVP",
        ri: "RIJCOSX",
        dispersion: "D4",
        jobType: "",
        scfConv: "",
      }),
    );
    const tokens = line.split(/\s+/);
    expect(tokens).not.toContain("def2-TZVP");
    expect(tokens).not.toContain("def2/J");
    expect(tokens).not.toContain("AutoAux");
    expect(tokens).not.toContain("RIJCOSX");
    expect(tokens).not.toContain("D4");
  });
});

describe("buildOrcaInput", () => {
  it("writes %maxcore without end and %pal with end", () => {
    const out = buildOrcaInput(state({ nprocs: 8, maxcore: 3000 }), null);
    expect(out).toContain("%pal nprocs 8 end");
    expect(out).toContain("%maxcore 3000");
    expect(out).not.toContain("%maxcore 3000 end");
  });

  it("writes the scene's canonical coordinate block", () => {
    const out = buildOrcaInput(state({ charge: 0, multiplicity: 1 }), scene());
    // Canonical merged rows (toFixed(8), padded) — geometry, not verbatim text.
    expect(out).toContain("O     0.00000000    0.00000000    0.11779000");
    expect(out).toContain("* xyz 0 1");
    expect(out.trimEnd().endsWith("*")).toBe(true);
  });

  it("emits a placeholder comment when there are no coordinates", () => {
    const out = buildOrcaInput(state(), null);
    expect(out).toContain("# paste coordinates here");
  });

  it("takes charge and multiplicity from the scene, overriding the form", () => {
    // Form says 0 / 1, scene says -1 / 2 — the scene wins.
    const out = buildOrcaInput(state({ charge: 0, multiplicity: 1 }), scene(-1, 2));
    expect(out).toContain("* xyz -1 2");
  });
});
