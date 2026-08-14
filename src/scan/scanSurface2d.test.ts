import { describe, it, expect } from "vitest";

import { parseScanSurface2d } from "./scanSurface2d";
import { DIELS_ALDER_10X10_ACT } from "./scanSurface2d.fixture";

describe("parseScanSurface2d — pinned against the real 10×10 Diels-Alder grid", () => {
  const grid = parseScanSurface2d(DIELS_ALDER_10X10_ACT);

  it("parses_the_10x10_grid", () => {
    expect(grid).not.toBeNull();
    expect(grid!.axis1.length).toBe(10);
    expect(grid!.axis2.length).toBe(10);
    expect(grid!.energies.length).toBe(10);
    expect(grid!.energies.flat().length).toBe(100);
  });

  it("row_major_outer_is_coord1 — energies[0][0] is the reactant corner (NOT transposed)", () => {
    // (3.446, 3.446) = -17.82302505 (reactant). A transposed parse would put a different E
    // at [0][0] and mirror the whole surface.
    expect(grid!.axis1[0]).toBeCloseTo(3.446, 5);
    expect(grid!.axis2[0]).toBeCloseTo(3.446, 5);
    expect(grid!.energies[0][0]).toBeCloseTo(-17.82302505, 6);
    // (1.50, 1.50) = -17.93071238 (product, the global minimum) at the far corner.
    expect(grid!.axis1[9]).toBeCloseTo(1.5, 5);
    expect(grid!.axis2[9]).toBeCloseTo(1.5, 5);
    expect(grid!.energies[9][9]).toBeCloseTo(-17.93071238, 6);
  });

  it("node_to_row_maps_to_point_file — the handoff-critical map (node → input.NNN.xyz)", () => {
    expect(grid!.nodeRow(0, 0)).toBe(1); // reactant → input.1.xyz  (== geometries[0])
    expect(grid!.nodeRow(9, 9)).toBe(100); // product  → input.100.xyz (== geometries[99])
    expect(grid!.nodeRow(0, 9)).toBe(10); // (3.446,1.50) stepwise → input.10.xyz
    expect(grid!.nodeRow(9, 0)).toBe(91); // (1.50,3.446) stepwise → input.91.xyz
  });

  it("global_max_is_a_stepwise_corner — documents WHY the UI must not auto-pick the max", () => {
    let max = -Infinity;
    let mi1 = -1;
    let mi2 = -1;
    for (let i1 = 0; i1 < grid!.energies.length; i1++) {
      for (let i2 = 0; i2 < grid!.energies[i1].length; i2++) {
        if (grid!.energies[i1][i2] > max) {
          max = grid!.energies[i1][i2];
          mi1 = i1;
          mi2 = i2;
        }
      }
    }
    // The global max is a STEPWISE CORNER — one coordinate long, the other short — NOT an
    // interior col/saddle. So "auto-pick the max" would send OptTS to a corner, never the TS.
    const lastI1 = grid!.axis1.length - 1;
    const lastI2 = grid!.axis2.length - 1;
    const isStepwiseCorner = (mi1 === 0 && mi2 === lastI2) || (mi1 === lastI1 && mi2 === 0);
    expect(isStepwiseCorner).toBe(true);
    expect(max).toBeCloseTo(-17.78703959, 6);
  });

  it("non_square_grid_parses — no square assumption (a 3×4)", () => {
    const a1 = [1.0, 1.1, 1.2];
    const a2 = [2.0, 2.1, 2.2, 2.3];
    const rows: string[] = [];
    for (const c1 of a1) for (const c2 of a2) rows.push(`   ${c1}   ${c2}   ${-(c1 + c2)}`);
    const g = parseScanSurface2d(rows.join("\n"));
    expect(g).not.toBeNull();
    expect(g!.axis1.length).toBe(3);
    expect(g!.axis2.length).toBe(4);
    expect(g!.energies.length).toBe(3);
    expect(g!.energies[0].length).toBe(4);
    expect(g!.nodeRow(2, 3)).toBe(12); // the last node → row 12
  });

  it("malformed / non-product input → null (all-or-nothing)", () => {
    expect(parseScanSurface2d("")).toBeNull();
    expect(parseScanSurface2d("1.0 2.0")).toBeNull(); // 2 tokens
    expect(parseScanSurface2d("1.0 2.0 notnum")).toBeNull(); // non-finite E
    // A ragged, non-rectangular grid (3 rows, but axis1={1,2} × axis2={2,3} would be 4).
    expect(parseScanSurface2d("1 2 -1\n1 3 -2\n2 2 -3")).toBeNull();
  });
});
