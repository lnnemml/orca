//! Pure width math + persistence for the resizable manual drawer (unit 4.14). Kept apart
//! from `ManualDrawer` so it is testable without jsdom (pass a fake `Storage`), and so the
//! clamp bounds live in one place.
//!
//! WHERE the width is stored — a NAMED decision, not a silent one. The app's data lives in
//! SQLite (ADR-004, Rust owns the DB), and one UI preference already follows that path
//! (`viewer_theme` via `set_setting`). The drawer width is a **deliberate exception**: it
//! is a pure UI preference (not data), and a **synchronous** `localStorage` read on first
//! render gives the correct width immediately — a SQLite read is async and would flash the
//! default width before the stored one arrives. So it uses `localStorage`, documented as
//! the exception in `wiki/modules/frontend.md`. No new dependency.

export const DRAWER_MIN = 320;
/** The max is a fraction of the viewport, so the editor beside it always keeps room. */
export const drawerMax = (viewportWidth: number) => Math.round(viewportWidth * 0.85);

/** Clamp a width to `[DRAWER_MIN, 85 % viewport]`. */
export function clampDrawerWidth(width: number, viewportWidth: number): number {
  return Math.max(DRAWER_MIN, Math.min(Math.round(width), drawerMax(viewportWidth)));
}

const KEY = "manualDrawerWidth";

/** The stored width, clamped to the CURRENT viewport (a saved width from a wider screen
 *  must not exceed today's max), or the legacy default `min(480, 42 % viewport)`. */
export function readDrawerWidth(
  store: Pick<Storage, "getItem">,
  viewportWidth: number,
): number {
  const parsed = parseInt(store.getItem(KEY) ?? "", 10);
  const fallback = Math.min(480, Math.round(viewportWidth * 0.42));
  return clampDrawerWidth(Number.isFinite(parsed) ? parsed : fallback, viewportWidth);
}

export function storeDrawerWidth(store: Pick<Storage, "setItem">, width: number): void {
  store.setItem(KEY, String(Math.round(width)));
}
