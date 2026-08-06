/**
 * Zustand scene store — a **fold over the operation log** (Phase 4.2 Stage 2 unit
 * 2b; ADR-017). The Scene is the single source of truth for geometry on the New
 * Job screen (ADR-008 #10), and now it is **derived**: `scene === current(log)`,
 * always. That equality is the store's core invariant and the thing that makes
 * the "mutator bypasses the log" defect *impossible by construction* rather than
 * merely forbidden.
 *
 * ── Why there is NO `setScene` ───────────────────────────────────────────────
 * The predecessor store held `scene` as an independent field that actions wrote
 * directly. The danger (ADR-017 / the 2b main risk): any path — an xtb apply, a
 * conformer swap, a Monaco collapse, future code — could set the scene *without*
 * a log entry, and the scene and the log would silently diverge (undo then rolls
 * back the wrong thing, or loses an edit). So `scene` is never independently
 * settable. Every write to `scene` in this file is `scene: current(log)` right
 * after the log changed, and the only two ways the log changes are:
 *   • `commit(op, resultScene)` — append one entry (the single door for a
 *     geometry op), and
 *   • `installLog(log)` — replace the whole log (a lifecycle event: seed / New
 *     iteration / clear).
 * plus the three pointer moves (`undo` / `redo` / `jumpTo`). A test asserts
 * `scene === current(log)` after *every* action, with a proven-biting negative
 * control (`store.test.ts`).
 *
 * Reference stability is still a contract (not an optimisation): `MoleculeViewer`
 * redraws on a new `scene` reference, so a no-op action returns the current state
 * unchanged, and `current(log)` returns the *same frozen snapshot object* across
 * undo/redo — so navigating history does not churn the viewer.
 */

import { create } from "zustand";

import {
  append,
  current,
  emptyLog,
  goto,
  redo as redoLog,
  undo as undoLog,
  type FragmentGeometryVia,
  type Op,
  type SceneLog,
  type SnapshotSource,
} from "./oplog";
import { stampFreshIds } from "./ids";
import {
  addFragment as addFragmentPure,
  atomCount,
  makeFragmentId,
  removeFragment as removeFragmentPure,
  renameFragment as renameFragmentPure,
  replaceFragmentAtoms as replaceFragmentAtomsPure,
  setMultiplicity as setMultiplicityPure,
  totalCharge,
} from "./scene";
import type { RawAtom, RawFragment, Scene } from "./types";

export interface SceneStore {
  /** The operation log — the source of truth. */
  log: SceneLog;
  /** DERIVED: `current(log)`. Never set except from the log (see file header). */
  scene: Scene | null;
  /** Set when a manual coordinate edit collapsed a multi-fragment scene. */
  resetNotice: { fragmentCount: number } | null;

  // ── Low-level doors (the only two ways the log changes, plus pointer moves) ──
  /** Append one operation with its resultant snapshot. */
  commit(op: Op, resultScene: Scene): void;
  /** Replace the whole log (lifecycle: seed / New iteration / clear). */
  installLog(log: SceneLog): void;

  // ── Lifecycle convenience ──
  /**
   * Start a fresh geometry lineage from a whole scene (or clear to none). A
   * `null` scene installs the empty log (`scene → null`); a scene installs a log
   * seeded with a single `restore-snapshot` entry tagged by `source`.
   */
  seedScene(scene: Scene | null, source: SnapshotSource): void;

  // ── Logged geometry ops (each computes the result and appends) ──
  addFragment(f: RawFragment): void; // atoms get scene-minted ids
  removeFragment(id: string): void;
  renameFragment(id: string, name: string): void;
  setMultiplicity(m: number): void;
  replaceFragmentAtoms(id: string, atoms: RawAtom[], via: FragmentGeometryVia): void;
  /** A manual coordinate edit diverged from the scene → collapse to the text. */
  collapseFromText(atoms: RawAtom[]): void;

  // ── History navigation (undo/redo fall out of the log — ADR-010) ──
  undo(): void;
  redo(): void;
  jumpTo(pointer: number): void;

  dismissResetNotice(): void;
}

/** The identity element for `addFragment` on an empty log (ids start at 0). */
const EMPTY_SCENE: Scene = { fragments: [], multiplicity: 1, nextAtomId: 0 };

/** The scene the log currently folds to (or the empty scene for op composition). */
function base(log: SceneLog): Scene {
  return current(log) ?? EMPTY_SCENE;
}

