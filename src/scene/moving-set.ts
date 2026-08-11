/**
 * The MOVING SET — the one rule that decides which atoms a drag or a single-side
 * edit moves (unified moving-set unit). Pure / node-tested: the perception inputs
 * (`fragmentAtoms`, `component`) are INJECTED, exactly like `resolveMovingSet`'s
 * siblings elsewhere, because bond perception has ONE home — the sidecar
 * (`/geometry/connected-component`; ADR-010 correction ii). This module never
 * perceives; it only chooses.
 *
 * ## THE ONE RULE
 * For any drag or single-side edit, the moving set is:
 *   1. an explicit atom **SELECTION** is present  → move EXACTLY the selection
 *      (the feature: the researcher chose the atoms);
 *   2. else the **toggle** decides:
 *        - `"fragment"`  → the whole fragment of the grabbed atom (rough placement,
 *          the historical default);
 *        - `"selection"` → the grabbed atom's PERCEIVED CONNECTED COMPONENT — so a
 *          broken-off / disconnected piece moves alone (the drag-fix behaviour).
 *
 * A selection ALWAYS wins over the toggle: the toggle is only consulted when the
 * pick list is empty. `"fragment"` and `"selection"` differ **only** when the
 * fragment has disconnected pieces (a fully-bonded fragment's component IS its whole
 * atom set, so both give the same answer — the backward-compatible whole-fragment
 * move).
 */

import type { AtomId } from "./ids";

/** The edit-rail "Move: Fragment | Selection" toggle (owned by the editor UI like
 * `hiddenBonds`; see `visualization.md`). */
export type MoveMode = "fragment" | "selection";

export interface MovingSetInput {
  /** The atom the gesture grabbed — the anchor `fragmentAtoms`/`component` were
   * resolved for. Part of the contract (the caller has it naturally); the rule
   * itself is decided by `selection`/`toggle`. */
  grabbed: AtomId;
  /** The editor's explicit pick list (empty when nothing is selected). */
  selection: readonly AtomId[];
  /** The rail toggle, consulted only when `selection` is empty. */
  toggle: MoveMode;
}

/**
 * Resolve the MOVING SET (THE ONE RULE above). `fragmentAtoms` = the grabbed atom's
 * whole fragment; `component` = the grabbed atom's perceived connected component
 * (the sidecar result). Returns a fresh array so callers may keep it.
 */
export function resolveMovingSet(
  { selection, toggle }: MovingSetInput,
  fragmentAtoms: readonly AtomId[],
  component: readonly AtomId[],
): AtomId[] {
  if (selection.length > 0) return [...selection]; // (1) selection wins over the toggle
  return toggle === "fragment" ? [...fragmentAtoms] : [...component]; // (2) toggle decides
}
