import { describe, it, expect } from "vitest";

import { buildReoptInput } from "./reopt";
import { parseEnsemble, type Conformer } from "./ensemble";

// A real (truncated) butane GOAT ensemble — the same fixture the D1 population
// tests use, so the conformer geometry is real, not synthetic.
import ENSEMBLE from "./__fixtures__/butane.finalensemble.xyz?raw";

/** A tiny 2-atom conformer with an easily-checked element order. */
function diatomic(a: string, b: string): Conformer {
  return {
    atoms: [
      { element: a, x: 0, y: 0, z: 0 },
      { element: b, x: 0.9, y: 0, z: 0 },
    ],
    energy: -1,
    index: 0,
  };
}

/** The `* xyz <c> <m>` header line of a built input. */
function xyzHeader(input: string): string {
  const line = input.split(/\r?\n/).find((l) => l.trim().startsWith("* xyz"));
  return line!.trim();
}

/** The `!` keyword line of a built input. */
function keywordLine(input: string): string {
  return input.split(/\r?\n/).find((l) => l.startsWith("!"))!;
}

describe("buildReoptInput — charge/multiplicity inheritance (THE footgun)", () => {
  const conf = diatomic("H", "H");

  it("carries an anion's `-1` from source to child (BH₄⁻ class)", () => {
    // The exact class that made CREST QCG useless on the anion: a child that
    // silently becomes `* xyz 0 1` terminates normally and is garbage.
    const source = "! XTB GOAT\n* xyz -1 1\nB 0 0 0\nH 1 1 1\n*\n";
    const child = buildReoptInput(source, conf);
    expect(xyzHeader(child)).toBe("* xyz -1 1");
    expect(child).not.toContain("* xyz 0 1");
  });

  it("carries a radical's multiplicity `0 2` from source to child", () => {
    const source = "! XTB GOAT\n* xyz 0 2\nO 0 0 0\nH 1 0 0\n*\n";
    const child = buildReoptInput(source, conf);
    expect(xyzHeader(child)).toBe("* xyz 0 2");
  });

  it("carries a `+2` cation from source to child", () => {
    const source = "! XTB GOAT\n* xyz 2 1\nMg 0 0 0\n*\n";
    const child = buildReoptInput(source, conf);
    expect(xyzHeader(child)).toBe("* xyz 2 1");
  });

  it("throws (never defaults to 0) when the source has no inline `* xyz` block", () => {
    // `* xyzfile geo.xyz` — external geometry; sceneFromOrcaInput returns null and
    // we must refuse rather than emit a charge-0 child.
    expect(() => buildReoptInput("! XTB GOAT\n* xyzfile 0 1 geo.xyz\n", conf)).toThrow(
      /coordinate block/i,
    );
    expect(() => buildReoptInput("! XTB GOAT\n", conf)).toThrow(/coordinate block/i);
  });
});

describe("buildReoptInput — method / Freq / SMD emission", () => {
  const conf = diatomic("H", "H");
  const source = "! XTB GOAT\n* xyz 0 1\nH 0 0 0\nH 0.7 0 0\n*\n";

  it("defaults to `! r2SCAN-3c Opt Freq`", () => {
    const kw = keywordLine(buildReoptInput(source, conf));
    expect(kw).toContain("r2SCAN-3c");
    expect(kw).toContain("Opt");
    expect(kw).toContain("Freq");
  });

  it("emits Freq iff opts.freq (Opt-only quick screen)", () => {
    const withFreq = keywordLine(buildReoptInput(source, conf, { freq: true }));
    const optOnly = keywordLine(buildReoptInput(source, conf, { freq: false }));
    expect(withFreq).toContain("Freq");
    expect(optOnly).not.toContain("Freq");
    expect(optOnly).toContain("Opt");
  });

  it("honors a custom method keyword", () => {
    const kw = keywordLine(buildReoptInput(source, conf, { method: "wB97X-3c" }));
    expect(kw).toContain("wB97X-3c");
    expect(kw).not.toContain("r2SCAN-3c");
  });

  it("emits SMD(<solvent>) iff opts.solvation, with the right solvent", () => {
    const plain = keywordLine(buildReoptInput(source, conf));
    const solv = keywordLine(
      buildReoptInput(source, conf, { solvation: { model: "smd", solvent: "methanol" } }),
    );
    expect(plain).not.toMatch(/SMD/i);
    expect(solv).toContain("SMD(methanol)");
  });
});

describe("buildReoptInput — conformer geometry (count + order preserved)", () => {
  it("emits exactly the conformer's atoms, in its order", () => {
    const conf = diatomic("C", "O"); // order C then O must survive
    const source = "! XTB GOAT\n* xyz 0 1\nC 0 0 0\nO 1 0 0\n*\n";
    const child = buildReoptInput(source, conf);
    const rows = child
      .split(/\r?\n/)
      .filter((l) => /^[A-Z][a-z]?\s/.test(l.trim()));
    expect(rows).toHaveLength(2);
    expect(rows[0].trim().startsWith("C")).toBe(true);
    expect(rows[1].trim().startsWith("O")).toBe(true);
  });

  it("carries a real 14-atom butane conformer through unchanged in count/order", () => {
    const conformers = parseEnsemble(ENSEMBLE)!;
    const source = "! XTB GOAT\n* xyz 0 1\nC 0 0 0\n*\n";
    const child = buildReoptInput(source, conformers[1]);
    const rows = child
      .split(/\r?\n/)
      .filter((l) => /^[A-Z][a-z]?\s/.test(l.trim()));
    expect(rows).toHaveLength(14);
    expect(rows.map((r) => r.trim().split(/\s+/)[0])).toEqual(
      conformers[1].atoms.map((a) => a.element),
    );
  });
});
