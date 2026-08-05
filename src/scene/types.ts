/**
 * Scene / SceneFragment data model — OrcaStudio's own abstraction for a
 * multi-molecule geometry. ORCA never sees a Scene: on export the fragments are
 * merged into one flat `* xyz totalCharge multiplicity ... *` block. Fragment
 * identity lives only in our state and (as a snapshot) in the DB. See
 * `wiki/architecture/adr-008-scene-fragment-model.md`.
 *
 * Types only — no runtime code, no React. The single import is the `AtomId` type
 * from `ids.ts` (a type-only import — erased at runtime, so this stays dependency-
 * free in practice).
 */

import type { AtomId } from "./ids";

/**
 * A **raw** atom: element symbol + Cartesian coordinates (Ångström), with **no
 * identity**. This is exactly what parsing produces (`parseAtomLines`,
 * `parseEnsemble`, xyz readers) and what a geometry backend (ASE / xtb / GOAT)
 * returns. It is deliberately id-free: identity is attached only when an atom
 * enters a Scene (unit 1b / ADR-010). A `RawAtom` is structurally a `SceneAtom`
 * minus `id`, so nothing that flows through a parser can carry a stale id.
 */
export interface RawAtom {
  element: string;
  x: number;
  y: number;
  z: number;
}

/**
 * One atom **inside a Scene**: a {@link RawAtom} plus a stable {@link AtomId}
 * assigned at the Scene boundary. The id is opaque, is not an array position, and
 * survives every geometry edit (it is carried positionally across a replace, not
 * re-minted — see `ids.ts`).
 */
export interface SceneAtom extends RawAtom {
  id: AtomId;
}

/** Where a fragment's geometry came from (provenance for the snapshot). */
export type FragmentSource =
  | "editor"
  | "import"
  | "smiles"
  | "library"
  | "fragment-library";

/** Every valid {@link FragmentSource}, for shape validation on deserialize. */
export const FRAGMENT_SOURCES: readonly FragmentSource[] = [
  "editor",
  "import",
  "smiles",
  "library",
  "fragment-library",
];

/**
 * A **detached** fragment: fragment metadata with **raw** (id-free) atoms. This
 * is a fragment that is not yet part of a Scene — a library instantiation, a
 * placement candidate, a just-parsed molecule. Atom identity is minted only when
 * it joins a Scene (`addFragment` / `sceneFrom*`), so a detached fragment
 * deliberately has none. {@link SceneFragment} is its in-Scene form and is a
 * structural subtype (its atoms carry the extra `id`), so anything that accepts a
 * `RawFragment` also accepts a `SceneFragment`.
 */
export interface RawFragment {
  id: string;
  name: string;
  atoms: RawAtom[];
  /** Formal charge of this fragment alone. */
  charge: number;
  source: FragmentSource;
  /** Provenance: filename, SMILES string, library molecule id. */
  sourceLabel?: string;
}

/** A fragment **inside a Scene**: like {@link RawFragment} but its atoms carry ids. */
export interface SceneFragment extends RawFragment {
  atoms: SceneAtom[];
}

export interface Scene {
  fragments: SceneFragment[];
  /** Spin multiplicity of the whole system (user-set, not derived). */
  multiplicity: number;
  /**
   * Monotonic allocation counter for {@link AtomId}s: the next id to hand out.
   * Never decreases and never re-uses an id after a remove (uniqueness is only
   * required within a single Scene). Advanced by `stampFreshIds` / `addFragment`;
   * untouched by geometry replacements (they carry existing ids). Persisted in the
   * v2 snapshot so ids stay stable across save/reopen.
   */
  nextAtomId: number;
}
