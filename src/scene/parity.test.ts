import { describe, it, expect } from "vitest";

import type { Scene } from "./types";
import { testScene, type RawFragment } from "./scene-test-util";
import { checkElectronParity } from "./parity";

/** A fragment with the given atoms/charge (geometry is irrelevant to parity). */
function frag(
  id: string,
  elements: string[],
  charge = 0,
): RawFragment {
  return {
    id,
    name: id,
    charge,
    source: "editor",
    atoms: elements.map((element) => ({ element, x: 0, y: 0, z: 0 })),
  };
}

function scene(multiplicity: number, ...fragments: RawFragment[]): Scene {
  return testScene(fragments, multiplicity);
}

const water = (c = 0) => frag("water", ["O", "H", "H"], c); // 10 e⁻ neutral
const bh4 = () => frag("bh4", ["B", "H", "H", "H", "H"], -1); // 10 e⁻ (9 + 1)
const ch3 = (c = 0) => frag("ch3", ["C", "H", "H", "H"], c); // 9 protons

describe("checkElectronParity", () => {
  it("accepts neutral water (10 e⁻) as a singlet", () => {
    expect(checkElectronParity(scene(1, water()))).toBeNull();
  });

  it("flags water as a doublet, suggesting [1, 3, 5]", () => {
    const issue = checkElectronParity(scene(2, water()));
    expect(issue).not.toBeNull();
    expect(issue!.kind).toBe("parity-mismatch");
    expect(issue!.electrons).toBe(10);
    expect(issue!.multiplicity).toBe(2);
    expect(issue!.suggested).toEqual([1, 3, 5]);
  });

  it("accepts BH4- (charge −1 ⇒ 10 e⁻, even) as a singlet", () => {
    expect(checkElectronParity(scene(1, bh4()))).toBeNull();
  });

  it("flags a CH3• radical (9 e⁻) as a singlet, suggesting even multiplicities", () => {
    const issue = checkElectronParity(scene(1, ch3()));
    expect(issue).not.toBeNull();
    expect(issue!.electrons).toBe(9);
    expect(issue!.suggested).toEqual([2, 4, 6]);
  });

  it("accepts a doublet for the CH3• radical", () => {
    expect(checkElectronParity(scene(2, ch3()))).toBeNull();
  });

  it("respects charge shifting parity: CH3+ (8 e⁻) is a valid singlet", () => {
    // Neutral CH3 has 9 e⁻ (odd); +1 removes one → 8 e⁻ (even) → singlet ok.
    expect(checkElectronParity(scene(1, ch3(1)))).toBeNull();
  });

  it("returns null for an empty scene (nothing to validate)", () => {
    expect(checkElectronParity(scene(1))).toBeNull();
    expect(checkElectronParity(scene(2))).toBeNull();
  });

  it("sums electrons across fragments (substrate + BH4-)", () => {
    // water (10) + BH4- (10) = 20 e⁻, even → singlet ok, doublet flagged.
    expect(checkElectronParity(scene(1, water(), bh4()))).toBeNull();
    expect(checkElectronParity(scene(2, water(), bh4()))).not.toBeNull();
  });

  it("writes an explanatory message (electron count + why + what to use)", () => {
    const issue = checkElectronParity(scene(2, water()))!;
    expect(issue.message).toContain("10 electrons");
    expect(issue.message).toContain("odd"); // required multiplicity parity
    expect(issue.message).toContain("singlet");
    // teaching, not diagnostic:
    expect(issue.message.toLowerCase()).not.toContain("invalid");
  });

  it("does not throw on an element beyond the table; returns null instead", () => {
    const heavy = frag("u", ["U"], 0); // uranium (Z=92) — beyond H–Rn
    expect(() => checkElectronParity(scene(1, heavy))).not.toThrow();
    expect(checkElectronParity(scene(1, heavy))).toBeNull();
  });

  it("now validates a Pd complex instead of silently declining (Z≤86)", () => {
    // Pd (46 e⁻, even) as a doublet is a real parity error — previously Pd was
    // outside the table so this returned null and the check vanished.
    const pd = frag("pd", ["Pd"], 0);
    const issue = checkElectronParity(scene(2, pd));
    expect(issue).not.toBeNull();
    expect(issue!.electrons).toBe(46);
    expect(issue!.suggested).toEqual([1, 3, 5]);
  });

  it("reports the *nearest* valid multiplicity, not the smallest", () => {
    // 10 e⁻ (even) with multiplicity 8: valid list is [1,3,5] but the nearest
    // valid value to 8 is 7, not 1.
    const issue = checkElectronParity(scene(8, water()))!;
    expect(issue.suggested).toEqual([1, 3, 5]);
    expect(issue.message).toContain("nearest valid value is 7");
    expect(issue.message).not.toContain("nearest valid value is 1");
  });
});
