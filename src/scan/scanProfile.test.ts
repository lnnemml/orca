import { describe, it, expect } from "vitest";

import type { ScanProfileJson, ScanGeometry } from "../types";
import {
  profileSeries,
  referenceEh,
  maxIndex,
  pointGeometryXyz,
  pointReadout,
  scanPointExportXyz,
  HARTREE_TO_KCAL,
  energyEh,
} from "./scanProfile";
import { finalGeometryXyz } from "../export/exporters";

/** The real ethane C–C scan (B1 fixture values): coordinate 1.4→2.4 Å, act ≠ scf. */
const ETHANE: ScanProfileJson = {
  kind: "B",
  atoms: [0, 1],
  coordinate_unit: "Å",
  points: [
    { coordinate: 1.4, energy_act_eh: -79.78236865, energy_scf_eh: -79.78571668 },
    { coordinate: 1.6, energy_act_eh: -79.78966757, energy_scf_eh: -79.79262364 },
    { coordinate: 1.8, energy_act_eh: -79.7697085, energy_scf_eh: -79.77234581 },
    { coordinate: 2.0, energy_act_eh: -79.74231885, energy_scf_eh: -79.74462689 },
    { coordinate: 2.2, energy_act_eh: -79.71507091, energy_scf_eh: -79.71719432 },
    { coordinate: 2.4, energy_act_eh: -79.69075938, energy_scf_eh: -79.69277066 },
  ],
};

// ── C-relative-energy — y is ΔE kcal/mol against a LABELLED reference (0 at ref) ──
describe("C-relative-energy — the plotted y is relative kcal/mol, reference == 0", () => {
  it("first-point reference: point 0 is exactly 0, others are (E−E0)·627.509", () => {
    const s = profileSeries(ETHANE.points, "act", "first");
    expect(s[0].relKcal).toBe(0); // the control: a raw-Eh plot would be ≈ −79.78, NOT 0
    // point 5 relative to point 0, computed independently.
    const expected =
      (ETHANE.points[5].energy_act_eh - ETHANE.points[0].energy_act_eh) * HARTREE_TO_KCAL;
    expect(s[5].relKcal).toBeCloseTo(expected, 9);
    // …and it is a large positive barrier-ish number (raw Eh would be ≈ −79.69).
    expect(s[5].relKcal).toBeGreaterThan(50);
  });

  it("minimum reference: the minimum-energy point is exactly 0 and all others ≥ 0", () => {
    const s = profileSeries(ETHANE.points, "act", "min");
    // min act energy is point 1 (−79.78966757, the deepest).
    expect(Math.min(...s.map((d) => d.relKcal))).toBe(0);
    expect(s.every((d) => d.relKcal >= 0)).toBe(true);
    expect(s[1].relKcal).toBe(0);
  });

  it("the reference energy differs for act vs scf (both labelled, not conflated)", () => {
    expect(referenceEh(ETHANE.points, "act", "first")).toBe(-79.78236865);
    expect(referenceEh(ETHANE.points, "scf", "first")).toBe(-79.78571668);
    expect(energyEh(ETHANE.points[0], "act")).not.toBe(energyEh(ETHANE.points[0], "scf"));
  });
});

// ── approximate-TS index (the maximum of the shown series) ─────────────────────
describe("maxIndex — the approximate-TS point is the maximum of the shown series", () => {
  it("ethane act: the last point (2.4 Å, most stretched) is the maximum", () => {
    // Along a bond-breaking scan energy rises monotonically → the max is the last point.
    expect(maxIndex(ETHANE.points, "act")).toBe(5);
    expect(maxIndex(ETHANE.points, "scf")).toBe(5);
  });
});

// ── C-app-owned-index — the app index selects the geometry fed to the viewer ────
describe("C-app-owned-index — the selected index maps to that point's geometry", () => {
  // Distinct per-point geometries: only the C–C separation differs (z of the 2 C's).
  const geoms: ScanGeometry[] = ETHANE.points.map((p) => ({
    elements: ["C", "C"],
    xyz_angstrom: [
      [0, 0, p.coordinate / 2],
      [0, 0, -p.coordinate / 2],
    ],
  }));

  it("selecting index i feeds the viewer geometry i (not a 3Dmol-owned frame)", () => {
    const r2 = pointGeometryXyz(geoms[2], ["C", "C"]);
    const r5 = pointGeometryXyz(geoms[5], ["C", "C"]);
    expect("xyz" in r2 && r2.xyz).toContain("0.900000"); // 1.8/2 = 0.9
    expect("xyz" in r5 && r5.xyz).toContain("1.200000"); // 2.4/2 = 1.2
    // Different app index → different geometry — the control: a version that read a
    // single 3Dmol frame index would return the same geometry regardless of `i`.
    expect("xyz" in r2 && "xyz" in r5 && r2.xyz).not.toBe("xyz" in r5 ? r5.xyz : "");
  });
});

