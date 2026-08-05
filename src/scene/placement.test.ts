import { describe, it, expect } from "vitest";

import { placeFragment } from "./placement";
import type { RawAtom, RawFragment, Scene } from "./types";
import { testScene } from "./scene-test-util";

function frag(id: string, atoms: RawAtom[]): RawFragment {
  return { id, name: id, atoms, charge: 0, source: "editor" };
}

function scene(...fragments: RawFragment[]): Scene {
  return testScene(fragments);
}

const dist = (p: RawAtom, q: RawAtom) =>
  Math.hypot(p.x - q.x, p.y - q.y, p.z - q.z);

/** Minimum distance between any atom of `a` and any atom of `b`. */
function minCross(a: RawAtom[], b: RawAtom[]): number {
  let m = Infinity;
  for (const p of a) for (const q of b) m = Math.min(m, dist(p, q));
  return m;
}

function centroidX(atoms: RawAtom[]): number {
  return atoms.reduce((s, a) => s + a.x, 0) / atoms.length;
}

// A small bent triangle (water-like) and a compact 4-atom cluster (BH4-like).
const water: RawAtom[] = [
  { element: "O", x: 0, y: 0, z: 0 },
  { element: "H", x: 0.76, y: 0, z: 0.59 },
  { element: "H", x: -0.76, y: 0, z: 0.59 },
];
const cluster: RawAtom[] = [
  { element: "B", x: 0, y: 0, z: 0 },
  { element: "H", x: 0.72, y: 0.72, z: 0.72 },
  { element: "H", x: -0.72, y: -0.72, z: 0.72 },
  { element: "H", x: 0.72, y: -0.72, z: -0.72 },
];

const GAP = 3.5;

describe("placeFragment", () => {
  it("separates the fragment from the scene by at least the gap", () => {
    const placed = placeFragment(scene(frag("s", water)), frag("r", cluster));
    expect(minCross(water, placed.atoms)).toBeGreaterThanOrEqual(GAP - 1e-9);
  });

  it("respects a custom gap", () => {
    const placed = placeFragment(scene(frag("s", water)), frag("r", cluster), 5);
    expect(minCross(water, placed.atoms)).toBeGreaterThanOrEqual(5 - 1e-9);
  });

  it("preserves the fragment's internal geometry (a translation, not a warp)", () => {
    const original = cluster;
    const placed = placeFragment(scene(frag("s", water)), frag("r", cluster));
    expect(placed.atoms).toHaveLength(original.length);
    for (let i = 0; i < original.length; i++) {
      expect(placed.atoms[i].element).toBe(original[i].element);
      for (let j = i + 1; j < original.length; j++) {
        expect(dist(placed.atoms[i], placed.atoms[j])).toBeCloseTo(
          dist(original[i], original[j]),
          9, // 1e-9
        );
      }
    }
  });

  it("approaches an X-elongated substrate off-axis (not down the chain)", () => {
    // Linear chain ~10 Å along x, no extent in y/z → smallest axis is y or z.
    const chain: RawAtom[] = Array.from({ length: 6 }, (_, i) => ({
      element: "C",
      x: i * 2,
      y: 0,
      z: 0,
    }));
    const placed = placeFragment(scene(frag("c", chain)), frag("r", cluster));
    const xs = chain.map((a) => a.x);
    // Placed off the x-axis ⇒ centred over the chain in x, so its centroid x
    // stays inside the chain's x-span (it would sit beyond max-x if placed along x).
    const cx = centroidX(placed.atoms);
    expect(cx).toBeGreaterThanOrEqual(Math.min(...xs) - 1e-9);
    expect(cx).toBeLessThanOrEqual(Math.max(...xs) + 1e-9);
    // And still clear of the chain.
    expect(minCross(chain, placed.atoms)).toBeGreaterThanOrEqual(GAP - 1e-9);
  });

  it("returns the first fragment unmoved for an empty scene", () => {
    const placed = placeFragment(scene(), frag("r", cluster));
    placed.atoms.forEach((a, i) => {
      expect(a.x).toBe(cluster[i].x);
      expect(a.y).toBe(cluster[i].y);
      expect(a.z).toBe(cluster[i].z);
    });
  });

  it("handles a monatomic fragment (Cl⁻)", () => {
    const cl: RawAtom[] = [{ element: "Cl", x: 0, y: 0, z: 0 }];
    const placed = placeFragment(scene(frag("s", water)), frag("cl", cl));
    expect(minCross(water, placed.atoms)).toBeGreaterThanOrEqual(GAP - 1e-9);
  });

  it("clears both the substrate and an already-placed reagent on a second add", () => {
    const substrate = frag("s", water);
    const first = placeFragment(scene(substrate), frag("r1", cluster));
    const second = placeFragment(scene(substrate, first), frag("r2", cluster));
    expect(minCross(water, second.atoms)).toBeGreaterThanOrEqual(GAP - 1e-9);
    expect(minCross(first.atoms, second.atoms)).toBeGreaterThanOrEqual(GAP - 1e-9);
  });
});
