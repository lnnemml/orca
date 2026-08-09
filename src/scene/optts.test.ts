import { describe, it, expect } from "vitest";

import { buildOptTSInput, type TsGuessGeometry } from "./optts";

/** A tiny N···C seed (the SN2 forming bond), easy to eyeball. */
const SEED: TsGuessGeometry = {
  elements: ["N", "C"],
  xyz_angstrom: [
    [0, 0, 0],
    [0, 0, 2.353],
  ],
};

/** The `* xyz <c> <m>` header line of a built input. */
function xyzHeader(input: string): string {
  return input.split(/\r?\n/).find((l) => l.trim().startsWith("* xyz"))!.trim();
}
/** The `!` keyword line. */
function keywordLine(input: string): string {
  return input.split(/\r?\n/).find((l) => l.startsWith("!"))!;
}

// ── C-charge-inherited — THE footgun: (c,m) come from the source, never a 0 default ──
describe("buildOptTSInput — charge/multiplicity inheritance (THE footgun)", () => {
  it("carries an anion's `-1` from source to child (BH₄⁻ class), never `0 1`", () => {
    const source = "! r2SCAN-3c OptTS SMD(DMF)\n* xyz -1 1\nB 0 0 0\nH 1 1 1\n*\n";
    const child = buildOptTSInput(source, SEED);
    expect(xyzHeader(child)).toBe("* xyz -1 1");
    expect(child).not.toContain("* xyz 0 1");
  });

  it("carries a radical multiplicity `0 2` from source to child", () => {
    const source = "! r2SCAN-3c LooseOpt\n* xyz 0 2\nO 0 0 0\nH 1 0 0\n*\n";
    expect(xyzHeader(buildOptTSInput(source, SEED))).toBe("* xyz 0 2");
  });

  it("throws (never defaults to 0) when the source has no inline `* xyz` block", () => {
    // Bite: a version that defaulted charge to 0 would emit a `* xyz 0 1` child here.
    expect(() => buildOptTSInput("! r2SCAN-3c LooseOpt\n* xyzfile 0 1 geo.xyz\n", SEED)).toThrow(
      /coordinate block/i,
    );
    expect(() => buildOptTSInput("! r2SCAN-3c\n", SEED)).toThrow(/coordinate block/i);
  });
});

// ── C-optts-freq-calchess — the OptTS recipe; the source's opt/Scan MUST NOT leak ──
describe("buildOptTSInput — OptTS + Freq + TightSCF + Calc_Hess; no Scan/LooseOpt leak", () => {
  // A realistic scan source: LooseOpt + a %geom Scan block — both must be gone from the child.
  const scanSource =
    "! r2SCAN-3c LooseOpt SMD(DMF) TightSCF\n" +
    "%geom Scan B 0 1 = 3.000, 1.800, 12 end end\n" +
    "%pal nprocs 4 end\n%maxcore 2000\n\n* xyz 0 1\nN 0 0 0\nC 0 0 1.8\n*\n";

  it("emits `! … OptTS Freq … TightSCF` and a `%geom Calc_Hess true` block", () => {
    const child = buildOptTSInput(scanSource, SEED);
    const kw = keywordLine(child);
    expect(kw).toContain("OptTS");
    expect(kw).toContain("Freq");
    expect(kw).toContain("TightSCF");
    expect(child).toMatch(/%geom\s+Calc_Hess\s+true\s+end/i);
  });

  it("does NOT carry the source's opt keyword or Scan block (bite: append keeps LooseOpt)", () => {
    const child = buildOptTSInput(scanSource, SEED);
    // Bite: a naive "append OptTS to the source `!` line" would still contain LooseOpt; and a
    // "reuse the source verbatim" would still contain the Scan block. buildOrcaInput builds fresh.
    expect(child).not.toContain("LooseOpt");
    expect(child).not.toMatch(/%geom\s+Scan/i);
    expect(child).not.toContain("Scan B");
  });
});

// ── C-method-inherited — method+solvation default to the source's (comparable by construction) ──
describe("buildOptTSInput — method + solvation inherited from the source (comparability)", () => {
  it("r2SCAN-3c + SMD(DMF) in → same out (not a hardcoded default)", () => {
    const source = "! r2SCAN-3c LooseOpt SMD(DMF) TightSCF\n* xyz 0 1\nN 0 0 0\nC 0 0 1.8\n*\n";
    const kw = keywordLine(buildOptTSInput(source, SEED));
    expect(kw).toContain("r2SCAN-3c");
    expect(kw).toContain("SMD(DMF)");
  });

  it("a DIFFERENT source method+solvation is inherited, not overridden (bite: hardcoded r2SCAN-3c)", () => {
    // Bite: reopt.ts defaults to r2SCAN-3c; an OptTS engine that did the same would fail here.
    const source =
      "! B3LYP def2-TZVP def2/J RIJCOSX D4 SMD(water) Opt TightSCF\n* xyz 0 1\nN 0 0 0\nC 0 0 1.8\n*\n";
    const kw = keywordLine(buildOptTSInput(source, SEED));
    expect(kw).toContain("B3LYP");
    expect(kw).toContain("def2-TZVP");
    expect(kw).toContain("SMD(water)");
    expect(kw).not.toContain("r2SCAN-3c");
  });

  it("options.method / options.solvation override the inherited defaults", () => {
    const source = "! r2SCAN-3c SMD(DMF) LooseOpt\n* xyz 0 1\nN 0 0 0\nC 0 0 1.8\n*\n";
    const kw = keywordLine(
      buildOptTSInput(source, SEED, { method: "wB97X-3c", solvation: "SMD(acetonitrile)" }),
    );
    expect(kw).toContain("wB97X-3c");
    expect(kw).toContain("SMD(acetonitrile)");
    expect(kw).not.toContain("r2SCAN-3c");
    expect(kw).not.toContain("SMD(DMF)");
  });
});

// ── seed geometry preserved (count + order) ──────────────────────────────────────
describe("buildOptTSInput — the seed geometry is emitted exactly (count + order)", () => {
  it("emits exactly the seed's atoms, in its order", () => {
    const seed: TsGuessGeometry = {
      elements: ["C", "N", "I"],
      xyz_angstrom: [
        [0, 0, 0],
        [0, 0, 2.35],
        [0, 0, -2.59],
      ],
    };
    const source = "! r2SCAN-3c LooseOpt\n* xyz 0 1\nC 0 0 0\n*\n";
    const child = buildOptTSInput(source, seed);
    const rows = child.split(/\r?\n/).filter((l) => /^[A-Z][a-z]?\s/.test(l.trim()));
    expect(rows).toHaveLength(3);
    expect(rows.map((r) => r.trim().split(/\s+/)[0])).toEqual(["C", "N", "I"]);
  });

  it("throws on an elements/coordinate length mismatch (rule #9)", () => {
    const bad: TsGuessGeometry = { elements: ["N", "C", "H"], xyz_angstrom: [[0, 0, 0]] };
    const source = "! r2SCAN-3c\n* xyz 0 1\nN 0 0 0\n*\n";
    expect(() => buildOptTSInput(source, bad)).toThrow();
  });
});
