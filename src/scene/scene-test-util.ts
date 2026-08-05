/**
 * Test-only constructors for id-bearing Scenes (unit 1b). Production code mints
 * `AtomId`s at the Scene boundary (`stampFreshIds`); tests that build a Scene by
 * hand use these so they get the SAME scene-wide allocation without spelling out
 * ids — and without `id` ever being optional on the real types (a missing
 * invariant), which the unit forbids.
 *
 * Not imported by any production module (its name carries `-test-util`).
 */

import { stampFreshIds } from "./ids";
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
