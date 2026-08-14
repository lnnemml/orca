/**
 * The index of the value in `values` closest to `target` (argmin `|v − target|`). Pure, no I/O.
 * Ties resolve to the FIRST (lowest) index; an empty array returns `-1`.
 *
 * Used to SNAP a 2D-contour click to the nearest grid node from the click's data-space
 * coordinate — so the whole surface is clickable, not just a pixel-exact marker hit (the m2 bug).
 * It selects by VALUE (not by index arithmetic), so it is correct whether an axis ascends or
 * descends and naturally clamps: a target below the smallest value → the index of the smallest
 * value; above the largest → the largest's index (never out of range).
 */
export function nearestIndex(values: number[], target: number): number {
  let best = -1;
  let bestDist = Infinity;
  for (let i = 0; i < values.length; i++) {
    const d = Math.abs(values[i] - target);
    if (d < bestDist) {
      bestDist = d;
      best = i;
    }
  }
  return best;
}
