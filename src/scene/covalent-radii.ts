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
//!
//! DOUBLE / TRIPLE radii (Pyykkö & Atsumi, Chem. Eur. J. 2009) are added for the
//! elements that form well-defined multiple bonds (`order` 2/3). These feed the
//! GEOMETRIC bond-order target (a double bond = a shorter set-distance) and the
//! bond-order ESTIMATE (nearest of the single/double/triple sums to a measured
//! length) — NOT bond perception, which stays Cordero-single × 1.2 (unchanged).
//! There is no second radius source: this ONE table carries all three orders, and
//! a missing (element, order) is a loud throw, never a guessed radius (rule #11).
//! Order-sum spot-checks: C=C 1.34, C≡C 1.20, C≡N 1.14, C=O 1.24 (Å).

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

/** Pyykkö & Atsumi 2009 DOUBLE-bond covalent radius (Å), by element symbol. Only
 * the elements that form well-defined double bonds — a missing one is a loud throw
 * (the estimator then simply doesn't offer order 2 for that pair). */
const DOUBLE_RADIUS_ANGSTROM: Readonly<Record<string, number>> = {
  B: 0.78, C: 0.67, N: 0.6, O: 0.57,
  Si: 1.07, P: 1.02, S: 0.94,
};

/** Pyykkö & Atsumi 2009 TRIPLE-bond covalent radius (Å). Same limited, cited set.
 * (Sulfur's triple 0.95 > its double 0.94 is the published value, kept as-is.) */
const TRIPLE_RADIUS_ANGSTROM: Readonly<Record<string, number>> = {
  B: 0.73, C: 0.6, N: 0.54, O: 0.53,
  Si: 1.02, P: 0.94, S: 0.95,
};

/** A bond order the radius table understands: single (Cordero), double / triple
 * (Pyykkö). Single is the perception basis; 2/3 are the geometric-order targets. */
export type BondOrder = 1 | 2 | 3;

/** Canonicalise an element symbol to `Xx` form so lookup is case-insensitive
 * (mirrors `bond-display.ts` — element symbols differ in casing across sources). */
function canonicalElement(el: string): string {
  const s = el.trim();
  return s.length === 0 ? s : s[0].toUpperCase() + s.slice(1).toLowerCase();
}

/**
 * The covalent radius (Å) for an element at a given bond `order` — single =
 * Cordero 2008 (default, the perception basis), double/triple = Pyykkö 2009.
 * Throws on an unknown symbol OR an element with no radius at that order — a
 * missing radius is a loud failure, never a guessed default, so neither a
 * form/break target nor a bond-order estimate is ever built on a fabricated
 * radius (rule #11).
 */
export function covalentRadius(element: string, order: BondOrder = 1): number {
  const el = canonicalElement(element);
  const table =
    order === 1
      ? COVALENT_RADIUS_ANGSTROM
      : order === 2
        ? DOUBLE_RADIUS_ANGSTROM
        : TRIPLE_RADIUS_ANGSTROM;
  const r = table[el];
  if (r === undefined) {
    const kind = order === 1 ? "covalent" : order === 2 ? "double-bond" : "triple-bond";
    throw new Error(`covalentRadius: no ${kind} covalent radius for element "${element}"`);
  }
  return r;
}
