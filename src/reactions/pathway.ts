//! Pure logic for the reaction/pathway management UI (Phase 4.5 Stage C2a).
//!
//! The testable slice of a mostly-UI unit: the predicate that marks/warns scan jobs
//! in the attach picker, and pathway-label validation. No component, no Tauri, no
//! energy/coordinate reading (that is C2b) — just functions over the shared types.

import type { ParsedResults } from "../types";

/**
 * Whether a job's parsed results carry a **relaxed-scan profile** — the jobs C2b's
 * comparative ΔΔE‡ overlay can actually plot. Drives the attach picker's mark/warn:
 * a scan job is offered plainly; a non-scan job is warned about (but still allowed —
 * C1's `attach_job_to_pathway` is permissive; the comparability guard is C2b).
 *
 * Results-based by definition: `results.scan` is non-null with at least one point.
 * `null`/`undefined` results (a job that never parsed, or a non-scan job) → false.
 */
export function isScanJob(results: ParsedResults | null | undefined): boolean {
  return !!results?.scan && results.scan.points.length > 0;
}

/**
 * Whether a job's parsed results carry a **NEB band with a converged TS** — the jobs
 * N4's comparison can plot as a pathway (via `nebMepCurve` + the located-TS ΔΔ table).
 * Mirrors {@link isScanJob}: results-based, `null`/non-NEB → false. A converged TS means
 * `neb.ts_energy_eh` is set (the saddle energy the G1 estimate reads) OR a `ts_geometry`
 * is present (always the case for a parsed NEB — `from_neb` requires it).
 */
export function isNebJob(results: ParsedResults | null | undefined): boolean {
  const neb = results?.neb;
  if (!neb) return false;
  return neb.ts_energy_eh != null || neb.ts_geometry != null;
}

/** A pathway label must be non-empty after trimming. */
export function isValidPathwayLabel(label: string): boolean {
  return label.trim().length > 0;
}

/** The label as stored — trimmed. Pair with {@link isValidPathwayLabel} before use. */
export function normalizePathwayLabel(label: string): string {
  return label.trim();
}
