//! Orbital-picker rows — pure, node-tested, React-free.
//!
//! The MO energies + occupancies are already parsed and stored (`orca_2json`,
//! `data_json.orbitals`): `orbitals[i] = [energyEh, occupancy]`, index i = the MO
//! number (0-based, ascending energy). This turns that into rows the picker draws,
//! marking **HOMO** (the highest occupied), **LUMO** (the first virtual), and the
//! **core** 1s-type orbitals — the deep, occluded ones whose empty picture confused
//! the author (a core 1s sits inside the atom's own sphere; MO 0 looked blank).
//!
//! **Core is DERIVED, not read** (the same discipline as T·S and the display scale):
//! it is computed from a small per-element table AND cross-checked against the big
//! core→valence energy gap; when the two disagree, or an element is outside the
//! table, NO core mark is placed (and the disagreement is reported), rather than a
//! plausible-but-wrong label. See `coreOrbitals`.

export type OrbitalKind = "HOMO" | "LUMO" | "core" | "occupied" | "virtual";

export interface OrbitalRow {
  /** MO number = index into the stored array = the number `orca_plot` wants. */
  index: number;
  energyEh: number;
  occupancy: number;
  kind: OrbitalKind;
}

/** Occupied iff occupancy is meaningfully non-zero. Threshold 0.5 cleanly separates
 * occupied (2.0 closed shell / 1.0 open shell) from virtual (0.0) — measured MOs are
 * exactly 2.0 or 0.0, so this is not a knife-edge. */
const OCCUPIED_THRESHOLD = 0.5;

/**
 * Number of **core** (1s-type, deeply bound) orbitals contributed by each element —
 * limited to what a full-electron basis makes defensible, and to the periods whose
 * core is unambiguous:
 *   - H, He → 0 (their 1s IS the valence);
 *   - Li…Ne → 1 (1s core);
 *   - Na…Ar → 5 (1s + 2s + 2p core).
 * Anything else (K onward, transition metals, or ECPs where core orbitals are absent
 * from the basis) is **not** in the table → no core marking at all. This is NOT
 * derived from "one 1s per heavy atom" — that rule holds only for the 2nd period.
 */
const CORE_ORBITALS: Record<string, number> = {
  H: 0, He: 0,
  Li: 1, Be: 1, B: 1, C: 1, N: 1, O: 1, F: 1, Ne: 1,
  Na: 5, Mg: 5, Al: 5, Si: 5, P: 5, S: 5, Cl: 5, Ar: 5,
};

/** Expected core-orbital count from the per-element table, or `null` if any element
 * is outside the table (then we place no core mark — we cannot justify a count). */
export function expectedCoreCount(elements: readonly string[]): number | null {
  let sum = 0;
  for (const el of elements) {
    const c = CORE_ORBITALS[el];
    if (c == null) return null;
    sum += c;
  }
  return sum;
}

export interface CoreOrbitalInfo {
  /** The number of orbitals to MARK as core — `null` when we will not mark any. */
  count: number | null;
  /** What the per-element table predicted (`null` = an element outside the table). */
  expectedFromFormula: number | null;
  /** Where the largest low-energy gap actually falls (orbital count before it), for
   * the report. `null` when there was nothing to check. */
  gapAt: number | null;
  /** A one-line reason the UI shows — always naming that the mark is DERIVED. */
  note: string;
}

/**
 * Decide the core-orbital count, DERIVED and cross-checked (never read from the file):
 *
 *  1. the per-element table predicts `expected` (or gives up → no mark);
 *  2. the physics says core orbitals sit far below valence — so the **largest energy
 *     gap in the low part of the spectrum** should fall right after the last core
 *     orbital. We find that gap's position and mark core **only if it equals
 *     `expected`**. A disagreement means our simple table is wrong for this system
 *     (heavy atom, ECP, near-degeneracy) → no mark, and the mismatch is reported.
 *
 * Measured (dexketoprofen C₁₆H₁₄O₃): expected 16·1 + 14·0 + 3·1 = 19; the −10.03→−1.08 Eh
 * gap falls after orbital 19 → they agree → MOs 0–18 marked core.
 */
