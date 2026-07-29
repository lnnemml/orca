/**
 * Geometry measurement off the pick list (2.5.2b). Pure / node-tested,
 * React-free (ADR-008 decision 10) — no imports from react / 3dmol / tauri. The
 * pick list is an ordered list of global atom indices (`selection.ts`); this
 * module reads it **positionally**: 2 atoms → distance, 3 → angle with the
 * MIDDLE pick as the vertex, 4 → dihedral along the chain i-j-k-l.
 *
 * ## Why the conventions are pinned to ASE, verified against source
 *
 * These numbers are not just a readout. In 2.5.2c the acceptance test applies a
 * target d/θ/φ through ASE, reads the coordinates back, and RE-derives all three
 * with THIS module to check them. If our dihedral convention diverged from ASE's,
 * that test would pass a wrong core or fail a right one. So the conventions are
 * fixed here from the real ASE source shipped in the sidecar venv
 * (`ase/geometry/geometry.py`, `ase/atoms.py`; ASE checked 2026-07-29), not from
 * memory:
 *
 * - **Angle vertex = the middle index.** `Atoms.get_angle(a1, a2, a3)` computes
 *   the angle between `a1-a2` and `a3-a2` — `a2` (the middle) is the vertex
 *   (`atoms.py::get_angles`: `v12 = a1s - a2s`, `v32 = a3s - a2s`). Our order
 *   `(i, vertex, j)` maps positionally: `a1=i`, `a2=vertex`, `a3=j`. Range
 *   `[0, 180]` (`arccos`).
 *
 * - **Dihedral range `[0, 360)`, NOT `(-180, 180]`.** `get_dihedrals`
 *   (`geometry.py`) builds `v0=a1-a0`, `v1=a2-a1`, `v2=a3-a2`, projects `-v0`
 *   and `v2` onto the plane ⊥ `v1` (call them `v`, `w`), then
 *   `atan2((v1n×v)·w, v·w)` — and crucially the next line
 *   `dihedrals[dihedrals < 0.] += 2*pi` folds the `[-pi, pi]` result into
 *   `[0, 2pi)` before converting to degrees. So ASE returns `[0, 360)`. We
 *   replicate that fold verbatim. Verified numerically against ASE on the butane
 *   ensemble fixture: `get_dihedral(0,1,2,3)` gives 179.998° (anti, conf 0) and
 *   67.523° (gauche, conf 1) — the gauche value lands on the **60 side**, not
 *   300, which is exactly what the `[0, 360)` fold with these vector directions
 *   produces. If 2.5.2c's ASE call and this function ever disagree on that fold,
 *   the gauche test here is what catches it.
 *
 * - **Reversal invariance.** `dihedral(i,j,k,l) === dihedral(l,k,j,i)` — the four
 *   points read backwards give the same angle (ASE-confirmed: 179.998 both ways).
 *   Reflecting the scene through one axis (improper rotation) sends `φ → 360 − φ`.
 *   Distance and angle are invariant under reflection; only the dihedral's
 *   handedness flips.
 *
 * Degenerate inputs return **null, never NaN** (coincident atoms, a zero vector,
 * collinearity for the dihedral). `measureSelection` maps any null to
 * `{ kind: "none" }`. Indices out of range → null (same non-throwing contract as
 * `locateAtom`).
 */

import type { Scene } from "./types";
import { locateAtom } from "./scene";

type Vec3 = [number, number, number];

const RAD_TO_DEG = 180 / Math.PI;

// Collinearity threshold for the dihedral, applied to the NORMALISED
// cross-product magnitude sin(θ) (dimensionless, scale-free) — a cross-product
// criterion, not an angle one: near θ=0/180 `sin θ` is well-conditioned where
// `acos` is not. Below this the inner angle is effectively planar and the
// dihedral is undefined.
const COLLINEAR_SIN = 1e-9;

/** Coordinates of the atom at a global index, or null if out of range. Routed
 * through `locateAtom` so this shares the one index→atom contract. */
function positionAt(scene: Scene, globalIndex: number): Vec3 | null {
  const located = locateAtom(scene, globalIndex);
  if (!located) return null;
  const a = located.fragment.atoms[located.localIndex];
  return [a.x, a.y, a.z];
}

