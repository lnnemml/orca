import { describe, it, expect } from "vitest";

import {
  type Constraint,
  ORCA_INDEX_BASE,
  toOrcaIndex,
  fromOrcaIndex,
  constraintsBlock,
  parseConstraintsBlock,
  inspectConstraintsBlock,
  injectConstraints,
  constraintIndexIssues,
  constraintFromSelection,
  sameConstraint,
} from "./constraints";

const BASE = `! r2SCAN-3c Opt
* xyz 0 1
C   0.000000   0.000000   0.000000
O   0.000000   0.000000   1.200000
H   0.000000   0.900000  -0.400000
*
`;

const WITH_GEOM = `! r2SCAN-3c Opt
%geom
  maxiter 200
end
* xyz 0 1
C   0.000000   0.000000   0.000000
O   0.000000   0.000000   1.200000
*
`;

// The four kinds, with and without an explicit value.
const ALL_KINDS: Constraint[] = [
  { kind: "distance", atoms: [0, 1], value: 1.234 },
  { kind: "distance", atoms: [1, 2] },
  { kind: "angle", atoms: [0, 1, 2], value: 109.5 },
  { kind: "angle", atoms: [2, 1, 0] },
  { kind: "dihedral", atoms: [0, 1, 2, 3], value: 60 },
  { kind: "dihedral", atoms: [3, 2, 1, 0] },
  { kind: "cartesian", atoms: [5] },
];

describe("index base — settled by the ORCA 6.1.0 experiment (2026-07-29)", () => {
  it("ORCA `%geom Constraints` is 0-based", () => {
    expect(ORCA_INDEX_BASE).toBe(0);
  });

  it("conversion is identity (both spaces 0-based) but honest via ORCA_INDEX_BASE", () => {
    for (const i of [0, 1, 2, 5, 33]) {
      expect(toOrcaIndex(i)).toBe(i);
      expect(fromOrcaIndex(i)).toBe(i);
      expect(fromOrcaIndex(toOrcaIndex(i))).toBe(i);
    }
  });

  it("reproduces the experiment's own constraint line verbatim", () => {
    // chloromethane order Cl,C,H,H,H → C is global 1, first H is global 2. The
    // line that froze the C–H bond at 1.234 Å in the real run.
    const block = constraintsBlock([{ kind: "distance", atoms: [1, 2], value: 1.234 }]);
    expect(block).toContain("{B 1 2 1.234 C}");
  });
});

describe("round-trip: parse(inject(x, cs)) === cs", () => {
  it("all four kinds, with and without value", () => {
    const injected = injectConstraints(BASE, ALL_KINDS);
    expect(parseConstraintsBlock(injected)).toEqual(ALL_KINDS);
  });

  it("each kind individually round-trips", () => {
    for (const c of ALL_KINDS) {
      const injected = injectConstraints(BASE, [c]);
      expect(parseConstraintsBlock(injected)).toEqual([c]);
    }
  });

  it("round-trips through the standalone block too", () => {
    const block = constraintsBlock(ALL_KINDS);
    expect(parseConstraintsBlock(block)).toEqual(ALL_KINDS);
  });
});

describe("injectConstraints — no %geom present", () => {
  const out = injectConstraints(BASE, [{ kind: "distance", atoms: [0, 1], value: 1.3 }]);

  it("creates a %geom Constraints block", () => {
    expect(out).toMatch(/%geom/i);
    expect(out).toContain("{B 0 1 1.3 C}");
    expect(parseConstraintsBlock(out)).toEqual([
      { kind: "distance", atoms: [0, 1], value: 1.3 },
    ]);
  });

  it("inserts the block BEFORE the coordinate block", () => {
    expect(out.indexOf("%geom")).toBeLessThan(out.indexOf("* xyz"));
  });

  it("leaves the ! line and geometry intact", () => {
    expect(out).toContain("! r2SCAN-3c Opt");
    expect(out).toContain("O   0.000000   0.000000   1.200000");
  });
});

