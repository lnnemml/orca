/**
 * The **user** side of the reagent catalog (Phase 4.2 tail-2). The curated built-in
 * reagents live in `fragment-library.ts` (with a `reference` internal-coordinate
 * contract the tests recompute); user reagents are `molecules` rows with role
 * `is_reagent` — persisted through the Rust `create_reagent` / `list_reagents`
 * commands — and carry **no** such contract (user provenance, no verified geometry).
 * This module converts a stored reagent to a scene fragment and is where the
 * curated↔user distinction is made explicit, so the palette can't blur it.
 *
 * Pure / React-free (no `invoke`) — the DB round-trip is the caller's; here we only
 * transform already-fetched data, so it is node-testable.
 */

import type { Molecule } from "../types";
import type { RawFragment, SceneFragment, Scene } from "./types";
import { mergeToXyz, sceneFromXyz } from "./scene";

/**
 * A user-saved reagent (a `molecules` row, role reagent) as a detached
 * {@link RawFragment}, mirroring `libraryFragmentToScene` but from DB data. Its
 * stored `charge` is carried onto the fragment, so it flows into the scene total
 * exactly like a built-in reagent's — **no special case** (ADR-008 #8). `source` is
 * `"library"` (a saved library item — **never** `"fragment-library"`, which is the
 * curated built-ins), and `sourceLabel` is the molecule id, so a user reagent stays
 * distinguishable from a curated one downstream. `null` if the stored xyz doesn't
 * parse (defensive — never throws, the `sceneFromXyz` contract).
 */
export function userReagentToFragment(m: Molecule): RawFragment | null {
  const scene = sceneFromXyz(m.xyz, {
    source: "library",
    sourceLabel: m.id,
    name: m.name,
    charge: m.charge,
    // Multiplicity isn't stored per reagent-role meaning (charge + electron parity
    // determine it; the Scene validates). The single-fragment scene is only a
    // vehicle to reuse the xyz parser; 1 is the neutral default.
    multiplicity: 1,
  });
  return scene ? scene.fragments[0] : null;
}

/**
 * A single scene fragment as a standalone xyz string — the geometry captured when a
 * fragment is saved "to my reagents". Builds a one-fragment scene and reuses the
 * canonical `mergeToXyz` (no second xyz formatter).
 */
export function fragmentToXyz(fragment: SceneFragment, comment = ""): string {
  const scene: Scene = { fragments: [fragment], multiplicity: 1, nextAtomId: 0 };
  return mergeToXyz(scene, comment);
}
