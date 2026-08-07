/**
 * The rotation-axis overlay choice (Phase 4.2 Stage 3, unit 3.3b). Pure — no 3Dmol,
 * no React — so the "exactly one overlay per pick" invariant is unit-tested apart
 * from the viewer (which has no jsdom).
 *
 * The problem (unit 3.3): while the Rotate panel holds an axis `[P, Q]`, the viewer
 * drew BOTH the extended axis cylinder AND the measurement distance line on the same
 * two atoms — two overlapping greenish objects of different length that read as "the
 * line is wrong". A2: draw **exactly one** overlay for that pair, chosen by a toggle;
 * the Å number stays either way (single source: `measure` distance), so length always
 * reads.
 */

export type RotateOverlay = "axis" | "distance";

/** Default to the **distance** overlay: on picking two atoms the researcher first
 * wants to read the separation (the reaction coordinate), and the green measurement
 * line reads unambiguously; the axis cylinder is one toggle away when actually rotating. */
export const DEFAULT_ROTATE_OVERLAY: RotateOverlay = "distance";

/** Flip the toggle. */
export function flipRotateOverlay(o: RotateOverlay): RotateOverlay {
  return o === "axis" ? "distance" : "axis";
}

/**
 * Which single overlay the viewer draws for the rotation-axis pair.
 * - **No axis** (Rotate not active for this pair): the measurement draws EXACTLY as
 *   before — `{ axis: false, measure: true }` regardless of `overlay`, so the measure
 *   tool outside Rotate is untouched.
 * - **Axis + "axis"**: the axis cylinder (with the Å label on the axis midpoint) and
 *   NO measurement line — `{ axis: true, measure: false }`.
 * - **Axis + "distance"**: the measurement line + label and NO cylinder —
 *   `{ axis: false, measure: true }`.
 *
 * The post-condition callers rely on: `axis && measure` is **never** both true — at
 * most one overlay is drawn for the pair (the whole point of the unit).
 */
export function chooseRotateOverlay(
  hasAxis: boolean,
  overlay: RotateOverlay,
): { axis: boolean; measure: boolean } {
  if (!hasAxis) return { axis: false, measure: true };
  return overlay === "axis"
    ? { axis: true, measure: false }
    : { axis: false, measure: true };
}
