//! Orbital-picker rows — pure, node-tested, React-free.
//!
//! The MO energies + occupancies are already parsed and stored (`orca_2json`,
//! `data_json.orbitals`): `orbitals[i] = [energyEh, occupancy]`, index i = the MO
//! number (0-based, ascending energy). This turns that into rows the picker draws,
//! marking **HOMO** (the highest occupied) and **LUMO** (the first virtual) — the
//! teaching pair, not just two more rows. Nothing here re-parses a file.

export type OrbitalKind = "HOMO" | "LUMO" | "occupied" | "virtual";

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
 * LUMO is the FIRST virtual after the HOMO, not merely the next index, so a gap in
 * occupancy can't mislabel it. Ascending by MO number, as stored.
 */
export function orbitalRows(
  orbitals: ReadonlyArray<readonly [number, number]>,
): OrbitalRow[] {
  const homo = homoIndex(orbitals);
  const lumo = homo != null && homo + 1 < orbitals.length ? homo + 1 : null;
  return orbitals.map(([energyEh, occupancy], index) => {
    let kind: OrbitalKind;
    if (index === homo) kind = "HOMO";
    else if (index === lumo) kind = "LUMO";
    else kind = occupancy > OCCUPIED_THRESHOLD ? "occupied" : "virtual";
    return { index, energyEh, occupancy, kind };
  });
}

/** The MO the picker opens on: the HOMO, or 0 if nothing is occupied (defensive). */
export function defaultOrbital(orbitals: ReadonlyArray<readonly [number, number]>): number {
  return homoIndex(orbitals) ?? 0;
}
