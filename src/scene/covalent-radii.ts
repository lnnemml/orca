//! Covalent radii (Cordero et al., Dalton Trans. 2008, single-bond, Å) — pure,
//! node-tested, React-free. The SAME basis distance-based bond perception uses:
//! two atoms are perceived bonded when their separation is below (rA + rB) × a
//! multiplier (the sidecar uses 1.2, `wiki/modules/sidecar.md`). Bond-editing
//! (`bond-edit.ts`) reuses this table so a "form bond" target lands INSIDE that
//! perception window and a "break bond" target clears it, by construction — no
//! second radius source, no stored connectivity.
//!
//! Values are the Cordero 2008 single-bond covalent radii. Spot-checks against the
//! sums bond-editing needs: C(0.76)+H(0.31)=1.07, C+C=1.52, C(0.76)+N(0.71)=1.47,
//! C+O(0.66)=1.42, C+I(1.39)=2.15 — the C–H…C–I ladder the tests assert.

/** Cordero 2008 single-bond covalent radius (Å), by element symbol. Covers the
 * main group + the transition metals this app targets (organometallics, ADR-007).
 * Not exhaustive: an element absent here is a loud failure, not a guessed radius
 * (rule #11 — a fabricated radius would emit a plausible-but-wrong bond distance). */
const COVALENT_RADIUS_ANGSTROM: Readonly<Record<string, number>> = {
  H: 0.31, He: 0.28,
  Li: 1.28, Be: 0.96, B: 0.84, C: 0.76, N: 0.71, O: 0.66, F: 0.57, Ne: 0.58,
  Na: 1.66, Mg: 1.41, Al: 1.21, Si: 1.11, P: 1.07, S: 1.05, Cl: 1.02, Ar: 1.06,
  K: 2.03, Ca: 1.76, Sc: 1.7, Ti: 1.6, V: 1.53, Cr: 1.39, Mn: 1.39, Fe: 1.32,
  Co: 1.26, Ni: 1.24, Cu: 1.32, Zn: 1.22, Ga: 1.22, Ge: 1.2, As: 1.19, Se: 1.2,
  Br: 1.2, Kr: 1.16,
  Rb: 2.2, Sr: 1.95, Y: 1.9, Zr: 1.75, Nb: 1.64, Mo: 1.54, Ru: 1.46, Rh: 1.42,
  Pd: 1.39, Ag: 1.45, Cd: 1.44, In: 1.42, Sn: 1.39, Sb: 1.39, Te: 1.38, I: 1.39,
  Xe: 1.4,
  Cs: 2.44, Ba: 2.15, La: 2.07, Hf: 1.75, Ta: 1.7, W: 1.62, Re: 1.51, Os: 1.44,
  Ir: 1.41, Pt: 1.36, Au: 1.36, Hg: 1.32, Tl: 1.45, Pb: 1.46, Bi: 1.48,
};

/** Canonicalise an element symbol to `Xx` form so lookup is case-insensitive
 * (mirrors `bond-display.ts` — element symbols differ in casing across sources). */
function canonicalElement(el: string): string {
  const s = el.trim();
  return s.length === 0 ? s : s[0].toUpperCase() + s.slice(1).toLowerCase();
}

/**
 * The Cordero single-bond covalent radius (Å) for an element. Throws on an unknown
 * symbol — a missing radius is a loud failure, never a guessed default, so a
 * form/break can never emit a fabricated bond distance (rule #11).
 */
export function covalentRadius(element: string): number {
  const r = COVALENT_RADIUS_ANGSTROM[canonicalElement(element)];
  if (r === undefined) {
    throw new Error(`covalentRadius: no covalent radius for element "${element}"`);
  }
  return r;
}
