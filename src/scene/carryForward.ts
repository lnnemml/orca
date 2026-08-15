//! Geometry carry-forward resolution + guard (the input-vs-output fix).
//!
//! When a new job is seeded from a completed job's geometry ("New iteration"), the geometry
//! MUST come from the parent's **converged output** — its parsed `results.final_geometry` (the
//! SAME source the 3D viewer and `finalGeometryXyz` use: `input.xyz` ≡ the last `_trj.xyz` frame ≡
//! the parsed final geometry), **never** the parent's `input_content` / `scene_json` (the
//! creation-time SEED). For an optimization the two differ (measured: a DA OptTS seed forming
//! C–C = 2.364 Å vs converged 2.289 Å), and using the seed silently seats every downstream
//! single-point on a non-stationary geometry → a wrong barrier with no crash. See
//! `wiki/debugging/021-input-vs-output-geometry-carry-forward.md`.
//!
//! This module is pure (no React, no Tauri) so the resolution + the defense-in-depth guard are
//! unit-tested in isolation.

import type { Job, ParsedResults } from "../types";

type FinalGeometry = ParsedResults["final_geometry"];

/** Why a geometry was carried: `converged` = the optimized stationary output; `single-point` = a
 * single-point job whose final geometry IS its input (it moves no atoms — no seed/output split). */
export type GeometryOrigin = "converged" | "single-point";

/** The resolved carry-forward: a converged/SP geometry to seed from, OR an honest refusal with a
 * reason (never a silent fallback to the seed). */
export type CarryForward =
  | {
      ok: true;
      geometry: FinalGeometry;
      origin: GeometryOrigin;
      sourceJobId: string;
      /** UI note — makes clear this is a fresh scene from the parent's output, not seed-editing. */
      note: string;
    }
  | { ok: false; reason: string };

/**
 * Resolve the geometry to carry forward from a completed job, from its **parsed results** (the
 * converged output), or refuse honestly. Order of refusals:
 *  - no parsed result (unparsed / GOAT ensemble) → refuse (pick a conformer elsewhere);
 *  - a **scan** or **NEB** (multi-structure — no single "output") → refuse, direct to the
 *    per-point / per-image handoff (never silently pick one frame);
 *  - a **non-converged** optimization (`converged === false`) → refuse: its last geometry is not
 *    stationary (rule #9), so it must not seed a downstream single-point silently;
 *  - otherwise carry `results.final_geometry` — `converged === true` → `converged`; a single point
 *    (`converged === null`, no scan/neb) → `single-point` (final == input, no bug there).
 */
export function resolveCarryForwardGeometry(
  job: Job,
  results: ParsedResults | null,
): CarryForward {
  if (!results || results.final_geometry.elements.length === 0) {
    return {
      ok: false,
      reason: `“${job.title}” has no parsed result geometry yet (or it is a GOAT ensemble — pick a conformer instead).`,
    };
  }
  if (results.scan) {
    return {
      ok: false,
      reason: `“${job.title}” is a relaxed scan (many geometries) — open it and pick a point/node to refine; there is no single output geometry to carry.`,
    };
  }
  if (results.neb) {
    return {
      ok: false,
      reason: `“${job.title}” is a NEB run (a band of geometries) — refine its TS or pick an image, not the whole job.`,
    };
  }
  if (results.converged === false) {
    return {
      ok: false,
      reason: `“${job.title}” did not converge (reached max cycles) — its last geometry is not a stationary point; re-run to convergence before carrying it forward.`,
    };
  }
  const origin: GeometryOrigin = results.converged === true ? "converged" : "single-point";
  const note =
    origin === "converged"
      ? `Seeded from the converged output geometry of “${job.title}” — a fresh scene from its optimized structure (not the parent's seed/edit history).`
      : `Seeded from the geometry of “${job.title}” (single point — its final geometry is its input).`;
  return { ok: true, geometry: results.final_geometry, origin, sourceJobId: job.id, note };
}

/**
 * Defense-in-depth guard: a geometry about to be carried downstream MUST bit-match the parent's
 * parsed **final** geometry (≡ the last `_trj.xyz` frame ≡ `input.xyz` ≡ the converged output).
 * A seed / first-frame geometry (the OptTS start, 2.364 vs converged 2.289) differs and is
 * **rejected** — even under a future regression that re-routes the source. Bit-exact (same stored
 * precision), element order included; NOT a tolerance (the seed is off by ~0.07 Å, but a stray
 * re-route could be off by less — only the exact final frame is accepted).
 */
export function geometryMatchesFinal(geometry: FinalGeometry, results: ParsedResults): boolean {
  const f = results.final_geometry;
  if (geometry.elements.length !== f.elements.length) return false;
  for (let i = 0; i < f.elements.length; i++) {
    if (geometry.elements[i] !== f.elements[i]) return false;
    for (let k = 0; k < 3; k++) {
      if (geometry.xyz_angstrom[i][k] !== f.xyz_angstrom[i][k]) return false;
    }
  }
  return true;
}

/** Provenance header line for a carried-forward input — records the source job + geometry origin
 * IN the input (visible on screen, persisted in `input_content`, and copied verbatim into an
 * export). This makes an input-vs-output swap impossible to ship silently. */
export function carryForwardProvenanceComment(
  cf: Extract<CarryForward, { ok: true }>,
): string {
  return `# geometry: ${cf.origin} output of job ${cf.sourceJobId}`;
}

/** Prepend the provenance comment to an input if it is not already present (idempotent). */
export function withProvenanceComment(content: string, comment: string): string {
  if (content.split(/\r?\n/).some((l) => l.trim() === comment.trim())) return content;
  return `${comment}\n${content}`;
}
