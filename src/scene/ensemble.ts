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

import { stampFreshIds } from "./ids";
import { normalizeElement } from "./scene";
import type { RawAtom, RawFragment, Scene, SceneFragment } from "./types";

export interface Conformer {
  /** Raw geometry — a parsed ensemble carries no atom identity. */
  atoms: RawAtom[];
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

/** R, the gas constant, in kcal/(mol·K) (CODATA). */
export const GAS_CONSTANT_KCAL = 1.987204259e-3;
/** Standard room temperature, K. */
export const ROOM_TEMPERATURE_K = 298.15;

/**
 * Boltzmann populations w_i = exp(−ΔE_i / RT) / Σ_j exp(−ΔE_j / RT) over the
 * conformer ensemble, at temperature `tempK`. Pure TS-only display analysis —
 * NOT a Rust/TS byte-identical pair: this is derived weighting shown in the
 * panel, not text emitted into an ORCA input file, so there is deliberately no
 * `orcastudio-core` mirror (don't "fix" this by adding one). Populations are
 * DERIVED, never persisted — computed on the fly from the parsed ensemble at
 * render time (same one-source-of-truth rule the C2b absolute-barrier work
 * settled: a cached scalar drifts from its source).
 *
 * Because the ensemble is xTB-level (GFN2), these are xTB-level populations —
 * the panel labels them so; D2's DFT re-opt will re-rank and re-weight and the
 * two must never be confused.
 *
 * Numerical stability: ΔE_i is taken relative to the lowest (`deltaEKcal`, min
 * = 0), so every exp argument is ≤ 0 and every exp value ∈ (0, 1] — no overflow.
 * The relative-to-min form IS the stability guard; there is no second one.
 *
 * NaN contract (mirrors `deltaEKcal`'s NaN pass-through — rule #9, honest-or-
 * absent): a conformer whose energy is NaN gets weight NaN and is EXCLUDED from
 * the normalization sum, so the finite weights still sum to 1. We never invent a
 * population for a conformer with no energy.
 *
 * Empty input → []. `tempK` ≤ 0 → throw (a temperature must be positive; don't
 * silently divide by zero or flip the sign of the exponent).
 */
export function boltzmannWeights(
  conformers: Conformer[],
  tempK = ROOM_TEMPERATURE_K,
): number[] {
  if (tempK <= 0) {
    throw new Error(`temperature must be positive, got ${tempK} K`);
  }
  if (conformers.length === 0) return [];
  const rt = GAS_CONSTANT_KCAL * tempK;
  const deltas = deltaEKcal(conformers);
  // exp(−ΔE/RT); NaN ΔE (no energy) → NaN, which drops out of the sum below.
  const boltz = deltas.map((d) => Math.exp(-d / rt));
  const z = boltz.reduce((acc, b) => (Number.isNaN(b) ? acc : acc + b), 0);
  return boltz.map((b) => b / z);
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

    const atoms: RawAtom[] = [];
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
  fragment: RawFragment,
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
  fragment: RawFragment,
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

/**
 * Is this ORCA input a GOAT (conformer-search) job? Looks **only at keyword
 * lines** — those starting with `!` — ignoring comments (`#`) and the geometry
 * block, and matches the `GOAT` token on word boundaries, case-insensitively.
 * So `! XTB GOAT` is GOAT, `! B3LYP def2-SVP Opt` is not, a `# GOAT ...` comment
 * is not, a keyword line with a **trailing** comment (`! XTB Opt # GOAT next
 * time`) is not, and an atom or fragment named "Goat…" in the `* xyz` block is
 * irrelevant. Drives the convergence panel's GOAT variant (2.5.2a §5): on a GOAT
 * run the per-cycle progress bar is one inner optimisation of one candidate, not
 * search progress, and must not be shown as if it were.
 */
export function isGoatInput(content: string): boolean {
  return content.split("\n").some((line) => {
    // Strip any trailing `#` comment first — `! XTB Opt # GOAT next time` is an
    // Opt job that merely mentions GOAT in a note, not a conformer search.
    const code = line.split("#")[0].trim();
    if (!code.startsWith("!")) return false;
    return /\bGOAT\b/i.test(code);
  });
}

/**
 * Whether a job status is a **terminal success** — the run finished OK and its
 * artifacts can be read. Both `completed` and `parsed` qualify: a GOAT job whose
 * single-structure `.property.txt` happened to parse reaches `parsed` (its
 * `.property.txt` has a first `$Geometry` ≈ the input and N opt cycles, so the
 * single-structure readers accept it), yet its authoritative result is still the
 * ensemble. The ensemble read must fire on ANY terminal success, never a single
 * narrow status — the regression was reading it only on exactly `completed`, so a
 * `parsed` GOAT job hid its ensemble. See `wiki/debugging/017-goat-parsed-hid-ensemble.md`.
 */
export function isTerminalSuccessStatus(status: string): boolean {
  return status === "completed" || status === "parsed";
}

/**
 * Whether to show the single-structure Results dashboard (`ResultsCard`) for a job.
 * A **GOAT** conformer search is a special job type whose authoritative result is the
 * ENSEMBLE, not a single-structure geometry — its `.property.txt`/`_trj.xyz` are the
 * internal optimization *cycles*, not conformers, and there is no meaningful single
 * "final" structure. So a GOAT job renders the ensemble panel and **suppresses** the
 * misleading "N optimization cycles" trajectory. Non-GOAT jobs are unaffected.
 */
export function showsSingleStructureResults(inputContent: string): boolean {
  return !isGoatInput(inputContent);
}

/** What "Use this conformer" should do, decided purely (so it's testable). */
export type ConformerApply =
  | { action: "replace"; fragmentId: string; atoms: RawAtom[] }
  | { action: "new"; fragment: SceneFragment; nextAtomId: number }
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
  snapshotFragment: RawFragment,
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
  // Fresh single-fragment scene: the conformer's raw atoms become SceneAtoms here
  // (the Scene boundary), ids minted from 0, so the caller sets `nextAtomId`.
  const { atoms, nextAtomId } = stampFreshIds(conformer.atoms, 0);
  return {
    action: "new",
    fragment: { ...snapshotFragment, atoms },
    nextAtomId,
  };
}
