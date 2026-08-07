import { describe, it, expect } from "vitest";

import type { ParsedResults, ScanProfileJson } from "../types";
import { isScanJob, isValidPathwayLabel, normalizePathwayLabel } from "./pathway";

/** A minimal ParsedResults with only the fields the predicate reads. The rest of the
 * shape is irrelevant to `isScanJob`, so we cast a partial through `unknown`. */
function results(scan: ScanProfileJson | null): ParsedResults {
  return { scan } as unknown as ParsedResults;
}

const SCAN: ScanProfileJson = {
  kind: "B",
  atoms: [0, 1],
  coordinate_unit: "Å",
  points: [
    { coordinate: 1.4, energy_act_eh: -79.782, energy_scf_eh: -79.785 },
    { coordinate: 1.6, energy_act_eh: -79.789, energy_scf_eh: -79.792 },
  ],
};

describe("isScanJob (C-scan-detection)", () => {
  it("is TRUE for results carrying a scan profile with points", () => {
    expect(isScanJob(results(SCAN))).toBe(true);
  });

  it("is FALSE for results without a scan profile (Opt/SP job)", () => {
    expect(isScanJob(results(null))).toBe(false);
  });

  it("is FALSE for null/undefined results (never parsed)", () => {
    expect(isScanJob(null)).toBe(false);
    expect(isScanJob(undefined)).toBe(false);
  });

  it("is FALSE for a scan object with zero points (nothing to plot)", () => {
    expect(isScanJob(results({ ...SCAN, points: [] }))).toBe(false);
  });

  // The bite: a version that treats every job as a scan (e.g. `return true`) would
  // pass the TRUE case but fail every FALSE case above — the picker would then never
  // warn on a non-scan job, defeating the mark/warn the gate checks (m2).
});

describe("pathway label validation (C-empty-label)", () => {
  it("rejects an empty or whitespace-only label", () => {
    expect(isValidPathwayLabel("")).toBe(false);
    expect(isValidPathwayLabel("   ")).toBe(false);
    expect(isValidPathwayLabel("\t\n")).toBe(false);
  });

  it("accepts a non-empty label and normalizes surrounding whitespace", () => {
    expect(isValidPathwayLabel("si face")).toBe(true);
    expect(isValidPathwayLabel("  re face  ")).toBe(true);
    expect(normalizePathwayLabel("  si face  ")).toBe("si face");
  });
});
