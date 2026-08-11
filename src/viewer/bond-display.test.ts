import { describe, it, expect } from "vitest";

import type { AtomId } from "../scene/ids";
import {
  shouldDrawBond,
  filterDrawnBonds,
  applyGeometricBondOrders,
  bondKey,
  isCationBond,
  type FilterableAtom,
  type BondKey,
  type OrderableAtom,
} from "./bond-display";
import { bondOrderEstimate } from "../scene/bond-edit";
import { testScene, type RawFragment } from "../scene/scene-test-util";
import { buildViewerFeed } from "../scene/scene";

const id = (n: number): AtomId => n as unknown as AtomId;
const NONE: ReadonlySet<BondKey> = new Set();

// ── c1 — cations are excluded; the list is the coordinating s-block metals ─────
describe("(c1) cation coordinate bonds are not drawn", () => {
  it("a bond with a cation end is NOT drawn; a covalent bond IS", () => {
    expect(shouldDrawBond("Na", "O", id(1), id(2), NONE)).toBe(false); // Na⁺···O=C
    expect(shouldDrawBond("Na", "H", id(1), id(2), NONE)).toBe(false); // Na⁺···H-aromatic
    expect(shouldDrawBond("C", "O", id(1), id(2), NONE)).toBe(true); // a real C=O
    expect(shouldDrawBond("C", "H", id(1), id(2), NONE)).toBe(true);
  });

  it("covers Li / Na / K / Mg / Ca (the seed cations and their neighbours)", () => {
    for (const m of ["Li", "Na", "K", "Mg", "Ca"]) {
      // The BITE: if any of these were left out of CATION_ELEMENTS, its coordinate
      // bond would be drawn and this would go red.
      expect(isCationBond(m, "O")).toBe(true);
      expect(shouldDrawBond(m, "O", id(1), id(2), NONE)).toBe(false);
    }
  });

  it("is NOT a blanket '+charge' rule: H and N (H⁺/NH₄⁺) and transition metals bond normally", () => {
    expect(isCationBond("H", "O")).toBe(false); // O–H is a real bond
    expect(isCationBond("N", "H")).toBe(false); // NH₄⁺ is covalent
    expect(isCationBond("Pd", "C")).toBe(false); // organometallic M–L is real (ADR-007)
  });

  it("the optional 'show cation bonds' toggle draws them again", () => {
    expect(shouldDrawBond("Na", "O", id(1), id(2), NONE, { showCationBonds: true })).toBe(true);
  });
});

