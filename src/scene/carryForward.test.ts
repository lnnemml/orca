import { describe, it, expect } from "vitest";

import type { Job, ParsedResults } from "../types";
import {
  resolveCarryForwardGeometry,
  geometryMatchesFinal,
  carryForwardProvenanceComment,
  withProvenanceComment,
  iterationFrames,
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

describe("iterationFrames — explicit frame picker, default = the optimized output (debugging/022)", () => {
  // A 3-cycle optimization trajectory: initial (frame 0), a middle cycle, the optimized output (last).
  // The last frame's forming C–C = 2.2893 (the converged output); frame 0 = the initial seed 2.3636.
  const TRAJ = {
    n_frames: 3,
    elements: ["C", "C"],
    frames: [
      { energy_eh: -234.30, xyz_angstrom: [[0, 0, 0], [0, 0, 2.3636]] as [number, number, number][] },
      { energy_eh: -234.48, xyz_angstrom: [[0, 0, 0], [0, 0, 2.31]] as [number, number, number][] },
      { energy_eh: -234.50, xyz_angstrom: [[0, 0, 0], [0, 0, 2.2893]] as [number, number, number][] },
    ],
  };

  it("default_is_the_last_optimized_frame_not_the_seed", () => {
    // The bug bite: a post-GOAT Opt parses converged === null, but it HAS a full trajectory — the
    // picker must still DEFAULT to the last (optimized) frame, never frame 0 / the input seed.
    const fr = iterationFrames(job(), results({ converged: null, trajectory: TRAJ }));
    expect(fr.ok).toBe(true);
    if (!fr.ok) return;
    expect(fr.defaultIndex).toBe(2); // the LAST frame, not 0
    // The default geometry is the OPTIMIZED output (2.2893), NOT the initial seed (2.3636).
    expect(fr.frames[fr.defaultIndex].geometry.xyz_angstrom[1][2]).toBe(2.2893);
    expect(fr.frames[0].geometry.xyz_angstrom[1][2]).toBe(2.3636); // frame 0 IS the seed — but not the default
    // Labels: last = optimized output (verdict null → no false convergence claim), 0 = initial, mid = cycle.
    expect(fr.frames[2].label).toMatch(/optimized output/i);
    expect(fr.frames[0].label).toBe("initial geometry");
    expect(fr.frames[1].label).toBe("cycle 1");
    // A converged verdict names the last frame "final (converged)".
    const conv = iterationFrames(job(), results({ converged: true, trajectory: TRAJ }));
    if (conv.ok) expect(conv.frames[2].label).toMatch(/final \(converged\)/i);
  });

  it("non_converged_last_frame_is_labeled_not_stationary", () => {
    // converged === false → the last frame is NOT stationary: label warns, but it stays SELECTABLE
    // (a real frame in the list, default still last) — informed, never refused/silent.
    const fr = iterationFrames(job(), results({ converged: false, trajectory: TRAJ }));
    expect(fr.ok).toBe(true);
    if (!fr.ok) return;
    expect(fr.defaultIndex).toBe(2);
    expect(fr.frames[2].label).toMatch(/did not converge|not stationary/i);
  });

  it("scan_or_neb_refuses_the_frame_picker", () => {
    // A scan / NEB has a trajectory field shape but keeps its per-point/per-image handoff — the frame
    // picker REFUSES, reusing the carry-forward reasons (never a whole-band single seed).
    const scan = iterationFrames(job(), results({ scan: {} as never, trajectory: TRAJ }));
    expect(scan.ok).toBe(false);
    if (!scan.ok) {
      expect(scan.kind).toBe("scan");
      expect(scan.reason).toMatch(/scan/i);
    }
    const neb = iterationFrames(job(), results({ neb: {} as never, trajectory: TRAJ }));
    expect(neb.ok).toBe(false);
    if (!neb.ok) {
      expect(neb.kind).toBe("neb");
      expect(neb.reason).toMatch(/NEB/i);
    }
    // A single point (no trajectory) refuses distinctly — its geometry IS its input, nothing to pick.
    const sp = iterationFrames(job(), results({ converged: null, trajectory: null }));
    expect(sp.ok).toBe(false);
    if (!sp.ok) expect(sp.kind).toBe("no-trajectory");
  });

  it("every_frame_geometry_comes_from_the_trajectory", () => {
    // The guard: no frame's geometry is reconstructed from input_content — each equals the matching
    // results.trajectory.frames[i] (elements shared, per-frame coords), value-for-value.
    const fr = iterationFrames(job(), results({ converged: true, trajectory: TRAJ }));
    expect(fr.ok).toBe(true);
    if (!fr.ok) return;
    fr.frames.forEach((choice, i) => {
      expect(choice.index).toBe(i);
      expect(choice.geometry.elements).toEqual(TRAJ.elements);
      expect(choice.geometry.xyz_angstrom).toEqual(TRAJ.frames[i].xyz_angstrom);
      expect(choice.energyEh).toBe(TRAJ.frames[i].energy_eh);
    });
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
