import { describe, it, expect } from "vitest";

import {
  type ScanCoordinate,
  scanBlock,
  parseScanBlock,
  inspectScanBlock,
  injectScan,
  scanOptIssue,
  scanFromSelection,
} from "./scan";
import { injectConstraints, parseConstraintsBlock } from "./constraints";
import { testScene, idsFor, borohydrideAfterWaterRemoved } from "./scene-test-util";

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

// ══ Stage A2 — panel/guard/selection pure logic + the three negative controls ══

/** A single 41-carbon fragment: on this fresh scene AtomId == global index, so
 * `scanFromSelection(big, idsFor(big, …))` emits exactly the indices passed. */
const big = testScene([
  {
    id: "m",
    name: "M",
    charge: 0,
    source: "editor",
    atoms: Array.from({ length: 41 }, (_, i) => ({ element: "C", x: i, y: 0, z: 0 })),
  },
]);

describe("scanFromSelection — length→kind, AtomId resolved at build time", () => {
  it("2/3/4 atoms → B/A/D with the given range", () => {
    expect(scanFromSelection(big, idsFor(big, 0, 1), { start: 1.4, end: 2.4, npoints: 6 })).toEqual({
      kind: "B",
      atoms: [0, 1],
      start: 1.4,
      end: 2.4,
      npoints: 6,
    });
    expect(scanFromSelection(big, idsFor(big, 2, 1, 0), { start: 100, end: 120, npoints: 5 })).toEqual(
      { kind: "A", atoms: [2, 1, 0], start: 100, end: 120, npoints: 5 },
    );
    expect(
      scanFromSelection(big, idsFor(big, 0, 1, 2, 3), { start: 0, end: 180, npoints: 10 }),
    ).toEqual({ kind: "D", atoms: [0, 1, 2, 3], start: 0, end: 180, npoints: 10 });
  });

  it("rejects a bad selection length or invalid range", () => {
    const r = { start: 1, end: 2, npoints: 6 };
    expect(scanFromSelection(big, idsFor(big, 7), r)).toBeNull(); // 1 atom
    expect(scanFromSelection(big, idsFor(big, 1, 2, 3, 4, 5), r)).toBeNull(); // 5 atoms
    expect(scanFromSelection(big, idsFor(big, 0, 1), { start: 1, end: 2, npoints: 1 })).toBeNull();
    expect(
      scanFromSelection(big, idsFor(big, 0, 1), { start: NaN, end: 2, npoints: 6 }),
    ).toBeNull();
  });

  it("returns null if a selected atom has left the scene", () => {
    const { scene } = borohydrideAfterWaterRemoved();
    const gone = idsFor(scene, 900)[0];
    const present = idsFor(scene, 0)[0];
    expect(scanFromSelection(scene, [present, gone], { start: 1, end: 2, npoints: 6 })).toBeNull();
  });
});

// ── C-atomid-pick — the coordinate survives a fragment index shift ─────────────
describe("C-atomid-pick — emits CURRENT 0-based indices from an AtomId selection", () => {
  it("boron (id 3, now global 0) + an H → atoms [0,1], NOT the raw ids", () => {
    // water removed → BH₄⁻ keeps ids 3..7 but occupies global 0..4 (mirror
    // constraints c2 / bond-display): the id-keyed pick must resolve to the CURRENT
    // index, else the %geom Scan line would scan the wrong / out-of-range atom.
    const { scene, boronId } = borohydrideAfterWaterRemoved();
    const someH = idsFor(scene, 1)[0];
    const s = scanFromSelection(scene, [boronId, someH], { start: 1.2, end: 2.2, npoints: 6 })!;
    expect(s.atoms).toEqual([0, 1]); // NOT [3, 1] (the AtomIds)
    // …and it round-trips through the %geom Scan text as a valid 0-based line.
    const text = injectScan("! r2SCAN-3c Opt\n* xyz 0 1\nB 0 0 0\nH 1 0 0\n*\n", s);
    expect(parseScanBlock(text)).toEqual(s);
  });
});

// ── C-view-over-text — a panel edit is a pure content transform, no parallel store
describe("C-view-over-text — every ScanPanel edit is a pure injectScan(content) transform", () => {
  const base = "! r2SCAN-3c Opt\n* xyz 0 1\nC 0 0 0\nC 0 0 1.5\nH 0 0.9 -0.4\n*\n";
  const scan: ScanCoordinate = { kind: "B", atoms: [0, 1], start: 1.4, end: 2.4, npoints: 6 };
  const content = injectScan(base, scan);

  it("editing npoints reflects EXACTLY in the text (inspect is the whole truth)", () => {
    // The exact transform ScanPanel.setN performs — no React state that IS the scan.
    const next = injectScan(content, { ...scan, npoints: 8 });
    const read = inspectScanBlock(next);
    expect(read.kind).toBe("parsed");
    expect(read.kind === "parsed" && read.scan.npoints).toBe(8);
  });

  it("editing start preserves the exact typed text (setStart with startText)", () => {
    // ScanPanel.setStart({ ...scan, start: Number("1.40"), startText: "1.40" }).
    const next = injectScan(content, { ...scan, start: 1.4, startText: "1.40" });
    expect(next).toContain("= 1.40, 2.4, 6");
    const read = inspectScanBlock(next);
    expect(read.kind === "parsed" && read.scan.startText).toBe("1.40");
  });

  it("a re-render from the edited content shows the edit — no stale parallel copy", () => {
    // What a re-rendered panel reads (inspectScanBlock(content)) IS the new value;
    // there is no second source that could disagree.
    const edited = injectScan(content, { ...scan, end: 3.0, endText: "3.0", npoints: 12 });
    const read = inspectScanBlock(edited);
    expect(read.kind === "parsed" && read.scan).toMatchObject({ end: 3, npoints: 12 });
  });
});

// ── C-tightopt-block — the guard family is MEASURED, not narrow ────────────────
describe("C-tightopt-block — a measured opt keyword is NOT false-blocked", () => {
  const geom = "\n* xyz 0 1\nC 0 0 0\nC 0 0 1.5\n*\n";
  const scan: ScanCoordinate = { kind: "B", atoms: [0, 1], start: 1.4, end: 2.4, npoints: 6 };

  // Each measured to trigger a relaxed scan (rule #10 — real ORCA runs, wiki/orca/scan.md).
  for (const kw of ["Opt", "OptTS", "TightOpt", "VeryTightOpt", "LooseOpt"]) {
    it(`! ${kw} scan is NOT blocked (measured relaxed-scan trigger)`, () => {
      const input = injectScan(`! r2SCAN-3c ${kw} TightSCF${geom}`, scan);
      expect(scanOptIssue(input)).toBeNull();
    });
  }

  it("the control bites: a scan with NO opt keyword IS blocked", () => {
    const input = injectScan(`! r2SCAN-3c TightSCF${geom}`, scan);
    expect(scanOptIssue(input)).toMatch(/Opt/);
  });

  it("a non-opt keyword that merely contains 'opt'-ish text does not count", () => {
    // `Optimizer`-like tokens aren't in the measured set — only exact keywords are.
    const input = injectScan(`! r2SCAN-3c SP TightSCF${geom}`, scan);
    expect(scanOptIssue(input)).toMatch(/Opt/);
  });
});
