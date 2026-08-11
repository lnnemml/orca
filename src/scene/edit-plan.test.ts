import { describe, it, expect } from "vitest";

import type { Scene } from "./types";
import { testScene, idsFor, type RawFragment } from "./scene-test-util";
import type { RawAtom } from "./types";
import {
  planEdit,
  swapToAlternative,
  applyResponseIssue,
  applyResponseToScene,
  maskRoleViolation,
  explainSplitViolation,
  axisTranslation,
  resolveComponentMove,
  pickedDistance,
  type NeedsComponentMove,
} from "./edit-plan";
import { measureSelection } from "./measure";
import { mergeToXyz, removeFragment, translateAtomsInScene } from "./scene";

// Two fragments of different sizes: water (global 0,1,2) + BH4⁻ (global 3..7).
function water(id = "wat"): RawFragment {
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

function borohydride(id = "bh4"): RawFragment {
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

function scene(...fragments: RawFragment[]): Scene {
  return testScene(fragments);
}

describe("planEdit — selection count", () => {
  const s = scene(water(), borohydride());
  it("rejects <2 or >4 atoms with a clear reason", () => {
    for (const sel of [[], [0], [0, 1, 2, 3, 4]]) {
      const p = planEdit(s, idsFor(s, ...sel));
      expect(p.kind).toBe("unavailable");
      if (p.kind === "unavailable") expect(p.reason).toMatch(/2, 3 or 4/);
    }
  });
});

// A big substrate (33 atoms) + BH4⁻ (5) — the ibuprofen + nucleophile shape from
// the screenshot. Positions are synthetic but make the picked angle B–C–O valid.
function bigSubstrate(id = "ibu"): RawFragment {
  const atoms: RawAtom[] = [];
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
function bh4Off(id = "bh4"): RawFragment {
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
    const p = planEdit(s, idsFor(s, 0, 3));
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
    const bh4First = planEdit(s, idsFor(s, 3, 0)); // BH4 clicked first
    const bh4Last = planEdit(s, idsFor(s, 0, 3)); // BH4 clicked last
    // Whatever the order, the SMALLER fragment (water) moves by default.
    if (bh4First.kind === "ready") expect(bh4First.movingFragmentId).toBe("wat");
    if (bh4Last.kind === "ready") expect(bh4Last.movingFragmentId).toBe("wat");
  });

  it("(task 4) ibuprofen(33) + BH₄⁻(5): default moves BH₄⁻ in ANY order", () => {
    const big = scene(bigSubstrate(), bh4Off());
    // an inter-fragment distance: an ibuprofen carbon (12) and BH4 boron (33).
    for (const sel of [[12, 33], [33, 12]]) {
      const p = planEdit(big, idsFor(big, ...sel));
      expect(p.kind).toBe("ready");
      if (p.kind === "ready") {
        expect(p.movingFragmentId).toBe("bh4"); // the 5-atom reagent, not 33 atoms
        expect(p.alternative?.movingFragmentId).toBe("ibu");
      }
    }
  });

  it("current === measureSelection value (math is not duplicated)", () => {
    for (const sel of [[0, 3], [1, 0, 3], [0, 1, 2, 3]]) {
      const ids = idsFor(s, ...sel);
      const p = planEdit(s, ids);
      const m = measureSelection(s, ids);
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
    const p = planEdit(big, idsFor(big, 33, 12, 14));
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
    const forward = planEdit(big, idsFor(big, 33, 12, 14)); // screenshot order
    const convenient = planEdit(big, idsFor(big, 14, 12, 33)); // reagent last
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
    const p = planEdit(s, idsFor(s, 0, 3)); // O(water) ··· B(bh4); default = water (smaller)
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
    const p = planEdit(s, idsFor(s, 3, 4, 5)); // all inside BH4⁻
    expect(p.kind).toBe("needs-split");
  });

  it("needs-split carries the right cut/moving/within per op", () => {
    // within = the ibuprofen fragment's global indices (0..32).
    const within = Array.from({ length: 33 }, (_, i) => i);
    // distance(i,j) → cut (i,j), move j
    const d = planEdit(big, idsFor(big, 5, 12));
    expect(d.kind).toBe("needs-split");
    if (d.kind === "needs-split") {
      expect(d.cut).toEqual([5, 12]);
      expect(d.moving).toBe(12);
      expect(d.within).toEqual(within);
    }
    // angle(i,v,j) → cut (v,j), move j
    const a = planEdit(big, idsFor(big, 5, 12, 20));
    if (a.kind === "needs-split") {
      expect(a.op).toBe("angle");
      expect(a.cut).toEqual([12, 20]);
      expect(a.moving).toBe(20);
      expect(a.within).toEqual(within);
    } else {
      throw new Error("expected needs-split");
    }
    // dihedral(i,j,k,l) → cut (j,k), move l
    const dih = planEdit(big, idsFor(big, 5, 12, 20, 25));
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
    const p = planEdit(solo, idsFor(solo, 5, 12, 20, 25));
    expect(p.kind).toBe("needs-split");
  });

  // On the divergent fixture (AtomId ≠ global) planEdit resolves the ids to the
  // CURRENT positional indices at its emit seam: the plan's `indices`/`cut` are
  // the post-removal global indices, not the ids. Green here only if the resolve
  // is real (on a fresh scene id==index would hide a broken resolve).
  it("resolves AtomIds to CURRENT global indices after a fragment removal", () => {
    // water(0,1,2) + BH₄⁻(3..7); pick two BH₄⁻ atoms by id, THEN remove water so
    // BH₄⁻ slides to global 0..4. The ids are stable; their indices are not.
    const full = scene(water(), borohydride());
    const [b, h] = idsFor(full, 3, 5); // boron (id 3) + an H (id 5)
    const after = removeFragment(full, "wat"); // BH₄⁻ now global 0..4
    const p = planEdit(after, [b, h]); // distance inside BH₄⁻ → needs-split
    expect(p.kind).toBe("needs-split");
    if (p.kind === "needs-split") {
      // boron is global 0 now (not 3), the H is global 2 (not 5): cut on the
      // resolved indices, move the second.
      expect(p.cut).toEqual([0, 2]);
      expect(p.moving).toBe(2);
    }
  });
});

// ── Unified moving-set: a same-fragment distance across DISCONNECTED pieces ────
// The Diels-Alder bug: a diene + dienophile imported as ONE xyz become ONE
// fragment holding TWO molecules (two perceived connected components). Setting a
// distance / forming a bond between a diene C and a dienophile C used to route to
// `needs-split` → the sidecar 422'd "atoms i and j are not bonded" (there is no
// bond to cut). It must instead yield `needs-component-move`, translating one
// molecule toward the other. planEdit is pure: the components are INJECTED (the
// `/geometry/connected-component` result), so the decision is testable without a
// sidecar.
describe("planEdit — same-fragment distance across components (needs-component-move)", () => {
  // ONE fragment, TWO molecules: piece A = atoms {0,1}, piece B = atoms {2,3,4}.
  // Geometry is irrelevant to the plan decision (planEdit does not perceive — the
  // components are injected), only the distance must be measurable (non-coincident).
  function combo(id = "combo"): RawFragment {
    const atoms: RawAtom[] = [
      { element: "C", x: 0, y: 0, z: 0 },
      { element: "C", x: 1.4, y: 0, z: 0 },
      { element: "C", x: 8, y: 0, z: 0 },
      { element: "C", x: 9.4, y: 0, z: 0 },
      { element: "C", x: 10.8, y: 0, z: 0 },
    ];
    return { id, name: "diene+dienophile", charge: 0, source: "editor", atoms };
  }
  // The perceived components the sidecar would return for this fragment.
  const twoComponents = new Map<number, readonly number[]>([
    [0, [0, 1]],
    [1, [0, 1]],
    [2, [2, 3, 4]],
    [3, [2, 3, 4]],
    [4, [2, 3, 4]],
  ]);
  // Everything in ONE component (a fully-bonded molecule / a real bond to stretch).
  const oneComponent = new Map<number, readonly number[]>(
    [0, 1, 2, 3, 4].map((k) => [k, [0, 1, 2, 3, 4]] as const),
  );

  it("distance between two disconnected atoms of one fragment → needs-component-move (the BITE)", () => {
    const s = scene(combo());
    const p = planEdit(s, idsFor(s, 0, 2), twoComponents);
    // NOT needs-split (no bond to cut → would 422), NOT unavailable — the bug.
    expect(p.kind).toBe("needs-component-move");
    if (p.kind === "needs-component-move") {
      expect(p.op).toBe("distance");
      expect(p.indices).toEqual([0, 2]);
      // Moves the SMALLER component {0,1}; the larger {2,3,4} is the "move other" set.
      expect(p.moving).toEqual([0, 1]);
      expect(p.other).toEqual([2, 3, 4]);
    }
  });

  it("picks the smaller component as the default mover regardless of click order", () => {
    const s = scene(combo());
    // Click the larger piece's atom first (2), the smaller piece's atom last (0):
    // the SMALLER {0,1} still moves by default.
    const p = planEdit(s, idsFor(s, 2, 0), twoComponents);
    expect(p.kind).toBe("needs-component-move");
    if (p.kind === "needs-component-move") {
      expect(p.moving).toEqual([0, 1]);
      expect(p.other).toEqual([2, 3, 4]);
    }
  });

  it("a bonded intra pair (SAME component) still → needs-split, not component-move (no regression)", () => {
    const s = scene(combo());
    // Same two atoms, but connectivity says they share ONE component (a real bond):
    // this is a torsion/stretch, so it must stay needs-split.
    const p = planEdit(s, idsFor(s, 0, 1), oneComponent);
    expect(p.kind).toBe("needs-split");
  });

  it("without injected connectivity a same-fragment distance stays needs-split (backward compatible)", () => {
    const s = scene(combo());
    const p = planEdit(s, idsFor(s, 0, 2)); // no components arg
    expect(p.kind).toBe("needs-split");
  });

  it("swapToAlternative swaps the two components (move the other instead)", () => {
    const s = scene(combo());
    const p = planEdit(s, idsFor(s, 0, 2), twoComponents);
    if (p.kind !== "needs-component-move") throw new Error("expected needs-component-move");
    const swapped = swapToAlternative(p);
    expect(swapped.kind).toBe("needs-component-move");
    if (swapped.kind === "needs-component-move") {
      // BITE: an impl that didn't swap would leave moving = [0,1].
      expect(swapped.moving).toEqual([2, 3, 4]);
      expect(swapped.other).toEqual([0, 1]);
    }
  });

  it("resolveComponentMove + axisTranslation set the distance EXACTLY via translateAtoms (the post-condition bite)", () => {
    const s = scene(combo()); // atom0 at x=0, atom2 at x=8 → current 8 Å
    const p = planEdit(s, idsFor(s, 0, 2), twoComponents) as NeedsComponentMove;
    expect(p.current).toBeCloseTo(8, 9);
    const r = resolveComponentMove(s, p);
    expect(r).not.toBeNull();
    if (!r) return;
    // Moves the smaller component {0,1}; mover = atom0 (x=0), reference = atom2 (x=8).
    expect(r.movingFragmentId).toBe("combo");
    expect(r.movingAtomIds).toEqual(idsFor(s, 0, 1));
    expect(r.movingPos).toEqual([0, 0, 0]);
    expect(r.refPos).toEqual([8, 0, 0]);
    // Set the pair to 1.5 Å by a rigid shift of the whole moving component.
    const target = 1.5;
    const [dx, dy, dz] = axisTranslation(r.movingPos, r.refPos, target);
    const moved = translateAtomsInScene(s, r.movingAtomIds, dx, dy, dz);
    // Post-condition (rule #9): the resulting separation IS the target — a wrong sign
    // or magnitude in axisTranslation would fail here (the bite).
    expect(pickedDistance(moved, p.indices)).toBeCloseTo(target, 9);
    // And atom1 (the rest of the moving component) shifted by the SAME delta — the
    // whole component moved rigidly, not just the picked atom.
    expect(moved.fragments[0].atoms[1].x).toBeCloseTo(1.4 + dx, 9);
    // The static component {2,3,4} did NOT move (count+order preserved by translateAtoms).
    expect(moved.fragments[0].atoms[2].x).toBeCloseTo(8, 9);
    expect(moved.fragments[0].atoms.map((a) => a.element)).toEqual(
      s.fragments[0].atoms.map((a) => a.element),
    );
  });

  it("axisTranslation moving the OTHER (larger) component also hits the target", () => {
    const s = scene(combo());
    const base = planEdit(s, idsFor(s, 0, 2), twoComponents) as NeedsComponentMove;
    const swapped = swapToAlternative(base) as NeedsComponentMove;
    const r = resolveComponentMove(s, swapped)!;
    expect(r.movingAtomIds).toEqual(idsFor(s, 2, 3, 4)); // the larger piece now moves
    const [dx, dy, dz] = axisTranslation(r.movingPos, r.refPos, 3);
    const moved = translateAtomsInScene(s, r.movingAtomIds, dx, dy, dz);
    expect(pickedDistance(moved, base.indices)).toBeCloseTo(3, 9);
    expect(moved.fragments[0].atoms[0].x).toBeCloseTo(0, 9); // atom0 (reference side) stayed
  });

  it("inter-fragment distance is unaffected by injected connectivity (still moves the smaller fragment)", () => {
    const s = scene(water(), borohydride()); // water 0..2, BH₄⁻ 3..7
    // A components map is present but the atoms are in different FRAGMENTS — the
    // inter-fragment path runs first and is unchanged.
    const comps = new Map<number, readonly number[]>([
      [0, [0, 1, 2]],
      [3, [3, 4, 5, 6, 7]],
    ]);
    const p = planEdit(s, idsFor(s, 0, 3), comps);
    expect(p.kind).toBe("ready");
    if (p.kind === "ready") {
      expect(p.movingFragmentId).toBe("wat"); // smaller fragment, as before
      expect(p.alternative?.movingFragmentId).toBe("bh4");
    }
  });
});

// ── The 2.5.4a fix: re-run the reference-atom rule on the RESOLVED split mask ──
// The 2.5.3b hole was that `planEdit` checked the rule for the inter-fragment
// case but not after the sidecar's bond-graph split came back, so a reference
// atom on the moving side slipped through to a 422 at Apply. Live repro (butane):
//   angle(3,1,2) → needs-split, cut (1,2), moving 2
//   /geometry/rotatable-mask → mask [2,3,9,10,11,12,13]  ← reference 3 IS inside
// `maskRoleViolation` is the single pure check now used by BOTH orientationFor
// (inter-fragment) and NewJobScreen (post-split); it must catch this.
function butane(id = "but"): RawFragment {
  // C0-C1-C2-C3 backbone (so a cut on 1–2 reads "C#1–C#2"), then 10 H (idx 4-13).
  // A deterministic zigzag; coordinates are irrelevant to the mask check, only
  // the ELEMENTS matter for the label.
  const atoms: RawAtom[] = [];
  for (let i = 0; i < 4; i++) atoms.push({ element: "C", x: i, y: (i % 2), z: 0 });
  for (let i = 0; i < 10; i++)
    atoms.push({ element: "H", x: i * 0.5, y: 1 + Math.sin(i), z: Math.cos(i) });
  return { id, name: "butane", charge: 0, source: "editor", atoms };
}

describe("split-mask reference-atom rule (2.5.4a)", () => {
  const s = scene(butane());
  // The exact live-endpoint repro: mask returned by the sidecar for cut (1,2).
  const cut: [number, number] = [1, 2];
  const moving = 2;
  const mask = [2, 3, 9, 10, 11, 12, 13];
  const references = [3, 1]; // selection [3,1,2] minus the mover (2)

  it("sanity: the butane selection reaches needs-split with cut (1,2), moving 2", () => {
    const p = planEdit(s, idsFor(s, 3, 1, 2));
    expect(p.kind).toBe("needs-split");
    if (p.kind === "needs-split") {
      expect(p.op).toBe("angle");
      expect(p.cut).toEqual([1, 2]);
      expect(p.moving).toBe(2);
    }
  });

  it("flags the reference atom that landed on the moving side", () => {
    const v = maskRoleViolation(mask, moving, references);
    expect(v).not.toBeNull();
    expect(v!.referencesOnMovingSide).toEqual([3]); // ref 3 ∈ mask
    expect(v!.moverOffMovingSide).toBe(false); // mover 2 ∈ mask
  });

  it("passes a clean split (mover in, references out) → null", () => {
    // A hypothetical clean split of a different coordinate: mover in, refs out.
    expect(maskRoleViolation([3, 9, 10], 3, [1, 0])).toBeNull();
  });

  it("flags a mover missing from its own rotatable side", () => {
    const v = maskRoleViolation([9, 10], 2, [3, 1]);
    expect(v!.moverOffMovingSide).toBe(true);
  });

  it("explains the violation in selection terms (not the sidecar's wording)", () => {
    const v = maskRoleViolation(mask, moving, references)!;
    const msg = explainSplitViolation(s, cut, moving, v);
    expect(msg).toContain("#3"); // names the offending reference
    expect(msg).toContain("moving side");
    expect(msg).toContain("C#1–C#2"); // the bond, with elements
    expect(msg).toContain("static side"); // tells the user the fix
    expect(msg).not.toMatch(/fragment|inter-fragment/i); // NOT the sidecar text
  });

  it("pluralises when several references land on the moving side", () => {
    const v = maskRoleViolation([2, 3, 9], 2, [3, 9])!;
    const msg = explainSplitViolation(s, cut, moving, v);
    expect(msg).toMatch(/atoms .* lie on the moving side/);
    expect(msg).toContain("reference atoms on the static side");
  });
});

describe("planEdit — the immovable-axis refusal (2.5.2d-2)", () => {
  const s = scene(water(), borohydride());

  it("(e) 2+2 dihedral across fragments → the immovable-axis reason, names culprits", () => {
    // dihedral [1,0,3,4]: water {1,0} | BH4 {3,4}. Whichever end moves, an axis
    // atom (0 or 3) moves with it → no orientation holds the j–k axis fixed.
    const p = planEdit(s, idsFor(s, 1, 0, 3, 4));
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
