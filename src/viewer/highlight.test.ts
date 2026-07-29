import { describe, it, expect } from "vitest";

import {
  highlightRadius,
  vdwRadius,
  vdwTableDrift,
  VDW_RADII,
  SPHERE_SCALE,
  HALO_MARGIN,
  MIN_HALO,
  FALLBACK_VDW,
} from "./highlight";

describe("vdwRadius", () => {
  it("reads the table, case-insensitively", () => {
    expect(vdwRadius("C")).toBe(1.7);
    expect(vdwRadius("c")).toBe(1.7); // normalised to "C"
    expect(vdwRadius("CL")).toBe(1.75); // "Cl"
    expect(vdwRadius("O")).toBe(1.52);
  });

  it("covers the ADR-007 cross-coupling metals Pd and Pt", () => {
    expect(vdwRadius("Pd")).toBe(1.63);
    expect(vdwRadius("Pt")).toBe(1.75);
  });

  it("falls back to 1.5 Å (3Dmol's defaultSphereRadius) off-table — not NaN/zero", () => {
    // Fe is genuinely absent from 3Dmol's table, so it must hit the fallback.
    expect(vdwRadius("Fe")).toBe(FALLBACK_VDW);
    expect(vdwRadius("Xx")).toBe(FALLBACK_VDW);
    expect(Number.isNaN(vdwRadius("Xx"))).toBe(false);
    expect(vdwRadius("Xx")).toBeGreaterThan(0);
  });
});

describe("highlightRadius", () => {
  it("is the drawn radius (vdw·scale) plus the constant shell", () => {
    for (const el of ["H", "C", "N", "O", "Pd", "Pt"]) {
      expect(highlightRadius(el)).toBeCloseTo(
        VDW_RADII[el] * SPHERE_SCALE + HALO_MARGIN,
        10,
      );
    }
  });

  it("gives every element the SAME visible shell (halo − drawn radius)", () => {
    // The bug fixed here: a constant radius gave 0.19 Å shell on H, 0.04 on C.
    // A constant margin gives the same shell on both.
    const shell = (el: string) =>
      highlightRadius(el) - VDW_RADII[el] * SPHERE_SCALE;
    expect(shell("H")).toBeCloseTo(HALO_MARGIN, 10);
    expect(shell("C")).toBeCloseTo(HALO_MARGIN, 10);
    expect(shell("H")).toBeCloseTo(shell("C"), 10);
  });

  it("is monotonic in vdW radius", () => {
    // Bigger atom → bigger (or equal, once floored) halo. Sort a sample by vdw.
    const els = ["Cu", "O", "N", "C", "Cl", "Br", "K"]; // ascending-ish vdw
    const byVdw = [...els].sort((a, b) => vdwRadius(a) - vdwRadius(b));
    const radii = byVdw.map(highlightRadius);
    for (let i = 1; i < radii.length; i++) {
      expect(radii[i]).toBeGreaterThanOrEqual(radii[i - 1]);
    }
  });

  it("respects the floor for every element (incl. the off-table fallback)", () => {
    const all = [...Object.keys(VDW_RADII), "Fe", "Xx"];
    for (const el of all) {
      expect(highlightRadius(el)).toBeGreaterThanOrEqual(MIN_HALO);
    }
  });

  it("Pd and Pt give real (non-fallback) halos", () => {
    // Regression guard on ADR-007's metals: their halo must be computed from
    // their OWN vdw, not the fallback (1.5·0.3+0.25 = 0.7).
    const fallbackHalo = FALLBACK_VDW * SPHERE_SCALE + HALO_MARGIN;
    expect(highlightRadius("Pd")).not.toBeCloseTo(fallbackHalo, 6);
    expect(highlightRadius("Pt")).not.toBeCloseTo(fallbackHalo, 6);
  });
});

describe("vdwTableDrift (two-directional dup guard)", () => {
  it("is empty in both directions against an identical reference", () => {
    expect(vdwTableDrift({ ...VDW_RADII })).toEqual({ changed: [], missing: [] });
  });

  it("changed: names our keys the reference has with a different value", () => {
    const ref: Record<string, number> = { ...VDW_RADII, C: 1.99 };
    const drift = vdwTableDrift(ref);
    expect(drift.changed).toEqual(["C"]);
    expect(drift.missing).toEqual([]);
  });

  it("missing: names a reference element absent from our copy (the closed blind spot)", () => {
    // 3Dmol gaining an element we don't mirror — the direction the ADR-007 metals
    // slipped through when the guard was one-way.
    const ref: Record<string, number> = { ...VDW_RADII, Rf: 1.6 };
    const drift = vdwTableDrift(ref);
    expect(drift.missing).toEqual(["Rf"]);
    expect(drift.changed).toEqual([]);
  });
});
