import { describe, it, expect } from "vitest";

import type { Molecule } from "../types";
import { userReagentToFragment, fragmentToXyz } from "./reagent-catalog";
import { FRAGMENT_LIBRARY, libraryFragmentToScene } from "./fragment-library";
import { testScene } from "./scene-test-util";
import { totalCharge, sceneFromXyz } from "./scene";

/** A stored user-reagent row (role reagent), overridable per test. */
function reagentRow(over: Partial<Molecule> = {}): Molecule {
  return {
    id: "r1",
    name: "Sodium cation",
    formula: "",
    xyz: "1\nNa+\nNa 0.0 0.0 0.0\n",
    charge: 1,
    multiplicity: 1,
    tags: "",
    created_at: "now",
    is_reagent: true,
    ...over,
  };
}

// ── c2 — a saved reagent carries its (mandatory) charge into the scene ─────────
describe("(c2) a user reagent's stored charge flows into the scene total", () => {
  it("userReagentToFragment keeps the stored charge (+1), not 0", () => {
    const f = userReagentToFragment(reagentRow({ charge: 1 }));
    expect(f).not.toBeNull();
    expect(f!.charge).toBe(1);
  });

  it("its charge shifts the scene total exactly like a built-in reagent", () => {
    // H₂O (built-in, 0) + a user Na⁺ reagent (+1) ⇒ total +1.
    const user = userReagentToFragment(reagentRow({ charge: 1 }))!;
    const scene = testScene([libraryFragmentToScene(byKey("water")), user]);
    expect(totalCharge(scene)).toBe(1);

    // The BITE: a reagent saved/loaded as charge 0 (the ADR-014 footgun) would leave
    // the total at 0 — so the +1 is genuinely sourced from the stored charge.
    const zeroed = userReagentToFragment(reagentRow({ charge: 0 }))!;
    const zeroedScene = testScene([libraryFragmentToScene(byKey("water")), zeroed]);
    expect(totalCharge(zeroedScene)).toBe(0);
  });

  it("round-trips a saved fragment's xyz back to the same geometry", () => {
    // fragmentToXyz(saved) → sceneFromXyz → same element + coords (the save capture).
    const frag = testScene([libraryFragmentToScene(byKey("bh4-"))]).fragments[0];
    const xyz = fragmentToXyz(frag, "bh4");
    const back = sceneFromXyz(xyz, { source: "import", name: "x", charge: -1 });
    expect(back).not.toBeNull();
    expect(back!.fragments[0].atoms).toHaveLength(frag.atoms.length);
    expect(back!.fragments[0].atoms[0].element).toBe(frag.atoms[0].element);
  });
});

// ── c4 — curated (reference contract) vs user (no contract) are not conflated ──
describe("(c4) curated ↔ user reagent distinction", () => {
  it("every built-in reagent carries a reference contract + non-empty provenance", () => {
    for (const f of FRAGMENT_LIBRARY) {
      expect(f).toHaveProperty("reference"); // the curated guarantee (even if {})
      expect(f.provenance.trim().length).toBeGreaterThan(0);
    }
  });

  it("a user reagent is source 'library', NEVER 'fragment-library' (not faked curated)", () => {
    const f = userReagentToFragment(reagentRow())!;
    expect(f.source).toBe("library");
    // The BITE: mislabelling a user reagent as the curated source would let the
    // palette present it with a reference guarantee it doesn't have.
    expect(f.source).not.toBe("fragment-library");
    // A stored reagent (Molecule) structurally has no `reference` field — it is not
    // a LibraryFragment, so the two lists can never be merged by type.
    expect("reference" in reagentRow()).toBe(false);
  });
});

function byKey(key: string) {
  return FRAGMENT_LIBRARY.find((f) => f.key === key)!;
}
