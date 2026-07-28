import { describe, it, expect } from "vitest";

import {
  FRAGMENT_LIBRARY,
  libraryFragmentToScene,
  type LibraryFragment,
} from "./fragment-library";
import type { SceneAtom } from "./types";

const dist = (atoms: SceneAtom[], i: number, j: number) =>
  Math.hypot(
    atoms[i].x - atoms[j].x,
    atoms[i].y - atoms[j].y,
    atoms[i].z - atoms[j].z,
  );

/** Angle (degrees) at vertex `j` between atoms `i` and `k`. */
function angle(atoms: SceneAtom[], i: number, j: number, k: number): number {
  const v1 = [atoms[i].x - atoms[j].x, atoms[i].y - atoms[j].y, atoms[i].z - atoms[j].z];
  const v2 = [atoms[k].x - atoms[j].x, atoms[k].y - atoms[j].y, atoms[k].z - atoms[j].z];
  const dot = v1[0] * v2[0] + v1[1] * v2[1] + v1[2] * v2[2];
  const n1 = Math.hypot(...v1);
  const n2 = Math.hypot(...v2);
  const cos = Math.max(-1, Math.min(1, dot / (n1 * n2)));
  return (Math.acos(cos) * 180) / Math.PI;
}

const byKey = (key: string): LibraryFragment =>
  FRAGMENT_LIBRARY.find((f) => f.key === key)!;

describe("FRAGMENT_LIBRARY geometry ↔ declared reference", () => {
  // The guard against invented numbers: recompute every declared internal FROM
  // the coordinates and require it to match (1e-3 Å bonds, 0.1° angles).
  FRAGMENT_LIBRARY.forEach((f) => {
    it(`${f.key}: coordinates satisfy its declared reference`, () => {
      for (const b of f.reference.bonds ?? []) {
        expect(Math.abs(dist(f.atoms, b.a, b.b) - b.value)).toBeLessThanOrEqual(
          1e-3,
        );
      }
      for (const a of f.reference.angles ?? []) {
        expect(
          Math.abs(angle(f.atoms, a.a, a.b, a.c) - a.value),
        ).toBeLessThanOrEqual(0.1);
      }
    });
  });
});

describe("FRAGMENT_LIBRARY invariants", () => {
  it("every entry has a non-empty provenance", () => {
    for (const f of FRAGMENT_LIBRARY) {
      expect(f.provenance.trim().length).toBeGreaterThan(0);
    }
  });

  it("keys are unique", () => {
    const keys = FRAGMENT_LIBRARY.map((f) => f.key);
    expect(new Set(keys).size).toBe(keys.length);
  });

  it("declares the expected charges (guards copy-paste)", () => {
    expect(byKey("bh4-").charge).toBe(-1);
    expect(byKey("water").charge).toBe(0);
    expect(byKey("nh3").charge).toBe(0);
    expect(byKey("cn-").charge).toBe(-1);
  });

  it("BH₄⁻ is tetrahedral (all six H–B–H angles ≈ 109.47°)", () => {
    const bh4 = byKey("bh4-").atoms; // B=0, H=1..4
    for (let i = 1; i <= 4; i++) {
      for (let j = i + 1; j <= 4; j++) {
        expect(Math.abs(angle(bh4, i, 0, j) - 109.47)).toBeLessThanOrEqual(0.1);
      }
    }
  });
});

describe("libraryFragmentToScene", () => {
  it("gives a fresh id each call and independent atoms", () => {
    const water = byKey("water");
    const a = libraryFragmentToScene(water);
    const b = libraryFragmentToScene(water);
    expect(a.id).not.toBe(b.id);
    expect(a.source).toBe("fragment-library");
    expect(a.sourceLabel).toBe("water");
    // Mutating one instance touches neither the other nor the library entry.
    a.atoms[0].x = 999;
    expect(b.atoms[0].x).not.toBe(999);
    expect(water.atoms[0].x).not.toBe(999);
  });

  it("carries the charge through", () => {
    expect(libraryFragmentToScene(byKey("bh4-")).charge).toBe(-1);
  });
});
