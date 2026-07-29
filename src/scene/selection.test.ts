import { describe, it, expect } from "vitest";

import type { Scene, SceneFragment } from "./types";
import { removeFragment, compositionSignature } from "./scene";
import {
  MAX_SELECTION,
  toggleAtom,
  validateSelection,
  selectionSurvives,
  describeAtom,
} from "./selection";

// ── Fixtures: three fragments of DIFFERENT sizes (3 + 5 + 1 = 9 atoms) ────────

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
      { element: "B", x: 0.0, y: 0.0, z: 0.0 },
      { element: "H", x: d, y: d, z: d },
      { element: "H", x: -d, y: -d, z: d },
      { element: "H", x: -d, y: d, z: -d },
      { element: "H", x: d, y: -d, z: -d },
    ],
  };
}

function chloride(id = "cl"): SceneFragment {
  return {
    id,
    name: "Cl-",
    charge: -1,
    source: "fragment-library",
    atoms: [{ element: "Cl", x: 3.0, y: 0.0, z: 0.0 }],
  };
}

function scene(...fragments: SceneFragment[]): Scene {
  return { fragments, multiplicity: 1 };
}

// ── toggleAtom ───────────────────────────────────────────────────────────────

describe("toggleAtom", () => {
  it("appends a new atom to the end (order preserved)", () => {
    expect(toggleAtom([], 2)).toEqual([2]);
    expect(toggleAtom([2], 5)).toEqual([2, 5]);
    expect(toggleAtom([2, 5], 0)).toEqual([2, 5, 0]);
  });

  it("removes an already-selected atom (toggle-off)", () => {
    expect(toggleAtom([2, 5, 0], 5)).toEqual([2, 0]);
    expect(toggleAtom([7], 7)).toEqual([]);
  });

  it("is idempotent in pairs — toggling the same index twice restores the set", () => {
    const start = [1, 4];
    for (const idx of [4 /* present */, 9 /* absent */]) {
      const twice = toggleAtom(toggleAtom(start, idx), idx);
      // Same *set* (order may differ: remove-then-add appends to the end).
      expect([...twice].sort()).toEqual([...start].sort());
    }
  });

  it("never grows beyond MAX_SELECTION; a full-list new click resets to [index]", () => {
    // Fill to the cap...
    let sel: number[] = [];
    for (let i = 0; i < MAX_SELECTION; i++) sel = toggleAtom(sel, i);
    expect(sel).toHaveLength(MAX_SELECTION);
    // ...one more NEW atom is not FIFO — it collapses to just that atom.
    const after = toggleAtom(sel, 99);
    expect(after).toEqual([99]);
  });

  it("length never exceeds MAX_SELECTION for any click sequence", () => {
    let sel: number[] = [];
    for (const idx of [0, 1, 2, 3, 4, 5, 2, 6, 7, 8, 1, 0]) {
      sel = toggleAtom(sel, idx);
      expect(sel.length).toBeLessThanOrEqual(MAX_SELECTION);
    }
  });

  it("does not mutate its input", () => {
    const input = [1, 2];
    toggleAtom(input, 3);
    toggleAtom(input, 1);
    expect(input).toEqual([1, 2]);
  });
});

// ── validateSelection ────────────────────────────────────────────────────────

describe("validateSelection", () => {
  it("returns the SAME reference when nothing is out of range", () => {
    const s = scene(water(), borohydride()); // 8 atoms → indices 0..7
    const sel = [0, 3, 7];
    expect(validateSelection(sel, s)).toBe(sel);
  });

  it("drops indices past the end of the scene", () => {
    const s = scene(water()); // 3 atoms → 0..2
    expect(validateSelection([0, 2, 5, 3], s)).toEqual([0, 2]);
  });

  it("clears everything for a null scene", () => {
    expect(validateSelection([0, 1, 2], null)).toEqual([]);
  });

  // The defect the 2.5.2b review found: range-only validation SURVIVES an index
  // shift. Removing the first fragment slides every later atom down, so a picked
  // index that is still in range silently re-points at a DIFFERENT atom. This
  // test documents that `validateSelection` alone cannot catch it — which is why
  // `selectionSurvives` (below) is the primary guard. (The old test here passed
  // for the wrong reason: it used index 8, which merely fell out of range.)
  it("does NOT clean an in-range index that a removal re-pointed (range-only)", () => {
    // water(0,1,2) + BH4-(3..7); pick global 3 = the boron (BH4- local 0).
    const s = scene(water(), borohydride());
    const sel = [3];
    expect(describeAtom(s, 3)).toMatchObject({ element: "B", localIndex: 0 });
    // Remove water → 5 atoms remain (0..4); global 3 is now an H (BH4- local 3).
    const after = removeFragment(s, "wat");
    expect(describeAtom(after, 3)).toMatchObject({ element: "H", localIndex: 3 });
    // Range-only check keeps it — the selection silently moved boron → hydrogen.
    expect(validateSelection(sel, after)).toEqual([3]);
  });
});

