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

import type { NebResults, NebIteration, NebImageGeometry } from "../types";
import { frameToXyz, elementsAgree } from "../trajectory/frame";
import { finalGeometryXyz } from "../export/exporters";
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

// ── MEP band GEOMETRIES (N3) — the 3D image stepper + saddle ────────────────────
// The band-image geometries are loaded on demand (`read_neb_geometries` →
// `input_MEP_trj.xyz`) and shown one at a time (ADR-011). Labelled by geometry +
// energy ONLY — never a source job's title (the HCN/HNC mislabel lesson): the
// endpoints are "Image 0/N (E=…)", the max-energy image is "≈ saddle".

/**
 * The index of the highest-energy MEP image — the **≈ saddle** (the converged band
 * carries the barrier in its interior, `wiki/orca/neb.md`). Argmax over each image's
 * comment energy (`null` energies sort lowest); ties take the first; empty → 0.
 */
export function bandMaxIndex(geoms: NebImageGeometry[]): number {
  let mi = 0;
  let mv = -Infinity;
  geoms.forEach((g, i) => {
    const v = g.energy_eh ?? -Infinity;
    if (v > mv) {
      mv = v;
      mi = i;
    }
  });
  return mi;
}

/**
 * Prepare ONE MEP band image for the viewer, keyed by the app-owned index (ADR-011).
 * Element-order identity is checked at this boundary (`elementsAgree` vs the converged-TS
 * element order) BEFORE any render — a mismatch is a loud refusal, never a silent wrong
 * molecule (the UI echo of the reader's element-order discipline). Mirrors
 * `scanProfile.pointGeometryXyz`.
 */
export function imageGeometryXyz(
  geom: NebImageGeometry,
  referenceElements: string[],
): { xyz: string } | { error: string } {
  if (!elementsAgree(geom.elements, referenceElements)) {
    return {
      error:
        `NEB image atom order does not match the converged-TS geometry ` +
        `(${geom.elements.length} vs ${referenceElements.length} atoms / different sequence). ` +
        `Not rendering — this would draw the wrong molecule.`,
    };
  }
  return {
    xyz: frameToXyz(geom.elements, {
      energy_eh: geom.energy_eh,
      xyz_angstrom: geom.xyz_angstrom,
    }),
  };
}

/**
 * The SELECTED MEP image as a standard `.xyz`, for export — labelled by INDEX + energy
 * only (never a source endpoint job's title; the HCN/HNC lesson). `isMax` tags the
 * ≈ saddle. REUSES `finalGeometryXyz` (the one xyz builder + its atoms+2 post-condition,
 * rule #9). `jobTitle` is the NEB job's own title (the current job — used for the file
 * name/comment exactly as the scan export does), never a reactant/product endpoint title.
 */
export function imageExportXyz(
  geom: NebImageGeometry,
  nImages: number,
  isMax: boolean,
  jobTitle: string,
): string {
  const tag = isMax ? " (≈ saddle / max-energy image)" : "";
  const comment = `${jobTitle} — NEB image ${geom.index}/${nImages - 1}${tag}`;
  return finalGeometryXyz(geom, comment, geom.energy_eh);
}
