import { describe, it, expect } from "vitest";

import {
  type Constraint,
  ORCA_INDEX_BASE,
  toOrcaIndex,
  fromOrcaIndex,
  constraintsBlock,
  parseConstraintsBlock,
  injectConstraints,
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

  it("strips a trailing inline comment on a constraint line", () => {
    const withComment = `%geom
  Constraints
    {B 0 1 C}   # freeze the forming bond
  end
end`;
    expect(parseConstraintsBlock(withComment)).toEqual([
      { kind: "distance", atoms: [0, 1] },
    ]);
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
