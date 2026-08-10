//! Normal-mode animation geometry — pure, node-tested, React-free (the same
//! discipline as `trajectory/frame.ts`, whose ownership rule this mirrors).
//!
//! A normal mode is a set of per-atom Cartesian displacement vectors; animating it
//! is `x(t) = x_eq + A · sin(2πt) · v̂`, one period back and forth, looped. All the
//! math lives here as named, checkable functions; the component owns only the phase,
//! amplitude, and timer (ADR-011 — the viewer gets ONE frame, never a timer).
//!
//! # Two facts this module rests on, both MEASURED (`wiki/orca/parse-sources.md`)
//!  * **The mode vectors are taken AS-IS — no rotation.** The gate (unit 3.12,
//!    `probes/hess_frame_kabsch.py`) proved the `.hess $atoms` frame is the
//!    reference geometry plus a **pure translation** on all three jobs.
//!  * **The modes are Cartesian, unit-normalized over all 3N components** (unit-3.6
//!    gate — no ÷√m). That last fact is exactly the amplitude trap below.
//!
//! # Amplitude = MAXIMUM ATOMIC DISPLACEMENT in Å (unit 3.13)
//! The mode is normalized over all **3N** components, so its unit "length" is split
//! across every moving atom. In a delocalized mode (a bend over 33 atoms) each atom
//! gets a crumb; in a localized one (a C=O or C–H stretch) almost the whole unit
//! sits on two atoms. The SAME scalar multiplier therefore produces wildly different
//! physical motion — which is why the earlier `A × v` normalization was "fine for
//! bends, collapsing for stretches" (measured: mode #84 C=O drove its bond to
//! **0.63 Å**). The fix: **A is the maximum atomic displacement, in Å** —
//!
//!     disp_i(phase) = A · sin(2π·phase) · v_i / max_j |v_j|
//!
//! where `max_j |v_j|` is the largest **atomic tri-vector norm** (NOT the largest
//! single component — those differ by up to √3, a silent error; `maxAtomicNorm`).
//! Now every mode — localized or delocalized — moves its busiest atom by exactly A,
//! so all 99 modes are comparably visible and no bond is over-driven. The default is
//! **measured**, not the old `orca_pltvib` 2.0 (which was a NORM multiplier, a
//! different quantity — see `parse-sources.md`).

import { frameToXyz } from "../trajectory/frame";

/**
 * Default max atomic displacement, Å. **Measured** (`probes/mode_amplitude.py`): an
 * exaggeration for visibility (real thermal amplitudes are ~0.04–0.07 Å), bounded by
 * the worst localized stretch keeping its bonds intact — mode #84's C=O (eq 1.213 Å)
 * closest approach vs A: 0.25→0.808, 0.20→0.889, **0.18→0.921**, so 0.18 keeps every
 * pair ≥ 0.9 Å. (The reviewer's ~0.25 ballpark, narrowed by the C=O measurement.)
 */
export const DEFAULT_AMPLITUDE_ANGSTROM = 0.18;
export const MIN_AMPLITUDE_ANGSTROM = 0.02;
export const MAX_AMPLITUDE_ANGSTROM = 0.6;
export const AMPLITUDE_STEP_ANGSTROM = 0.02;

/** Frames sampled over one full period. Phase is `p / PHASE_FRAMES`, p = 0…N−1, so
 * phase 0 is exactly the equilibrium (sin 0 = 0). A UI/app timer advances p. */
export const PHASE_FRAMES = 40;

/**
 * Collapse-guard floor, Å — the LAST line of defence for a large amplitude, no
 * longer the main mechanism (the max-displacement normalization is). Below this two
 * atoms read as merged mush. At the default 0.18 Å no mode trips it; it only fires
 * if the user drags the slider well up. Kept (rule #9), not relied upon.
 */
export const MIN_SAFE_DISTANCE_ANGSTROM = 0.5;

/** Physical-amplitude constants (CODATA). Named, like `BOHR_TO_ANGSTROM`. */
const HBAR_J_S = 1.054_571_817e-34;
const SPEED_OF_LIGHT_CM_S = 2.997_924_58e10;
const AMU_KG = 1.660_539_066_6e-27;

/**
 * Standard atomic weights (amu), IUPAC — VERIFIED to equal the `.hess $atoms` 2nd
 * (mass) column on the dexketoprofen run (C 12.011, H 1.008, O 15.999 to 4 dp,
 * `probes/mode_amplitude.py`), so deriving mass from the element symbol here matches
 * the artifact without the reader carrying masses (rule #10 — measured, not assumed).
 * An element absent here yields no physical amplitude (shown as such, never guessed).
 */
