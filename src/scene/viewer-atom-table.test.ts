/**
 * ViewerAtomTable (unit 2c1) — the AtomId↔viewer-index table 3Dmol is fed
 * alongside the geometry. The tests assert the ONE property the whole unit exists
 * to guarantee: the table names the geometry it was built with, so a pick resolves
 * to the physical atom actually drawn — even after the composition changes.
 *
 * The two negative controls (a)/(b) are the demonstrated bites (CLAUDE.md: a gate
 * whose ability to fail is not shown is green for an unknown reason). Each
 * constructs the wrong table — one NOT rebuilt with the geometry (staleness), one
 * with its direction reversed — and confirms it resolves a pick to the WRONG
 * physical atom, which the correct construction does not.
 */

import { describe, expect, it } from "vitest";

import type { AtomId } from "./ids";
import { buildViewerAtomTable, buildViewerFeed, mergeToXyz, removeFragment } from "./scene";
import { testScene, type RawFragment } from "./scene-test-util";
import type { Scene } from "./types";

/** A fragment of `nAtoms` carbons; `id === name` for deterministic assertions. */
function frag(name: string, nAtoms: number): RawFragment {
  return {
    id: name,
    name,
    charge: 0,
    source: "editor",
    atoms: Array.from({ length: nAtoms }, (_, i) => ({
      element: "C",
      x: i,
      y: 0,
      z: 0,
    })),
  };
}

/** Which fragment holds the atom with this id (its `id`/name), or undefined. */
function fragmentOf(scene: Scene, id: AtomId): string | undefined {
  return scene.fragments.find((f) => f.atoms.some((a) => a.id === id))?.id;
}

describe("ViewerAtomTable — built with the geometry", () => {
  it("is a bijection consistent with the order fed to 3Dmol", () => {
    const s = testScene([frag("A", 3), frag("B", 2), frag("C", 1)]);
    const { xyz, table } = buildViewerFeed(s);
    const atoms = s.fragments.flatMap((f) => f.atoms);

    expect(table.length).toBe(atoms.length);
    // Forward: viewer index i names the atom on merged-xyz line i.
    atoms.forEach((a, i) => expect(table.atomIdAt(i)).toBe(a.id));
    // Inverse round-trips both ways — a bijection.
    atoms.forEach((a, i) => {
      expect(table.viewerIndexOf(a.id)).toBe(i);
      expect(table.atomIdAt(table.viewerIndexOf(a.id)!)).toBe(a.id);
    });
    // Out of range on both directions.
    expect(table.atomIdAt(atoms.length)).toBeUndefined();
    expect(table.atomIdAt(-1)).toBeUndefined();
    expect(table.viewerIndexOf(9999 as AtomId)).toBeUndefined();

    // The table's order IS the geometry's: one line per atom, count matches.
    const lines = xyz.trimEnd().split("\n").slice(2);
    expect(lines.length).toBe(atoms.length);
  });

  it("draws byte-identical geometry to mergeToXyz (only the table is new)", () => {
    const s = testScene([frag("A", 3), frag("B", 2)]);
    expect(buildViewerFeed(s).xyz).toBe(mergeToXyz(s));
  });

  it("buildViewerAtomTable agrees with the feed's table (same source, no drift)", () => {
    const s = testScene([frag("A", 3), frag("B", 2)]);
    const feed = buildViewerFeed(s);
    const alone = buildViewerAtomTable(s);
    for (let i = 0; i < feed.table.length; i++) {
      expect(alone.atomIdAt(i)).toBe(feed.table.atomIdAt(i));
    }
  });

  it("a pick resolves to the SAME physical atom after a fragment is removed", () => {
    const s1 = testScene([frag("A", 3), frag("B", 2)]);
    const s2 = removeFragment(s1, "A");
    const feed1 = buildViewerFeed(s1);
    const feed2 = buildViewerFeed(s2);

    // Fragment B's first atom is one physical atom with a stable AtomId. It is
    // drawn at viewer index 3 in s1 and at index 0 in s2 — the tables, rebuilt
    // with the geometry, name it correctly at BOTH indices.
    const physical = s1.fragments[1].atoms[0].id;
    expect(feed1.table.viewerIndexOf(physical)).toBe(3);
    expect(feed2.table.viewerIndexOf(physical)).toBe(0);
    expect(feed2.table.atomIdAt(0)).toBe(physical);
  });
});

describe("ViewerAtomTable — negative controls (demonstrated bites)", () => {
  it("(a) a table NOT rebuilt with the geometry mis-resolves the pick", () => {
    // Simulate staleness: keep s1's table and use it to interpret a pick on s2's
    // drawn geometry (the fragment was removed). This is the silent worst case the
    // unit prevents — a click returns a DIFFERENT atom's id.
    const s1 = testScene([frag("A", 3), frag("B", 2)]);
    const s2 = removeFragment(s1, "A");
    const stale = buildViewerFeed(s1).table; // built from s1 — the wrong geometry
    const fresh = buildViewerFeed(s2).table; // rebuilt with s2 — correct

    const drawnAtIndex0 = fresh.atomIdAt(0)!; // the atom actually drawn at slot 0
    // The correct (rebuilt) table names it; the stale table names a now-removed
    // atom for the same drawn slot — the bite.
    expect(fresh.atomIdAt(0)).toBe(drawnAtIndex0);
    expect(stale.atomIdAt(0)).not.toBe(drawnAtIndex0);
    // And the stale table's answer is an atom no longer in the scene at all.
    expect(fragmentOf(s2, stale.atomIdAt(0)!)).toBeUndefined();
  });

  it("(b) reversing the table direction returns a cross-fragment atom", () => {
    // Asymmetric fragments (3 + 2) so a reversal lands in the OTHER fragment — on
    // symmetric sizes a reversal could coincidentally stay within the right one.
    const s = testScene([frag("A", 3), frag("B", 2)]);
    const { table } = buildViewerFeed(s);
    const n = table.length;
    const correct = Array.from({ length: n }, (_, i) => table.atomIdAt(i)!);
    const reversed = [...correct].reverse(); // the broken direction

    const drawnFirst = correct[0]; // atom drawn at viewer index 0 — fragment A
    const reversedFirst = reversed[0]; // what the reversed table would return

    expect(reversedFirst).not.toBe(drawnFirst); // the reversal is visible
    // …and it points into a DIFFERENT fragment than the one actually drawn.
    expect(fragmentOf(s, reversedFirst)).toBe("B");
    expect(fragmentOf(s, drawnFirst)).toBe("A");
    expect(fragmentOf(s, reversedFirst)).not.toBe(fragmentOf(s, drawnFirst));
  });
});
