/**
 * Test-only constructors for id-bearing Scenes (unit 1b). Production code mints
 * `AtomId`s at the Scene boundary (`stampFreshIds`); tests that build a Scene by
 * hand use these so they get the SAME scene-wide allocation without spelling out
 * ids — and without `id` ever being optional on the real types (a missing
 * invariant), which the unit forbids.
 *
 * Not imported by any production module (its name carries `-test-util`).
 */

import { makeAtomId, stampFreshIds } from "./ids";
import type { AtomId } from "./ids";
import { atomIdAtIndex } from "./scene";
import type { RawFragment, Scene, SceneAtom, SceneFragment } from "./types";

// `RawFragment` now lives in ./types (a detached fragment is a first-class shape);
// re-exported here so existing test imports `from "./scene-test-util"` keep working.
export type { RawFragment };

/**
 * Build a valid Scene from fragments whose atoms are raw — stamps scene-wide ids
 * in fragment order and sets `nextAtomId`, exactly as production allocation does.
 */
export function testScene(frags: RawFragment[], multiplicity = 1): Scene {
  let next = 0;
  const fragments: SceneFragment[] = frags.map((f) => {
    const { atoms, nextAtomId } = stampFreshIds(f.atoms, next);
    next = nextAtomId;
    return { ...f, atoms };
  });
  return { fragments, multiplicity, nextAtomId: next };
}

/**
 * Resolve global indices to the {@link AtomId}s they currently address — the test
 * counterpart of the production resolver, so a test that means "the atoms at
 * global 0,1,2" can hand `AtomId`s to the (now id-native) selection/measure/plan
 * API. An index with no atom (an out-of-range probe) maps to a guaranteed-absent
 * sentinel id, so it resolves back to "not in scene" exactly as a real stale id
 * would — never to some other atom.
 */
export function idsFor(scene: Scene, ...globalIndices: number[]): AtomId[] {
  return globalIndices.map(
    (gi) => atomIdAtIndex(scene, gi) ?? makeAtomId(900_000 + gi),
  );
}

/**
 * The canonical **AtomId ≠ global-index** fixture for unit 2c2 (the divergence the
 * whole unit's tests must be built on — on a fresh scene id == index, so a test
 * that doesn't diverge them is green under both correct and broken code):
 *   water (ids 0,1,2 = global 0,1,2) + BH₄⁻ [B,H,H,H,H] (ids 3..7 = global 3..7),
 *   then **water removed** → BH₄⁻ keeps ids 3..7 but now occupies global 0..4.
 * So AtomId 3 (the boron) sits at global 0. Returns the post-removal scene plus
 * the boron's id and the id of the atom that moved into water's old global slot 0.
 */
export function borohydrideAfterWaterRemoved(): {
  scene: Scene;
  boronId: AtomId;
} {
  const water: RawFragment = {
    id: "water",
    name: "Water",
    charge: 0,
    source: "editor",
    atoms: [
      { element: "O", x: 0, y: 0, z: 0 },
      { element: "H", x: 0.76, y: 0.59, z: 0 },
      { element: "H", x: -0.76, y: 0.59, z: 0 },
    ],
  };
  const bh4: RawFragment = {
    id: "bh4",
    name: "BH4",
    charge: -1,
    source: "library",
    atoms: [
      { element: "B", x: 5, y: 0, z: 0 },
      { element: "H", x: 5.6, y: 0.6, z: 0.6 },
      { element: "H", x: 5.6, y: -0.6, z: -0.6 },
      { element: "H", x: 4.4, y: 0.6, z: -0.6 },
      { element: "H", x: 4.4, y: -0.6, z: 0.6 },
    ],
  };
  const full = testScene([water, bh4]);
  const boronId = full.fragments[1].atoms[0].id; // BH₄⁻ boron, id 3
  const scene: Scene = {
    ...full,
    fragments: full.fragments.filter((f) => f.id !== "water"),
  };
  return { scene, boronId };
}

/** One `SceneAtom` with an explicit id — for tests that assert on a single atom. */
export function testAtom(
  element: string,
  x: number,
  y: number,
  z: number,
  id = 0,
): SceneAtom {
  return stampFreshIds([{ element, x, y, z }], id).atoms[0];
}
