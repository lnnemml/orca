import { describe, it, expect } from "vitest";

import {
  finalGeometryXyz,
  frequenciesCsv,
  chargesCsv,
  orbitalsCsv,
  thermochemistryCsv,
} from "./exporters";

describe("finalGeometryXyz — Å, stored order, post-condition on line count", () => {
  const geom = {
    elements: ["C", "H", "H"],
    xyz_angstrom: [
      [0, 0, 0],
      [1.09, 0, 0],
      [-0.54, 0.94, 0],
    ] as [number, number, number][],
  };
  it("writes count / comment / one line per atom, full precision", () => {
    const xyz = finalGeometryXyz(geom, "my job", -79.7918513760713);
    const lines = xyz.trimEnd().split("\n");
    expect(lines[0]).toBe("3");
    expect(lines[1]).toBe("my job · E = -79.7918513760713 Eh"); // full precision, not rounded
    expect(lines[2]).toBe("C 0 0 0");
    expect(lines[3]).toBe("H 1.09 0 0");
    expect(lines.length).toBe(5); // count + comment + 3 atom lines
  });
  it("throws when elements and coordinates disagree (the post-condition)", () => {
    expect(() =>
      finalGeometryXyz({ elements: ["C", "H"], xyz_angstrom: [[0, 0, 0]] }, "x", null),
    ).toThrow();
  });
  it("handles a missing energy", () => {
    expect(finalGeometryXyz(geom, "x", null).split("\n")[1]).toBe("x");
  });
});

describe("frequenciesCsv — active modes, units in the header, scaled column only when scaled", () => {
  // 6 exact-zero trans/rot + one imaginary excluded; two real modes kept.
  const freq = {
    frequencies_cm: [0, 0, 0, 0, 0, 0, -30, 1000, 1750.5],
    ir_intensity_km_mol: [0, 0, 0, 0, 0, 0, 5, 12.3, 632.8],
  } as NonNullable<import("../types").ParsedResults["frequencies"]>;

  it("has raw columns with units and only the real vibrations", () => {
    const csv = frequenciesCsv(freq, 1);
    const lines = csv.trimEnd().split("\n");
    expect(lines[0]).toBe("mode,wavenumber_cm-1_raw,IR_intensity_km/mol");
    expect(lines[1]).toBe("1,1000,12.3");
    expect(lines[2]).toBe("2,1750.5,632.8");
    expect(lines.length).toBe(3); // header + 2 active modes (imaginary/zero excluded)
  });

  it("adds a labelled derived scaled column when scale ≠ 1", () => {
    const csv = frequenciesCsv(freq, 0.96);
    const lines = csv.trimEnd().split("\n");
    expect(lines[0]).toBe("mode,wavenumber_cm-1_raw,wavenumber_cm-1_scaled_x0.96_derived,IR_intensity_km/mol");
    expect(lines[1]).toBe("1,1000,960,12.3");
  });
});

describe("chargesCsv — one column per scheme, element order kept", () => {
  it("writes atom, element, and each scheme's charge with units", () => {
    const charges = [
      { scheme: "mulliken", elements: ["C", "O"], atomic_numbers: [6, 8], charges: [0.12, -0.34] },
      { scheme: "loewdin", elements: ["C", "O"], atomic_numbers: [6, 8], charges: [0.1, -0.3] },
    ] as import("../types").ParsedResults["charges"];
    const lines = chargesCsv(charges).trimEnd().split("\n");
    expect(lines[0]).toBe("atom_index,element,mulliken_charge_e,loewdin_charge_e");
    expect(lines[1]).toBe("0,C,0.12,0.1");
    expect(lines[2]).toBe("1,O,-0.34,-0.3");
  });
  it("is empty when there are no charges (SP/GOAT — button disabled upstream)", () => {
    expect(chargesCsv([])).toBe("");
  });
});

describe("orbitalsCsv — MO number, Eh, eV, occupancy", () => {
  it("converts Eh→eV and keeps occupancy", () => {
    const orb = { energy_unit: "Eh", orbitals: [[-0.5, 2.0], [-0.1, 0.0]] as [number, number][], homo_lumo: null };
    const lines = orbitalsCsv(orb).trimEnd().split("\n");
    expect(lines[0]).toBe("mo_index,energy_Eh,energy_eV,occupancy");
    expect(lines[1].startsWith("0,-0.5,-13.6")).toBe(true); // -0.5 Eh ≈ -13.6 eV
    expect(lines[1].endsWith(",2")).toBe(true);
  });
});

describe("thermochemistryCsv — T·S labelled as such, derived S separate, units everywhere", () => {
  const t = {
    temperature_k: 298.15,
    el_energy_eh: -843.7,
    zpe_eh: 0.28,
    inner_energy_u_eh: -843.4,
    enthalpy_h_eh: -843.39,
    t_times_s_eh: 0.05,
    free_energy_g_eh: -843.44,
  };
  const csv = thermochemistryCsv(t);
  it("labels the T·S field as the entropy TERM, never 'entropy'", () => {
    expect(csv).toMatch(/T\*S_entropy_term,0.05,Eh/);
    expect(csv).not.toMatch(/^entropy,/m); // never a bare "entropy" row for the T·S value
  });
  it("adds a DERIVED entropy S in J/(mol*K), separately", () => {
    const line = csv.trimEnd().split("\n").find((l) => l.startsWith("entropy_S_derived,"))!;
    expect(line).toContain("J/(mol*K)");
    // S = 0.05/298.15 * 2_625_499.6 ≈ 440.2
    expect(Number(line.split(",")[1])).toBeCloseTo(440.2, 0);
  });
  it("has a units column in the header", () => {
    expect(csv.split("\n")[0]).toBe("quantity,value,unit");
  });
});