const ATOMIC_MASS_AMU: Record<string, number> = {
  H: 1.008, He: 4.0026, Li: 6.94, Be: 9.0122, B: 10.81, C: 12.011, N: 14.007,
  O: 15.999, F: 18.998, Ne: 20.18, Na: 22.99, Mg: 24.305, Al: 26.982, Si: 28.085,
  P: 30.974, S: 32.06, Cl: 35.45, Ar: 39.948, K: 39.098, Ca: 40.078, Fe: 55.845,
  Cu: 63.546, Zn: 65.38, Br: 79.904, I: 126.9,
};

type Vec3 = [number, number, number];

/**
 * Mode `modeIndex`'s per-atom displacement, extracted as the **column** of the
 * row-major 3N×3N `$normal_modes` matrix (column k = mode k — the measured
 * convention, `parse-sources.md`; a row would be a different thing). Row `3a+c` is
 * atom `a`, Cartesian component `c`, so `disp[a][c] = normalModes[(3a+c)·n + k]`.
 *
 * Throws (never returns a wrong-length vector) on a matrix that is not n², an
 * `nModes` not divisible by 3, or an out-of-range mode index (rule #9).
 */
export function modeDisplacements(
  normalModes: number[],
  nModes: number,
  modeIndex: number,
): Vec3[] {
  if (normalModes.length !== nModes * nModes) {
    throw new Error(
      `normal_modes has ${normalModes.length} entries, expected ${nModes}² = ${nModes * nModes}`,
    );
  }
  if (nModes % 3 !== 0) {
    throw new Error(`nModes ${nModes} is not a multiple of 3 (not 3N)`);
  }
  if (modeIndex < 0 || modeIndex >= nModes) {
    throw new Error(`mode index ${modeIndex} out of range [0, ${nModes})`);
  }
  const nAtoms = nModes / 3;
  const out: Vec3[] = [];
  for (let a = 0; a < nAtoms; a++) {
    out.push([
      normalModes[(3 * a + 0) * nModes + modeIndex],
      normalModes[(3 * a + 1) * nModes + modeIndex],
      normalModes[(3 * a + 2) * nModes + modeIndex],
    ]);
  }
  return out;
}

/**
 * The largest **atomic** displacement magnitude in a mode — `max_j |v_j|`, where
 * `|v_j|` is the tri-vector norm of atom j. This is the normalization denominator so
 * that `A` means the max atomic displacement. It is deliberately NOT `max |component|`
 * (which is smaller by up to √3 for an atom moving on a diagonal — the silent error
 * this function's name guards against).
 */
export function maxAtomicNorm(disp: Vec3[]): number {
  let m = 0;
  for (const v of disp) {
    const n = Math.hypot(v[0], v[1], v[2]);
    if (n > m) m = n;
  }
  return m;
}

/**
 * The animated geometry at `phase` ∈ [0,1): `x_eq + A·sin(2π·phase)·v̂`, where the
 * mode is scaled so the **busiest atom moves exactly `amplitudeAngstrom` at the
 * sin=±1 extreme**. At phase 0 (and 0.5) this is exactly the equilibrium (sin = 0).
 * Throws on an atom-count mismatch — never a silent draw.
 */
export function modeFrameCoords(
  equilibrium: Vec3[],
  disp: Vec3[],
  amplitudeAngstrom: number,
  phase: number,
): Vec3[] {
  if (equilibrium.length !== disp.length) {
    throw new Error(
      `equilibrium has ${equilibrium.length} atoms but the mode has ${disp.length}`,
    );
  }
  const vmax = maxAtomicNorm(disp);
  // s carries BOTH the phase and the Å-per-unit scaling. vmax === 0 (a rigid/zero
  // mode we never animate) → no motion rather than a divide-by-zero.
  const s = vmax > 0 ? (amplitudeAngstrom / vmax) * Math.sin(2 * Math.PI * phase) : 0;
  return equilibrium.map((p, a) => [
    p[0] + s * disp[a][0],
    p[1] + s * disp[a][1],
    p[2] + s * disp[a][2],
  ]);
}

/** One animated frame as a standard xyz string for the viewer. Reuses the
 * trajectory formatter (same count-mismatch throw), so animation and trajectory
 * feed the dumb renderer through one code path. */
export function modeFrameXyz(
  elements: string[],
  equilibrium: Vec3[],
  disp: Vec3[],
  amplitudeAngstrom: number,
  phase: number,
): string {
  const coords = modeFrameCoords(equilibrium, disp, amplitudeAngstrom, phase);
  return frameToXyz(elements, { energy_eh: null, xyz_angstrom: coords });
}

