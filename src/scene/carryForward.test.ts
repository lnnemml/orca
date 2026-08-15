import { describe, it, expect } from "vitest";

import type { Job, ParsedResults } from "../types";
import {
  resolveCarryForwardGeometry,
  geometryMatchesFinal,
  carryForwardProvenanceComment,
  withProvenanceComment,
} from "./carryForward";

/** A minimal Job (only the fields the resolver reads). */
function job(overrides: Partial<Job> = {}): Job {
  return { id: "parent-1", title: "DA OptTS", status: "parsed", ...overrides } as Job;
}

/** A minimal ParsedResults with the given final geometry + convergence verdict (+ scan/neb flags). */
function results(over: Partial<ParsedResults> = {}): ParsedResults {
  return {
    parser_version: 6,
    converged: true,
    final_energy_eh: -234.5,
    final_geometry: { elements: ["C", "C"], xyz_angstrom: [[0, 0, 0], [0, 0, 2.2893]] },
    dipole: null,
    charges: [],
    thermochemistry: null,
    gradient: null,
    frequencies: null,
    trajectory: null,
    orbitals: null,
    scan: null,
    neb: null,
    mayer_bond_orders: null,
    unknown_blocks: [],
    ...over,
  } as ParsedResults;
}

// The real DA numbers: the OptTS SEED (input_content) forming C–C = 2.3636, the CONVERGED output
// (results.final_geometry ≡ input.xyz ≡ last _trj frame) = 2.2893.
const CONVERGED: ParsedResults["final_geometry"] = {
  elements: ["C", "C"],
  xyz_angstrom: [[0, 0, 0], [0, 0, 2.2893]],
};
const SEED: ParsedResults["final_geometry"] = {
  elements: ["C", "C"],
  xyz_angstrom: [[0, 0, 0], [0, 0, 2.3636]],
};

describe("resolveCarryForwardGeometry — carry the CONVERGED output, not the seed", () => {
  it("carries results.final_geometry (the converged output) for a converged optimization", () => {
    const cf = resolveCarryForwardGeometry(job(), results({ converged: true, final_geometry: CONVERGED }));
    expect(cf.ok).toBe(true);
    if (cf.ok) {
      expect(cf.origin).toBe("converged");
      // The forming C–C is the CONVERGED 2.2893, NOT the seed 2.3636.
      expect(cf.geometry.xyz_angstrom[1][2]).toBe(2.2893);
      expect(cf.sourceJobId).toBe("parent-1");
      expect(cf.note).toMatch(/converged output/i);
    }
  });

  it("refuses a NON-CONVERGED optimization (last geometry is not stationary) — never silent", () => {
    const cf = resolveCarryForwardGeometry(job(), results({ converged: false }));
    expect(cf.ok).toBe(false);
    if (!cf.ok) expect(cf.reason).toMatch(/did not converge/i);
  });

  it("refuses a scan (many geometries) and a NEB (a band) — no single output to carry", () => {
    const scan = resolveCarryForwardGeometry(job(), results({ converged: null, scan: {} as never }));
    expect(scan.ok).toBe(false);
    if (!scan.ok) expect(scan.reason).toMatch(/scan/i);
    const neb = resolveCarryForwardGeometry(job(), results({ converged: null, neb: {} as never }));
    expect(neb.ok).toBe(false);
    if (!neb.ok) expect(neb.reason).toMatch(/NEB/i);
  });

  it("refuses when there is no parsed result (unparsed / GOAT ensemble)", () => {
    expect(resolveCarryForwardGeometry(job(), null).ok).toBe(false);
    const empty = resolveCarryForwardGeometry(job(), results({ final_geometry: { elements: [], xyz_angstrom: [] } }));
    expect(empty.ok).toBe(false);
  });

  it("a single point (converged === null, no scan/neb) carries its geometry as single-point (no bug)", () => {
    const cf = resolveCarryForwardGeometry(job(), results({ converged: null }));
    expect(cf.ok).toBe(true);
    if (cf.ok) {
      expect(cf.origin).toBe("single-point");
      expect(cf.note).toMatch(/single point/i);
    }
  });
});

describe("geometryMatchesFinal — the guard rejects the seed (the negative control)", () => {
  it("passes for the converged output (bit-match) and REJECTS the seed frame", () => {
    const r = results({ final_geometry: CONVERGED });
    // The converged output bit-matches itself → accepted.
    expect(geometryMatchesFinal(CONVERGED, r)).toBe(true);
    // BITE (the whole bug): the SEED frame (2.3636) does NOT match the converged final (2.2893) →
    // rejected. A future regression re-routing the source to the seed is caught here.
    expect(geometryMatchesFinal(SEED, r)).toBe(false);
  });

  it("rejects an element-order or atom-count mismatch", () => {
    const r = results({ final_geometry: CONVERGED });
    expect(geometryMatchesFinal({ elements: ["C"], xyz_angstrom: [[0, 0, 0]] }, r)).toBe(false);
    expect(geometryMatchesFinal({ elements: ["C", "N"], xyz_angstrom: CONVERGED.xyz_angstrom }, r)).toBe(false);
  });
});

describe("provenance — the swap can't ship silently", () => {
  it("records the source job + geometry origin, idempotently", () => {
    const cf = resolveCarryForwardGeometry(job(), results({ converged: true }));
    if (!cf.ok) throw new Error("expected ok");
    const comment = carryForwardProvenanceComment(cf);
    expect(comment).toBe("# geometry: converged output of job parent-1");
    const once = withProvenanceComment("! r2SCAN-3c Opt\n* xyz 0 1\n*\n", comment);
    expect(once.startsWith(comment)).toBe(true);
    // Idempotent — a second application does not duplicate the line.
    expect(withProvenanceComment(once, comment)).toBe(once);
  });
});
