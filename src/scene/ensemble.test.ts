import { describe, it, expect } from "vitest";

import {
  conformerMatchesFragment,
  goatInputForFragment,
  parseEnsemble,
  type Conformer,
} from "./ensemble";
import type { SceneFragment } from "./types";

// A real (truncated to 3 structures) `butane.finalensemble.xyz` from an actual
// ORCA 6.1.0 `! XTB GOAT` run — declaration must agree with reality (§ the rule
// that saved the fragment library). Loaded as a raw string (Vite `?raw`).
import ENSEMBLE from "./__fixtures__/butane.finalensemble.xyz?raw";

/** A butane fragment whose atoms are the ensemble's first conformer. */
function butaneFragment(conformers: Conformer[]): SceneFragment {
  return {
    id: "but",
    name: "butane",
    charge: 0,
    source: "smiles",
    atoms: conformers[0].atoms.map((a) => ({ ...a })),
  };
}

describe("parseEnsemble (real GOAT fixture)", () => {
  it("parses every structure in the file", () => {
    const c = parseEnsemble(ENSEMBLE)!;
    expect(c).not.toBeNull();
    expect(c).toHaveLength(3);
    expect(c.every((f) => f.atoms.length === 14)).toBe(true);
    expect(c.map((f) => f.index)).toEqual([0, 1, 2]);
  });

  it("reads the energy from the comment's leading token (Eh)", () => {
    const c = parseEnsemble(ENSEMBLE)!;
    expect(c[0].energy).toBeCloseTo(-13.6651277570, 8);
    expect(Number.isFinite(c[0].energy)).toBe(true);
  });

  it("the ensemble is sorted ascending by energy (verified on the real file)", () => {
    const c = parseEnsemble(ENSEMBLE)!;
    for (let i = 1; i < c.length; i++) {
      expect(c[i].energy).toBeGreaterThan(c[i - 1].energy);
    }
  });

  it("every structure has the same element sequence as the first", () => {
    const c = parseEnsemble(ENSEMBLE)!;
    const first = c[0].atoms.map((a) => a.element);
    for (const conf of c) {
      expect(conf.atoms.map((a) => a.element)).toEqual(first);
    }
    // GOAT preserved the input order: 4 carbons then 10 hydrogens.
    expect(first).toEqual(["C", "C", "C", "C", ...Array(10).fill("H")]);
  });

  it("leaves energy NaN when the comment has no leading number (never invents)", () => {
    const oneAtom = "1\nno energy here\nH 0 0 0\n";
    const c = parseEnsemble(oneAtom)!;
    expect(c).toHaveLength(1);
    expect(Number.isNaN(c[0].energy)).toBe(true);
  });

  it("returns null on empty / garbage input (never throws)", () => {
    expect(parseEnsemble("")).toBeNull();
    expect(parseEnsemble("   \n  \n")).toBeNull();
    expect(parseEnsemble("not a molecule at all")).toBeNull();
    expect(parseEnsemble("3\ncomment\nO 0 0 0\n")).toBeNull(); // count 3 but 1 atom
    expect(parseEnsemble("2\nc\nO 0 0\nH 0 0 0")).toBeNull(); // short atom row
  });
});

describe("conformerMatchesFragment", () => {
  const conformers = parseEnsemble(ENSEMBLE)!;

  it("is true for the fragment's own conformer", () => {
    const frag = butaneFragment(conformers);
    expect(conformerMatchesFragment(frag, conformers[0])).toBe(true);
    expect(conformerMatchesFragment(frag, conformers[2])).toBe(true);
  });

  it("is false for a different atom count", () => {
    const frag = butaneFragment(conformers);
    frag.atoms = frag.atoms.slice(0, 13);
    expect(conformerMatchesFragment(frag, conformers[0])).toBe(false);
  });

  it("is false for a different element sequence", () => {
    const frag = butaneFragment(conformers);
    frag.atoms = frag.atoms.map((a, i) =>
      i === 0 ? { ...a, element: "N" } : a,
    );
    expect(conformerMatchesFragment(frag, conformers[0])).toBe(false);
  });
});

describe("goatInputForFragment", () => {
  it("emits `! XTB GOAT` and the fragment's own charge (not the scene total)", () => {
    const bh4: SceneFragment = {
      id: "bh4",
      name: "BH4-",
      charge: -1,
      source: "fragment-library",
      atoms: [
        { element: "B", x: 0, y: 0, z: 0 },
        { element: "H", x: 1, y: 1, z: 1 },
      ],
    };
    const inp = goatInputForFragment(bh4);
    expect(inp).toContain("! XTB GOAT");
    expect(inp).toContain("* xyz -1 1"); // fragment.charge, default mult 1
    expect(inp.trimEnd().endsWith("*")).toBe(true);
    // 2 atom rows between header and closing *.
    expect(inp.match(/^[A-Z][a-z]?\s/gm)).toHaveLength(2);
  });
});