/** Smallest interatomic distance in a geometry (Å) — plain O(N²), N is small. */
export function minInteratomicDistance(coords: Vec3[]): number {
  let best = Infinity;
  for (let i = 0; i < coords.length; i++) {
    for (let j = i + 1; j < coords.length; j++) {
      const dx = coords[i][0] - coords[j][0];
      const dy = coords[i][1] - coords[j][1];
      const dz = coords[i][2] - coords[j][2];
      const d = Math.sqrt(dx * dx + dy * dy + dz * dz);
      if (d < best) best = d;
    }
  }
  return best;
}

/**
 * The closest two atoms get anywhere in the mode's period at this amplitude — the
 * number the collapse guard tests against `MIN_SAFE_DISTANCE_ANGSTROM`. The per-pair
 * distance is convex in `sin`, so the minimum can be interior; we sample the period
 * and take the global min. Deterministic.
 */
export function modeMinDistanceOverPeriod(
  equilibrium: Vec3[],
  disp: Vec3[],
  amplitudeAngstrom: number,
  samples: number = PHASE_FRAMES,
): number {
  let best = Infinity;
  for (let p = 0; p < samples; p++) {
    const d = minInteratomicDistance(
      modeFrameCoords(equilibrium, disp, amplitudeAngstrom, p / samples),
    );
    if (d < best) best = d;
  }
  return best;
}

/** Atomic masses (amu) for an element list, or `null` if any element is not in the
 * verified table — so the physical amplitude is shown only when every mass is known. */
export function atomicMasses(elements: string[]): number[] | null {
  const out: number[] = [];
  for (const el of elements) {
    const m = ATOMIC_MASS_AMU[el];
    if (m == null) return null;
    out.push(m);
  }
  return out;
}

/**
 * The mode's effective reduced mass (amu): `μ = 1 / Σ_i(|v_i|²/m_i)`. With the mode
 * unit-normalized (`Σ|v_i|² = 1`), a light atom carrying much of the motion pulls μ
 * down. `disp` and `massesAmu` must be index-aligned and same length.
 */
export function reducedMassAmu(disp: Vec3[], massesAmu: number[]): number {
  let s = 0;
  for (let a = 0; a < disp.length; a++) {
    const n2 = disp[a][0] ** 2 + disp[a][1] ** 2 + disp[a][2] ** 2;
    s += n2 / massesAmu[a];
  }
  return 1 / s;
}

/**
 * The mode's zero-point (real thermal) amplitude in Å: `A0 = √(ħ / (2 μ ω))`, with
 * `ω = 2πc·ν̃` (ν̃ in cm⁻¹) and μ from {@link reducedMassAmu}. This is the number the
 * label shows to make the point that `A` (the drawn max displacement) is a viewing
 * choice: real vibrations are ~0.04–0.07 Å, we draw ~0.18 for visibility. `null` for
 * a non-positive frequency (imaginary/zero modes have no zero-point amplitude) or
 * unknown masses. Measured on #84: μ ≈ 3.12 amu → A0 ≈ 0.055 Å.
 */
export function zeroPointAmplitudeAngstrom(
  disp: Vec3[],
  massesAmu: number[],
  frequencyCm: number,
): number | null {
  if (frequencyCm <= 0) return null;
  const mu = reducedMassAmu(disp, massesAmu) * AMU_KG;
  const omega = 2 * Math.PI * SPEED_OF_LIGHT_CM_S * frequencyCm;
  const a0_m = Math.sqrt(HBAR_J_S / (2 * mu * omega));
  return a0_m * 1e10;
}

// --------------------------------------------------------------------------- //
// Stage E2 — connectivity: displace a TS along its imaginary mode into 2 basins //
// --------------------------------------------------------------------------- //
//
// The imaginary mode of a first-order saddle IS the reaction coordinate. Stepping
// off the saddle a small distance ±δ along it and relaxing (plain Opt) lands in the
// two minima the TS connects — a poor-man's IRC that answers "does this TS join the
// two basins I meant?". Validated on the real MeNH₂+EtI TS: forward (N···C 1.668 →
// product N–C 1.51 / C–I 4.12), backward (N···C 3.039 → reactant N–C 3.6 / C–I 2.2).
// The displacement REUSES the animation math (`modeFrameCoords` at sin = ±1) — the
// same normalized imaginary-mode vector, never re-parsed. See `wiki/orca/connectivity.md`.

/** A molecular geometry: element symbols and Å coordinates, index-aligned.
 * Structurally compatible with `optts.ts`'s `TsGuessGeometry`. */
