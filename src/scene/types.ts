/**
 * Scene / SceneFragment data model — OrcaStudio's own abstraction for a
 * multi-molecule geometry. ORCA never sees a Scene: on export the fragments are
 * merged into one flat `* xyz totalCharge multiplicity ... *` block. Fragment
 * identity lives only in our state and (as a snapshot) in the DB. See
 * `wiki/architecture/adr-008-scene-fragment-model.md`.
 *
 * Types only — no runtime code, no React, no dependencies.
 */

/** One atom in a fragment: element symbol + Cartesian coordinates (Ångström). */
export interface SceneAtom {
  element: string;
  x: number;
  y: number;
  z: number;
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

export interface SceneFragment {
  id: string;
  name: string;
  atoms: SceneAtom[];
  /** Formal charge of this fragment alone. */
  charge: number;
  source: FragmentSource;
  /** Provenance: filename, SMILES string, library molecule id. */
  sourceLabel?: string;
}

export interface Scene {
  fragments: SceneFragment[];
  /** Spin multiplicity of the whole system (user-set, not derived). */
  multiplicity: number;
}
