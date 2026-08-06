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
 * conformer swap, a text re-adopt, future code — could set the scene *without*
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
import {
  addFragment as addFragmentPure,
  atomCount,
  removeFragment as removeFragmentPure,
  renameFragment as renameFragmentPure,
  replaceFragmentAtoms as replaceFragmentAtomsPure,
  setMultiplicity as setMultiplicityPure,
  translateFragmentInScene,
  rotateFragmentInScene,
} from "./scene";
import type { AtomId } from "./ids";
import type { RawAtom, RawFragment, Scene } from "./types";

export interface SceneStore {
  /** The operation log — the source of truth. */
  log: SceneLog;
  /** DERIVED: `current(log)`. Never set except from the log (see file header). */
  scene: Scene | null;

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
  /**
   * Rigid-body translate a fragment by a TOTAL (dx, dy, dz) Å — the single op a
   * drag commits on release (Stage 3; ADR-010: the ephemeral 60fps motion is a
   * viewer-only overlay and is NOT logged; exactly one `translate-fragment` op
   * lands here on mouseup). No-op on a zero delta or an absent id.
   */
  translateFragment(id: string, dx: number, dy: number, dz: number): void;
  /**
   * Rigid-body ROTATE a fragment by `angleRad` about the axis two picked atoms
   * `[P, Q]` define (`dir = normalize(Q − P)`, `pivot = P`) — the single op the
   * "Rotate about axis" tool commits on Apply (Stage 3, unit 3.3; ADR-010: the
   * live ephemeral preview is a viewer-only overlay and is NOT logged; exactly one
   * `rotate-fragment` op lands here). No-op on a zero angle, an absent id, or a
   * degenerate axis (P ≡ Q). The op stores `[P, Q]` (the approach axis is two atoms
   * by definition — ADR-007); the result is materialized here from the current scene.
   */
  rotateFragment(id: string, axisAtoms: [AtomId, AtomId], angleRad: number): void;

  // ── History navigation (undo/redo fall out of the log — ADR-010) ──
  undo(): void;
  redo(): void;
  jumpTo(pointer: number): void;
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

  // The single low-level append. `scene` is re-derived, never set apart.
  commit: (op, resultScene) =>
    set((s) => {
      const log = append(s.log, op, resultScene);
      return { log, scene: current(log) };
    }),

  installLog: (log) => set({ log, scene: current(log) }),

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

  translateFragment: (id, dx, dy, dz) => {
    const cur = current(get().log);
    if (!cur) return;
    const frag = cur.fragments.find((f) => f.id === id);
    if (!frag) return;
    if (dx === 0 && dy === 0 && dz === 0) return; // a click with no drag → no op
    get().commit(
      { type: "translate-fragment", fragmentId: id, name: frag.name, delta: [dx, dy, dz] },
      translateFragmentInScene(cur, id, dx, dy, dz),
    );
  },

  rotateFragment: (id, axisAtoms, angleRad) => {
    const cur = current(get().log);
    if (!cur) return;
    const frag = cur.fragments.find((f) => f.id === id);
    if (!frag) return;
    if (angleRad === 0) return; // no turn → no op
    // `rotateFragmentInScene` returns the SAME reference on a degenerate axis (P ≡ Q
    // or an absent axis atom); an identity result must not append a log entry.
    const next = rotateFragmentInScene(cur, id, axisAtoms, angleRad);
    if (next === cur) return;
    get().commit(
      { type: "rotate-fragment", fragmentId: id, name: frag.name, axisAtoms, angleRad },
      next,
    );
  },

  undo: () =>
    set((s) => {
      const log = undoLog(s.log);
      return { log, scene: current(log) };
    }),

  redo: () =>
    set((s) => {
      const log = redoLog(s.log);
      return { log, scene: current(log) };
    }),

  jumpTo: (pointer) =>
    set((s) => {
      const log = goto(s.log, pointer);
      return { log, scene: current(log) };
    }),
}));
