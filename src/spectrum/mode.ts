//! Normal-mode animation geometry — pure, node-tested, React-free (the same
//! discipline as `trajectory/frame.ts`, whose ownership rule this mirrors).
//!
//! A normal mode is a set of per-atom Cartesian displacement vectors; animating it
//! is `x(t) = x_eq + A · sin(2πt) · v`, one period back and forth, looped. All the
//! math lives here as named, checkable functions; the component owns only the phase,
//! amplitude, and timer (ADR-011 — the viewer gets ONE frame, never a timer).
//!
//! # Two facts this module rests on, both MEASURED (`wiki/orca/parse-sources.md`)
//!  * **The mode vectors are taken AS-IS — no rotation.** The gate (unit 3.12,
//!    `probes/hess_frame_kabsch.py`) proved the `.hess $atoms` frame is the
//!    reference (`.property.txt` final) geometry plus a **pure translation** on all
//!    three jobs (max|R−I| ≤ 3e-13, incl. the asymmetric 33-atom dexketoprofen).
//!    Translation does not rotate a displacement vector, so a mode column added to
//!    the reference geometry animates in the correct direction.
//!  * **The modes are Cartesian, unit-normalized** (unit-3.6 gate — no ÷√m). So the
//!    displacement is used directly; the only free number is the amplitude.
//!
//! # Amplitude is a DISPLAY choice, with no absolute meaning
//! A normalized mode has no intrinsic amplitude — only a direction. The default
//! `DEFAULT_AMPLITUDE = 2.0` is the multiplier `orca_pltvib` itself applies
//! (measured, unit-3.6 gate), stated so the number is not magic. It is a slider,
//! like the FWHM and the display-scale factor.

import { frameToXyz } from "../trajectory/frame";

/** The `orca_pltvib` multiplier (measured, unit-3.6). A display default, not a
 * molecular property — the mode is normalized and has no absolute amplitude. */
export const DEFAULT_AMPLITUDE = 2.0;
export const MIN_AMPLITUDE = 0.25;
export const MAX_AMPLITUDE = 3.0;
export const AMPLITUDE_STEP = 0.25;

/** Frames sampled over one full period. Phase is `p / PHASE_FRAMES`, p = 0…N−1, so
 * phase 0 is exactly the equilibrium (sin 0 = 0). A UI/app timer advances p. */
export const PHASE_FRAMES = 40;

/**
 * Collapse-guard floor, Å. Below this two atoms read as merged mush, not a molecule.
 * Measured basis (`probes/hess_frame_kabsch.py`): equilibrium min interatomic
 * distances are ≈1.0 Å; at A=2.0 ordinary modes keep a median ≈0.95 Å, but the
 * sharpest localized C–H stretches drive atoms to 0.02–0.07 Å at the sin=±1 extreme
 * (2.0 overshoots them). 0.5 Å cleanly separates a genuine collapse from ordinary
 * bond compression — it is a *guard that warns*, not a hard block (rule #9).
 */
export const MIN_SAFE_DISTANCE_ANGSTROM = 0.5;

type Vec3 = [number, number, number];

/**
 * Mode `modeIndex`'s per-atom displacement, extracted as the **column** of the
 * row-major 3N×3N `$normal_modes` matrix (column k = mode k — the measured
 * convention, `parse-sources.md`; a row would be atom-component k, a different
 * thing entirely). Row `3a+c` is atom `a`, Cartesian component `c`, so
 * `disp[a][c] = normalModes[(3a+c)·n + k]`.
 *
 * Throws (never returns a wrong-length vector) on a matrix that is not n², an
 * `nModes` not divisible by 3, or an out-of-range mode index — the UI-boundary echo
 * of the reader's shape invariants (rule #9).
 */
export function modeDisplacements(
  normalModes: number[],
  nModes: number,
  modeIndex: number,
): Vec3[] {
  if (normalModes.length !== nModes * nModes) {
    throw new Error(
      `normal_modes has ${normalModes.length} entries, expected ${nModes}² = ${nModes * nModes}`,
    );
  }
  if (nModes % 3 !== 0) {
    throw new Error(`nModes ${nModes} is not a multiple of 3 (not 3N)`);
  }
  if (modeIndex < 0 || modeIndex >= nModes) {
    throw new Error(`mode index ${modeIndex} out of range [0, ${nModes})`);
  }
  const nAtoms = nModes / 3;
  const out: Vec3[] = [];
  for (let a = 0; a < nAtoms; a++) {
    out.push([
      normalModes[(3 * a + 0) * nModes + modeIndex],
      normalModes[(3 * a + 1) * nModes + modeIndex],
      normalModes[(3 * a + 2) * nModes + modeIndex],
    ]);
  }
  return out;
}

/**
 * The animated geometry at `phase` ∈ [0,1): `x_eq + A·sin(2π·phase)·v`. At phase 0
 * (and 0.5) this is exactly the equilibrium (sin = 0). Throws on an atom-count
 * mismatch between the equilibrium and the displacement — never a silent draw.
 */
export function modeFrameCoords(
  equilibrium: Vec3[],
  disp: Vec3[],
  amplitude: number,
  phase: number,
): Vec3[] {
  if (equilibrium.length !== disp.length) {
    throw new Error(
      `equilibrium has ${equilibrium.length} atoms but the mode has ${disp.length}`,
    );
  }
  const s = amplitude * Math.sin(2 * Math.PI * phase);
  return equilibrium.map((p, a) => [
    p[0] + s * disp[a][0],
    p[1] + s * disp[a][1],
    p[2] + s * disp[a][2],
  ]);
}

/** One animated frame as a standard xyz string for the viewer. Reuses the
 * trajectory formatter (same count-mismatch throw), so the animation and the
 * trajectory feed the dumb renderer through one code path. */
export function modeFrameXyz(
  elements: string[],
  equilibrium: Vec3[],
  disp: Vec3[],
  amplitude: number,
  phase: number,
): string {
  const coords = modeFrameCoords(equilibrium, disp, amplitude, phase);
  return frameToXyz(elements, { energy_eh: null, xyz_angstrom: coords });
}

/** Smallest interatomic distance in a geometry (Å) — plain O(N²), N is small. */
export function minInteratomicDistance(coords: Vec3[]): number {
  let best = Infinity;
  for (let i = 0; i < coords.length; i++) {
    for (let j = i + 1; j < coords.length; j++) {
      const dx = coords[i][0] - coords[j][0];
      const dy = coords[i][1] - coords[j][1];
      const dz = coords[i][2] - coords[j][2];
      const d = Math.sqrt(dx * dx + dy * dy + dz * dz);
      if (d < best) best = d;
    }
  }
  return best;
}

/**
 * The closest two atoms get anywhere in the mode's period at this amplitude — the
 * number the collapse guard tests against `MIN_SAFE_DISTANCE_ANGSTROM`. The per-pair
 * distance is convex in `sin`, so the minimum can be interior; we sample the period
 * (not just the sin=±1 extremes) and take the global min. Deterministic.
 */
export function modeMinDistanceOverPeriod(
  equilibrium: Vec3[],
  disp: Vec3[],
  amplitude: number,
  samples: number = PHASE_FRAMES,
): number {
  let best = Infinity;
  for (let p = 0; p < samples; p++) {
    const d = minInteratomicDistance(modeFrameCoords(equilibrium, disp, amplitude, p / samples));
    if (d < best) best = d;
  }
  return best;
}