describe("injectConstraints — existing %geom with other settings", () => {
  const cs: Constraint[] = [{ kind: "dihedral", atoms: [0, 1, 2, 3], value: 180 }];
  const out = injectConstraints(WITH_GEOM, cs);

  it("preserves the sibling %geom setting (maxiter)", () => {
    expect(out).toContain("maxiter 200");
  });

  it("does not create a second %geom block", () => {
    expect(out.match(/%geom/gi)?.length).toBe(1);
  });

  it("the injected constraints parse back", () => {
    expect(parseConstraintsBlock(out)).toEqual(cs);
  });
});

describe("injectConstraints — re-injection replaces, never duplicates", () => {
  const first = injectConstraints(WITH_GEOM, [
    { kind: "distance", atoms: [0, 1], value: 1.3 },
  ]);
  const second = injectConstraints(first, [
    { kind: "distance", atoms: [0, 1], value: 1.9 },
  ]);

  it("keeps exactly one Constraints block", () => {
    expect(second.match(/Constraints/gi)?.length).toBe(1);
  });

  it("holds the new value, not the old", () => {
    expect(second).toContain("{B 0 1 1.9 C}");
    expect(second).not.toContain("1.3");
    expect(parseConstraintsBlock(second)).toEqual([
      { kind: "distance", atoms: [0, 1], value: 1.9 },
    ]);
  });

  it("still preserves maxiter across the replace", () => {
    expect(second).toContain("maxiter 200");
  });
});

describe("parseConstraintsBlock — tolerance and comment safety", () => {
  it("returns null when there is no Constraints block", () => {
    expect(parseConstraintsBlock(BASE)).toBeNull();
    expect(parseConstraintsBlock(WITH_GEOM)).toBeNull();
    expect(parseConstraintsBlock("")).toBeNull();
  });

  it("is tolerant of case and whitespace", () => {
    const messy = `%geom
   CONSTRAINTS
        {  b   0   1   1.234   c  }
   END
END`;
    expect(parseConstraintsBlock(messy)).toEqual([
      { kind: "distance", atoms: [0, 1], value: 1.234 },
    ]);
  });

  it("parses the inline `%geom Constraints` form", () => {
    const inline = `%geom Constraints
  {A 0 1 2 C}
end
end`;
    expect(parseConstraintsBlock(inline)).toEqual([{ kind: "angle", atoms: [0, 1, 2] }]);
  });

  it("does NOT mistake a commented-out block for a live one", () => {
    const commented = `! r2SCAN-3c Opt
# Constraints
#   {B 0 1 C}
%geom
  maxiter 200
end
* xyz 0 1
C 0 0 0
O 0 0 1.2
*
`;
    expect(parseConstraintsBlock(commented)).toBeNull();
  });

  it("a comment INSIDE the block → unrecognised, not parsed (2.5.5: we can't preserve it)", () => {
    const withComment = `%geom
  Constraints
    {B 0 1 C}   # freeze the forming bond
  end
end`;
    // Parse returns null (we won't hand back a list we can't safely rewrite);
    // inspect names it unrecognised so the panel goes read-only instead.
    expect(parseConstraintsBlock(withComment)).toBeNull();
    expect(inspectConstraintsBlock(withComment).kind).toBe("unrecognised");
  });

  it("returns null on a malformed constraint line (never silently drops)", () => {
    const bad = `%geom
  Constraints
    {B 0 C}
  end
end`;
    expect(parseConstraintsBlock(bad)).toBeNull(); // bond needs two atoms
  });
});

// ── 2.5.4b: the two guards + the "Constrain selection" builder ────────────────

