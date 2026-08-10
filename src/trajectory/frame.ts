//! Trajectory playback helpers — pure, node-tested, React-free.
//!
//! The trajectory is already parsed and stored (unit 3.7,
//! `data_json.trajectory`): `elements` (the order, stored once) + `frames`, each
//! a per-atom Å coordinate list and an optional comment energy (Eh, measured).
//! **Frames are optimization CYCLES, not scan points** (measured: a 6-point scan
//! has 26 frames — `wiki/orca/parse-sources.md`). Nothing here re-parses a file;
//! it shapes the stored arrays for the player.
//!
//! Frame ownership: the *number* of the current frame is application state (held
//! in `TrajectoryPlayer`), never the viewer's — the viewer is a dumb renderer
//! fed one frame's geometry (ADR-011). These functions build that one frame's
//! xyz and the honest labels around it.

import { HARTREE_TO_KCAL } from "../units";

/** A stored trajectory frame (mirrors `results.rs::TrajFrame`). */
export interface Frame {
  energy_eh: number | null;
  xyz_angstrom: [number, number, number][];
}

/**
 * Build the standard xyz string for ONE frame (the geometry handed to the
 * viewer). `count\ncomment\nrows`, coordinates fixed to 6 dp — enough for a
 * display, deterministic for tests. Throws on an element/coordinate length
 * mismatch: a frame whose atom count disagrees with the element order is a
 * programming error, not something to render silently (the UI-boundary echo of
 * the readers' count invariant).
 */
export function frameToXyz(elements: string[], frame: Frame): string {
  if (frame.xyz_angstrom.length !== elements.length) {
    throw new Error(
      `frame has ${frame.xyz_angstrom.length} atoms but the element order has ${elements.length}`,
    );
  }
  const rows = elements.map((el, i) => {
    const [x, y, z] = frame.xyz_angstrom[i];
    return `${el} ${x.toFixed(6)} ${y.toFixed(6)} ${z.toFixed(6)}`;
  });
  return `${elements.length}\nframe\n${rows.join("\n")}`;
}

/**
 * The frame's HONEST label. These are optimization cycles — never call them
 * "scan steps" (measured: they are per-cycle, not per scan point). 1-based for
 * display.
 */
export function frameLabel(frameIndex: number, total: number): string {
  return `cycle ${frameIndex + 1} / ${total}`;
}

/** The frame's energy for display, Eh to 6 dp, or `—` when the comment carried
 * none (`energy_eh` is `Option<f64>` — a frame may legitimately lack it). */
export function frameEnergyText(frame: Frame): string {
  return frame.energy_eh == null ? "—" : `${frame.energy_eh.toFixed(6)} Eh`;
}

/**
 * ΔE of a frame relative to the FIRST frame, in kcal/mol — the number that shows
 * the descent (the learning value: "how far it fell"). `null` when either energy
 * is absent. The Hartree→kcal/mol factor is the named constant in `src/units.ts`.
 */
// Re-exported here so existing `../trajectory/frame` importers keep working; the
// single definition lives in `src/units.ts`.
export { HARTREE_TO_KCAL };
export function frameDeltaKcal(frames: Frame[], frameIndex: number): number | null {
  const e = frames[frameIndex]?.energy_eh;
  const e0 = frames[0]?.energy_eh;
  if (e == null || e0 == null) return null;
  return (e - e0) * HARTREE_TO_KCAL;
}

/** Data for the energy-per-cycle chart: only frames that carry an energy, as
 * `{ cycle (1-based), energy, index }`. Empty when no frame has an energy. */
export interface EnergyDatum {
  cycle: number;
  energy: number;
  index: number;
}
export function energySeries(frames: Frame[]): EnergyDatum[] {
  const out: EnergyDatum[] = [];
  frames.forEach((f, i) => {
    if (f.energy_eh != null) out.push({ cycle: i + 1, energy: f.energy_eh, index: i });
  });
  return out;
}

/**
 * Do a trajectory's element order and a reference order (the order the current
 * static scene/card is drawn in) agree — same count AND same sequence? A
 * mismatch is an error to surface, never a silent substitution (the UI-boundary
 * echo of the readers' element-order post-condition). Case-insensitive, since
 * element symbols may differ only in casing across sources.
 */
export function elementsAgree(a: string[], b: string[]): boolean {
  if (a.length !== b.length) return false;
  for (let i = 0; i < a.length; i++) {
    if (a[i].toLowerCase() !== b[i].toLowerCase()) return false;
  }
  return true;
}
