/**
 * Zustand store that makes the Scene the single source of truth for geometry on
 * the New Job screen (ADR-008 #10). Every action is a **thin wrapper over the
 * pure functions in `scene.ts`** — no geometry logic lives here, so that logic
 * stays React-free and node-testable. The store only holds state and the
 * reset/undo bookkeeping the pure layer has no place for.
 *
 * Reference stability matters: `MoleculeViewer` re-renders on a new `scene`
 * reference (`useEffect([scene])`). Selectors must return the stored object
 * directly (`useSceneStore((s) => s.scene)`) so an unchanged scene keeps its
 * identity and the viewer does not redraw on every keystroke. Actions that would
 * be no-ops (e.g. a mutator on a null scene) return the current state unchanged,
 * preserving identity.
 */

import { create } from "zustand";

import { makeFragmentId, setMultiplicity, totalCharge } from "./scene";
import {
  addFragment as addFragmentPure,
  removeFragment as removeFragmentPure,
  replaceFragmentAtoms as replaceFragmentAtomsPure,
} from "./scene";
import type { Scene, SceneAtom, SceneFragment } from "./types";

export interface SceneStore {
  scene: Scene | null;
  /** Previous scene, kept for the reset Undo. */
  previous: Scene | null;
  /** Set when a manual coordinate edit collapsed a multi-fragment scene. */
  resetNotice: { fragmentCount: number } | null;

  setScene(scene: Scene | null): void;
  addFragment(f: SceneFragment): void; // used by d-2
  removeFragment(id: string): void;
  replaceFragmentAtoms(id: string, atoms: SceneAtom[]): void;
  setMultiplicity(m: number): void;
  /** Manual coordinate edit detected → collapse to one fragment, keep undo. */
  collapseToSingleFragment(atoms: SceneAtom[]): void;
  undoReset(): void;
  dismissResetNotice(): void;
}

export const useSceneStore = create<SceneStore>((set) => ({
  scene: null,
  previous: null,
  resetNotice: null,

  // A fresh scene is an explicit replacement — clear any pending reset notice.
  setScene: (scene) => set({ scene, resetNotice: null }),

  addFragment: (f) =>
    set((s) => ({
      scene: s.scene
        ? addFragmentPure(s.scene, f)
        : { fragments: [f], multiplicity: 1 },
      resetNotice: null,
    })),

  removeFragment: (id) =>
    set((s) => (s.scene ? { scene: removeFragmentPure(s.scene, id) } : s)),

  replaceFragmentAtoms: (id, atoms) =>
    set((s) =>
      s.scene ? { scene: replaceFragmentAtomsPure(s.scene, id, atoms) } : s,
    ),

  setMultiplicity: (m) =>
    set((s) => (s.scene ? { scene: setMultiplicity(s.scene, m) } : s)),

  // A manual coordinate edit diverged from the scene → the scene follows the
  // text: one fragment holding the parsed atoms, preserving the system's total
  // charge and multiplicity. The notice only fires when >1 fragment was lost
  // (a single-fragment collapse is geometrically a no-op — see NewJobScreen).
  collapseToSingleFragment: (atoms) =>
    set((s) => {
      const prev = s.scene;
      const fragmentCount = prev ? prev.fragments.length : 0;
      const collapsed: Scene = {
        fragments: [
          {
            id: makeFragmentId(),
            name: prev?.fragments[0]?.name ?? "Molecule",
            atoms,
            charge: prev ? totalCharge(prev) : 0,
            source: "editor",
          },
        ],
        multiplicity: prev?.multiplicity ?? 1,
      };
      return {
        scene: collapsed,
        previous: prev,
        resetNotice: fragmentCount > 1 ? { fragmentCount } : null,
      };
    }),

  undoReset: () =>
    set((s) =>
      s.previous
        ? { scene: s.previous, previous: null, resetNotice: null }
        : s,
    ),

  dismissResetNotice: () => set({ resetNotice: null }),
}));
