//! Relaxed-scan energy-profile helpers — pure, node-tested, React-free (Phase 4.5
//! Stage B2). Nothing here re-parses a file: it shapes the already-parsed B1
//! `ParsedResults.scan` (ADR-012) for the chart, and prepares one scan-point
//! geometry (loaded via `read_scan_geometries`) for the viewer.
//!
//! Two disciplines carried from Phase 3 live here as pure functions:
//!  - **Relative energy.** A barrier is a *relative* quantity; raw Eh is an
//!    unreadable large number. The plotted y is ΔE in kcal/mol against a **labelled
//!    reference** (first point, or the minimum) — the reference point is exactly 0.
//!  - **Honest labelling.** The maximum of the shown series is an *approximate* TS
//!    (a ΔE‡ estimate on a relaxed surface — ADR-007 §"ΔE‡ vs ΔG‡"), never the
//!    transition state and never ΔG‡. This module exposes the index; the UI labels
//!    it "approximate TS (scan maximum) — refine with OptTS".
//!
//! The current point is application state (the viewer is fed ONE geometry, ADR-011).
//! This module maps an app-owned index → that point's xyz, checking element-order
//! identity at the boundary (`elementsAgree`, like the trajectory).

import type { ScanProfileJson, ScanGeometry } from "../types";
import { frameToXyz, elementsAgree } from "../trajectory/frame";
import { finalGeometryXyz } from "../export/exporters";
import { HARTREE_TO_KCAL } from "../units";

/** The Hartree→kcal/mol factor — single definition in `src/units.ts`. Re-exported
 * here so `../scan/scanProfile` importers (compare.ts, CompareView.tsx) keep working. */
export { HARTREE_TO_KCAL };

/** Which energy column to plot: `act` = composite/actual (gCP+D4, the physically
 * meaningful total, default), `scf` = bare SCF. A labelled display choice. */
export type EnergyChoice = "act" | "scf";
/** The kcal/mol zero: the first scan point (default) or the minimum of the shown
 * series. A labelled display choice — it does not change the physics, only the axis. */
export type RefChoice = "first" | "min";

type ScanPoint = ScanProfileJson["points"][number];

/** The chosen energy of a point, in Eh. */
export function energyEh(p: ScanPoint, which: EnergyChoice): number {
  return which === "act" ? p.energy_act_eh : p.energy_scf_eh;
}

/** The reference energy (the kcal/mol zero) in Eh, for the chosen series + choice. */
export function referenceEh(points: ScanPoint[], which: EnergyChoice, ref: RefChoice): number {
  if (points.length === 0) return 0;
  if (ref === "first") return energyEh(points[0], which);
  return Math.min(...points.map((p) => energyEh(p, which)));
}

/** One plotted datum: the scanned coordinate (Å for B / ° for A/D) and ΔE in
 * kcal/mol relative to the chosen reference. `index` is the original point index
 * (app-owned selection maps through it, as the trajectory chart does). */
export interface ProfileDatum {
  index: number;
  coordinate: number;
  relKcal: number;
}

/** The chart series: ΔE(coordinate) in kcal/mol. The reference point's `relKcal`
 * is exactly 0 by construction. */
export function profileSeries(
  points: ScanPoint[],
  which: EnergyChoice,
  ref: RefChoice,
): ProfileDatum[] {
  const e0 = referenceEh(points, which, ref);
  return points.map((p, index) => ({
    index,
    coordinate: p.coordinate,
    relKcal: (energyEh(p, which) - e0) * HARTREE_TO_KCAL,
  }));
}

/** The index of the maximum of the shown series — the **approximate-TS** point (a
 * scan-maximum ΔE‡ estimate; the UI labels it honestly). Ties take the first. */
export function maxIndex(points: ScanPoint[], which: EnergyChoice): number {
  let mi = 0;
  let mv = -Infinity;
  points.forEach((p, i) => {
    const v = energyEh(p, which);
    if (v > mv) {
      mv = v;
      mi = i;
    }
  });
  return mi;
}

/**
 * Prepare ONE scan point's geometry for the viewer, keyed by the app-owned index.
 * Element-order identity is checked at this boundary (`elementsAgree` vs the result
 * geometry) BEFORE any render — a mismatch is a loud refusal, never a silent wrong
 * molecule (the UI-boundary echo of the readers' element-order post-condition). The
 * xyz is built from the point's OWN element order (already confirmed to match).
 */
export function pointGeometryXyz(
  geometry: ScanGeometry,
  referenceElements: string[],
): { xyz: string } | { error: string } {
  if (!elementsAgree(geometry.elements, referenceElements)) {
    return {
      error:
        `Scan point atom order does not match the result geometry ` +
        `(${geometry.elements.length} vs ${referenceElements.length} atoms / different sequence). ` +
        `Not rendering — this would draw the wrong molecule.`,
    };
  }
  return {
    xyz: frameToXyz(geometry.elements, {
      energy_eh: null,
      xyz_angstrom: geometry.xyz_angstrom,
    }),
  };
}

/**
 * The SELECTED scan point's geometry as a standard `.xyz`, for export — the geometry the
 * panel is SHOWING (default the approximate-TS maximum), NOT `results.final_geometry` (which
 * is the LAST scan point; seeding OptTS from it is the bug this replaces — debugging/018).
 *
 * REUSES `finalGeometryXyz`: the xyz body + the atoms+2 post-condition (rule #9) come from the
 * one canonical builder — there is no second xyz formatter. The point's own coordinate/number
 * are composed into the comment; `point.energy_act_eh` (the composite total, the plotted energy)
 * is the exported energy. The `(approx TS / scan maximum)` tag is honest — present ONLY when this
 * point IS the shown-series maximum (`isMax`). This max-point extraction is the Stage-E seam:
 * an OptTS-refine child job (E1) reuses exactly this selected geometry as its seed.
 */
export function scanPointExportXyz(
  geometry: ScanGeometry,
  point: ScanPoint,
  pointIndex0: number,
  npoints: number,
  unit: string,
  isMax: boolean,
  jobTitle: string,
): string {
  const tag = isMax ? " (approx TS / scan maximum)" : "";
  const comment = `${jobTitle} — scan point ${pointIndex0 + 1}/${npoints} @ ${point.coordinate.toFixed(3)} ${unit}${tag}`;
  return finalGeometryXyz(geometry, comment, point.energy_act_eh);
}

/** A readout of the selected point: its coordinate (+ unit) and ΔE (kcal/mol). */
export function pointReadout(
  points: ScanPoint[],
  index: number,
  which: EnergyChoice,
  ref: RefChoice,
  unit: string,
): { coordinate: string; delta: string } | null {
  const p = points[index];
  if (!p) return null;
  const e0 = referenceEh(points, which, ref);
  const rel = (energyEh(p, which) - e0) * HARTREE_TO_KCAL;
  return {
    coordinate: `${p.coordinate.toFixed(3)} ${unit}`,
    delta: `${rel >= 0 ? "+" : ""}${rel.toFixed(2)} kcal/mol`,
  };
}
