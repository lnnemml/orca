import { describe, it, expect } from "vitest";

import {
  homoIndex,
  orbitalRows,
  defaultOrbital,
  expectedCoreCount,
  coreOrbitals,
} from "./orbitalList";

/** A closed-shell fragment: MOs 0..3 occupied (2.0), 4..6 virtual (0.0). HOMO = 3. */
const closed: [number, number][] = [
  [-0.8, 2.0],
  [-0.6, 2.0],
  [-0.4, 2.0],
  [-0.3, 2.0], // HOMO
  [-0.1, 0.0], // LUMO
  [0.05, 0.0],
  [0.2, 0.0],
];

describe("homoIndex — the highest occupied MO", () => {
  it("finds the HOMO of a closed-shell list", () => {
    expect(homoIndex(closed)).toBe(3);
  });
  it("is null when nothing is occupied", () => {
    expect(homoIndex([[0.1, 0], [0.2, 0]])).toBeNull();
  });
  it("handles an open-shell (occupancy 1.0) as occupied", () => {
    expect(homoIndex([[-0.5, 2.0], [-0.3, 1.0], [0.1, 0.0]])).toBe(1);
  });
});

describe("orbitalRows — marks exactly one HOMO and one LUMO", () => {
  const rows = orbitalRows(closed);
  it("marks the HOMO and the LUMO once each", () => {
    expect(rows.filter((r) => r.kind === "HOMO").map((r) => r.index)).toEqual([3]);
    expect(rows.filter((r) => r.kind === "LUMO").map((r) => r.index)).toEqual([4]);
  });
  it("labels the rest occupied / virtual by occupancy", () => {
    expect(rows[0].kind).toBe("occupied");
    expect(rows[6].kind).toBe("virtual");
  });
  it("keeps the MO index (= the number orca_plot wants) and the energy/occupancy", () => {
    expect(rows[3]).toEqual({ index: 3, energyEh: -0.3, occupancy: 2.0, kind: "HOMO" });
  });
  it("has no LUMO when the HOMO is the last MO", () => {
    const r = orbitalRows([[-0.5, 2.0], [-0.3, 2.0]]);
    expect(r.some((x) => x.kind === "LUMO")).toBe(false);
    expect(r[1].kind).toBe("HOMO");
  });
});

describe("core orbitals — DERIVED per element, cross-checked by the energy gap", () => {
  it("expectedCoreCount: H/He→0, 2nd period→1, 3rd period→5; null outside the table", () => {
    expect(expectedCoreCount(["C", "C", "H", "O"])).toBe(3); // 2 C + 1 O core (H=0)
    expect(expectedCoreCount(["S", "H", "H"])).toBe(5); // S core is FIVE, not one
    expect(expectedCoreCount(["Fe", "C"])).toBeNull(); // Fe outside → no count
    expect(expectedCoreCount(["H", "He"])).toBe(0);
  });

  // The dexketoprofen shape (C₁₆H₁₄O₃): 3 O-1s at −19, 16 C-1s at −10, then the big gap
  // to the first valence at −1.08. Expected core = 16 + 3 = 19; the gap sits after 19.
  const dexElements = [
    ...Array(16).fill("C"),
    ...Array(14).fill("H"),
    ...Array(3).fill("O"),
  ];
  const dexOrbitals: [number, number][] = [
    ...Array(3).fill(0).map((_, i) => [-19.0 - i * 0.01, 2.0] as [number, number]), // O 1s
    ...Array(16).fill(0).map((_, i) => [-10.17 + i * 0.01, 2.0] as [number, number]), // C 1s
    [-1.079, 2.0], // 19: first valence — big gap before it
    [-1.013, 2.0],
    ...Array(40).fill(0).map((_, i) => [-0.9 + i * 0.02, i < 45 ? 2.0 : 0.0] as [number, number]),
  ];

  it("marks 19 core orbitals when the table and the gap AGREE (measured dexketoprofen)", () => {
    const info = coreOrbitals(dexOrbitals, dexElements);
    expect(info.expectedFromFormula).toBe(19);
    expect(info.gapAt).toBe(19);
    expect(info.count).toBe(19);
    expect(info.note).toMatch(/derived/);
    // orbitalRows tags MOs 0..18 as core, and 19 is NOT core.
    const rows = orbitalRows(dexOrbitals, dexElements);
    const coreIdx = rows.filter((r) => r.kind === "core").map((r) => r.index);
    expect(coreIdx[coreIdx.length - 1]).toBe(18);
    expect(rows[19].kind).not.toBe("core");
  });

  it("places NO core mark when the gap disagrees with the table (reports the mismatch)", () => {
    // Same elements (expect 19) but a spectrum with no clean gap at 19 — a flat ramp.
    const flat: [number, number][] = Array(60)
      .fill(0)
      .map((_, i) => [-20 + i * 0.3, i < 45 ? 2.0 : 0.0]);
    const info = coreOrbitals(flat, dexElements);
    expect(info.count).toBeNull();
    expect(info.expectedFromFormula).toBe(19);
    expect(info.note).toMatch(/omitted|disagreement/);
    expect(orbitalRows(flat, dexElements).some((r) => r.kind === "core")).toBe(false);
  });

  it("places NO core mark for an element outside the table", () => {
    const info = coreOrbitals(dexOrbitals, ["Fe", ...dexElements]);
    expect(info.count).toBeNull();
    expect(info.expectedFromFormula).toBeNull();
    expect(info.note).toMatch(/outside/);
  });

  it("orbitalRows without elements places no core mark (opt-in)", () => {
    expect(orbitalRows(dexOrbitals).some((r) => r.kind === "core")).toBe(false);
  });
});

describe("defaultOrbital — opens on the HOMO", () => {
  it("returns the HOMO index", () => {
    expect(defaultOrbital(closed)).toBe(3);
  });
  it("falls back to 0 when nothing is occupied", () => {
    expect(defaultOrbital([[0.1, 0], [0.2, 0]])).toBe(0);
  });
});
