import { describe, it, expect } from "vitest";

import { homoIndex, orbitalRows, defaultOrbital } from "./orbitalList";

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

describe("defaultOrbital — opens on the HOMO", () => {
  it("returns the HOMO index", () => {
    expect(defaultOrbital(closed)).toBe(3);
  });
  it("falls back to 0 when nothing is occupied", () => {
    expect(defaultOrbital([[0.1, 0], [0.2, 0]])).toBe(0);
  });
});