// ── c2 — manual hide keyed by AtomId PAIR survives a global-index shift ────────
describe("(c2) a hidden bond is keyed by AtomId pair, not position", () => {
  // Four atoms, all carbon so the cation rule is irrelevant — this isolates the
  // hide mechanism. A bond between the atoms carrying AtomId 12 and 13.
  function layout(idsByIndex: AtomId[]): FilterableAtom[] {
    // A simple chain 0-1-2-3 (each bonded to its neighbours) with viewer indices ==
    // array positions, elements all C.
    const atoms: FilterableAtom[] = idsByIndex.map((_, i) => ({
      index: i,
      elem: "C",
      bonds: [] as number[],
    }));
    for (let i = 0; i + 1 < atoms.length; i++) {
      atoms[i].bonds.push(i + 1);
      atoms[i + 1].bonds.push(i);
    }
    return atoms;
  }
  const resolver = (idsByIndex: AtomId[]) => (vi: number) => idsByIndex[vi];

  it("hides exactly the keyed pair, in ANY index layout", () => {
    const hidden = new Set<BondKey>([bondKey(id(12), id(13))]);

    // Layout A: ids [10,11,12,13] at indices [0,1,2,3] — the 12-13 bond is 2-3.
    const idsA = [id(10), id(11), id(12), id(13)];
    const a = layout(idsA);
    const removedA = filterDrawnBonds(a, resolver(idsA), hidden);
    expect(removedA).toBe(1);
    expect(a[2].bonds).not.toContain(3); // the 2-3 bond (ids 12-13) is gone
    expect(a[3].bonds).not.toContain(2);
    expect(a[0].bonds).toContain(1); // the 0-1 bond (ids 10-11) survives

    // Layout B (the 2c2 shift): the SAME atoms 12,13 now sit at indices 0,1 (the
    // earlier atoms were removed). The AtomId key must still hide the 12-13 bond,
    // now the 0-1 bond.
    const idsB = [id(12), id(13)];
    const b = layout(idsB);
    const removedB = filterDrawnBonds(b, resolver(idsB), hidden);
    expect(removedB).toBe(1);
    expect(b[0].bonds).not.toContain(1); // 0-1 (ids 12-13) hidden — the RIGHT bond
    expect(b[1].bonds).not.toContain(0);
  });

  it("the BITE: a POSITIONAL key would hide the wrong bond after the shift", () => {
    // A positional hide of viewer pair {2,3} (correct in layout A) applied to layout
    // B targets indices 2,3 — DIFFERENT atoms (here absent), so the real 12-13 bond
    // (now 0-1) would stay drawn. We show the AtomId key does NOT have this failure.
    const idsB = [id(12), id(13)];
    const b = layout(idsB);
    // AtomId key → correct bond hidden:
    filterDrawnBonds(b, resolver(idsB), new Set([bondKey(id(12), id(13))]));
    expect(b[0].bonds).not.toContain(1);

    // A positional scheme keyed on "{2,3}" cannot even name atoms 12,13 here — it
    // would address indices 2,3 which don't exist in the 2-atom layout, so the
    // forming bond would remain. The AtomId key is what makes the shift safe.
    const b2 = layout(idsB);
    const positionalMiss = filterDrawnBonds(b2, () => undefined, new Set(["2:3"]));
    expect(positionalMiss).toBe(0); // nothing matched → the real bond stayed drawn
    expect(b2[0].bonds).toContain(1);
  });
});

// ── c3 — DISPLAY-ONLY: the filter never touches the Scene or its xyz ───────────
describe("(c3) filtering is display-only — Scene and generated xyz are untouched", () => {
  function scene() {
    const frag: RawFragment = {
      id: "f",
      name: "NaOMe-ish",
      charge: 0,
      source: "editor",
      atoms: [
        { element: "Na", x: 0, y: 0, z: 0 },
        { element: "O", x: 2.2, y: 0, z: 0 }, // within 3Dmol distance → spurious Na–O
        { element: "C", x: 3.6, y: 0, z: 0 },
      ],
    };
    return testScene([frag]);
  }

  it("running the filter leaves the Scene and buildViewerFeed(xyz) byte-identical", () => {
    const s = scene();
    const before = buildViewerFeed(s).xyz;
    const sJson = JSON.stringify(s);

    // Build a 3Dmol-like atom array and filter it (removes the Na–O cation bond).
    const atoms: FilterableAtom[] = [
      { index: 0, elem: "Na", bonds: [1] },
      { index: 1, elem: "O", bonds: [0, 2] },
      { index: 2, elem: "C", bonds: [1] },
    ];
    const table = buildViewerFeed(s).table;
    const removed = filterDrawnBonds(atoms, (i) => table.atomIdAt(i), NONE);
    expect(removed).toBe(1); // the Na–O stick

    // The BITE would be a filter that wrote back into the Scene: geometry is the
    // source of truth for Generate/Run, and it must be identical.
    expect(buildViewerFeed(s).xyz).toBe(before);
    expect(JSON.stringify(s)).toBe(sJson);
  });
});

