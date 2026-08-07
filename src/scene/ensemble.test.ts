import { describe, it, expect } from "vitest";

import {
  conformerMatchesFragment,
  deltaEKcal,
  goatInputForFragment,
  isGoatInput,
  isTerminalSuccessStatus,
  parseEnsemble,
  planConformerApply,
  showsSingleStructureResults,
  type Conformer,
} from "./ensemble";
import { restoreScene } from "./restore";
import { serializeScene } from "./scene";
import type { RawFragment, Scene } from "./types";
import { testScene } from "./scene-test-util";

// A real (truncated to 3 structures) `butane.finalensemble.xyz` from an actual
// ORCA 6.1.0 `! XTB GOAT` run — declaration must agree with reality (§ the rule
// that saved the fragment library). Loaded as a raw string (Vite `?raw`).
import ENSEMBLE from "./__fixtures__/butane.finalensemble.xyz?raw";

/** A butane fragment whose atoms are the ensemble's first conformer. */
function butaneFragment(conformers: Conformer[]): RawFragment {
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

describe("deltaEKcal (real butane fixture)", () => {
  it("gives 0 for the global minimum and ~0.6 kcal/mol for the next (anti→gauche)", () => {
    const c = parseEnsemble(ENSEMBLE)!;
    const d = deltaEKcal(c);
    expect(d[0]).toBe(0);
    // -13.66418 vs -13.66513 Eh → ~0.596 kcal/mol — a number checkable against
    // chemistry (butane anti/gauche gap ~0.9 at experiment, ~0.6 at xTB), not
    // against itself.
    expect(d[1]).toBeCloseTo(0.596, 2);
    expect(d[2]).toBeGreaterThan(d[1]);
  });

  it("passes NaN energies through as NaN (UI shows a dash)", () => {
    const c: Conformer[] = [
      { atoms: [], energy: -10, index: 0 },
      { atoms: [], energy: NaN, index: 1 },
    ];
    expect(deltaEKcal(c)[0]).toBe(0);
    expect(Number.isNaN(deltaEKcal(c)[1])).toBe(true);
  });
});

describe("GOAT scene_json semantics (§1): one fragment, survives restoreScene", () => {
  it("a GOAT job's snapshot is a single fragment matching its own input", () => {
    const frag = butaneFragment(parseEnsemble(ENSEMBLE)!);
    // What the Find-conformers flow persists:
    const input = goatInputForFragment(frag);
    const sceneJson = serializeScene(testScene([frag], 1));
    // Opening/iterating that job must honour the snapshot (not reject it).
    const { scene, snapshotRejected } = restoreScene(input, sceneJson);
    expect(snapshotRejected).toBe(false);
    expect(scene!.fragments).toHaveLength(1);
    expect(scene!.fragments[0].id).toBe(frag.id);
  });
});

describe("planConformerApply (§4 — both branches + refusal)", () => {
  const conformers = parseEnsemble(ENSEMBLE)!;
  const frag = butaneFragment(conformers);

  it("REPLACE when the snapshot fragment is still in the store scene", () => {
    const store: Scene = testScene([frag], 1);
    const plan = planConformerApply(store, frag, conformers[1]);
    expect(plan.action).toBe("replace");
    if (plan.action === "replace") {
      expect(plan.fragmentId).toBe(frag.id);
      expect(plan.atoms).toBe(conformers[1].atoms);
    }
  });

  it("NEW single-fragment scene when the fragment is gone (other session)", () => {
    const plan = planConformerApply(null, frag, conformers[0]);
    expect(plan.action).toBe("new");
    if (plan.action === "new") {
      expect(plan.fragment.id).toBe(frag.id);
      expect(plan.fragment.charge).toBe(frag.charge);
      expect(plan.fragment.atoms).toHaveLength(14);
      expect(plan.fragment.atoms).not.toBe(conformers[0].atoms); // copied
    }
  });

  it("REFUSE (no throw) when the live fragment's composition changed", () => {
    const changed: RawFragment = { ...frag, atoms: frag.atoms.slice(0, 13) };
    const store: Scene = testScene([changed], 1);
    let plan: ReturnType<typeof planConformerApply>;
    expect(() => {
      plan = planConformerApply(store, frag, conformers[0]);
    }).not.toThrow();
    expect(plan!.action).toBe("refuse");
    if (plan!.action === "refuse") expect(plan!.reason).toMatch(/composition/i);
  });
});

describe("goatInputForFragment", () => {
  it("emits `! XTB GOAT` and the fragment's own charge (not the scene total)", () => {
    const bh4: RawFragment = {
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

describe("isGoatInput", () => {
  it("is true when a keyword line carries the GOAT token", () => {
    expect(isGoatInput("! XTB GOAT\n* xyz 0 1\nH 0 0 0\n*\n")).toBe(true);
    expect(isGoatInput(goatInputForFragment({
      id: "x", name: "x", charge: 0, source: "smiles",
      atoms: [{ element: "H", x: 0, y: 0, z: 0 }],
    }))).toBe(true);
    expect(isGoatInput("! goat xtb")).toBe(true); // case-insensitive
  });

  it("is false for a non-GOAT input", () => {
    expect(isGoatInput("! B3LYP def2-SVP Opt\n* xyz 0 1\nH 0 0 0\n*\n")).toBe(false);
    expect(isGoatInput("")).toBe(false);
  });

  it("ignores comment lines — a `# GOAT` mention is not a GOAT job", () => {
    expect(isGoatInput("# GOAT was considered but rejected\n! B3LYP Opt\n")).toBe(
      false,
    );
  });

  it("ignores the geometry block — an atom/label named GOAT doesn't count", () => {
    // Only `!` lines are inspected; a Goat-like token in the coord block is inert.
    expect(isGoatInput("! B3LYP Opt\n* xyz 0 1\nGOAT 0 0 0\n*\n")).toBe(false);
  });

  it("ignores a trailing comment on a keyword line", () => {
    // The `#` note mentions GOAT but the actual keywords are an Opt job.
    expect(isGoatInput("! XTB Opt # GOAT next time")).toBe(false);
    // ...and a real GOAT keyword with a trailing comment still counts.
    expect(isGoatInput("! XTB GOAT # find all conformers")).toBe(true);
  });

  it("matches on word boundaries, not substrings", () => {
    expect(isGoatInput("! SCAPEGOATED")).toBe(false);
    expect(isGoatInput("! XTB GOAT MAXITER 100")).toBe(true);
  });
});

// C-goat-parsed-shows-ensemble (the guard half): the ensemble read must fire on ANY
// terminal success, not exactly `completed`. A GOAT job whose single-structure
// `.property.txt` parsed reaches `parsed`; the old `status === "completed"` guard
// returned false for it and hid the ensemble (debugging/017).
describe("isTerminalSuccessStatus", () => {
  it("is true for both completed and parsed", () => {
    expect(isTerminalSuccessStatus("completed")).toBe(true);
    expect(isTerminalSuccessStatus("parsed")).toBe(true);
  });

  it("is false for non-terminal / failure statuses", () => {
    for (const s of ["draft", "queued", "running", "failed", "cancelled"]) {
      expect(isTerminalSuccessStatus(s)).toBe(false);
    }
  });

  // The bite: the regressed guard was `status === "completed"`, which returns false for
  // a `parsed` GOAT job. This asserts the broadened guard accepts `parsed`.
  it("accepts the `parsed` GOAT job the narrow guard rejected", () => {
    expect(isTerminalSuccessStatus("parsed")).toBe(true);
  });
});

// C-goat-parsed-shows-ensemble (the display half): a GOAT job suppresses the
// single-structure Results dashboard (its "N optimization cycles" trajectory is
// misleading for a conformer search); a non-GOAT job still shows it.
describe("showsSingleStructureResults", () => {
  it("is false for a GOAT job (ensemble is shown instead of the trajectory)", () => {
    expect(showsSingleStructureResults("! XTB GOAT\n* xyz 0 1\nH 0 0 0\n*\n")).toBe(false);
  });

  it("is true for a normal Opt/SP job (C-nongoat-unaffected)", () => {
    expect(showsSingleStructureResults("! r2SCAN-3c Opt Freq\n* xyz 0 1\nH 0 0 0\n*\n")).toBe(true);
    expect(showsSingleStructureResults("! B3LYP def2-SVP\n")).toBe(true);
  });
});
