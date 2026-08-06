import { describe, it, expect } from "vitest";

import type { Scene } from "./types";
import type { AtomId } from "./ids";
import { makeAtomId } from "./ids";
import { testScene, idsFor, type RawFragment } from "./scene-test-util";
import { removeFragment } from "./scene";
import {
  MAX_SELECTION,
  toggleAtom,
  filterSelection,
  describeAtom,
  describeAtomById,
} from "./selection";

/** Bare integers → AtomIds, for the pure toggleAtom tests (the id is opaque). */
const id = (n: number): AtomId => makeAtomId(n);
const sel = (...ns: number[]): AtomId[] => ns.map(id);

// ── Fixtures: three fragments of DIFFERENT sizes (3 + 5 + 1 = 9 atoms) ────────

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
      { element: "B", x: 0.0, y: 0.0, z: 0.0 },
      { element: "H", x: d, y: d, z: d },
      { element: "H", x: -d, y: -d, z: d },
      { element: "H", x: -d, y: d, z: -d },
      { element: "H", x: d, y: -d, z: -d },
    ],
  };
}

function chloride(id = "cl"): RawFragment {
  return {
    id,
    name: "Cl-",
    charge: -1,
    source: "fragment-library",
    atoms: [{ element: "Cl", x: 3.0, y: 0.0, z: 0.0 }],
  };
}

function scene(...fragments: RawFragment[]): Scene {
  return testScene(fragments);
}

// ── toggleAtom ───────────────────────────────────────────────────────────────

describe("toggleAtom", () => {
  it("appends a new atom to the end (order preserved)", () => {
    expect(toggleAtom([], id(2))).toEqual(sel(2));
    expect(toggleAtom(sel(2), id(5))).toEqual(sel(2, 5));
    expect(toggleAtom(sel(2, 5), id(0))).toEqual(sel(2, 5, 0));
  });

  it("removes an already-selected atom (toggle-off)", () => {
    expect(toggleAtom(sel(2, 5, 0), id(5))).toEqual(sel(2, 0));
    expect(toggleAtom(sel(7), id(7))).toEqual([]);
  });

  it("is idempotent in pairs — toggling the same id twice restores the set", () => {
    const start = sel(1, 4);
    for (const idx of [id(4) /* present */, id(9) /* absent */]) {
      const twice = toggleAtom(toggleAtom(start, idx), idx);
      // Same *set* (order may differ: remove-then-add appends to the end).
      expect([...twice].sort()).toEqual([...start].sort());
    }
  });

  it("never grows beyond MAX_SELECTION; a full-list new click resets to [id]", () => {
    // Fill to the cap...
    let s: AtomId[] = [];
    for (let i = 0; i < MAX_SELECTION; i++) s = toggleAtom(s, id(i));
    expect(s).toHaveLength(MAX_SELECTION);
    // ...one more NEW atom is not FIFO — it collapses to just that atom.
    expect(toggleAtom(s, id(99))).toEqual(sel(99));
  });

  it("length never exceeds MAX_SELECTION for any click sequence", () => {
    let s: AtomId[] = [];
    for (const idx of [0, 1, 2, 3, 4, 5, 2, 6, 7, 8, 1, 0]) {
      s = toggleAtom(s, id(idx));
      expect(s.length).toBeLessThanOrEqual(MAX_SELECTION);
    }
  });

  it("does not mutate its input", () => {
    const input = sel(1, 2);
    toggleAtom(input, id(3));
    toggleAtom(input, id(1));
    expect(input).toEqual(sel(1, 2));
  });
});

// ── filterSelection (the 2c2 dividend) ───────────────────────────────────────
// Every assertion is on ids that address atoms across fragments, so a removal
// genuinely diverges "still in the scene" from "still in range".

describe("filterSelection", () => {
  it("keeps the selection (SAME reference) when an UNRELATED fragment is removed", () => {
    const s = scene(water(), borohydride(), chloride()); // wat 0-2, bh4 3-7, cl 8
    const picked = idsFor(s, 3, 4); // two BH₄⁻ atoms
    // Removing water (before them) OR chloride (after them) touches neither id —
    // the dividend: same reference back, no clear, no churn. The positional guard
    // this replaces cleared the whole selection on ANY composition change.
    expect(filterSelection(picked, removeFragment(s, "wat"))).toBe(picked);
    expect(filterSelection(picked, removeFragment(s, "cl"))).toBe(picked);
  });

  it("drops only the ids whose fragment was removed, keeping click order", () => {
    const s = scene(water(), borohydride(), chloride());
    const [cl8, b3, b5] = idsFor(s, 8, 3, 5); // chloride atom, then two BH₄⁻ atoms
    const after = removeFragment(s, "cl");
    expect(filterSelection([cl8, b3, b5], after)).toEqual([b3, b5]);
  });

  it("clears for a null scene; empty selection returns the same reference", () => {
    const s = scene(water());
    expect(filterSelection(idsFor(s, 0, 1), null)).toEqual([]);
    const empty: AtomId[] = [];
    expect(filterSelection(empty, s)).toBe(empty);
  });
});

// ── describeAtomById (the 2c2 id-native describe) ────────────────────────────

describe("describeAtomById", () => {
  it("follows the PHYSICAL atom after a removal shifts its global index", () => {
    // water(0,1,2) + BH₄⁻(3..7); the boron is AtomId 3, global 3.
    const s = scene(water(), borohydride());
    const boron = idsFor(s, 3)[0];
    expect(describeAtomById(s, boron)).toMatchObject({ element: "B", localIndex: 0 });
    // Remove water → boron keeps its id but is now global 0. describeAtomById
    // still names the boron; the STALE positional describeAtom(after, 3) names an H.
    const after = removeFragment(s, "wat");
    expect(describeAtomById(after, boron)).toMatchObject({ element: "B" });
    expect(describeAtom(after, 3)).toMatchObject({ element: "H" }); // the bug id-space avoids
  });

  it("returns null for an id no longer in the scene", () => {
    const s = scene(water(), borohydride());
    const boron = idsFor(s, 3)[0];
    expect(describeAtomById(removeFragment(s, "bh4"), boron)).toBeNull();
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
