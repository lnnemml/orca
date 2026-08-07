/**
 * Bond DISPLAY filter (unit bond-display-control) — pure, node-tested, 3Dmol-free
 * (so vitest can import it; the 3Dmol bundle needs WebGL and won't load in jsdom).
 *
 * 3Dmol perceives bonds from interatomic distances. That is right for covalent
 * molecules but wrong for two cases this filter removes **from the drawing only**:
 *
 *  1. **Cation coordinate bonds (default).** An s-block metal cation (Na⁺, K⁺, Mg²⁺,
 *     …) *coordinates* an O / N / π face electrostatically; it does not form a
 *     covalent bond. A distance perceiver draws a spurious stick (Na⁺ next to a
 *     carbonyl O or an aromatic H). We exclude any bond with a cation end by default.
 *  2. **Manually hidden bonds.** A general escape hatch: the user can hide/show any
 *     specific bond (e.g. a forming C···Nu contact in a compressed TS guess that
 *     3Dmol draws as a bond). Keyed by the **AtomId pair**, so it survives
 *     re-perception, geometry edits, and drag/rotate (a positional key would hide the
 *     wrong bond after an index shift — the 2.5.2b defect class).
 *
 * **DISPLAY-ONLY.** None of this touches the Scene, the ORCA input (which is
 * coordinates + charge — there is no bond list; `wiki/orca/parse-sources.md`), or the
 * sidecar's own mask perception (which has its own `within`). The filter mutates the
 * throwaway 3Dmol atom array 3Dmol built, exactly as `frozenTopology.ts` mutates it
 * to persist a topology — it never sees a Scene.
 */

import type { AtomId } from "../scene/ids";

/**
 * Elements whose "bonds" 3Dmol draws are coordinate/ionic, not covalent — the
 * **s-block metals**: alkali (group 1) + alkaline-earth (group 2). These form
 * predominantly electrostatic contacts, so a distance perceiver invents a stick to
 * whatever they sit near.
 *
 * Deliberately **NOT** a "+charge" test and deliberately **NOT** the transition
 * metals:
 *  - `H` is excluded — H⁺ is a bare proton, and every C–H / O–H is a real bond;
 *  - `N` is excluded — NH₄⁺ has genuine covalent N–H bonds;
 *  - transition metals (Pd, Pt, Fe, Ni, …) are excluded — their **metal–ligand bonds
 *    are chemically real** in the organometallics this app targets (ADR-007), and
 *    drawing them is correct.
 */
export const CATION_ELEMENTS: ReadonlySet<string> = new Set([
  "Li", "Na", "K", "Rb", "Cs", // group 1 (alkali)
  "Be", "Mg", "Ca", "Sr", "Ba", // group 2 (alkaline earth)
]);

/** Canonicalise an element symbol to `Xx` form so lookup is case-insensitive. */
function canonicalElement(el: string): string {
  const s = el.trim();
  return s.length === 0 ? s : s[0].toUpperCase() + s.slice(1).toLowerCase();
}

export function isCationElement(el: string): boolean {
  return CATION_ELEMENTS.has(canonicalElement(el));
}

/** A bond is a "cation coordinate bond" if EITHER end is an s-block metal cation. */
export function isCationBond(elA: string, elB: string): boolean {
  return isCationElement(elA) || isCationElement(elB);
}

/** A normalized, order-independent key for an AtomId pair — the identity of a bond
 * for hide/show. Sorted numerically so `bondKey(a,b) === bondKey(b,a)`. */
export type BondKey = string;
export function bondKey(a: AtomId, b: AtomId): BondKey {
  const x = a as unknown as number;
  const y = b as unknown as number;
  return x <= y ? `${x}:${y}` : `${y}:${x}`;
}

/** Is the bond between these two AtomIds manually hidden? `false` when either id is
 * absent (an unresolved atom can't match a hide — the cation rule still applies via
 * elements). */
export function isBondHidden(
  a: AtomId | undefined,
  b: AtomId | undefined,
  hidden: ReadonlySet<BondKey>,
): boolean {
  if (a === undefined || b === undefined) return false;
  return hidden.has(bondKey(a, b));
}

export interface BondDisplayOptions {
  /** Draw cation coordinate bonds anyway (default `false` — they are excluded). */
  showCationBonds?: boolean;
}

/**
 * The one predicate: should this bond be DRAWN? `true` iff it is **not** a hidden
 * pair **and** (not a cation bond, or cation bonds are explicitly shown).
 */
export function shouldDrawBond(
  elA: string,
  elB: string,
  idA: AtomId | undefined,
  idB: AtomId | undefined,
  hidden: ReadonlySet<BondKey>,
  opts?: BondDisplayOptions,
): boolean {
  if (isBondHidden(idA, idB, hidden)) return false;
  if (!opts?.showCationBonds && isCationBond(elA, elB)) return false;
  return true;
}

/** The minimal 3Dmol atom shape the filter touches. `bonds` holds the array indices
 * of bonded atoms; `bondOrder` is the parallel order array; `index` is the viewer
 * index (== array position on a normal parse). */
export interface FilterableAtom {
  index?: number;
  elem: string;
  bonds: number[];
  bondOrder?: number[];
}

function removeHalfEdge(atom: FilterableAtom, k: number): void {
  atom.bonds.splice(k, 1);
  if (atom.bondOrder) atom.bondOrder.splice(k, 1);
}

/**
 * Remove from the live 3Dmol atom array every bond `shouldDrawBond` rejects, mutating
 * `bonds`/`bondOrder` in place (the frozenTopology technique) so 3Dmol's stick pass
 * draws only the survivors — **no second bond perception**, we filter the one 3Dmol
 * already did. `resolveId` maps a viewer index to its AtomId (the `ViewerAtomTable`
 * in the scene path; `() => undefined` where there is no table, so only the
 * element-based cation rule applies). Returns the number of DISTINCT bonds removed.
 * Idempotent: a second call removes 0 (the rejected bonds are already gone).
 */
export function filterDrawnBonds(
  atoms: FilterableAtom[],
  resolveId: (viewerIndex: number) => AtomId | undefined,
  hidden: ReadonlySet<BondKey>,
  opts?: BondDisplayOptions,
): number {
  let removed = 0;
  for (let i = 0; i < atoms.length; i++) {
    const a = atoms[i];
    if (!a || !a.bonds) continue;
    // Walk backwards so a splice doesn't skip the next entry.
    for (let k = a.bonds.length - 1; k >= 0; k--) {
      const j = a.bonds[k];
      const b = atoms[j];
      if (!b) continue;
      const idA = a.index !== undefined ? resolveId(a.index) : undefined;
      const idB = b.index !== undefined ? resolveId(b.index) : undefined;
      if (shouldDrawBond(a.elem, b.elem, idA, idB, hidden, opts)) continue;
      removeHalfEdge(a, k);
      const back = b.bonds.indexOf(i);
      if (back >= 0) removeHalfEdge(b, back);
      removed++;
    }
  }
  return removed;
}
