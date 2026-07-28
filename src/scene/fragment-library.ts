/**
 * Curated library of reagent fragments — the nucleophiles/reagents a reaction
 * study actually starts from (ADR-008 #9). Pure/React-free.
 *
 * Every geometry is built from **ideal internal coordinates + symmetry** or an
 * explicit computation, never "recalled" numbers, and each entry carries a
 * non-empty `provenance` naming where its bond lengths / angles come from. The
 * `reference` internals are the contract: `fragment-library.test.ts` recomputes
 * them *from the coordinates* and fails if they disagree — so a mistyped
 * coordinate can't silently ship a wrong (but converging) geometry, which is the
 * worst class of bug here.
 *
 * We deliberately do NOT generate these at runtime with RDKit: MMFF silently
 * lacks parameters for exotic ions like BH₄⁻, and runtime generation is a moving
 * part for what is really a handful of fixed structures.
 */

import { makeFragmentId } from "./scene";
import type { SceneAtom, SceneFragment } from "./types";

export interface LibraryFragment {
  key: string; // "bh4-", "water"
  name: string; // "BH₄⁻"
  charge: number;
  atoms: SceneAtom[];
  /** Where this geometry came from — required, never empty. */
  provenance: string;
  /** Documented ideal internals the geometry must satisfy (see tests). */
  reference: {
    bonds?: { a: number; b: number; value: number }[];
    angles?: { a: number; b: number; c: number; value: number }[];
  };
}

// ── Geometry builders (ideal internal coordinates → SceneAtom[]) ──────────────

const DEG = Math.PI / 180;

/** A monatomic fragment at the origin. */
function monatomic(element: string): SceneAtom[] {
  return [{ element, x: 0, y: 0, z: 0 }];
}

/** A–B diatomic: A at the origin, B at (0, 0, r). */
function diatomic(a: string, b: string, r: number): SceneAtom[] {
  return [
    { element: a, x: 0, y: 0, z: 0 },
    { element: b, x: 0, y: 0, z: r },
  ];
}

/**
 * Tetrahedral XY₄: X at the origin, four Y along the four alternating cube
 * diagonals normalised to length `r`. T_d and all Y–X–Y angles = 109.47° follow
 * from the construction.
 */
function tetrahedral(x: string, y: string, r: number): SceneAtom[] {
  const dirs = [
    [1, 1, 1],
    [1, -1, -1],
    [-1, 1, -1],
    [-1, -1, 1],
  ];
  const s = r / Math.sqrt(3);
  return [
    { element: x, x: 0, y: 0, z: 0 },
    ...dirs.map(([dx, dy, dz]) => ({
      element: y,
      x: dx * s,
      y: dy * s,
      z: dz * s,
    })),
  ];
}

/**
 * Bent XY₂ (C2v): X at the origin, two Y in the xz-plane symmetric about +z,
 * bond length `r`, Y–X–Y = `angleDeg`. By construction the angle between the two
 * bond vectors is exactly `angleDeg`.
 */
function bent(x: string, y: string, r: number, angleDeg: number): SceneAtom[] {
  const half = (angleDeg * DEG) / 2;
  const sx = r * Math.sin(half);
  const cz = r * Math.cos(half);
  return [
    { element: x, x: 0, y: 0, z: 0 },
    { element: y, x: sx, y: 0, z: cz },
    { element: y, x: -sx, y: 0, z: cz },
  ];
}

/**
 * Trigonal-pyramidal XY₃ (C3v): X at the origin, three Y on a cone about +z at
 * azimuths 0/120/240°, bond length `r`, Y–X–Y = `angleDeg`. The polar angle β
 * comes from cos(Y–X–Y) = 1 − 1.5·sin²β (the C3v relation), so the pairwise
 * angle is exactly `angleDeg` by construction.
 */
function pyramidal(
  x: string,
  y: string,
  r: number,
  angleDeg: number,
): SceneAtom[] {
  const sin2b = (1 - Math.cos(angleDeg * DEG)) / 1.5;
  const sinb = Math.sqrt(sin2b);
  const cosb = Math.sqrt(1 - sin2b);
  const phis = [0, 120, 240].map((p) => p * DEG);
  return [
    { element: x, x: 0, y: 0, z: 0 },
    ...phis.map((phi) => ({
      element: y,
      x: r * sinb * Math.cos(phi),
      y: r * sinb * Math.sin(phi),
      z: r * cosb,
    })),
  ];
}

// ── The library ──────────────────────────────────────────────────────────────

