//! NEB band-viewer transforms — pure, node-tested, React-free.
//!
//! The NEB band is already parsed and stored (`ParsedResults.neb`, `parse/neb.rs`):
//! `iterations` (per-iteration bands, ABSOLUTE Eh energies) + `mep` (the converged
//! smooth minimum-energy path, RELATIVE energies, image 0 = 0) + `final_barrier_eh`.
//! Nothing here re-parses a file; it shapes the stored arrays into the two chart
//! series the viewer draws.
//!
//! **The honest-units rule this file exists to enforce (rule #11, presentation
//! side).** The `.NEB.log` per-iteration energies are ABSOLUTE (Eh); the
//! `.final.interp` MEP energies are RELATIVE (image 0 = 0). They are NOT comparable
//! and must never share one absolute y-axis. So `iterationSeries` relativizes each
//! iteration to ITS OWN image-0 energy (ΔE vs that iteration's reactant end) — which
//! makes successive iterations overlay sensibly AND puts them on the same relative
//! footing as the already-relative MEP. Both series are then ΔE-in-kcal/mol, each
//! honestly relative to its own reactant, never a shared absolute quantity.

import type { NebResults, NebIteration } from "../types";
import { HARTREE_TO_KCAL } from "../units";

/** One point of a band/MEP line chart: arc-length distance (Å) vs ΔE (kcal/mol). */
export interface BandPoint {
  distance: number;
  deltaE_kcal: number;
}

/** One point of the barrier-convergence line: barrier (kcal/mol) vs iteration index. */
export interface BarrierPoint {
  iteration: number;
  barrier_kcal: number;
}

/**
 * One iteration's band as ΔE relative to ITS OWN image-0 energy, in kcal/mol.
 * Subtracting the iteration's reactant-end energy (absolute Eh → relative) is what
 * lets successive iterations overlay on one axis and share the MEP's relative
 * footing. An empty image list yields an empty series.
 */
export function iterationSeries(it: NebIteration): BandPoint[] {
  const e0 = it.images[0]?.energy_eh;
  if (e0 == null) return [];
  return it.images.map((img) => ({
    distance: img.distance_angstrom,
    deltaE_kcal: (img.energy_eh - e0) * HARTREE_TO_KCAL,
  }));
}

/**
 * The converged smooth MEP, in kcal/mol. Its energies are ALREADY relative
 * (image 0 = 0, from `.final.interp`), so this only converts Eh → kcal/mol — no
 * subtraction. Point 0 is therefore exactly 0.
 */
export function mepSeries(neb: NebResults): BandPoint[] {
  return neb.mep.map((img) => ({
    distance: img.distance_angstrom,
    deltaE_kcal: img.energy_eh * HARTREE_TO_KCAL,
  }));
}

/**
 * The barrier-convergence curve: each iteration's `barrier_eh` (already a relative
 * quantity — E(climbing image) − E(reactant end)) in kcal/mol, keyed by iteration
 * index. The final value equals the converged NEB barrier.
 */
export function barrierSeries(neb: NebResults): BarrierPoint[] {
  return neb.iterations.map((it) => ({
    iteration: it.index,
    barrier_kcal: it.barrier_eh * HARTREE_TO_KCAL,
  }));
}