describe("constraintIndexIssues — the range guard (ЗАХИСТ 1)", () => {
  const cs: Constraint[] = [{ kind: "distance", atoms: [0, 37] }];

  it("38-atom scene, constraint on 37 → clean", () => {
    expect(constraintIndexIssues(cs, 38)).toEqual([]);
  });

  it("after a fragment is removed (33 atoms), 37 is flagged out of range", () => {
    const issues = constraintIndexIssues(cs, 33);
    expect(issues).toHaveLength(1);
    expect(issues[0].constraint).toBe(cs[0]);
    expect(issues[0].badIndices).toEqual([37]);
  });

  it("flags a negative index and reports every bad index of a constraint", () => {
    const many: Constraint[] = [{ kind: "dihedral", atoms: [-1, 2, 40, 50] }];
    expect(constraintIndexIssues(many, 33)[0].badIndices).toEqual([-1, 40, 50]);
  });

  it("reports one entry per offending constraint, clean ones omitted", () => {
    const mixed: Constraint[] = [
      { kind: "distance", atoms: [0, 1] }, // fine
      { kind: "angle", atoms: [0, 1, 99] }, // bad
      { kind: "cartesian", atoms: [40] }, // bad
    ];
    const issues = constraintIndexIssues(mixed, 33);
    expect(issues).toHaveLength(2);
    expect(issues.map((i) => i.badIndices)).toEqual([[99], [40]]);
  });
});

describe("constraintFromSelection — length → kind (same rule as measureSelection)", () => {
  it("2/3/4 atoms → distance/angle/dihedral, frozen by default (no value)", () => {
    expect(constraintFromSelection([12, 33])).toEqual({
      kind: "distance",
      atoms: [12, 33],
    });
    expect(constraintFromSelection([5, 12, 20])).toEqual({
      kind: "angle",
      atoms: [5, 12, 20],
    });
    expect(constraintFromSelection([5, 12, 20, 25])).toEqual({
      kind: "dihedral",
      atoms: [5, 12, 20, 25],
    });
  });

  it("carries an explicit value when given", () => {
    expect(constraintFromSelection([12, 33], 1.85)).toEqual({
      kind: "distance",
      atoms: [12, 33],
      value: 1.85,
    });
  });

  it("returns null for a selection that isn't 2/3/4 atoms", () => {
    expect(constraintFromSelection([])).toBeNull();
    expect(constraintFromSelection([7])).toBeNull();
    expect(constraintFromSelection([1, 2, 3, 4, 5])).toBeNull();
  });
});

describe("panel round-trip — view over the text, no drift", () => {
  it("Constrain selection → inject → parse gives back exactly what the panel shows", () => {
    // The exact acceptance step: pick carbonyl C(12) and B(33), no value.
    const built = constraintFromSelection([12, 33])!;
    const text = injectConstraints(BASE, [built]);
    // What the panel reads is what the text says — same object, no parallel state.
    expect(parseConstraintsBlock(text)).toEqual([built]);
  });

  it("a manual edit of the block is reflected immediately (different text → different list)", () => {
    const a = injectConstraints(BASE, [constraintFromSelection([12, 33])!]);
    // Simulate the user hand-editing the index 33 → 30 in Monaco.
    const b = a.replace("{B 12 33 C}", "{B 12 30 C}");
    expect(parseConstraintsBlock(a)).toEqual([{ kind: "distance", atoms: [12, 33] }]);
    expect(parseConstraintsBlock(b)).toEqual([{ kind: "distance", atoms: [12, 30] }]);
  });

  it("deleting one row keeps the rest of the block and the rest of %geom", () => {
    const withMaxiter = `! r2SCAN-3c Opt
%geom
  maxiter 200
end
* xyz 0 1
C 0 0 0
O 0 0 1.2
*
`;
    const two = injectConstraints(withMaxiter, [
      { kind: "distance", atoms: [0, 1], value: 1.3 },
      { kind: "angle", atoms: [0, 1, 2] },
    ]);
    // Delete the first (what the panel's × does: inject the remaining list).
    const remaining = (parseConstraintsBlock(two) ?? []).filter((_, i) => i !== 0);
    const after = injectConstraints(two, remaining);
    expect(parseConstraintsBlock(after)).toEqual([{ kind: "angle", atoms: [0, 1, 2] }]);
    expect(after).toContain("maxiter 200"); // sibling %geom setting survives
    expect(after).not.toContain("{B 0 1"); // the deleted row is gone
  });
});

describe("sameConstraint — dedupe guard for repeated Constrain selection", () => {
  it("true for identical kind + atoms, false otherwise", () => {
    const a = constraintFromSelection([12, 33])!;
    expect(sameConstraint(a, constraintFromSelection([12, 33])!)).toBe(true);
    expect(sameConstraint(a, constraintFromSelection([33, 12])!)).toBe(false); // order matters
    expect(sameConstraint(a, constraintFromSelection([12, 33, 40])!)).toBe(false);
  });
});

