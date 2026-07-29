import { describe, it, expect } from "vitest";

import type { Scene, SceneAtom, SceneFragment } from "./types";
import {
  planEdit,
  swapToAlternative,
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

// A big substrate (33 atoms) + BH4⁻ (5) — the ibuprofen + nucleophile shape from
// the screenshot. Positions are synthetic but make the picked angle B–C–O valid.
function bigSubstrate(id = "ibu"): SceneFragment {
  const atoms: SceneAtom[] = [];
  // A non-collinear zigzag so any 3/4 picked atoms form a valid angle/dihedral
  // (deterministic — no Math.random).
  for (let i = 0; i < 33; i++) {
    atoms.push({
      element: i === 14 ? "O" : "C",
      x: i,
      y: Math.sin(i * 1.3),
      z: Math.cos(i * 0.9),
    });
  }
  return { id, name: "ibuprofen", charge: 0, source: "editor", atoms };
}
/** BH4⁻ whose boron sits off the substrate axis so B–C(#12)–O(#14) isn't collinear. */
function bh4Off(id = "bh4"): SceneFragment {
  return {
    id,
    name: "BH4-",
    charge: -1,
    source: "fragment-library",
    atoms: [
      { element: "B", x: 12, y: 5, z: 0 },
      { element: "H", x: 12.7, y: 5.7, z: 0.7 },
      { element: "H", x: 11.3, y: 5.7, z: -0.7 },
      { element: "H", x: 12.7, y: 5.7, z: -0.7 },
      { element: "H", x: 11.3, y: 5.7, z: 0.7 },
    ],
  };
}

describe("planEdit — both orientations (2.5.2d-2)", () => {
  const s = scene(water(), borohydride());

  it("2 atoms across fragments → moves the SMALLER fragment, OTHER as alternative", () => {
    // water(3 atoms) vs BH4⁻(5). Either side is movable → default moves the
    // SMALLER (water, 2.5.3b), BH4⁻ offered as the alternative.
    const p = planEdit(s, [0, 3]);
    expect(p.kind).toBe("ready");
    if (p.kind === "ready") {
      expect(p.op).toBe("distance");
      expect(p.movingFragmentId).toBe("wat"); // smaller fragment
      expect(p.mask).toEqual([0, 1, 2]);
      expect(p.alternative?.movingFragmentId).toBe("bh4"); // the bigger one
      expect(p.alternative?.mask).toEqual([3, 4, 5, 6, 7]);
    }
  });

  it("smaller-fragment default is independent of click order", () => {
    const bh4First = planEdit(s, [3, 0]); // BH4 clicked first
    const bh4Last = planEdit(s, [0, 3]); // BH4 clicked last
    // Whatever the order, the SMALLER fragment (water) moves by default.
    if (bh4First.kind === "ready") expect(bh4First.movingFragmentId).toBe("wat");
    if (bh4Last.kind === "ready") expect(bh4Last.movingFragmentId).toBe("wat");
  });

  it("(task 4) ibuprofen(33) + BH₄⁻(5): default moves BH₄⁻ in ANY order", () => {
    const big = scene(bigSubstrate(), bh4Off());
    // an inter-fragment distance: an ibuprofen carbon (12) and BH4 boron (33).
    for (const sel of [[12, 33], [33, 12]]) {
      const p = planEdit(big, sel);
      expect(p.kind).toBe("ready");
      if (p.kind === "ready") {
        expect(p.movingFragmentId).toBe("bh4"); // the 5-atom reagent, not 33 atoms
        expect(p.alternative?.movingFragmentId).toBe("ibu");
      }
    }
  });

  it("current === measureSelection value (math is not duplicated)", () => {
    for (const sel of [[0, 3], [1, 0, 3], [0, 1, 2, 3]]) {
      const p = planEdit(s, sel);
      const m = measureSelection(s, sel);
      expect(p.kind).toBe("ready");
      if (p.kind === "ready" && m.kind !== "none") {
        expect(p.current).toBeCloseTo(m.value, 10);
      }
    }
  });

  // ── (a) the EXACT screenshot selection ──────────────────────────────────────
  it("(a) screenshot case [33,12,14]: reagent moves via REVERSED chain", () => {
    // ibuprofen = indices 0..32, BH4⁻ = 33..37 (B at 33). Click B(33) → C(12) →
    // O(14). Last-clicked (O, ibuprofen) can't move — ref C#12 is in ibuprofen.
    // Read the other way, BH4⁻ moves with both refs (C#12, O#14) static.
    const big = scene(bigSubstrate(), bh4Off());
    const p = planEdit(big, [33, 12, 14]);
    expect(p.kind).toBe("ready");
    if (p.kind === "ready") {
      expect(p.op).toBe("angle");
      expect(p.movingFragmentId).toBe("bh4");
      expect(p.mask).toEqual([33, 34, 35, 36, 37]); // the whole BH4⁻
      expect(p.reversed).toBe(true);
      expect(p.alternative).toBeNull(); // the ibuprofen side is NOT movable
    }
  });

  // ── (b) the same angle in the convenient order + value identical ────────────
  it("(b) same selection reversed [14,12,33]: not reversed, same mask, SAME value", () => {
    const big = scene(bigSubstrate(), bh4Off());
    const forward = planEdit(big, [33, 12, 14]); // screenshot order
    const convenient = planEdit(big, [14, 12, 33]); // reagent last
    expect(convenient.kind).toBe("ready");
    if (convenient.kind === "ready" && forward.kind === "ready") {
      expect(convenient.movingFragmentId).toBe("bh4");
      expect(convenient.reversed).toBe(false);
      expect(convenient.mask).toEqual(forward.mask);
      // The value is the SAME regardless of orientation — the crux of the fix.
      expect(convenient.current).toBeCloseTo(forward.current, 9);
    }
  });

  // ── (c) inter-fragment distance: alternative present, swap mirrors ──────────
  it("(c) distance: either side movable → alternative, swap gives the mirror", () => {
    const p = planEdit(s, [0, 3]); // O(water) ··· B(bh4); default = water (smaller)
    expect(p.kind).toBe("ready");
    if (p.kind !== "ready") return;
    expect(p.movingFragmentId).toBe("wat");
    expect(p.alternative).not.toBeNull();
    const swapped = swapToAlternative(p);
    if (swapped.kind !== "ready") throw new Error("swap dropped ready");
    expect(swapped.movingFragmentId).toBe("bh4"); // "Move BH4 instead"
    expect(swapped.mask).toEqual([3, 4, 5, 6, 7]);
    expect(swapped.reversed).toBe(!p.reversed);
    expect(swapped.current).toBeCloseTo(p.current, 12); // value unchanged
    // swapping again returns the original mover
    const back = swapToAlternative(swapped);
    if (back.kind === "ready") expect(back.movingFragmentId).toBe("wat");
  });
});

describe("planEdit — intra-fragment → needs-split (2.5.3b)", () => {
  const s = scene(water(), borohydride());
  const big = scene(bigSubstrate(), bh4Off()); // ibuprofen 0..32, BH4 33..37

  it("all atoms in one fragment → needs-split (not a refusal)", () => {
    const p = planEdit(s, [3, 4, 5]); // all inside BH4⁻
    expect(p.kind).toBe("needs-split");
  });

  it("needs-split carries the right cut/moving/within per op", () => {
    // within = the ibuprofen fragment's global indices (0..32).
    const within = Array.from({ length: 33 }, (_, i) => i);
    // distance(i,j) → cut (i,j), move j
    const d = planEdit(big, [5, 12]);
    expect(d.kind).toBe("needs-split");
    if (d.kind === "needs-split") {
      expect(d.cut).toEqual([5, 12]);
      expect(d.moving).toBe(12);
      expect(d.within).toEqual(within);
    }
    // angle(i,v,j) → cut (v,j), move j
    const a = planEdit(big, [5, 12, 20]);
    if (a.kind === "needs-split") {
      expect(a.op).toBe("angle");
      expect(a.cut).toEqual([12, 20]);
      expect(a.moving).toBe(20);
      expect(a.within).toEqual(within);
    } else {
      throw new Error("expected needs-split");
    }
    // dihedral(i,j,k,l) → cut (j,k), move l
    const dih = planEdit(big, [5, 12, 20, 25]);
    if (dih.kind === "needs-split") {
      expect(dih.op).toBe("dihedral");
      expect(dih.cut).toEqual([12, 20]);
      expect(dih.moving).toBe(25);
    } else {
      throw new Error("expected needs-split");
    }
  });

  it("still needs-split when the fragment is the ONLY one in the scene", () => {
    const solo = scene(bigSubstrate()); // one fragment, whole scene
    const p = planEdit(solo, [5, 12, 20, 25]);
    expect(p.kind).toBe("needs-split");
  });
});

describe("planEdit — the immovable-axis refusal (2.5.2d-2)", () => {
  const s = scene(water(), borohydride());

  it("(e) 2+2 dihedral across fragments → the immovable-axis reason, names culprits", () => {
    // dihedral [1,0,3,4]: water {1,0} | BH4 {3,4}. Whichever end moves, an axis
    // atom (0 or 3) moves with it → no orientation holds the j–k axis fixed.
    const p = planEdit(s, [1, 0, 3, 4]);
    expect(p.kind).toBe("unavailable");
    if (p.kind === "unavailable") {
      expect(p.reason).not.toMatch(/bond-graph/); // NOT the intra reason
      expect(p.reason).toMatch(/axis/i);
      // Names the offending atoms: 0 (water axis atom) and 3 (BH4 axis atom).
      expect(p.reason).toContain("#0");
      expect(p.reason).toContain("#3");
    }
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
