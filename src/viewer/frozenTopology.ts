//! Frozen-topology animation helpers — pure, node-tested, 3Dmol-free (so vitest can
//! import them; the 3Dmol bundle needs WebGL and does not load in jsdom).
//!
//! Unit 3.14. When animating a normal mode the molecule's bond graph must stay fixed
//! (a vibration is the same molecule), but 3Dmol perceives bonds from each frame's
//! distances. The fix (`MoleculeViewer`) is to build the model ONCE from the
//! equilibrium — 3Dmol perceives bonds and assigns `atom.index` normally — and then
//! only **update coordinates** per frame. These helpers are the coordinate-update
//! and the drawability check that the update must preserve.
//!
//! # Why the 3.13 attempt drew nothing (measured from the 3Dmol bundle, rule #10)
//! The 3.13 fix parsed each frame with `assignBonds:false` and set the bonds by hand.
//! The bonds reached the live atoms — but `drawBondSticks` (GLModel.ts) draws a bond
//! only when `atom.index < atom2.index` (its dedup gate, so each bond is drawn once).
//! `assignBonds:false` leaves `atom.index` **unset** (the XYZ parser sets `serial`,
//! only `assignBonds` assigns `index`), so `undefined < undefined` is `false` for
//! EVERY bond → zero sticks. `drawableBondCount` below mirrors that exact gate.

/** The 0-based coordinates of an xyz string (skips the count + comment lines). */
export function parseXyzCoords(xyz: string): Array<[number, number, number]> {
  const lines = xyz.trimStart().split(/\r?\n/);
  const n = Number.parseInt(lines[0], 10);
  const out: Array<[number, number, number]> = [];
  if (!Number.isFinite(n)) return out;
  for (let i = 0; i < n && i + 2 < lines.length; i++) {
    const t = lines[i + 2].trim().split(/\s+/);
    out.push([Number.parseFloat(t[1]), Number.parseFloat(t[2]), Number.parseFloat(t[3])]);
  }
  return out;
}

/** Minimal shape of a 3Dmol atom this module touches (it mutates the LIVE atom
 * objects `model.selectedAtoms({})` returns, never copies — so bonds/index survive). */
export interface CoordAtom {
  x: number;
  y: number;
  z: number;
}

/**
 * Move existing atoms to new coordinates **in place**, changing only x/y/z. Every
 * other property — crucially `bonds`, `bondOrder` and `index` — is left untouched, so
 * the topology and 3Dmol's draw gate survive the frame. Updates `min(atoms, coords)`
 * (a count mismatch shouldn't happen; it is bounded rather than throwing mid-render).
 */
export function applyCoordsToAtoms(
  atoms: CoordAtom[],
  coords: Array<[number, number, number]>,
): void {
  const n = Math.min(atoms.length, coords.length);
  for (let i = 0; i < n; i++) {
    atoms[i].x = coords[i][0];
    atoms[i].y = coords[i][1];
    atoms[i].z = coords[i][2];
  }
}

/** An atom as far as bond DRAWING is concerned. */
export interface BondAtom {
  index?: number;
  bonds: number[];
}

/**
 * The number of bonds 3Dmol would actually DRAW — a faithful mirror of its stick
 * gate `atom.index < atoms[j].index` (each bond drawn once, from the lower index).
 * With `atom.index` unset this is 0 for any topology (the 3.13 regression); with the
 * indices 3Dmol assigns on a normal parse it equals the number of unique bonds.
 * Used by a test (and a DEV assertion in the viewer) to check the OUTPUT — that
 * sticks will render — not just that we stored a bond list.
 */
export function drawableBondCount(atoms: BondAtom[]): number {
  let count = 0;
  for (let i = 0; i < atoms.length; i++) {
    const a = atoms[i];
    if (!a || !a.bonds) continue;
    for (const j of a.bonds) {
      const b = atoms[j];
      if (b && (a.index as number) < (b.index as number)) count++;
    }
  }
  return count;
}
