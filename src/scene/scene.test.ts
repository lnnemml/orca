import { describe, it, expect } from "vitest";

import type { Scene, SceneFragment } from "./types";
import {
  addFragment,
  atomCount,
  atomicNumber,
  compositionSignature,
  deserializeScene,
  electronCount,
  fragmentAtomIndices,
  fragmentRanges,
  globalIndex,
  injectSceneIntoInput,
  locateAtom,
  mergeToAtomLines,
  mergeToXyz,
  parseAtomLines,
  removeFragment,
  sceneFromOrcaInput,
  sceneFromXyz,
  renameFragment,
  replaceFragmentAtoms,
  sceneFromAtomLines,
  serializeScene,
  setFragmentCharge,
  setMultiplicity,
  totalCharge,
  xyzMatchesScene,
} from "./scene";

// ── Fixtures (literal ids → deterministic, no makeFragmentId) ────────────────

/** Real water geometry (O + 2 H), Å. */
function water(id = "wat"): SceneFragment {
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

/** BH₄⁻: T_d, B–H 1.24 Å (charge −1). */
function borohydride(id = "bh4"): SceneFragment {
  const d = 1.24 / Math.sqrt(3);
  return {
    id,
    name: "BH4-",
    charge: -1,
    source: "fragment-library",
    sourceLabel: "library:bh4",
    atoms: [
      { element: "B", x: 0.0, y: 0.0, z: 0.0 },
      { element: "H", x: d, y: d, z: d },
      { element: "H", x: -d, y: -d, z: d },
      { element: "H", x: -d, y: d, z: -d },
      { element: "H", x: d, y: -d, z: -d },
    ],
  };
}

function scene(...fragments: SceneFragment[]): Scene {
  return { fragments, multiplicity: 1 };
}

// ── compositionSignature ─────────────────────────────────────────────────────

describe("compositionSignature", () => {
  it("is id:size per fragment, joined — coordinates excluded", () => {
    expect(compositionSignature(scene(water(), borohydride()))).toBe(
      "wat:3|bh4:5",
    );
  });

  it("is invariant to a coordinate-only change (same atoms moved)", () => {
    const a = scene(water());
    const moved = scene({
      ...water(),
      atoms: water().atoms.map((at) => ({ ...at, x: at.x + 1.5 })),
    });
    expect(compositionSignature(moved)).toBe(compositionSignature(a));
  });

  it("changes when a fragment is added or removed", () => {
    const one = compositionSignature(scene(water()));
    const two = compositionSignature(scene(water(), borohydride()));
    expect(two).not.toBe(one);
    expect(compositionSignature(removeFragment(scene(water(), borohydride()), "bh4"))).toBe(
      one,
    );
  });
});

// ── mergeToXyz golden ────────────────────────────────────────────────────────

describe("mergeToXyz", () => {
  it("emits the canonical format for a single fragment (golden water)", () => {
    const expected =
      "3\n\n" +
      "O     0.00000000    0.00000000    0.11779000\n" +
      "H     0.00000000    0.75545300   -0.47116100\n" +
      "H     0.00000000   -0.75545300   -0.47116100\n";
    expect(mergeToXyz(scene(water()))).toBe(expected);
  });

  it("writes the comment on line 2", () => {
    expect(mergeToXyz(scene(water()), "hello").split("\n")[1]).toBe("hello");
  });
});

describe("mergeToAtomLines", () => {
  it("preserves fragment order and concatenates rows; count = sum", () => {
    const s = scene(water(), borohydride());
    const rows = mergeToAtomLines(s);
    expect(rows).toHaveLength(3 + 5);
    expect(atomCount(s)).toBe(8);
    // first three rows are water, next five are BH4
    expect(rows[0].startsWith("O ")).toBe(true);
    expect(rows[3].startsWith("B ")).toBe(true);
  });
});

// ── Aggregates ───────────────────────────────────────────────────────────────

describe("totalCharge", () => {
  it("sums fragment charges: neutral substrate 0 + BH4- -1 = -1", () => {
    expect(totalCharge(scene(water(), borohydride()))).toBe(-1);
  });
});

describe("electronCount", () => {
  it("is Σ Z − totalCharge (BH4- = 9 protons + 1 = 10 electrons)", () => {
    expect(electronCount(scene(borohydride()))).toBe(10);
    expect(electronCount(scene(water()))).toBe(10);
  });

  it("throws naming the symbol on an unknown element", () => {
    const bad = scene({
      ...water("x"),
      atoms: [{ element: "Xx", x: 0, y: 0, z: 0 }],
    });
    expect(() => electronCount(bad)).toThrow(/Xx/);
  });
});

describe("atomicNumber", () => {
  it("is case-insensitive", () => {
    expect(atomicNumber("cl")).toBe(17);
    expect(atomicNumber("CL")).toBe(17);
    expect(atomicNumber("Cl")).toBe(17);
  });

  it("covers the cross-coupling metals and up to Rn (Z ≤ 86)", () => {
    expect(atomicNumber("Pd")).toBe(46);
    expect(atomicNumber("Pt")).toBe(78);
    expect(atomicNumber("Rn")).toBe(86);
  });

  it("throws naming an element beyond the table", () => {
    expect(() => atomicNumber("U")).toThrow(/U/); // Z=92, unsupported
  });
});

// ── Index space ──────────────────────────────────────────────────────────────

describe("index space", () => {
  const s = scene(water(), borohydride());

  it("globalIndex ↔ locateAtom round-trip, including across the boundary", () => {
    for (const [fragId, local] of [
      ["wat", 0],
      ["wat", 2],
      ["bh4", 0], // first atom of the second fragment — the boundary
      ["bh4", 4],
    ] as const) {
      const g = globalIndex(s, fragId, local);
      const loc = locateAtom(s, g);
      expect(loc).not.toBeNull();
      expect(loc!.fragment.id).toBe(fragId);
      expect(loc!.localIndex).toBe(local);
    }
    expect(globalIndex(s, "bh4", 0)).toBe(3);
  });

  it("locateAtom returns null out of range", () => {
    expect(locateAtom(s, 8)).toBeNull();
    expect(locateAtom(s, -1)).toBeNull();
  });

  it("fragmentAtomIndices is the correct contiguous range for the 2nd fragment", () => {
    expect(fragmentAtomIndices(s, "bh4")).toEqual([3, 4, 5, 6, 7]);
  });

  it("fragmentRanges are start-inclusive / end-exclusive", () => {
    expect(fragmentRanges(s)).toEqual([
      { fragmentId: "wat", start: 0, end: 3 },
      { fragmentId: "bh4", start: 3, end: 8 },
    ]);
  });
});

// ── replaceFragmentAtoms invariant ───────────────────────────────────────────

describe("replaceFragmentAtoms", () => {
  const s = scene(water(), borohydride());

  it("moves atoms while keeping other fragments and ordering", () => {
    const moved = water().atoms.map((a) => ({ ...a, x: a.x + 1.5 }));
    const next = replaceFragmentAtoms(s, "wat", moved);
    expect(next.fragments[0].atoms[0].x).toBe(1.5);
    expect(next.fragments[1]).toEqual(s.fragments[1]); // BH4 untouched
    expect(next.fragments.map((f) => f.id)).toEqual(["wat", "bh4"]);
  });

  it("rejects a changed atom count", () => {
    expect(() =>
      replaceFragmentAtoms(s, "wat", [{ element: "O", x: 0, y: 0, z: 0 }]),
    ).toThrow(/atom count/);
  });

  it("rejects a changed element sequence", () => {
    const swapped = water().atoms.map((a, i) =>
      i === 0 ? { ...a, element: "N" } : a,
    );
    expect(() => replaceFragmentAtoms(s, "wat", swapped)).toThrow(/element sequence/);
  });
});

// ── Immutability ─────────────────────────────────────────────────────────────

describe("immutability", () => {
  it("no mutator alters the input scene", () => {
    const s = scene(water(), borohydride());
    const snapshot = structuredClone(s);

    addFragment(s, water("extra"));
    removeFragment(s, "wat");
    renameFragment(s, "wat", "renamed");
    setFragmentCharge(s, "wat", 5);
    setMultiplicity(s, 3);
    replaceFragmentAtoms(
      s,
      "wat",
      water().atoms.map((a) => ({ ...a, x: a.x + 9 })),
    );

    expect(s).toEqual(snapshot);
  });

  it("mutators return the intended new scene", () => {
    const s = scene(water());
    expect(addFragment(s, borohydride()).fragments).toHaveLength(2);
    expect(removeFragment(s, "wat").fragments).toHaveLength(0);
    expect(renameFragment(s, "wat", "H2O").fragments[0].name).toBe("H2O");
    expect(setFragmentCharge(s, "wat", -2).fragments[0].charge).toBe(-2);
    expect(setMultiplicity(s, 3).multiplicity).toBe(3);
  });
});

// ── Serialization ────────────────────────────────────────────────────────────

describe("serialize/deserialize", () => {
  it("round-trips a scene", () => {
    const s = scene(water(), borohydride());
    const back = deserializeScene(serializeScene(s));
    expect(back).toEqual(s);
  });

  it("writes version 1", () => {
    expect(JSON.parse(serializeScene(scene(water()))).version).toBe(1);
  });

  it("returns null for the wrong version", () => {
    const obj = JSON.parse(serializeScene(scene(water())));
    obj.version = 2;
    expect(deserializeScene(JSON.stringify(obj))).toBeNull();
  });

  it("returns null for malformed JSON", () => {
    expect(deserializeScene("{ not json")).toBeNull();
  });

  it("returns null for a missing field", () => {
    const obj = JSON.parse(serializeScene(scene(water())));
    delete obj.multiplicity;
    expect(deserializeScene(JSON.stringify(obj))).toBeNull();
  });

  it("returns null for a bad fragment source", () => {
    const obj = JSON.parse(serializeScene(scene(water())));
    obj.fragments[0].source = "nonsense";
    expect(deserializeScene(JSON.stringify(obj))).toBeNull();
  });
});

// ── xyzMatchesScene (reset-detection primitive) ──────────────────────────────

describe("xyzMatchesScene", () => {
  const s = scene(water());
  const lines = () => [
    "O   0.0        0.0         0.11779",
    "H   0.0        0.755453   -0.471161",
    "H   0.0       -0.755453   -0.471161",
  ];

  it("is true when the same numbers are formatted differently", () => {
    // `0.0` vs `0.00000000`, `0.11779` vs `0.11779000`
    expect(xyzMatchesScene(s, lines())).toBe(true);
  });

  it("is true within tolerance (1e-7 drift)", () => {
    const l = lines();
    l[0] = "O   0.0   0.0   0.1177901"; // +1e-7 on z
    expect(xyzMatchesScene(s, l)).toBe(true);
  });

  it("is false beyond tolerance (1e-3 drift)", () => {
    const l = lines();
    l[0] = "O   0.0   0.0   0.11879"; // +1e-3 on z
    expect(xyzMatchesScene(s, l)).toBe(false);
  });

  it("is false on a changed element", () => {
    const l = lines();
    l[0] = "N   0.0   0.0   0.11779";
    expect(xyzMatchesScene(s, l)).toBe(false);
  });

  it("is false on a changed atom count", () => {
    expect(xyzMatchesScene(s, lines().slice(0, 2))).toBe(false);
  });

  it("is false for null input", () => {
    expect(xyzMatchesScene(s, null)).toBe(false);
  });
});

// ── parse / sceneFromAtomLines ───────────────────────────────────────────────

describe("parseAtomLines", () => {
  it("skips blanks and comments, keeps valid rows", () => {
    const atoms = parseAtomLines(["", "# comment", "O 0 0 0.1", "  ", "H 0 0 1"]);
    expect(atoms).toHaveLength(2);
    expect(atoms![0]).toEqual({ element: "O", x: 0, y: 0, z: 0.1 });
  });

  it("returns null when nothing parses", () => {
    expect(parseAtomLines(["", "# only comments"])).toBeNull();
  });
});

describe("sceneFromAtomLines", () => {
  it("builds a single-fragment scene from a real ORCA water block", () => {
    const s = sceneFromAtomLines(
      [
        "O   0.00000000   0.00000000   0.11779000",
        "H   0.00000000   0.75545300  -0.47116100",
        "H   0.00000000  -0.75545300  -0.47116100",
      ],
      { id: "w", name: "Water", charge: 0, source: "editor" },
    );
    expect(s).not.toBeNull();
    expect(s!.fragments).toHaveLength(1);
    expect(s!.fragments[0].id).toBe("w");
    expect(atomCount(s!)).toBe(3);
    expect(s!.multiplicity).toBe(1);
  });

  it("returns null when no atoms parse", () => {
    expect(sceneFromAtomLines(["# nothing"], { id: "w" })).toBeNull();
  });
});

describe("sceneFromOrcaInput", () => {
  const input = [
    "! r2SCAN-3c Opt Freq TightSCF",
    "%pal nprocs 4 end",
    "%maxcore 2000",
    "",
    "* xyz -1 2",
    "O   0.00000000   0.00000000   0.11779000",
    "H   0.00000000   0.75545300  -0.47116100",
    "H   0.00000000  -0.75545300  -0.47116100",
    "*",
    "",
  ].join("\n");

  it("extracts a single fragment with charge + multiplicity from the header", () => {
    const s = sceneFromOrcaInput(input, { id: "f", name: "Sub" });
    expect(s).not.toBeNull();
    expect(s!.fragments).toHaveLength(1);
    expect(s!.fragments[0].id).toBe("f");
    expect(s!.fragments[0].charge).toBe(-1); // from `* xyz -1 2`
    expect(s!.multiplicity).toBe(2);
    expect(atomCount(s!)).toBe(3);
  });

  it("returns null for a `* xyzfile` external-geometry block", () => {
    expect(sceneFromOrcaInput("* xyzfile 0 1 mol.xyz")).toBeNull();
  });

  it("returns null when there is no coordinate block", () => {
    expect(sceneFromOrcaInput("! B3LYP def2-SVP\n%pal nprocs 2 end")).toBeNull();
  });

  it("defaults to neutral singlet when the header numbers are absent", () => {
    const s = sceneFromOrcaInput("* xyz\nHe 0 0 0\n*");
    expect(s).not.toBeNull();
    expect(totalCharge(s!)).toBe(0);
    expect(s!.multiplicity).toBe(1);
  });
});

describe("sceneFromXyz", () => {
  const xyz =
    "3\nwater\n" +
    "O 0 0 0.11779\nH 0 0.755453 -0.471161\nH 0 -0.755453 -0.471161\n";

  it("parses a standard xyz string (skips count + comment)", () => {
    const s = sceneFromXyz(xyz, { id: "w", charge: -1, multiplicity: 2 });
    expect(s).not.toBeNull();
    expect(atomCount(s!)).toBe(3);
    expect(s!.fragments[0].id).toBe("w");
    expect(totalCharge(s!)).toBe(-1);
    expect(s!.multiplicity).toBe(2);
  });

  it("returns null for a bad atom count or too-short input", () => {
    expect(sceneFromXyz("notanumber\nc\nO 0 0 0")).toBeNull();
    expect(sceneFromXyz("3\n")).toBeNull();
  });
});

describe("injectSceneIntoInput", () => {
  const s = sceneFromOrcaInput("* xyz -1 2\nO 0 0 0.11779\nH 0 0 1\n*", {
    id: "f",
  })!;

  it("replaces an existing block, preserving surrounding lines", () => {
    const content = "! B3LYP def2-SVP\n%pal nprocs 2 end\n\n* xyz 0 1\nH 0 0 0\n*\n";
    const out = injectSceneIntoInput(content, s);
    expect(out).toContain("! B3LYP def2-SVP");
    expect(out).toContain("%pal nprocs 2 end");
    expect(out).toContain("* xyz -1 2"); // header from the scene
    expect(out).toContain("O     0.00000000    0.00000000    0.11779000");
    expect(out).not.toContain("* xyz 0 1"); // old header replaced
  });

  it("appends a block when the input has none", () => {
    const out = injectSceneIntoInput("! HF def2-SVP", s);
    expect(out).toContain("! HF def2-SVP");
    expect(out).toContain("* xyz -1 2");
  });

  it("round-trips with sceneFromOrcaInput (geometry preserved)", () => {
    const out = injectSceneIntoInput("", s);
    const back = sceneFromOrcaInput(out)!;
    expect(xyzMatchesScene(s, mergeToAtomLines(back))).toBe(true);
    expect(totalCharge(back)).toBe(-1);
    expect(back.multiplicity).toBe(2);
  });
});