const sub = (a: Vec3, b: Vec3): Vec3 => [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
const dot = (a: Vec3, b: Vec3): number => a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
const cross = (a: Vec3, b: Vec3): Vec3 => [
  a[1] * b[2] - a[2] * b[1],
  a[2] * b[0] - a[0] * b[2],
  a[0] * b[1] - a[1] * b[0],
];
const norm = (a: Vec3): number => Math.sqrt(dot(a, a));
const scale = (a: Vec3, s: number): Vec3 => [a[0] * s, a[1] * s, a[2] * s];

/** True when `a` and `b` are (anti)parallel to within {@link COLLINEAR_SIN},
 * measured by the normalised cross-product magnitude (== |sin θ|). */
function collinear(a: Vec3, b: Vec3): boolean {
  const na = norm(a);
  const nb = norm(b);
  if (na === 0 || nb === 0) return true;
  return norm(cross(a, b)) / (na * nb) < COLLINEAR_SIN;
}

/**
 * Bond length `i–j` in Å, or null if either index is out of range or the two
 * atoms coincide (same index, or identical coordinates → a zero vector).
 */
export function distance(scene: Scene, i: number, j: number): number | null {
  const p = positionAt(scene, i);
  const q = positionAt(scene, j);
  if (!p || !q) return null;
  const d = norm(sub(p, q));
  return d < 1e-12 ? null : d;
}

/**
 * Angle `i–vertex–j` in degrees `[0, 180]`, the MIDDLE argument being the
 * vertex (ASE `get_angle(a1, a2, a3)` convention: vertex `a2`). Null on an
 * out-of-range index, a repeated index, or a zero bond vector.
 */
export function angle(
  scene: Scene,
  i: number,
  vertex: number,
  j: number,
): number | null {
  if (new Set([i, vertex, j]).size < 3) return null;
  const a = positionAt(scene, i);
  const v = positionAt(scene, vertex);
  const b = positionAt(scene, j);
  if (!a || !v || !b) return null;
  const u = sub(a, v); // vertex → i  (ASE v12 = a1 - a2)
  const w = sub(b, v); // vertex → j  (ASE v32 = a3 - a2)
  const nu = norm(u);
  const nw = norm(w);
  if (nu === 0 || nw === 0) return null;
  const cosine = Math.min(1, Math.max(-1, dot(u, w) / (nu * nw)));
  return Math.acos(cosine) * RAD_TO_DEG;
}

/**
 * Dihedral `i–j–k–l` in degrees `[0, 360)`, ASE `get_dihedral(a0,a1,a2,a3)`
 * convention exactly (see the module header for the verified derivation). The
 * axis is `j–k`. Null on an out-of-range index, a repeated index, or a collinear
 * inner angle (either end triplet planar → dihedral undefined).
 */
export function dihedral(
  scene: Scene,
  i: number,
  j: number,
  k: number,
  l: number,
): number | null {
  if (new Set([i, j, k, l]).size < 4) return null;
  const p0 = positionAt(scene, i);
  const p1 = positionAt(scene, j);
  const p2 = positionAt(scene, k);
  const p3 = positionAt(scene, l);
  if (!p0 || !p1 || !p2 || !p3) return null;

  const v0 = sub(p1, p0); // a0 → a1
  const v1 = sub(p2, p1); // a1 → a2  (the axis)
  const v2 = sub(p3, p2); // a2 → a3
  const n1 = norm(v1);
  if (n1 === 0) return null;
  // Undefined when a0-a1-a2 or a1-a2-a3 is collinear (ASE raises here).
  if (collinear(v0, v1) || collinear(v2, v1)) return null;

  const v1n = scale(v1, 1 / n1);
  // v, w: projections of -v0 and v2 onto the plane ⊥ v1 (ASE's `v`, `w`).
  const negV0 = scale(v0, -1);
  const v = sub(negV0, scale(v1n, dot(negV0, v1n)));
  const w = sub(v2, scale(v1n, dot(v2, v1n)));

  const x = dot(v, w);
  const y = dot(cross(v1n, v), w);
  let deg = Math.atan2(y, x) * RAD_TO_DEG; // [-180, 180]
  if (deg < 0) deg += 360; // fold to [0, 360) — the ASE convention
  return deg;
}

/** The result of measuring the current pick list. `sameFragment` distinguishes
 * an internal-geometry read from an inter-fragment one (a future reaction
 * coordinate). */
export type Measurement =
  | { kind: "none" }
  | {
      kind: "distance";
      value: number;
      unit: "Å";
      atoms: number[];
      sameFragment: boolean;
    }
  | {
      kind: "angle";
      value: number;
      unit: "°";
      atoms: number[];
      sameFragment: boolean;
    }
  | {
      kind: "dihedral";
      value: number;
      unit: "°";
      atoms: number[];
      sameFragment: boolean;
    };

/** Do all picked indices resolve and share one fragment? False if any is
 * out of range. Used to flag inter-fragment measurements. */
function allSameFragment(scene: Scene, atoms: number[]): boolean {
  const ids = atoms.map((gi) => locateAtom(scene, gi)?.fragment.id);
  if (ids.some((id) => id == null)) return false;
  return ids.every((id) => id === ids[0]);
}

/**
 * Interpret the pick list positionally: 2 → distance, 3 → angle (middle =
 * vertex), 4 → dihedral (chain i-j-k-l). 0/1 atoms, or any degenerate value,
 * → `{ kind: "none" }` (the panel then shows only the atom description).
 */
export function measureSelection(scene: Scene, selection: number[]): Measurement {
  const atoms = [...selection];
  const sameFragment = allSameFragment(scene, atoms);

  if (atoms.length === 2) {
    const value = distance(scene, atoms[0], atoms[1]);
    if (value == null) return { kind: "none" };
    return { kind: "distance", value, unit: "Å", atoms, sameFragment };
  }
  if (atoms.length === 3) {
    const value = angle(scene, atoms[0], atoms[1], atoms[2]);
    if (value == null) return { kind: "none" };
    return { kind: "angle", value, unit: "°", atoms, sameFragment };
  }
  if (atoms.length === 4) {
    const value = dihedral(scene, atoms[0], atoms[1], atoms[2], atoms[3]);
    if (value == null) return { kind: "none" };
    return { kind: "dihedral", value, unit: "°", atoms, sameFragment };
  }
  return { kind: "none" };
}

/**
 * A compact `value + unit` string for a measurement (viewer label + inspector):
 * distance to 3 decimals in Å, angles/dihedrals to 1 decimal in degrees. Null
 * for `kind: "none"`.
 */
export function formatMeasurementValue(m: Measurement): string | null {
  switch (m.kind) {
    case "distance":
      return `${m.value.toFixed(3)} ${m.unit}`;
    case "angle":
    case "dihedral":
      return `${m.value.toFixed(1)}${m.unit}`;
    default:
      return null;
  }
}
