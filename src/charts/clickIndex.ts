//! Pure resolver for a recharts chart-`onClick` → the clicked data position.
//!
//! Why this exists (debugging/016): recharts **v3** changed the chart-click payload —
//! `activeTooltipIndex` now comes through as a **string** (`TooltipIndex = string | null`,
//! measured in `node_modules/recharts/types/state/tooltipSlice.d.ts`), where v2 gave a
//! number. The old inline handlers guarded `typeof i === "number"` and so **silently
//! dropped every click** in both the scan profile and the trajectory chart. The fix is a
//! single pure, tested resolver both charts route through, so the next recharts payload
//! change is caught by a unit test, not a manual gate. The shape is not guaranteed across
//! recharts versions, so every field is guarded and this never throws.

/** The subset of a recharts chart-`onClick` / active-dot state we read. Both fields are
 *  loosely typed and optional — v3 delivers `activeTooltipIndex` as a string, v2 as a
 *  number; `activeLabel` is the clicked x-axis value (the fallback). */
export interface ChartClickState {
  activeTooltipIndex?: number | string | null;
  activeLabel?: number | string | null;
}

/** x-value match tolerance for the `activeLabel` fallback (coordinates are exact doubles
 *  from the same series, so this only absorbs float round-trip noise). */
const LABEL_EPS = 1e-6;

/**
 * Resolve a recharts chart-click to the **array position** in `series`, or `null` when
 * nothing resolves. Handles the recharts v3 space:
 *  1. `activeTooltipIndex` as a number (v2) OR string (v3) → `Number(i)`, accepted only
 *     as an integer in `[0, series.length)`;
 *  2. else, if `getX` is given, fall back to `activeLabel` (the clicked x value) → the
 *     `series` element whose x matches within `LABEL_EPS`.
 *
 * The caller maps the returned position to its own index convention
 * (`series[pos].index` — the scan point / original frame index). Tolerant of an
 * unknown-shaped `state`; never throws.
 */
export function resolveClickedIndex<T>(
  state: ChartClickState | null | undefined,
  series: readonly T[],
  getX?: (datum: T) => number,
): number | null {
  if (!state || series.length === 0) return null;

  // (1) activeTooltipIndex — number (v2) or the v3 string. A non-numeric string (e.g. a
  // composite tooltip id) falls through to the label fallback rather than throwing.
  const raw = state.activeTooltipIndex;
  if (raw != null && raw !== "") {
    const n = Number(raw);
    if (Number.isInteger(n) && n >= 0 && n < series.length) return n;
  }

  // (2) activeLabel fallback — match the clicked x value against each element's x.
  if (getX && state.activeLabel != null && state.activeLabel !== "") {
    const label = Number(state.activeLabel);
    if (!Number.isNaN(label)) {
      for (let k = 0; k < series.length; k++) {
        if (Math.abs(getX(series[k]) - label) <= LABEL_EPS) return k;
      }
    }
  }
  return null;
}