export interface Geometry {
  elements: string[];
  xyz_angstrom: Vec3[];
}

/**
 * Default displacement off the TS along the imaginary mode, Å. **Measured**: 0.5 Å
 * splits the MeNH₂+EtI TS cleanly into product/reactant; too small and an endpoint
 * relaxes back to the saddle (no split). User-adjustable in the UI.
 */
export const DEFAULT_CONNECTIVITY_DELTA_ANGSTROM = 0.5;

/**
 * Displace a located-TS geometry ±δ along its imaginary normal mode → the two Opt
 * seeds for the connectivity check. REUSES {@link modeFrameCoords}: at phase 0.25
 * (sin = +1) it returns `x_TS + δ·v̂`, at 0.75 (sin = −1) `x_TS − δ·v̂`, where the
 * mode is normalized so the busiest atom moves exactly δ (`maxAtomicNorm`) — the exact
 * validated math, added once rather than oscillated.
 *
 * `mode` is the **flat 3N** imaginary-mode vector (`modeDisplacements(...).flat()` —
 * do NOT re-parse it). `deltaAngstrom = 0` (or a zero mode) → both endpoints == TS.
 * Throws on a 3N / atom-count mismatch (never a silent wrong-length displacement).
 */
export function displaceAlongImaginaryMode(
  tsGeometry: Geometry,
  mode: number[],
  deltaAngstrom: number,
): { forward: Geometry; backward: Geometry } {
  const nAtoms = tsGeometry.elements.length;
  if (tsGeometry.xyz_angstrom.length !== nAtoms) {
    throw new Error(
      `TS geometry has ${nAtoms} elements but ${tsGeometry.xyz_angstrom.length} coordinate rows`,
    );
  }
  if (mode.length !== 3 * nAtoms) {
    throw new Error(`imaginary mode has ${mode.length} entries, expected 3N = ${3 * nAtoms}`);
  }
  const disp: Vec3[] = [];
  for (let a = 0; a < nAtoms; a++) {
    disp.push([mode[3 * a], mode[3 * a + 1], mode[3 * a + 2]]);
  }
  const forward = modeFrameCoords(tsGeometry.xyz_angstrom, disp, deltaAngstrom, 0.25);
  const backward = modeFrameCoords(tsGeometry.xyz_angstrom, disp, deltaAngstrom, 0.75);
  return {
    forward: { elements: tsGeometry.elements, xyz_angstrom: forward },
    backward: { elements: tsGeometry.elements, xyz_angstrom: backward },
  };
}

/**
 * The largest change in any single interatomic distance between two index-aligned
 * geometries (Å). Rotation- and translation-invariant by construction (it compares
 * distances, never coordinates) — the same discipline the Rust geometry
 * post-conditions use (`parse/mo.rs`, `parse/hess.rs`), so NO Kabsch/SVD alignment is
 * needed. It is a **max, not a mean**: a single bond breaking/forming reads at its full
 * magnitude, not diluted by the many unchanged pairs — so the connectivity thresholds
 * are size-independent (a whole-matrix RMS would shrink with molecule size). Throws on
 * an atom-count mismatch.
 */
export function maxInteratomicDistanceDelta(a: Vec3[], b: Vec3[]): number {
  if (a.length !== b.length) {
    throw new Error(`geometries differ in atom count: ${a.length} vs ${b.length}`);
  }
  const dist = (g: Vec3[], i: number, j: number) =>
    Math.hypot(g[i][0] - g[j][0], g[i][1] - g[j][1], g[i][2] - g[j][2]);
  let m = 0;
  for (let i = 0; i < a.length; i++) {
    for (let j = i + 1; j < a.length; j++) {
      const d = Math.abs(dist(a, i, j) - dist(b, i, j));
      if (d > m) m = d;
    }
  }
  return m;
}

/** An endpoint must differ from the TS by ≥ this in some interatomic distance to have
 * "left the saddle" (Å). Below it, the endpoint relaxed back to the TS (δ too small).
 * Provisional default, confirmed by the E2 manual gate (validated case shifts ~1.8 Å). */
export const MIN_SHIFT_FROM_TS_ANGSTROM = 0.3;
/** The two endpoints must differ from EACH OTHER by ≥ this in some interatomic distance
 * to be two DISTINCT basins (Å) — the validated Menshutkin case separates by ~2 Å
 * (N–C 1.51 vs 3.6). Provisional default, confirmed by the manual gate. */
export const MIN_ENDPOINT_SEPARATION_ANGSTROM = 0.5;

