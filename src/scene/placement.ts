/**
 * Coarse placement for a newly added scene fragment (ADR-008 #7). Pure/
 * React-free.
 *
 * The problem: when the user adds a reagent to a scene, it must not land *inside*
 * the substrate. The naive "centre of mass + fixed vector" collides for an
 * elongated substrate (the offset can land mid-chain). Instead we separate the
 * two axis-aligned bounding boxes (AABBs) along the axis where the scene is
 * *smallest* — for a chain lying along x that means approaching from the side,
 * not down the chain.
 *
 * The separation guarantee is structural, not hopeful: after placement every
 * fragment atom has its coordinate on the chosen axis ≥ (scene max on that axis)
 * + gap, while every scene atom is ≤ (scene max). So the difference on that one
 * axis alone is ≥ gap for every scene/fragment atom pair, hence the Euclidean
 * distance is ≥ gap. No pairwise scan, no luck.
 *
 * The *orientation* is deliberately crude and chemically meaningless — exact
 * positioning (Bürgi-Dunitz distance/angle/dihedral) is the geometry editor's
 * job in 2.5.3. This only has to produce a non-overlapping starting point.
 */

import { translateFragment } from "./scene";
import type { Scene, SceneAtom, SceneFragment } from "./types";

/** Default clearance between the two bounding boxes, in Ångström. */
const DEFAULT_GAP = 3.5;

interface Aabb {
  min: [number, number, number];
  max: [number, number, number];
}

/** Axis-aligned bounding box of a non-empty atom list. */
function aabb(atoms: SceneAtom[]): Aabb {
  const min: [number, number, number] = [Infinity, Infinity, Infinity];
  const max: [number, number, number] = [-Infinity, -Infinity, -Infinity];
  for (const a of atoms) {
    const c: [number, number, number] = [a.x, a.y, a.z];
    for (let i = 0; i < 3; i++) {
      if (c[i] < min[i]) min[i] = c[i];
      if (c[i] > max[i]) max[i] = c[i];
    }
  }
  return { min, max };
}

/**
 * Translate a copy of `fragment` so it sits clear of everything already in
 * `scene`, separated by at least `gap` Å. An empty scene returns the fragment
 * unmoved — it *is* the first fragment. Composition and internal geometry are
 * untouched (it is a pure translation).
 */
export function placeFragment(
  scene: Scene,
  fragment: SceneFragment,
  gap: number = DEFAULT_GAP,
): SceneFragment {
  const sceneAtoms = scene.fragments.flatMap((f) => f.atoms);
  // First fragment — nothing to avoid, leave it where it is.
  if (sceneAtoms.length === 0) return translateFragment(fragment, 0, 0, 0);
  if (fragment.atoms.length === 0) return translateFragment(fragment, 0, 0, 0);

  const s = aabb(sceneAtoms);
  const f = aabb(fragment.atoms);

  // Placement axis = the one along which the scene is smallest (ties → the
  // lower-index axis, so a genuine tie resolves to x). Strict `<` keeps the
  // earlier axis on equality.
  const size = [
    s.max[0] - s.min[0],
    s.max[1] - s.min[1],
    s.max[2] - s.min[2],
  ];
  let axis = 0;
  if (size[1] < size[axis]) axis = 1;
  if (size[2] < size[axis]) axis = 2;

  const d: [number, number, number] = [0, 0, 0];
  for (let i = 0; i < 3; i++) {
    if (i === axis) {
      // Butt the fragment's near face up against the scene's far face + gap.
      d[i] = s.max[i] + gap - f.min[i];
    } else {
      // Centre the fragment across the scene on the other two axes, so it faces
      // the object rather than sitting off in a corner.
      const sceneCentre = (s.min[i] + s.max[i]) / 2;
      const fragCentre = (f.min[i] + f.max[i]) / 2;
      d[i] = sceneCentre - fragCentre;
    }
  }

  return translateFragment(fragment, d[0], d[1], d[2]);
}
