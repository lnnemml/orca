import { describe, it, expect } from "vitest";

import { detectClashes, undeterminedElements } from "./clash";
import { testScene, type RawFragment } from "./scene-test-util";
import type { Constraint } from "./constraints";

const at = (element: string, x: number, y = 0, z = 0) => ({ element, x, y, z });
function frag(id: string, atoms: { element: string; x: number; y: number; z: number }[]): RawFragment {
  return { id, name: id, charge: 0, source: "editor", atoms };
}
// vdW: C 1.70, O 1.52 (Bondi); B 1.92 (Mantina). Sums: C+C 3.40, C+O 3.22, C+B 3.62.

describe("detectClashes", () => {
  // ── c1: a real inter-fragment clash is flagged for the RIGHT pair ────────────
  it("flags an inter-fragment pair inside k·(rᵢ+rⱼ), naming the two atoms (c1)", () => {
    const s = testScene([frag("a", [at("C", 0)]), frag("b", [at("C", 2.0)])]);
    const idA = s.fragments[0].atoms[0].id;
    const idB = s.fragments[1].atoms[0].id;

    const r = detectClashes(s, 0.65, []);
    expect(r.clashes).toHaveLength(1);
    expect(new Set([r.clashes[0].a, r.clashes[0].b])).toEqual(new Set([idA, idB]));
    expect(r.clashes[0].distance).toBeCloseTo(2.0, 6);
    expect(r.clashes[0].threshold).toBeCloseTo(0.65 * (1.7 + 1.7), 6); // 2.21 Å
    expect(r.undetermined).toHaveLength(0);
  });

  // ── c2: no false positives; INTRA-fragment / own bonds are NEVER flagged ─────
  it("leaves well-separated fragments clean and never flags a fragment's own atoms (c2)", () => {
    // Fragment a is a bonded C–C at 1.5 Å (well inside the vdW sum) — but INTRA, so
    // it must never clash. Fragment b sits 10 Å away.
    const s = testScene([
      frag("a", [at("C", 0), at("C", 1.5)]),
      frag("b", [at("C", 10)]),
    ]);
    expect(detectClashes(s, 0.65, []).clashes).toHaveLength(0);
  });

  // ── c3: an UNDETERMINED element skips the pair (+flag), never radius 0 ───────
  it("skips a pair with an UNDETERMINED radius and reports it apart, not as a clash (c3)", () => {
    // Tungsten has no cited radius here → UNDETERMINED. Placed 1.0 Å from a carbon
    // (absurdly close): treated as radius 0 it would clash; correctly it is skipped.
    const s = testScene([frag("a", [at("W", 0)]), frag("b", [at("C", 1.0)])]);
    const r = detectClashes(s, 0.65, []);
    expect(r.clashes).toHaveLength(0);
    expect(r.undetermined).toHaveLength(1);
    expect(new Set(r.undetermined[0].elements)).toEqual(new Set(["W", "C"]));
    expect(undeterminedElements(r)).toEqual(["W"]);
  });

  // ── c4: a distance-constrained pair is NOT a clash — the mission gate ────────
  it("does not flag a pair carrying an active distance constraint, even inside the sum (c4)", () => {
    // C···O at 1.2 Å is deep inside the vdW sum (3.22·0.65 = 2.09 Å) → a clash…
    const s = testScene([frag("a", [at("C", 0)]), frag("b", [at("O", 1.2)])]);
    expect(detectClashes(s, 0.65, []).clashes).toHaveLength(1);

    // …but with a distance constraint on the pair (ORCA indices 0,1 == global here)
    // it is an INTENTIONAL contact — never a clash.
    const cons: Constraint[] = [{ kind: "distance", atoms: [0, 1] }];
    expect(detectClashes(s, 0.65, cons).clashes).toHaveLength(0);
  });

  // ── c5: k is really applied — monotone in the threshold ─────────────────────
  it("is monotone in k: a higher threshold flags at least as many clashes (c5)", () => {
    // One inter-fragment C···C pair at 2.5 Å. C+C sum 3.40 → threshold 0.65·=2.21
    // (clear), 0.75·=2.55 (clash). So k crosses the pair between 0.65 and 0.75.
    const s = testScene([frag("a", [at("C", 0)]), frag("b", [at("C", 2.5)])]);
    const n = (k: number) => detectClashes(s, k, []).clashes.length;
    expect(n(0.55)).toBeLessThanOrEqual(n(0.65));
    expect(n(0.85)).toBeGreaterThanOrEqual(n(0.65));
    expect(n(0.65)).toBe(0);
    expect(n(0.75)).toBe(1); // proves k is used, not hardcoded
  });
});
