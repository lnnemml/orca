//! IR spectrum **presentation** choices — the plot-construction layer that sits on
//! top of the measured physics in `ir.ts`. Everything here is a CHOICE about how to
//! *draw* the spectrum, never a property of the molecule (the same discipline as the
//! FWHM slider). Kept pure, React-free, and node-tested, so each choice is a named,
//! checkable transform rather than something buried in JSX.
//!
//! Two things live here:
//!
//!  * **frequency display scaling.** Harmonic frequencies are systematically high
//!    (measured on dexketoprofen: C–H at 3025–3193 where a chemist expects
//!    2900–3000). A researcher may want to multiply them by a *citable* scale factor
//!    to compare with experiment. That factor is the USER's choice — default
//!    **1.00** — NOT a number we bake in per method (we have neither measured nor
//!    cited one), and NOT the `.hess $frequency_scale_factor` (measured **1.0**,
//!    meaning "ORCA applied none"). It is applied by transforming the mode list fed
//!    to `ir.ts`, so the physics module stays untouched and curve + sticks always
//!    agree.
//!
//!  * **the single-source tooltip model.** One wavenumber under the cursor yields the
//!    curve value THERE and the NEAREST mode (labelled as nearest, never as "the
//!    value at this point"). Built as a pure function so a test can lock that the
//!    label and every value in the tooltip come from the same x — the fix for the
//!    unit-3.10 tooltip that mixed a label from one series with a value from another.
//!
//!  * **a scale-independent x-grid** (`fixedGrid`). The whole point of the scale
//!    slider is to see peaks slide against a FIXED ruler and compare with
//!    experiment. So the grid bounds must come from the RAW frequencies and the
//!    slider's full range — never the current scale (unit 3.11: deriving them from
//!    the already-scaled modes multiplied both the data and the ruler by the same
//!    number, a self-similar picture where the peaks never moved in pixels).

import { autoGrid, type Grid, type IrMode } from "./ir";

/** Display scale factor bounds. Default is the identity: the raw, measured
 * frequencies. The range brackets the usual harmonic→fundamental factors
 * (~0.95–0.99) without implying any specific one — the choice of a citable value is
 * the chemist's, not ours. */
export const DEFAULT_SCALE = 1.0;
export const MIN_SCALE = 0.9;
export const MAX_SCALE = 1.1;
export const SCALE_STEP = 0.005;

/**
 * Apply a display scale factor to the mode wavenumbers: a stick at `cm` moves to
 * `factor·cm`; intensity and original index are unchanged. Identity (returns the
 * input array) at `factor === 1`, so the common case allocates nothing and the
 * "no scaling" path is provably a no-op. The broadened curve is recomputed from
 * these scaled modes (`spectrum(scaledModes(...), ...)`), so curve and sticks share
 * one x-axis. This is a DRAWING choice — see the module note.
 */
export function scaledModes(active: IrMode[], factor: number): IrMode[] {
  if (factor === 1) return active;
  return active.map((m) => ({ ...m, cm: m.cm * factor }));
}

/**
 * The x-grid the chart draws against — deliberately **independent of the current
 * scale**, so moving the slider moves the peaks but never the ruler.
 *
 * The bounds must cover every position a peak can reach across the WHOLE slider
 * range `[minScale, maxScale]`: the leftmost is `(lowest raw mode)·minScale`, the
 * rightmost `(highest raw mode)·maxScale` (all frequencies are positive, so the
 * extreme scale factors give the extreme positions). We hand those two extremes to
 * `autoGrid`, which then applies the exact same padding, 0-clamp, and (scale-free,
 * FWHM-only) step it uses everywhere — so `ir.ts` is untouched and the step stays
 * constant while the slider moves.
 *
 * Consequence: at scale 1.0 the frame is a little wider than the data (there is room
 * on the right for peaks to slide into as you scale up) — that headroom is the ruler
 * the parameter needs, not wasted space. The broadened curve is still sampled from
 * the *scaled* modes over this fixed grid (`spectrum(scaledModes(...), fixedGrid(...))`),
 * so the peaks land at their scaled wavenumbers inside a stationary axis.
 */
export function fixedGrid(
  active: IrMode[],
  fwhm: number,
  minScale: number = MIN_SCALE,
  maxScale: number = MAX_SCALE,
): Grid {
  if (active.length === 0) return autoGrid([], fwhm);
  let lo = Infinity;
  let hi = -Infinity;
  for (const m of active) {
    if (m.cm < lo) lo = m.cm;
    if (m.cm > hi) hi = m.cm;
  }
  // Two synthetic modes at the extreme reachable positions; intensity is irrelevant
  // to the grid (autoGrid only reads `cm`). This reuses autoGrid's pad/step/clamp
  // verbatim rather than re-deriving them here.
  const extremes: IrMode[] = [
    { cm: lo * minScale, kmMol: 0, index: -1 },
    { cm: hi * maxScale, kmMol: 0, index: -1 },
  ];
  return autoGrid(extremes, fwhm);
}

/** The mode whose wavenumber is closest to `cm`, or `null` on an empty list. Ties
 * resolve to the first (deterministic). */
export function nearestMode(modes: IrMode[], cm: number): IrMode | null {
  let best: IrMode | null = null;
  let bestDelta = Infinity;
  for (const m of modes) {
    const delta = Math.abs(m.cm - cm);
    if (delta < bestDelta) {
      bestDelta = delta;
      best = m;
    }
  }
  return best;
}

export interface IrTooltipModel {
  /** The wavenumber under the cursor, cm⁻¹ — the ONE x everything is derived from. */
  cm: number;
  /** The broadened-curve value at that wavenumber (km/mol·cm⁻¹). */
  curve: number;
  /** The nearest MODE to the cursor — explicitly the nearest, with the distance so
   * the UI can say so; NOT "the value at this point". `null` when there are no
   * active modes. */
  nearest: { cm: number; kmMol: number; deltaCm: number; index: number } | null;
}

/**
 * Assemble the tooltip state from a SINGLE wavenumber `cm` and the curve value at
 * that same wavenumber. The nearest-mode fields are derived from the same `cm`, so
 * the label (`cm`) and every value in the tooltip provably come from one x.
 *
 * This is the fix for the unit-3.10 bug: two recharts series (the curve Line and a
 * markers Scatter) were merged in one tooltip, taking the label from one series and
 * a value from another (header `115 cm⁻¹` next to a `9.350` that was the O–H peak
 * height at 3714 cm⁻¹). Here `curve` MUST be the value the caller read at `cm`, and
 * `nearest` is whatever mode is actually closest to `cm` — never a value from
 * elsewhere in the spectrum.
 */
export function irTooltipModel(cm: number, curve: number, modes: IrMode[]): IrTooltipModel {
  const n = nearestMode(modes, cm);
  return {
    cm,
    curve,
    nearest: n
      ? { cm: n.cm, kmMol: n.kmMol, deltaCm: Math.abs(n.cm - cm), index: n.index }
      : null,
  };
}