// ── selectionSurvives (2.5.2b — the composition-signature guard) ──────────────

describe("selectionSurvives", () => {
  it("survives an unchanged signature (a coordinate-only edit)", () => {
    const sig = "a:3|b:5";
    expect(selectionSurvives(sig, sig)).toBe(true);
  });

  it("survives a pure append (a fragment added LAST — indices don't shift)", () => {
    expect(selectionSurvives("a:3", "a:3|b:5")).toBe(true);
    expect(selectionSurvives("a:3|b:5", "a:3|b:5|c:1")).toBe(true);
  });

  it("does NOT append-match on a size prefix (the trailing '|' is load-bearing)", () => {
    // "a:3" must not be read as a prefix of "a:30|b:2".
    expect(selectionSurvives("a:3", "a:30|b:2")).toBe(false);
  });

  it("does not survive a removal, a recomposition, or a cleared scene", () => {
    expect(selectionSurvives("a:3|b:5", "b:5")).toBe(false); // first removed
    expect(selectionSurvives("a:3|b:5", "a:3")).toBe(false); // last removed
    expect(selectionSurvives("a:3|b:5", "a:4|b:5")).toBe(false); // count changed
    expect(selectionSurvives("a:3", null)).toBe(false); // scene cleared
    expect(selectionSurvives(null, "a:3")).toBe(false); // scene appeared
  });

  it("survives when both are null (no scene throughout)", () => {
    expect(selectionSurvives(null, null)).toBe(true);
  });

  // The full reproduction, driven through real signatures: removing the FIRST
  // fragment must NOT survive, so NewJobScreen clears the selection instead of
  // letting index 3 re-point boron → hydrogen (the defect above).
  it("clears the selection on removeFragment of the FIRST fragment", () => {
    const s = scene(water(), borohydride()); // sig: wat:3|bh4:5
    const after = removeFragment(s, "wat"); // sig: bh4:5
    const survives = selectionSurvives(
      compositionSignature(s),
      compositionSignature(after),
    );
    expect(survives).toBe(false); // → NewJobScreen setSelection([])
  });
});

// ── describeAtom ─────────────────────────────────────────────────────────────

describe("describeAtom", () => {
  const s = scene(water(), borohydride(), chloride());

  it("agrees with the global index across three fragments of different sizes", () => {
    // global 0 → water O (fragment 0, local 0)
    expect(describeAtom(s, 0)).toMatchObject({
      element: "O",
      fragmentId: "wat",
      fragmentName: "Water",
      fragmentIndex: 0,
      localIndex: 0,
    });
    // global 3 → BH4- B (fragment 1, local 0)
    expect(describeAtom(s, 3)).toMatchObject({
      element: "B",
      fragmentId: "bh4",
      fragmentIndex: 1,
      localIndex: 0,
    });
    // global 7 → BH4- last H (fragment 1, local 4)
    expect(describeAtom(s, 7)).toMatchObject({
      element: "H",
      fragmentId: "bh4",
      fragmentIndex: 1,
      localIndex: 4,
    });
    // global 8 → Cl- (fragment 2, local 0)
    expect(describeAtom(s, 8)).toMatchObject({
      element: "Cl",
      fragmentId: "cl",
      fragmentIndex: 2,
      localIndex: 0,
    });
  });

  it("carries the atom's coordinates verbatim", () => {
    const d = describeAtom(s, 1); // water H1
    expect(d).toMatchObject({ x: 0.0, y: 0.755453, z: -0.471161 });
  });

  it("returns null for an out-of-range index (no throw)", () => {
    expect(describeAtom(s, 9)).toBeNull();
    expect(describeAtom(s, -1)).toBeNull();
  });
});