// ── 2.5.5 item 0: the 2.5.4b data-loss bug — rewrite ONLY what we recognised ───

describe("inspectConstraintsBlock — absent / parsed / unrecognised", () => {
  it("absent when there is no block", () => {
    expect(inspectConstraintsBlock(BASE).kind).toBe("absent");
    expect(inspectConstraintsBlock("").kind).toBe("absent");
  });

  it("absent for a fully commented-out block (not a live one)", () => {
    const commented = `! r2SCAN-3c Opt
# %geom
#   Constraints
#     {B 0 1 C}
#   end
# end
`;
    expect(inspectConstraintsBlock(commented).kind).toBe("absent");
  });

  it("parsed when every token is understood", () => {
    const text = injectConstraints(BASE, [
      { kind: "distance", atoms: [0, 1], value: 1.3 },
    ]);
    const ins = inspectConstraintsBlock(text);
    expect(ins.kind).toBe("parsed");
    if (ins.kind === "parsed") {
      expect(ins.cs).toEqual([{ kind: "distance", atoms: [0, 1], value: 1.3 }]);
    }
  });

  it("REGRESSION A — a hand-written comment inside the block is not destroyed", () => {
    // The 2.5.4b bug: adding a constraint via the button rewrote the block and the
    // comment vanished. Now the block is unrecognised → the add path is blocked,
    // so injectConstraints is never called and the text is left alone.
    const withComment = `%geom
  Constraints
    {B 0 1 C}
#   {B 1 2 C}
  end
end
* xyz 0 1
C 0 0 0
O 0 0 1.2
H 0 1 0
*
`;
    const ins = inspectConstraintsBlock(withComment);
    expect(ins.kind).toBe("unrecognised");
    // The guard the UI relies on: we never call inject on an unrecognised block.
    expect(parseConstraintsBlock(withComment)).toBeNull();
  });

  it("REGRESSION B — an unknown token does not wipe the user's valid constraints", () => {
    // {X 9 9 C} is syntax we don't model. Old behaviour: parse → null, panel empty,
    // a button-add rewrote the block from [] and destroyed TWO valid constraints.
    // Now: unrecognised, so the block is read-only and untouched.
    const withUnknown = `%geom
  Constraints
    {B 0 1 C}
    {X 9 9 C}
    {A 0 1 2 C}
  end
end`;
    const ins = inspectConstraintsBlock(withUnknown);
    expect(ins.kind).toBe("unrecognised");
    if (ins.kind === "unrecognised") expect(ins.sample).toContain("X");
    expect(parseConstraintsBlock(withUnknown)).toBeNull();
  });
});

describe("value text is preserved as written (2.5.5)", () => {
  it("90.0 survives a rewrite (was flattened to 90)", () => {
    const text = `%geom
  Constraints
    {D 0 1 2 3 90.0 C}
  end
end`;
    const ins = inspectConstraintsBlock(text);
    expect(ins.kind).toBe("parsed");
    if (ins.kind !== "parsed") throw new Error("expected parsed");
    // The semantic value is the number; the exact text rides alongside.
    expect(ins.cs[0]).toMatchObject({ kind: "dihedral", value: 90, valueText: "90.0" });
    // A rewrite (add another constraint) keeps 90.0, not 90.
    const rewritten = injectConstraints(text, [
      ...ins.cs,
      { kind: "distance", atoms: [0, 1] },
    ]);
    expect(rewritten).toContain("{D 0 1 2 3 90.0 C}");
    expect(rewritten).not.toContain("90 C");
  });

  it("a canonical number carries no valueText (clean round-trip)", () => {
    const text = injectConstraints(BASE, [
      { kind: "dihedral", atoms: [0, 1, 2, 3], value: 90 },
    ]);
    const ins = inspectConstraintsBlock(text);
    if (ins.kind === "parsed") expect(ins.cs[0]).not.toHaveProperty("valueText");
  });
});