export interface ConnectivityVerdict {
  /** Both endpoints left the TS AND landed in different basins. */
  distinctBasins: boolean;
  /** Max interatomic-distance change, forward vs TS (Å). */
  fwdShiftFromTs: number;
  /** Max interatomic-distance change, backward vs TS (Å). */
  bwdShiftFromTs: number;
  /** Max interatomic-distance change between the two endpoints (Å). */
  endpointSeparation: number;
}

/**
 * Does a TS connect two distinct basins? A pure geometry test on the two relaxed
 * endpoints + the TS: `distinctBasins` ⟺ each endpoint moved off the TS
 * (≥ {@link MIN_SHIFT_FROM_TS_ANGSTROM}) AND the two endpoints differ from each other
 * (≥ {@link MIN_ENDPOINT_SEPARATION_ANGSTROM}). The endpoint-separation clause is the
 * one a δ-too-small run fails: if both relaxed back to the TS they sit close to it AND
 * to each other, so the verdict is (correctly) false — not a trivial pass. WHICH basin
 * is reactant vs product is read from the reaction-coordinate distance (Part B, the
 * scanned pair); this only certifies "two distinct minima". Metric is
 * rotation/translation-invariant, so ORCA's per-job reframing is irrelevant.
 */
export function connectivityVerdict(
  forward: Geometry,
  backward: Geometry,
  ts: Geometry,
): ConnectivityVerdict {
  const fwdShiftFromTs = maxInteratomicDistanceDelta(forward.xyz_angstrom, ts.xyz_angstrom);
  const bwdShiftFromTs = maxInteratomicDistanceDelta(backward.xyz_angstrom, ts.xyz_angstrom);
  const endpointSeparation = maxInteratomicDistanceDelta(
    forward.xyz_angstrom,
    backward.xyz_angstrom,
  );
  const distinctBasins =
    fwdShiftFromTs >= MIN_SHIFT_FROM_TS_ANGSTROM &&
    bwdShiftFromTs >= MIN_SHIFT_FROM_TS_ANGSTROM &&
    endpointSeparation >= MIN_ENDPOINT_SEPARATION_ANGSTROM;
  return { distinctBasins, fwdShiftFromTs, bwdShiftFromTs, endpointSeparation };
}

export interface CoordinateChange {
  /** Atom indices of the pair (i < j), 0-based. */
  i: number;
  j: number;
  /** Element symbols of the pair, for the label (e.g. "N", "C"). */
  elements: [string, string];
  distForwardAngstrom: number;
  distBackwardAngstrom: number;
  distTsAngstrom: number;
}

/**
 * The top-K interatomic distances that changed MOST between the two relaxed
 * endpoints — the reaction coordinate(s) made legible. A bond forming in one basin
 * and breaking in the other (e.g. Menshutkin N–C 1.51 ⇄ 3.6, C–I 4.12 ⇄ 2.2) surfaces
 * here at the top, so the chemist reads WHICH endpoint is reactant vs product from the
 * numbers. Pure, and deliberately **self-contained**: it needs no pathway / scanned-
 * pair input — the endpoints themselves reveal the bonds that define the basins (so it
 * also works for a hand-built or NEB-sourced TS with no scan ancestor). Sorted by
 * |forward − backward| descending. Throws on an atom-count mismatch.
 */
export function reactionCoordinateChanges(
  forward: Geometry,
  backward: Geometry,
  ts: Geometry,
  topK = 3,
): CoordinateChange[] {
  const n = forward.elements.length;
  if (backward.elements.length !== n || ts.elements.length !== n) {
    throw new Error(
      `geometries differ in atom count: fwd ${n}, bwd ${backward.elements.length}, ts ${ts.elements.length}`,
    );
  }
  const d = (g: Geometry, i: number, j: number) =>
    Math.hypot(
      g.xyz_angstrom[i][0] - g.xyz_angstrom[j][0],
      g.xyz_angstrom[i][1] - g.xyz_angstrom[j][1],
      g.xyz_angstrom[i][2] - g.xyz_angstrom[j][2],
    );
  const out: CoordinateChange[] = [];
  for (let i = 0; i < n; i++) {
    for (let j = i + 1; j < n; j++) {
      out.push({
        i,
        j,
        elements: [forward.elements[i], forward.elements[j]],
        distForwardAngstrom: d(forward, i, j),
        distBackwardAngstrom: d(backward, i, j),
        distTsAngstrom: d(ts, i, j),
      });
    }
  }
  out.sort(
    (a, b) =>
      Math.abs(b.distForwardAngstrom - b.distBackwardAngstrom) -
      Math.abs(a.distForwardAngstrom - a.distBackwardAngstrom),
  );
  return out.slice(0, topK);
}