export const useSceneStore = create<SceneStore>((set, get) => ({
  log: emptyLog(),
  scene: null,
  resetNotice: null,

  // The single low-level append. Clears the reset notice (a fresh op supersedes
  // the "coordinates edited" warning). `scene` is re-derived, never set apart.
  commit: (op, resultScene) =>
    set((s) => {
      const log = append(s.log, op, resultScene);
      return { log, scene: current(log), resetNotice: null };
    }),

  installLog: (log) => set({ log, scene: current(log), resetNotice: null }),

  seedScene: (scene, source) => {
    if (scene === null) {
      get().installLog(emptyLog());
      return;
    }
    const op: Op = {
      type: "restore-snapshot",
      source,
      fragmentCount: scene.fragments.length,
      atomCount: atomCount(scene),
    };
    get().installLog(append(emptyLog(), op, scene));
  },

  addFragment: (f) => {
    const next = addFragmentPure(base(get().log), f);
    const added = next.fragments[next.fragments.length - 1];
    get().commit(
      {
        type: "add-fragment",
        fragmentId: added.id,
        name: added.name,
        source: added.source,
        sourceLabel: added.sourceLabel ?? null,
        atomCount: added.atoms.length,
      },
      next,
    );
  },

  removeFragment: (id) => {
    const cur = current(get().log);
    if (!cur) return;
    const frag = cur.fragments.find((f) => f.id === id);
    if (!frag) return;
    get().commit(
      { type: "remove-fragment", fragmentId: id, name: frag.name },
      removeFragmentPure(cur, id),
    );
  },

  renameFragment: (id, name) => {
    const cur = current(get().log);
    if (!cur) return;
    const frag = cur.fragments.find((f) => f.id === id);
    if (!frag || frag.name === name) return;
    get().commit(
      { type: "rename-fragment", fragmentId: id, from: frag.name, to: name },
      renameFragmentPure(cur, id, name),
    );
  },

  setMultiplicity: (m) => {
    const cur = current(get().log);
    if (!cur || cur.multiplicity === m) return;
    get().commit(
      { type: "set-multiplicity", multiplicity: m },
      setMultiplicityPure(cur, m),
    );
  },

  replaceFragmentAtoms: (id, atoms, via) => {
    const cur = current(get().log);
    if (!cur) return;
    const frag = cur.fragments.find((f) => f.id === id);
    if (!frag) return;
    get().commit(
      { type: "replace-fragment-atoms", fragmentId: id, name: frag.name, edit: via },
      replaceFragmentAtomsPure(cur, id, atoms),
    );
  },

  // A manual coordinate edit diverged from the scene → the scene follows the
  // text: one fragment holding the parsed atoms, preserving total charge and
  // multiplicity. The notice fires only when >1 fragment was lost (a
  // single-fragment collapse is geometrically a no-op — see NewJobScreen). This
  // is a LOGGED op (append CollapseFromText), not a reset: undoing it brings the
  // pre-collapse layout back, and the Scene→Monaco sync re-injects it so the
  // text matches again and no second collapse fires (the dead loop, test c).
  collapseFromText: (atoms) =>
    set((s) => {
      const prev = current(s.log);
      const fragmentCount = prev ? prev.fragments.length : 0;
      // Identity continuity with arbitrarily hand-edited coordinates is undefined,
      // so the collapsed fragment gets FRESH ids from 0 (the Monaco identity
      // boundary; goes away in 2d when the block becomes a read-only projection).
      const { atoms: stamped, nextAtomId } = stampFreshIds(atoms, 0);
      const collapsed: Scene = {
        fragments: [
          {
            id: makeFragmentId(),
            name: prev?.fragments[0]?.name ?? "Molecule",
            atoms: stamped,
            charge: prev ? totalCharge(prev) : 0,
            source: "editor",
          },
        ],
        multiplicity: prev?.multiplicity ?? 1,
        nextAtomId,
      };
      const log = append(s.log, { type: "collapse-from-text", fragmentCount }, collapsed);
      return {
        log,
        scene: current(log),
        resetNotice: fragmentCount > 1 ? { fragmentCount } : null,
      };
    }),

  undo: () =>
    set((s) => {
      const log = undoLog(s.log);
      return { log, scene: current(log), resetNotice: null };
    }),

  redo: () =>
    set((s) => {
      const log = redoLog(s.log);
      return { log, scene: current(log), resetNotice: null };
    }),

  jumpTo: (pointer) =>
    set((s) => {
      const log = goto(s.log, pointer);
      return { log, scene: current(log), resetNotice: null };
    }),

  dismissResetNotice: () => set({ resetNotice: null }),
}));
