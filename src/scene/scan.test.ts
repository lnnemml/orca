import { describe, it, expect } from "vitest";

import {
  type ScanCoordinate,
  scanBlock,
  parseScanBlock,
  inspectScanBlock,
  injectScan,
  scanOptIssue,
} from "./scan";
import { injectConstraints, parseConstraintsBlock } from "./constraints";

/** BASE mirrors the constraints fixtures: a keyword line WITH `Opt`, then a 3-atom
 * geometry (indices 0,1,2 resolve). */
const BASE = `! r2SCAN-3c Opt
* xyz 0 1
C   0.000000   0.000000   0.000000
C   0.000000   0.000000   1.530000
H   0.000000   0.900000  -0.400000
*
`;

/** The ethane C–C scan of unit 3.3 (indices 0,1; 1.4→2.4; 6 points). */
const ETHANE_CC: ScanCoordinate = { kind: "B", atoms: [0, 1], start: 1.4, end: 2.4, npoints: 6 };

/** The exact canonical block — the byte-identity gate, mirrored verbatim in the
 * Rust golden `emit::tests::scan_block_golden_ethane`. */
const CANONICAL = "%geom\n  Scan\n    B 0 1 = 1.4, 2.4, 6\n  end\nend";

const countGeom = (s: string) => (s.match(/%geom/gi) ?? []).length;

describe("scanBlock — the byte-identity golden (twin of the Rust golden)", () => {
  it("emits the exact canonical ethane C–C block", () => {
    expect(scanBlock(ETHANE_CC)).toBe(CANONICAL);
  });

  it("angle (A) and dihedral (D) forms", () => {
    expect(scanBlock({ kind: "A", atoms: [2, 1, 0], start: 100, end: 120, npoints: 5 })).toBe(
      "%geom\n  Scan\n    A 2 1 0 = 100, 120, 5\n  end\nend",
    );
    expect(scanBlock({ kind: "D", atoms: [0, 1, 2, 3], start: 0, end: 180, npoints: 10 })).toBe(
      "%geom\n  Scan\n    D 0 1 2 3 = 0, 180, 10\n  end\nend",
    );
  });
});

describe("round-trip: parseScanBlock(injectScan(x, s)) === s", () => {
  it("canonical endpoints round-trip with no startText/endText baggage", () => {
    const out = injectScan(BASE, ETHANE_CC);
    expect(parseScanBlock(out)).toEqual(ETHANE_CC);
  });

  it("non-canonical endpoint text is preserved through the round-trip", () => {
    const s: ScanCoordinate = {
      kind: "B",
      atoms: [0, 1],
      start: 1.4,
      startText: "1.40", // String(1.4) === "1.4" ≠ "1.40" → must be preserved
      end: 2.4,
      npoints: 6,
    };
    const out = injectScan(BASE, s);
    expect(out).toContain("= 1.40, 2.4, 6");
    expect(parseScanBlock(out)).toEqual(s);
  });
});

describe("injectScan — no %geom present", () => {
  const out = injectScan(BASE, ETHANE_CC);
  it("inserts a single %geom before the coordinate block", () => {
    expect(countGeom(out)).toBe(1);
    expect(out.indexOf("%geom")).toBeLessThan(out.indexOf("* xyz"));
    expect(parseScanBlock(out)).toEqual(ETHANE_CC);
  });
  it("null scan on scan-less input is a no-op", () => {
    expect(injectScan(BASE, null)).toBe(BASE);
  });
});

// ── ЗАХИСТ 1 (C-two-geom): the central correctness property of the unit ────────
describe("C-two-geom — Scan composes into the ONE %geom, never a second", () => {
  // A pre-existing Constraints block (distance C–H) to compose alongside.
  const withConstraint = injectConstraints(BASE, [{ kind: "distance", atoms: [1, 2], value: 1.5 }]);

  // The WRONG mirror the composition rule forbids: an injector that writes its own
  // `%geom Scan … end end` (a naive copy of the no-%geom path). Kept in the test to
  // PROVE the property below can go red — the negative control must bite.
  const injectScanNaive = (input: string, s: ScanCoordinate): string => {
    const coordIdx = input.search(/^[ \t]*\*/m);
    return input.slice(0, coordIdx) + scanBlock(s) + "\n" + input.slice(coordIdx);
  };

  it("the naive parallel injector BITES — produces two %geom (control is live)", () => {
    expect(countGeom(injectScanNaive(withConstraint, ETHANE_CC))).toBe(2);
  });

  it("injectScan yields exactly one %geom holding BOTH sub-blocks", () => {
    const good = injectScan(withConstraint, ETHANE_CC);
    expect(countGeom(good)).toBe(1);
    expect(parseConstraintsBlock(good)).not.toBeNull(); // Constraints intact
    expect(parseScanBlock(good)).toEqual(ETHANE_CC); // Scan present
  });

  it("removing the scan (null) leaves the Constraints intact, still one %geom", () => {
    const both = injectScan(withConstraint, ETHANE_CC);
    const removed = injectScan(both, null);
    expect(countGeom(removed)).toBe(1);
    expect(parseScanBlock(removed)).toBeNull();
    expect(parseConstraintsBlock(removed)).not.toBeNull();
  });

  it("re-injection replaces the Scan, never duplicates it", () => {
    const first = injectScan(withConstraint, ETHANE_CC);
    const second = injectScan(first, { kind: "B", atoms: [0, 1], start: 1.5, end: 2.5, npoints: 8 });
    expect(countGeom(second)).toBe(1);
    // \b so the `SCAN` in `r2SCAN-3c` on the keyword line isn't miscounted.
    expect((second.match(/\bScan\b/gi) ?? []).length).toBe(1);
    expect(parseScanBlock(second)).toMatchObject({ start: 1.5, end: 2.5, npoints: 8 });
  });
});