// ── C-element-order — a mismatched point geometry is refused at the boundary ────
describe("C-element-order — a point whose element sequence differs is refused, not drawn", () => {
  it("matching order → xyz; different order → a loud error, no render", () => {
    const ok = pointGeometryXyz({ elements: ["C", "C"], xyz_angstrom: [[0, 0, 0.7], [0, 0, -0.7]] }, ["C", "C"]);
    expect("xyz" in ok).toBe(true);

    // The control: a point geometry claiming O,H,H (or any different sequence) must
    // NOT render — it would draw the wrong molecule.
    const bad = pointGeometryXyz(
      { elements: ["O", "H", "H"], xyz_angstrom: [[0, 0, 0], [0, 0.9, 0], [0, -0.9, 0]] },
      ["C", "C"],
    );
    expect("error" in bad).toBe(true);
    expect("error" in bad && bad.error).toMatch(/wrong molecule/);
  });
});

// ── readout ────────────────────────────────────────────────────────────────────
describe("pointReadout — coordinate + ΔE of the selected point", () => {
  it("point 3 relative to point 1 (min ref) reads its coordinate and ΔE", () => {
    const r = pointReadout(ETHANE.points, 3, "act", "min", "Å")!;
    expect(r.coordinate).toBe("2.000 Å");
    // point 3 act is above the min (point 1) → positive kcal/mol.
    expect(r.delta.startsWith("+")).toBe(true);
  });
  it("out-of-range index → null (no crash)", () => {
    expect(pointReadout(ETHANE.points, 99, "act", "first", "Å")).toBeNull();
  });
});

// ── scanPointExportXyz — the SELECTED point's geometry, reusing the canonical builder ──
describe("scanPointExportXyz — export the selected scan point via the one xyz builder", () => {
  const GEOM: ScanGeometry = {
    elements: ["C", "C"],
    xyz_angstrom: [
      [0, 0, 1.209],
      [0, 0, -1.209],
    ],
  };

  it("C-reuses-canonical — the xyz BODY is byte-identical to finalGeometryXyz", () => {
    const p = ETHANE.points[5];
    const out = scanPointExportXyz(GEOM, p, 5, ETHANE.points.length, "Å", true, "ethane");
    // The one canonical builder on the SAME geometry — atom lines must match byte-for-byte.
    // Bite: a hand-rolled body (different formatting/precision) would fail this.
    const canonical = finalGeometryXyz(GEOM, "any comment", p.energy_act_eh);
    const body = (s: string) => s.split("\n").slice(2); // drop count + comment lines
    expect(body(out)).toEqual(body(canonical));
    // …and the post-condition still holds: count line + comment + one line per atom.
    expect(out.split("\n").slice(0, 1)[0]).toBe("2");
  });

  it("C-approx-ts-tag — the (approx TS…) tag appears iff isMax; number+coord either way", () => {
    const p = ETHANE.points[3];
    const asMax = scanPointExportXyz(GEOM, p, 3, ETHANE.points.length, "Å", true, "ethane");
    const notMax = scanPointExportXyz(GEOM, p, 3, ETHANE.points.length, "Å", false, "ethane");
    expect(asMax).toContain("(approx TS");
    // Bite: an always-tag version would fail this non-max assertion.
    expect(notMax).not.toContain("(approx TS");
    // The point number + coordinate are present regardless of isMax.
    for (const s of [asMax, notMax]) {
      expect(s).toContain("scan point 4/6");
      expect(s).toContain("@ 2.000 Å");
    }
  });

  it("C-atom-count-inherited — elements/coords length mismatch THROWS (rule #9)", () => {
    // The finalGeometryXyz post-condition is inherited — never a silently short file.
    const bad: ScanGeometry = { elements: ["C", "C", "H"], xyz_angstrom: [[0, 0, 0]] };
    expect(() => scanPointExportXyz(bad, ETHANE.points[0], 0, 6, "Å", false, "ethane")).toThrow();
  });
});