export const FRAGMENT_LIBRARY: readonly LibraryFragment[] = [
  {
    key: "bh4-",
    name: "BH₄⁻",
    charge: -1,
    atoms: tetrahedral("B", "H", 1.24),
    provenance:
      "Ideal T_d symmetry; B–H 1.24 Å (NaBH₄ crystallographic value, cf. ADR-008).",
    reference: {
      bonds: [
        { a: 0, b: 1, value: 1.24 },
        { a: 0, b: 2, value: 1.24 },
        { a: 0, b: 3, value: 1.24 },
        { a: 0, b: 4, value: 1.24 },
      ],
      angles: [
        { a: 1, b: 0, c: 2, value: 109.47 },
        { a: 3, b: 0, c: 4, value: 109.47 },
      ],
    },
  },
  {
    key: "h-",
    name: "H⁻",
    charge: -1,
    atoms: monatomic("H"),
    provenance: "Monatomic hydride ion — no internal geometry.",
    reference: {},
  },
  {
    key: "oh-",
    name: "OH⁻",
    charge: -1,
    atoms: diatomic("O", "H", 0.964),
    provenance:
      "Diatomic; O–H 0.964 Å (gas-phase hydroxide equilibrium bond length, spectroscopic literature).",
    reference: { bonds: [{ a: 0, b: 1, value: 0.964 }] },
  },
  {
    key: "cn-",
    name: "CN⁻",
    charge: -1,
    atoms: diatomic("C", "N", 1.16),
    provenance:
      "Diatomic; C≡N 1.16 Å (cyanide ion reference bond length).",
    reference: { bonds: [{ a: 0, b: 1, value: 1.16 }] },
  },
  {
    key: "cl-",
    name: "Cl⁻",
    charge: -1,
    atoms: monatomic("Cl"),
    provenance: "Monatomic chloride ion — no internal geometry.",
    reference: {},
  },
  {
    key: "water",
    name: "H₂O",
    charge: 0,
    atoms: bent("O", "H", 0.9572, 104.52),
    provenance:
      "Ideal C2v; O–H 0.9572 Å, H–O–H 104.52° (experimental gas-phase water reference structure).",
    reference: {
      bonds: [
        { a: 0, b: 1, value: 0.9572 },
        { a: 0, b: 2, value: 0.9572 },
      ],
      angles: [{ a: 1, b: 0, c: 2, value: 104.52 }],
    },
  },
  {
    key: "nh3",
    name: "NH₃",
    charge: 0,
    atoms: pyramidal("N", "H", 1.012, 106.67),
    provenance:
      "Ideal C3v; N–H 1.012 Å, H–N–H 106.67° (experimental gas-phase ammonia reference structure).",
    reference: {
      bonds: [
        { a: 0, b: 1, value: 1.012 },
        { a: 0, b: 2, value: 1.012 },
        { a: 0, b: 3, value: 1.012 },
      ],
      angles: [
        { a: 1, b: 0, c: 2, value: 106.67 },
        { a: 2, b: 0, c: 3, value: 106.67 },
      ],
    },
  },
  {
    key: "methanol",
    name: "CH₃OH",
    charge: 0,
    // Optimised geometry — not hand-built (a Z-matrix is too error-prone here).
    atoms: [
      { element: "C", x: -0.00498544, y: 0.00566558, z: 0.0 },
      { element: "O", x: 1.42528295, y: -0.0081438, z: 0.0 },
      { element: "H", x: 1.74116749, y: 0.89810407, z: 0.0 },
      { element: "H", x: -0.37896793, y: -0.51448489, z: 0.89069343 },
      { element: "H", x: -0.37896793, y: -0.51448489, z: -0.89069343 },
      { element: "H", x: -0.41352915, y: 1.02334393, z: 0.0 },
    ],
    provenance:
      "r²SCAN-3c Opt (ORCA 6, 2026-07-28, isolated job dir, cleaned up); " +
      "C–O 1.4303 Å, O–H 0.9597 Å, C–O–H 108.66°.",
    reference: {
      bonds: [
        { a: 0, b: 1, value: 1.4303 }, // C–O
        { a: 1, b: 2, value: 0.9597 }, // O–H
        { a: 0, b: 3, value: 1.0972 }, // C–H
      ],
      angles: [{ a: 0, b: 1, c: 2, value: 108.66 }], // C–O–H
    },
  },
];

/**
 * Instantiate a library entry as a fresh scene fragment: a new id each call,
 * deep-copied atoms (so the caller can move it without touching the library),
 * `source: "fragment-library"` and `sourceLabel` = the library key.
 */
export function libraryFragmentToScene(f: LibraryFragment): SceneFragment {
  return {
    id: makeFragmentId(),
    name: f.name,
    atoms: f.atoms.map((a) => ({ ...a })),
    charge: f.charge,
    source: "fragment-library",
    sourceLabel: f.key,
  };
}
