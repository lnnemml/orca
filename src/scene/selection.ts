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
import { atomCount, locateAtom } from "./scene";

/**
 * The most atoms a measurement needs — a dihedral's four. The cap is a hard
 * limit, not FIFO: see `toggleAtom`.
 */
export const MAX_SELECTION = 4;

/**
 * Apply one click to the pick list, returning a NEW list (never mutates):
 *  - click on an already-selected atom → remove it (a toggle-off);
 *  - click on a new atom with fewer than {@link MAX_SELECTION} picked → append
 *    it (order preserved — the measurement reads positionally);
 *  - click on a new atom when the list is FULL → the selection becomes just
 *    `[index]`.
 *
 * The full-list rule is deliberately **not FIFO**. Silently dropping the oldest
 * atom and appending the new one would leave the user measuring a set that
 * differs from the atoms they see highlighted — a distance/angle read off the
 * wrong atoms with no visible cause. A hard reset to the single just-clicked
 * atom is unambiguous: the highlight collapses to exactly what was clicked.
 */
export function toggleAtom(selection: number[], index: number): number[] {
  if (selection.includes(index)) return selection.filter((i) => i !== index);
  if (selection.length < MAX_SELECTION) return [...selection, index];
  return [index];
}

/**
 * Drop any picked index that no longer addresses an atom in `scene` (a fragment
 * was removed, the scene was cleared). Returns the **same array reference** when
 * nothing is dropped, so a no-op validation doesn't churn React state / re-run
 * effects. Order of the survivors is preserved.
 */
export function validateSelection(
  selection: number[],
  scene: Scene | null,
): number[] {
  const count = scene ? atomCount(scene) : 0;
  const kept = selection.filter((i) => i >= 0 && i < count);
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