export function coreOrbitals(
  orbitals: ReadonlyArray<readonly [number, number]>,
  elements: readonly string[],
): CoreOrbitalInfo {
  const expected = expectedCoreCount(elements);
  if (expected == null) {
    return {
      count: null,
      expectedFromFormula: null,
      gapAt: null,
      note: "core mark omitted — an element is outside the H–Ar core table (derived, not read)",
    };
  }
  if (expected === 0) {
    return { count: 0, expectedFromFormula: 0, gapAt: null, note: "no core orbitals (H/He only)" };
  }
  // Need at least `expected` + 1 orbitals to see the core→valence gap.
  if (orbitals.length < expected + 1) {
    return {
      count: null,
      expectedFromFormula: expected,
      gapAt: null,
      note: `core mark omitted — too few orbitals to confirm the gap (expected ${expected})`,
    };
  }
  // Largest consecutive-energy gap in the lower region (search a window comfortably
  // past `expected`, staying within the occupied+low-virtual orbitals).
  const window = Math.min(orbitals.length - 1, expected * 2 + 6);
  let bestGap = -Infinity;
  let bestBoundary = 0; // orbitals before the gap
  for (let i = 0; i < window; i++) {
    const gap = orbitals[i + 1][0] - orbitals[i][0];
    if (gap > bestGap) {
      bestGap = gap;
      bestBoundary = i + 1;
    }
  }
  if (bestBoundary === expected) {
    return {
      count: expected,
      expectedFromFormula: expected,
      gapAt: bestBoundary,
      note: `${expected} core orbitals (derived from the formula, confirmed by the core→valence gap)`,
    };
  }
  return {
    count: null,
    expectedFromFormula: expected,
    gapAt: bestBoundary,
    note: `core mark omitted — table expects ${expected} but the largest low gap is after ${bestBoundary} (disagreement reported, not guessed)`,
  };
}

/**
 * The HOMO index — the highest occupied MO — or `null` if none is occupied. This is the
 * default the picker selects and the seam that decides HOMO/LUMO marking.
 */
export function homoIndex(orbitals: ReadonlyArray<readonly [number, number]>): number | null {
  let homo: number | null = null;
  for (let i = 0; i < orbitals.length; i++) {
    if (orbitals[i][1] > OCCUPIED_THRESHOLD) homo = i;
  }
  return homo;
}

/**
 * Rows for the picker, each marked. Exactly one HOMO and (if it exists) one LUMO — the
 * LUMO is the FIRST virtual after the HOMO. The lowest `coreCount` occupied orbitals are
 * marked **core** (derived — pass `elements` to enable it; omit to disable). Precedence:
 * HOMO/LUMO first (specific), then core, then occupied/virtual by occupancy.
 */
export function orbitalRows(
  orbitals: ReadonlyArray<readonly [number, number]>,
  elements?: readonly string[],
): OrbitalRow[] {
  const homo = homoIndex(orbitals);
  const lumo = homo != null && homo + 1 < orbitals.length ? homo + 1 : null;
  const core = elements ? coreOrbitals(orbitals, elements).count : null;
  return orbitals.map(([energyEh, occupancy], index) => {
    let kind: OrbitalKind;
    if (index === homo) kind = "HOMO";
    else if (index === lumo) kind = "LUMO";
    else if (core != null && index < core) kind = "core";
    else kind = occupancy > OCCUPIED_THRESHOLD ? "occupied" : "virtual";
    return { index, energyEh, occupancy, kind };
  });
}

/** The MO the picker opens on: the HOMO, or 0 if nothing is occupied (defensive). */
export function defaultOrbital(orbitals: ReadonlyArray<readonly [number, number]>): number {
  return homoIndex(orbitals) ?? 0;
}