// ── c4 — orthogonal to representation + frozenTopology; no double perception ───
describe("(c4) filter removes only excluded bonds and is idempotent", () => {
  // Methane-like C(H4) with a stray Na⁺ coordinated to one H — the Na bond is
  // spurious, the four C–H are real.
  function atoms(): FilterableAtom[] {
    return [
      { index: 0, elem: "C", bonds: [1, 2, 3, 4] },
      { index: 1, elem: "H", bonds: [0] },
      { index: 2, elem: "H", bonds: [0] },
      { index: 3, elem: "H", bonds: [0] },
      { index: 4, elem: "H", bonds: [0, 5] }, // H4 also "bonded" to Na by distance
      { index: 5, elem: "Na", bonds: [4] },
    ];
  }
  const resolveId = (i: number) => id(i);

  it("removes ONLY the cation bond; the covalent bonds all survive", () => {
    const a = atoms();
    const removed = filterDrawnBonds(a, resolveId, NONE);
    expect(removed).toBe(1); // just Na–H4
    // All four C–H intact (no collateral loss — the frozenTopology animation and
    // stick/line rendering downstream draw the full molecule minus the cation stick).
    expect(a[0].bonds.slice().sort()).toEqual([1, 2, 3, 4]);
    expect(a[4].bonds).toEqual([0]); // H4 keeps C, loses Na
    expect(a[5].bonds).toEqual([]); // Na left with none
  });

  it("is idempotent — a second pass removes nothing (no double perception)", () => {
    const a = atoms();
    filterDrawnBonds(a, resolveId, NONE);
    const again = filterDrawnBonds(a, resolveId, NONE);
    expect(again).toBe(0);
  });
});

// ── c5 — geometric bond order (2/3 lines) is re-derived from geometry, not stored ─
// applyGeometricBondOrders overwrites each drawn bond's `bondOrder` from the current
// interatomic distance (nearest single/double/triple sum), so 3Dmol draws 2/3 sticks.
// DISPLAY-ONLY, nothing stored: the order is a function of geometry every rebuild.
describe("(c5) geometric bond order — re-derived from geometry each pass", () => {
  const estimate = (a: string, b: string, d: number) => bondOrderEstimate(a, b, d).order;

  // Two carbons `dist` Å apart, each carrying a (deliberately wrong) stored order.
  function ccPair(dist: number, storedOrder: number): OrderableAtom[] {
    return [
      { index: 0, elem: "C", x: 0, y: 0, z: 0, bonds: [1], bondOrder: [storedOrder] },
      { index: 1, elem: "C", x: dist, y: 0, z: 0, bonds: [0], bondOrder: [storedOrder] },
    ];
  }

  it("sets order 1 / 2 / 3 from a single / double / triple C–C length", () => {
    for (const [dist, order] of [[1.54, 1], [1.34, 2], [1.2, 3]] as const) {
      const atoms = ccPair(dist, 1);
      applyGeometricBondOrders(atoms, estimate);
      expect(atoms[0].bondOrder![0]).toBe(order);
      expect(atoms[1].bondOrder![0]).toBe(order); // both half-edges agree
    }
  });

  it("NEGATIVE control — reads GEOMETRY, not the stored bondOrder", () => {
    // A long single-bond geometry (1.54) but a stored double-bond order → must be
    // OVERWRITTEN to 1. A bite that trusted the stored order would keep 2.
    const atoms = ccPair(1.54, 2);
    applyGeometricBondOrders(atoms, estimate);
    expect(atoms[0].bondOrder![0]).toBe(1);
  });

  it("an element with no double/triple radius (C–H) stays a single line, never throws", () => {
    const atoms: OrderableAtom[] = [
      { index: 0, elem: "C", x: 0, y: 0, z: 0, bonds: [1], bondOrder: [1] },
      { index: 1, elem: "H", x: 1.09, y: 0, z: 0, bonds: [0], bondOrder: [1] },
    ];
    expect(() => applyGeometricBondOrders(atoms, estimate)).not.toThrow();
    expect(atoms[0].bondOrder![0]).toBe(1);
  });
});
