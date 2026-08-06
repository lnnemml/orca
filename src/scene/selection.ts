/**
 * Atom selection for the geometry editor (2.5.2). Pure / React-free (ADR-008
 * decision 10): node-testable, no imports from react / 3dmol / tauri. The UI
 * (NewJobScreen) holds the selection in component state and drives every change
 * through these functions; no selection logic lives in the React layer, so it
 * stays testable as invariants, not rendered output.
 *
 * A selection is an **ordered pick list of global atom indices** (the same
 * 0-based merged-xyz / ASE-mask index space every other scene function uses —
 * see `locateAtom`). Order matters: 2.5.2b reads it positionally as (a,b) for a
 * distance, (a,vertex,b) for an angle, or a 4-atom chain for a dihedral.
 */

import type { Scene } from "./types";
import type { AtomId } from "./ids";
import { globalIndexOfAtom, locateAtom } from "./scene";

/**
 * The most atoms a measurement needs — a dihedral's four. The cap is a hard
 * limit, not FIFO: see `toggleAtom`.
 */
export const MAX_SELECTION = 4;

/**
 * Apply one click to the pick list, returning a NEW list (never mutates). The
 * list holds stable {@link AtomId}s (unit 2c2): an id survives a fragment
 * removal, so the pick means "this physical atom", not "whatever is at index N".
 *  - click on an already-selected atom → remove it (a toggle-off);
 *  - click on a new atom with fewer than {@link MAX_SELECTION} picked → append
 *    it (order preserved — the measurement reads positionally);
 *  - click on a new atom when the list is FULL → the selection becomes just
 *    `[id]`.
 *
 * The full-list rule is deliberately **not FIFO**. Silently dropping the oldest
 * atom and appending the new one would leave the user measuring a set that
 * differs from the atoms they see highlighted — a distance/angle read off the
 * wrong atoms with no visible cause. A hard reset to the single just-clicked
 * atom is unambiguous: the highlight collapses to exactly what was clicked.
 */
export function toggleAtom(selection: AtomId[], id: AtomId): AtomId[] {
  if (selection.includes(id)) return selection.filter((x) => x !== id);
  if (selection.length < MAX_SELECTION) return [...selection, id];
  return [id];
}

/**
 * Drop any picked {@link AtomId} that is no longer in `scene` (its fragment was
 * removed, the scene was cleared). Returns the **same array reference** when
 * nothing is dropped, so a no-op doesn't churn React state / re-run effects.
 * Click order of the survivors is preserved.
 *
 * **This is the 2c2 dividend** (ADR-010 gave `AtomId` an operational "the same
 * atom" that positional indices never had): an atom survives ⟺ its id is still
 * in the scene. Removing a fragment that does *not* contain any picked atom
 * therefore leaves the selection **untouched** — the old positional guards
 * (removed in 2c2) had to clear the whole selection on any composition change
 * because a kept index would silently re-point at a different atom. There is no
 * such ambiguity with an id, so there is no clear.
 */
export function filterSelection(selection: AtomId[], scene: Scene | null): AtomId[] {
  const kept = scene
    ? selection.filter((id) => globalIndexOfAtom(scene, id) !== null)
    : [];
  return kept.length === selection.length ? selection : kept;
}

/** A resolved atom: element + coordinates + which fragment it belongs to. */
export interface AtomDescription {
  element: string;
  x: number;
  y: number;
  z: number;
  fragmentId: string;
  fragmentName: string;
  /** 0-based position of the fragment in the scene — the palette key. */
  fragmentIndex: number;
  /** 0-based index of the atom within its fragment. */
  localIndex: number;
}

/**
 * Describe the atom at a global index — a thin wrapper over `locateAtom` (no
 * own fragment walk). `fragmentIndex` is the fragment's position in the scene,
 * needed to colour the atom by the shared `fragmentColor` palette. `null` for
 * an out-of-range index (same non-throwing contract as `locateAtom`).
 */
export function describeAtom(
  scene: Scene,
  globalIndex: number,
): AtomDescription | null {
  const located = locateAtom(scene, globalIndex);
  if (!located) return null;
  const { fragment, localIndex } = located;
  const atom = fragment.atoms[localIndex];
  return {
    element: atom.element,
    x: atom.x,
    y: atom.y,
    z: atom.z,
    fragmentId: fragment.id,
    fragmentName: fragment.name,
    fragmentIndex: scene.fragments.findIndex((f) => f.id === fragment.id),
    localIndex,
  };
}

/**
 * Describe the atom carrying a stable {@link AtomId} — resolves the id to its
 * current global index ({@link globalIndexOfAtom}) then reuses {@link describeAtom}.
 * `null` for an id no longer in the scene. This is the id-native entry the 2c2 UI
 * uses (the selection is `AtomId[]`); `describeAtom` stays for the positional
 * consumers (the constraint panel, which speaks ORCA indices).
 */
export function describeAtomById(
  scene: Scene,
  id: AtomId,
): AtomDescription | null {
  const gi = globalIndexOfAtom(scene, id);
  return gi === null ? null : describeAtom(scene, gi);
}
