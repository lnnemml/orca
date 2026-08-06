/**
 * Inter-fragment steric-clash detection (Phase 4.2 Stage 3, unit 3.2). Pure /
 * node-tested, React-free (ADR-008 decision 10) — no import of react / 3dmol / tauri.
 *
 * After any geometry change (a drag release, an edit apply, a fragment placement —
 * all mutate the Scene) the coarse positioning can overlap two fragments' atoms.
 * This flags atoms from **different fragments** closer than a fraction `k` of their
 * van der Waals sum — a **warning, not a block** (the drag is deliberately coarse;
 * a close contact at setup time is expected and is refined in the editor). It is a
 * pure function of `(scene, k, activeConstraints)` — clash state is *derived over the
 * Scene*, never stored in it; `k` is app-owned (ADR-011 style), a labeled heuristic.
 *
 * ## Four decisions, each a domain rule
 *
 * 1. **Inter-fragment only.** A fragment is rigid (Stage 3), so its internal
 *    geometry — including its own bonds — can never clash with itself; only two
 *    *different* fragments can overlap. Testing intra-fragment pairs would flag a
 *    fragment's own bonded atoms (control c2).
 * 2. **Reuse `measure.ts` `distance`** for the geometry — one distance
 *    implementation, not a second.
 * 3. **Excludes intentional contacts.** A pair carrying an active **distance
 *    constraint** is the researcher deliberately forming a bond (the Bürgi–Dunitz
 *    approach, a forming C···Nu pair) — it is NOT a clash, even inside the vdW sum.
 *    Read from the SAME `constraints.ts` parser (control c4 — the mission gate).
 * 4. **UNDETERMINED, not guessed.** A pair touching an element with no cited vdW
 *    radius (`vdw-radii.ts`) is **skipped and surfaced** (rule #11) — never treated
 *    as radius 0 (which would clash always or never). Reported separately from
 *    clashes (control c3), so the UI can say "couldn't check M–X" quietly.
 */

import type { Scene } from "./types";
import type { AtomId } from "./ids";
import type { Constraint } from "./constraints";
import { fromOrcaIndex } from "./constraints";
import { distance } from "./measure";
import { vdwRadius } from "./vdw-radii";

/** A confirmed inter-fragment steric clash: two atoms (different fragments) closer
 * than `k·(rᵢ+rⱼ)`. Atoms are stable {@link AtomId}s (2c2) so the highlight follows
 * the physical atom. */
export interface Clash {
  a: AtomId;
  b: AtomId;
  elements: [string, string];
  /** Actual separation, Å. */
  distance: number;
  /** The threshold this pair fell under, `k·(rᵢ+rⱼ)` Å. */
  threshold: number;
}

/** An inter-fragment pair that could NOT be checked because at least one element has
 * no cited vdW radius (UNDETERMINED). Skipped — never counted as a clash. */
export interface UndeterminedPair {
  a: AtomId;
  b: AtomId;
  elements: [string, string];
}

export interface ClashReport {
  clashes: Clash[];
  /** Pairs skipped for a missing (UNDETERMINED) radius — surfaced separately. */
  undetermined: UndeterminedPair[];
}

/** Order-independent key for an atom pair by global index. */
function pairKey(i: number, j: number): string {
  return i < j ? `${i},${j}` : `${j},${i}`;
}

/**
 * The set of global-index pairs carrying an active **distance** constraint — the
 * intentional contacts to exclude. Built from the SAME `Constraint`s the panel
 * parses; constraint atoms are ORCA indices, mapped back to global via
 * `fromOrcaIndex`. Only `distance` constraints define a contact pair (an angle or
 * dihedral does not).
 */
function constrainedPairs(constraints: Constraint[]): Set<string> {
  const keys = new Set<string>();
  for (const c of constraints) {
    if (c.kind !== "distance") continue;
    keys.add(pairKey(fromOrcaIndex(c.atoms[0]), fromOrcaIndex(c.atoms[1])));
  }
  return keys;
}

/**
 * Flag inter-fragment atom pairs closer than `k·(rᵢ+rⱼ)` (van der Waals overlap).
 * `k` is the heuristic threshold fraction (default ≈ 0.65, a labeled slider — NOT a
 * physical cutoff). Distance-constrained pairs are excluded; pairs with an
 * UNDETERMINED radius are skipped and reported apart.
 */
export function detectClashes(
  scene: Scene,
  k: number,
  activeConstraints: Constraint[],
): ClashReport {
  const clashes: Clash[] = [];
  const undetermined: UndeterminedPair[] = [];

  // Flatten atoms to (global index, id, element, fragment index) in global order.
  const atoms: { gi: number; id: AtomId; element: string; frag: number }[] = [];
  let gi = 0;
  scene.fragments.forEach((f, frag) => {
    for (const a of f.atoms) atoms.push({ gi: gi++, id: a.id, element: a.element, frag });
  });

  const excluded = constrainedPairs(activeConstraints);

  for (let i = 0; i < atoms.length; i++) {
    for (let j = i + 1; j < atoms.length; j++) {
      if (atoms[i].frag === atoms[j].frag) continue; // inter-fragment only (decision 1)
      if (excluded.has(pairKey(atoms[i].gi, atoms[j].gi))) continue; // intentional contact (decision 3)

      const ri = vdwRadius(atoms[i].element);
      const rj = vdwRadius(atoms[j].element);
      if (ri === undefined || rj === undefined) {
        // UNDETERMINED (decision 4): skip — never treat a missing radius as 0.
        undetermined.push({
          a: atoms[i].id,
          b: atoms[j].id,
          elements: [atoms[i].element, atoms[j].element],
        });
        continue;
      }

      const threshold = k * (ri + rj);
      const d = distance(scene, atoms[i].gi, atoms[j].gi) ?? 0; // null = coincident → 0
      if (d < threshold) {
        clashes.push({
          a: atoms[i].id,
          b: atoms[j].id,
          elements: [atoms[i].element, atoms[j].element],
          distance: d,
          threshold,
        });
      }
    }
  }

  return { clashes, undetermined };
}

/** The distinct atoms in any clash — for the viewer highlight (deduped `AtomId[]`). */
export function clashAtomIds(report: ClashReport): AtomId[] {
  const ids = new Set<AtomId>();
  for (const c of report.clashes) {
    ids.add(c.a);
    ids.add(c.b);
  }
  return [...ids];
}

/** The distinct elements we couldn't check (missing vdW radius) — for the quiet
 * UNDETERMINED notice, so it names elements once, not every pair. */
export function undeterminedElements(report: ClashReport): string[] {
  const els = new Set<string>();
  for (const p of report.undetermined) {
    if (vdwRadius(p.elements[0]) === undefined) els.add(p.elements[0]);
    if (vdwRadius(p.elements[1]) === undefined) els.add(p.elements[1]);
  }
  return [...els];
}
