import { describe, it, expect } from "vitest";

import type { Scene, SceneFragment } from "./types";
import {
  planEdit,
  applyResponseIssue,
  applyResponseToScene,
} from "./edit-plan";
import { measureSelection } from "./measure";
import { mergeToXyz } from "./scene";

// Two fragments of different sizes: water (global 0,1,2) + BH4⁻ (global 3..7).
function water(id = "wat"): SceneFragment {
  return {
    id,
    name: "Water",
    charge: 0,
    source: "editor",
    atoms: [
      { element: "O", x: 0.0, y: 0.0, z: 0.11779 },
      { element: "H", x: 0.0, y: 0.755453, z: -0.471161 },
      { element: "H", x: 0.0, y: -0.755453, z: -0.471161 },
    ],
  };
}

function borohydride(id = "bh4"): SceneFragment {
  const d = 1.24 / Math.sqrt(3);
  return {
    id,
    name: "BH4-",
    charge: -1,
    source: "fragment-library",
    atoms: [
      { element: "B", x: 3.0, y: 0.0, z: 0.0 },
      { element: "H", x: 3.0 + d, y: d, z: d },
      { element: "H", x: 3.0 - d, y: -d, z: d },
      { element: "H", x: 3.0 - d, y: d, z: -d },
      { element: "H", x: 3.0 + d, y: -d, z: -d },
    ],
  };
}

function scene(...fragments: SceneFragment[]): Scene {
  return { fragments, multiplicity: 1 };
}

describe("planEdit — selection count", () => {
  const s = scene(water(), borohydride());
  it("rejects <2 or >4 atoms with a clear reason", () => {
    for (const sel of [[], [0], [0, 1, 2, 3, 4]]) {
      const p = planEdit(s, sel);
      expect(p.kind).toBe("unavailable");
      if (p.kind === "unavailable") expect(p.reason).toMatch(/2, 3 or 4/);
    }
  });
});

describe("planEdit — inter-fragment (ready)", () => {
  const s = scene(water(), borohydride());

  it("2 atoms across fragments → ready, mask = the LAST-clicked fragment", () => {
    // pick water-O (0), then BH4 boron (3): boron clicked last → BH4⁻ moves.
    const p = planEdit(s, [0, 3]);
    expect(p.kind).toBe("ready");
    if (p.kind === "ready") {
      expect(p.op).toBe("distance");
      expect(p.movingFragmentId).toBe("bh4");
      expect(p.mask).toEqual([3, 4, 5, 6, 7]); // the whole BH4⁻ fragment
      expect(p.unit).toBe("Å");
    }
  });

  it("click ORDER decides which fragment moves (reverse → the other moves)", () => {
    const bh4Last = planEdit(s, [0, 3]); // water first, BH4 last
    const waterLast = planEdit(s, [3, 0]); // BH4 first, water last
    expect(bh4Last.kind).toBe("ready");
    expect(waterLast.kind).toBe("ready");
    if (bh4Last.kind === "ready") expect(bh4Last.movingFragmentId).toBe("bh4");
    if (waterLast.kind === "ready") {
      expect(waterLast.movingFragmentId).toBe("wat");
      expect(waterLast.mask).toEqual([0, 1, 2]); // the whole water fragment
    }
  });

  it("current === measureSelection value (math is not duplicated)", () => {
    for (const sel of [
      [0, 3], // distance
      [1, 0, 3], // angle, vertex = water-O
      [0, 1, 2, 3], // dihedral: 3 water refs + BH4 boron last (all refs static)
    ]) {
      const p = planEdit(s, sel);
      const m = measureSelection(s, sel);
      expect(p.kind).toBe("ready");
      if (p.kind === "ready" && m.kind !== "none") {
        expect(p.current).toBeCloseTo(m.value, 10);
      }
    }
  });

  it("carries the pick list verbatim as indices (order preserved)", () => {
    const p = planEdit(s, [0, 1, 2, 3]);
    if (p.kind === "ready") expect(p.indices).toEqual([0, 1, 2, 3]);
  });
});

describe("planEdit — intra-fragment (unavailable, explained)", () => {
  const s = scene(water(), borohydride());

  it("all atoms in one fragment → unavailable, names the bond-graph reason", () => {
    // three atoms all inside BH4⁻ (3,4,5)
    const p = planEdit(s, [3, 4, 5]);
    expect(p.kind).toBe("unavailable");
    if (p.kind === "unavailable") {
      expect(p.reason).toMatch(/same fragment/i);
      expect(p.reason).toMatch(/bond-graph|2\.5\.3/);
    }
  });

  it("a reference atom in the moving fragment is rejected (would move a ref)", () => {
    // dihedral l=4 (BH4), but k=3 (BH4 boron) is also in the moving fragment →
    // the mask would contain a reference atom → rejected by the mirror rule.
    const p = planEdit(s, [1, 0, 3, 4]);
    // (1,0 are water; 3,4 are BH4) — here 3 is a REFERENCE and in the mask.
    // The moving fragment is BH4 (last atom 4); ref 3 ∈ mask → unavailable.
    expect(p.kind).toBe("unavailable");
    if (p.kind === "unavailable") expect(p.reason).toMatch(/same fragment/i);
  });
});

// ── applyResponseToScene: slice the moving fragment out of the response xyz ────

describe("applyResponseToScene", () => {
  const s = scene(water(), borohydride());

  it("replaces ONLY the moving fragment's atoms, by index range", () => {
    // A response xyz where BH4⁻ (rows 3..7) is shifted by +1 in x, water unchanged.
    const rows = mergeToXyz(s).trim().split("\n").slice(2);
    const shifted = rows.map((r, i) => {
      if (i < 3) return r; // water unchanged
      const p = r.split(/\s+/);
      return `${p[0]} ${Number(p[1]) + 1} ${p[2]} ${p[3]}`;
    });
    const respXyz = `8\n\n${shifted.join("\n")}\n`;
    const out = applyResponseToScene(s, "bh4", respXyz);
    // water untouched
    expect(out.fragments[0].atoms).toEqual(s.fragments[0].atoms);
    // BH4 boron x moved from 3.0 to 4.0
    expect(out.fragments[1].atoms[0].x).toBeCloseTo(4.0, 6);
    // count + element order preserved (replaceFragmentAtoms would throw otherwise)
    expect(out.fragments[1].atoms.map((a) => a.element)).toEqual(
      s.fragments[1].atoms.map((a) => a.element),
    );
  });
});

// ── applyResponseIssue: the front-of-the-boundary check before applying ──────

describe("applyResponseIssue", () => {
  const s = scene(water(), borohydride()); // 8 atoms
  const goodXyz = mergeToXyz(s);

  it("passes a clean response (static atoms frozen, count matches)", () => {
    expect(applyResponseIssue(s, goodXyz, 0)).toBeNull();
    expect(applyResponseIssue(s, goodXyz, 5e-7)).toBeNull(); // under tol
  });

  it("rejects a response that moved static atoms", () => {
    const issue = applyResponseIssue(s, goodXyz, 1e-3);
    expect(issue).toMatch(/outside the mask/i);
  });

  it("rejects a response whose atom count differs from the scene", () => {
    const wrong = "7\n\n" + goodXyz.trim().split("\n").slice(2, 9).join("\n") + "\n";
    const issue = applyResponseIssue(s, wrong, 0);
    expect(issue).toMatch(/7 atoms but the scene has 8/);
  });
});
