/**
 * Parse a GOAT conformer ensemble (ORCA 6's `! XTB GOAT`) and build its input.
 * Pure / React-free. See `wiki/orca/goat.md` for the observed file format this
 * parser was written against (ORCA 6.1.0, verified on an n-butane run 2026-07-28
 * — not from memory).
 *
 * Observed `*.finalensemble.xyz` format: a multi-frame xyz — `count` line,
 * comment line `"<energy_in_Eh> converged=true"`, then `count` atom rows —
 * repeated per conformer, **sorted ascending by energy** (global minimum first),
 * and the **atom order is identical in every frame** (same as the input). That
 * order-invariance is what lets 2.5.1b drop a chosen conformer back into a
 * fragment via `replaceFragmentAtoms` with no atom mapping.
 */

import { normalizeElement } from "./scene";
import type { Scene, SceneAtom, SceneFragment } from "./types";

export interface Conformer {
  atoms: SceneAtom[];
  /** Energy in Hartree (Eh). `NaN` when the comment had no leading number. */
  energy: number;
  /** 0-based position in the ensemble (ascending energy). */
  index: number;
}

/** 1 Hartree in kcal/mol (CODATA). */
export const HARTREE_TO_KCAL_MOL = 627.5094740631;

/**
 * ΔE of each conformer relative to the lowest (conformers[0]), in **kcal/mol** —
 * the unit a chemist reads, not the absolute Hartree values. `NaN` energies pass
 * through as `NaN` (the UI shows a dash, not "NaN"). Ordering isn't assumed;
 * the reference is the first entry (GOAT writes the global minimum first).
 */
export function deltaEKcal(conformers: Conformer[]): number[] {
  if (conformers.length === 0) return [];
  const e0 = conformers[0].energy;
  return conformers.map((c) => (c.energy - e0) * HARTREE_TO_KCAL_MOL);
}

/**
 * Energy from a GOAT comment line. The energy is the leading whitespace-token
 * (`"-13.6651277570 converged=true"` → -13.6651277570); anything else the line
 * carries is ignored. Returns `NaN` if the leading token isn't a finite number —
 * we never invent a value.
 */
function parseEnergy(comment: string): number {
  const first = comment.trim().split(/\s+/)[0];
  const e = Number(first);
  return Number.isFinite(e) ? e : NaN;
}

/**
 * Parse a multi-frame xyz ensemble into conformers. Returns `null` on malformed
 * or empty input — never throws (same contract as `deserializeScene`). A frame
 * is malformed if its count line isn't a positive integer, an atom row lacks
 * `element x y z`, or a coordinate isn't finite.
 */
export function parseEnsemble(text: string): Conformer[] | null {
  const lines = text.split(/\r?\n/);
  const conformers: Conformer[] = [];
  let i = 0;
  const skipBlank = () => {
    while (i < lines.length && lines[i].trim() === "") i++;
  };

  skipBlank();
  while (i < lines.length) {
    const count = Number(lines[i].trim());
    if (!Number.isInteger(count) || count <= 0) return null;
    // count line, comment line, then `count` atom lines must all be present.
    if (i + 2 + count > lines.length) return null;
    const comment = lines[i + 1];

    const atoms: SceneAtom[] = [];
    for (let k = 0; k < count; k++) {
      const parts = lines[i + 2 + k].trim().split(/\s+/);
      if (parts.length < 4) return null;
      const [element, xs, ys, zs] = parts;
      const x = Number(xs);
      const y = Number(ys);
      const z = Number(zs);
      if (![x, y, z].every((n) => Number.isFinite(n))) return null;
      atoms.push({ element, x, y, z });
    }

    conformers.push({ atoms, energy: parseEnergy(comment), index: conformers.length });
    i += 2 + count;
    skipBlank();
  }

  return conformers.length > 0 ? conformers : null;
}

/**
 * Does a conformer have the same composition as a fragment — the same atom count
 * and the same element sequence (case-insensitive)? The exact check
 * `replaceFragmentAtoms` enforces, but as a **predicate** so the UI can show a
 * clear refusal instead of catching a thrown error. GOAT preserves atom order,
 * so for a fragment's own ensemble this is always true; it guards against feeding
 * the wrong ensemble to the wrong fragment.
 */
export function conformerMatchesFragment(
  fragment: SceneFragment,
  conformer: Conformer,
): boolean {
  if (fragment.atoms.length !== conformer.atoms.length) return false;
  for (let i = 0; i < fragment.atoms.length; i++) {
    if (
      normalizeElement(fragment.atoms[i].element) !==
      normalizeElement(conformer.atoms[i].element)
    ) {
      return false;
    }
  }
  return true;
}

/**
 * Build a `! XTB GOAT` input to run a conformer search on ONE fragment in
 * isolation. Charge is the **fragment's own** charge (GOAT runs on the fragment
 * alone, decoupled from the rest of the scene — so `fragment.charge`, not the
 * scene's `totalCharge`). Multiplicity defaults to 1: every curated library
 * fragment is closed-shell (even electron count), so a singlet is safe; an
 * open-shell fragment would need it set explicitly.
 */
export function goatInputForFragment(
  fragment: SceneFragment,
  multiplicity = 1,
): string {
  const rows = fragment.atoms.map(
    (a) => `${a.element}   ${a.x}   ${a.y}   ${a.z}`,
  );
  return (
    `! XTB GOAT\n` +
    `* xyz ${fragment.charge} ${multiplicity}\n` +
    `${rows.join("\n")}\n*\n`
  );
}

/** What "Use this conformer" should do, decided purely (so it's testable). */
export type ConformerApply =
  | { action: "replace"; fragmentId: string; atoms: SceneAtom[] }
  | { action: "new"; fragment: SceneFragment }
  | { action: "refuse"; reason: string };

/**
 * Decide how to apply a chosen conformer back (2.5.1b §4), given the current
 * store scene and the GOAT job's single-fragment snapshot:
 * - the snapshot's fragment id is **still in the store scene** → `replace` its
 *   atoms in place (exact addressing, no composition guessing);
 * - otherwise (other session / scene cleared) → `new` single-fragment scene from
 *   the snapshot's name/charge + the conformer's coordinates.
 *
 * Both branches first run `conformerMatchesFragment` against the fragment they'd
 * touch (the live store fragment, or the snapshot); a mismatch (the fragment's
 * composition changed) returns `refuse` — the caller shows a clear message, no
 * throw. That's the whole reason the predicate exists.
 */
export function planConformerApply(
  storeScene: Scene | null,
  snapshotFragment: SceneFragment,
  conformer: Conformer,
): ConformerApply {
  const live = storeScene?.fragments.find((f) => f.id === snapshotFragment.id);
  const target = live ?? snapshotFragment;
  if (!conformerMatchesFragment(target, conformer)) {
    return {
      action: "refuse",
      reason:
        "This conformer no longer matches the fragment — its composition changed " +
        "since the search was launched.",
    };
  }
  if (live) {
    return {
      action: "replace",
      fragmentId: snapshotFragment.id,
      atoms: conformer.atoms,
    };
  }
  return {
    action: "new",
    fragment: {
      ...snapshotFragment,
      atoms: conformer.atoms.map((a) => ({ ...a })),
    },
  };
}