// ── C-index-base ──────────────────────────────────────────────────────────────
describe("C-index-base — atoms emit as app-global 0-based (same space as Constraint)", () => {
  it("0-based index 0,1 emits verbatim as `B 0 1`", () => {
    expect(scanBlock(ETHANE_CC)).toContain("B 0 1 =");
  });
  it("a 1-based (viewer/ASE) index would mis-emit — the control", () => {
    const wrong = scanBlock({ kind: "B", atoms: [1, 2], start: 1.4, end: 2.4, npoints: 6 });
    expect(wrong).toContain("B 1 2 =");
    expect(wrong).not.toBe(CANONICAL); // shifting the index changes the block
  });
});

// ── C-byte-parity ─────────────────────────────────────────────────────────────
describe("C-byte-parity — the canonical form is byte-exact (twin pins the same string)", () => {
  it("scanBlock is the canonical string; any formatting drift diverges", () => {
    expect(scanBlock(ETHANE_CC)).toBe(CANONICAL);
    // Drifts the Rust twin would NOT produce → the goldens would disagree (red).
    expect(scanBlock(ETHANE_CC)).not.toBe(CANONICAL.replace(" = ", "="));
    expect(scanBlock(ETHANE_CC)).not.toBe(CANONICAL.replace(", ", ","));
  });
});

// ── C-opt-guard ───────────────────────────────────────────────────────────────
describe("C-opt-guard — a relaxed scan without ! Opt is loud", () => {
  const NO_OPT = `! r2SCAN-3c TightSCF
* xyz 0 1
C   0.0   0.0   0.0
C   0.0   0.0   1.53
*
`;
  it("scan present + no Opt on the ! line → fires (control bites)", () => {
    const withScan = injectScan(NO_OPT, ETHANE_CC);
    expect(scanOptIssue(withScan)).toMatch(/Opt/);
  });
  it("scan present + Opt → silent", () => {
    expect(scanOptIssue(injectScan(BASE, ETHANE_CC))).toBeNull();
  });
  it("OptTS also satisfies the guard", () => {
    const ts = injectScan(NO_OPT.replace("TightSCF", "OptTS TightSCF"), ETHANE_CC);
    expect(scanOptIssue(ts)).toBeNull();
  });
  it("no scan → no diagnostic, with or without Opt", () => {
    expect(scanOptIssue(NO_OPT)).toBeNull();
    expect(scanOptIssue(BASE)).toBeNull();
  });
  it("a commented-out Opt does not count", () => {
    const commented = injectScan("! r2SCAN-3c TightSCF # Opt\n* xyz 0 1\nC 0 0 0\nC 0 0 1.53\n*\n", ETHANE_CC);
    expect(scanOptIssue(commented)).toMatch(/Opt/);
  });
});

// ── inspect: absent / parsed / unrecognised (non-destructive discipline) ───────
describe("inspectScanBlock — tells 'no block' from 'a block I can't fully own'", () => {
  it("absent when there is no Scan", () => {
    expect(inspectScanBlock(BASE).kind).toBe("absent");
    expect(parseScanBlock(BASE)).toBeNull();
  });
  it("a comment inside the block → unrecognised (won't be rewritten)", () => {
    const withComment = "%geom\n  Scan\n    B 0 1 = 1.4, 2.4, 6 # tweak\n  end\nend\n";
    expect(inspectScanBlock(withComment).kind).toBe("unrecognised");
    expect(parseScanBlock(withComment)).toBeNull();
  });
  it("a multi-coordinate Scan → unrecognised (A1 owns a single coordinate)", () => {
    const multi = "%geom\n  Scan\n    B 0 1 = 1.4, 2.4, 6\n    A 2 1 0 = 100, 120, 5\n  end\nend\n";
    expect(inspectScanBlock(multi).kind).toBe("unrecognised");
  });
  it("a commented-out Scan block reads as absent", () => {
    const commented = "# %geom\n#   Scan\n#     B 0 1 = 1.4, 2.4, 6\n#   end\n# end\n" + BASE;
    expect(inspectScanBlock(commented).kind).toBe("absent");
  });
  it("npoints < 2 is malformed → unrecognised", () => {
    const bad = "%geom\n  Scan\n    B 0 1 = 1.4, 2.4, 1\n  end\nend\n";
    expect(parseScanBlock(bad)).toBeNull();
  });
});
