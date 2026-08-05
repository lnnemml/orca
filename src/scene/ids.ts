/**
 * `AtomId` — a stable, opaque atom identity for the Scene model (ADR-010 I1;
 * Phase 4.2 unit 1b). An `AtomId` is assigned when an atom **enters a Scene** and
 * is invariant until the atom is deleted; it is **not** a position in any array
 * and it is **not** an ORCA / ASE index (those are the branded `OrcaIndex` /
 * `AseIndex` of later units — 1c onward — and do not exist here).
 *
 * Pure and dependency-free: no React, no fetch, no runtime state. The allocation
 * counter lives on the `Scene` (`nextAtomId`), never in a module-level global, so
 * every function that mints ids stays pure — it takes a start value and returns
 * the atoms plus the advanced counter.
 *
 * The value is a plain `number` at runtime (branding is a compile-time-only
 * phantom), so `JSON.stringify` writes it as a bare integer in the v2 snapshot and
 * no custom (de)serialization is needed for the brand itself.
 */

import type { RawAtom, SceneAtom } from "./types";

/**
 * A branded integer. The `unique symbol` phantom field makes `AtomId`
 * assignment-incompatible with a bare `number` and with any other branded index
 * (mixing does not compile), while erasing to a plain `number` at runtime.
 */
export type AtomId = number & { readonly __atomIdBrand: unique symbol };

/** Brand a raw integer as an `AtomId`. The one sanctioned `number → AtomId`. */
export function makeAtomId(n: number): AtomId {
  return n as AtomId;
}

/**
 * Stamp a list of **raw** atoms (no identity) with sequential `AtomId`s starting
 * at `startId`, returning the id-bearing atoms and the next free id. This is the
 * one place identity is *minted*. Used at the Scene boundary — when raw parser
 * output first becomes part of a Scene, and by `addFragment` when new atoms join
 * an existing Scene. It is NOT used by `replaceFragmentAtoms` / `replaceAllAtoms`:
 * a geometry edit re-uses the OLD atoms' ids positionally (see `carryIds`), it
 * does not mint new ones.
 */
export function stampFreshIds(
  atoms: readonly RawAtom[],
  startId: number,
): { atoms: SceneAtom[]; nextAtomId: number } {
  const stamped = atoms.map((a, i) => ({
    id: makeAtomId(startId + i),
    element: a.element,
    x: a.x,
    y: a.y,
    z: a.z,
  }));
  return { atoms: stamped, nextAtomId: startId + atoms.length };
}

/**
 * Carry the identities of `oldAtoms` onto `newAtoms` **positionally** — atom `i`
 * of the result keeps `oldAtoms[i].id` and takes `newAtoms[i]`'s coordinates and
 * element. This is the identity-preservation rule for every geometry replacement
 * (`replaceFragmentAtoms`, `replaceAllAtoms`): the incoming atoms come from
 * ASE / xtb / GOAT / parsed xyz and **carry no id of their own** (they are
 * `RawAtom`s — the type has no `id` field, so there is structurally nothing to
 * mis-transfer). The positional carry is correct because the callers enforce the
 * count + element-order invariant (ADR-008 one-index-space); minting fresh ids
 * here instead would silently void atom identity on every edit.
 *
 * Requires `oldAtoms.length === newAtoms.length` (the caller has already checked
 * it); throws otherwise rather than truncate.
 */
export function carryIds(
  oldAtoms: readonly SceneAtom[],
  newAtoms: readonly RawAtom[],
): SceneAtom[] {
  if (oldAtoms.length !== newAtoms.length) {
    throw new Error(
      `carryIds length mismatch: ${oldAtoms.length} old vs ${newAtoms.length} new`,
    );
  }
  return newAtoms.map((a, i) => ({
    id: oldAtoms[i].id,
    element: a.element,
    x: a.x,
    y: a.y,
    z: a.z,
  }));
}
