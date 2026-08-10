/**
 * Connectivity check — the CREATE side (Phase 4.5 Stage E2).
 *
 * A located transition state's single imaginary mode IS the reaction coordinate.
 * Displacing the TS geometry ±δ along it and relaxing (plain Opt) lands in the two
 * minima the saddle connects — a poor-man's IRC that answers "does this TS join the
 * two basins I meant?". Validated on the real MeNH₂+EtI TS (forward → product N–C 1.51
 * / C–I 4.12; backward → reactant N–C 3.6 / C–I 2.2). See `wiki/orca/connectivity.md`.
 *
 * This module turns a TS geometry + its imaginary mode into the TWO plain-Opt child
 * inputs. It is PURE / React-free and REUSES, without a second implementation:
 *  - `displaceAlongImaginaryMode` (`spectrum/mode.ts`) — the ±δ displacement, itself
 *    the validated animation math; the imaginary-mode vector is passed in, never
 *    re-parsed here.
 *  - `buildReoptInput` (`scene/reopt.ts`) — the plain-Opt (`freq: false`) child
 *    builder, carrying THE CHARGE FOOTGUN discipline: (charge, mult) are inherited from
 *    the TS input's `* xyz` line and asserted back out of each emitted child.
 *
 * COMPARABILITY: the endpoints must relax on the SAME surface as the TS, so method +
 * solvation are inherited **verbatim** from the TS input (`methodSolvationKeywords`,
 * the shared `!`-line reader) and carried in `buildReoptInput`'s composite slot — never
 * defaulted to r2SCAN-3c / no-solvent. Only the job type changes (OptTS Freq → Opt).
 */

import { buildReoptInput } from "./reopt";
import { methodSolvationKeywords } from "../reactions/compare";
import { displaceAlongImaginaryMode, type Geometry } from "../spectrum/mode";
import type { Conformer } from "./ensemble";

/** A displaced endpoint as a single-conformer seed for `buildReoptInput`. Energy/index
 * are immaterial here (the child re-optimizes the geometry); only `atoms` is used. */
function geometryToConformer(g: Geometry): Conformer {
  return {
    atoms: g.elements.map((element, i) => ({
      element,
      x: g.xyz_angstrom[i][0],
      y: g.xyz_angstrom[i][1],
      z: g.xyz_angstrom[i][2],
    })),
    energy: Number.NaN,
    index: 0,
  };
}

export interface ConnectivityChildren {
  /** `! … Opt` input for the +δ endpoint (relaxes toward one basin). */
  forwardInput: string;
  /** `! … Opt` input for the −δ endpoint (relaxes toward the other basin). */
  backwardInput: string;
}

/**
 * Build the two plain-Opt child inputs for a connectivity check: displace `tsGeometry`
 * ±`deltaAngstrom` along `imaginaryMode` (flat 3N — from `modeDisplacements(...).flat()`,
 * NOT re-parsed) and emit each endpoint via {@link buildReoptInput} with `freq: false`.
 * Method + solvation are inherited from `tsInput` verbatim (comparability); charge is
 * inherited and asserted by `buildReoptInput`.
 *
 * @throws (via the reused helpers) on a 3N/atom mismatch, a source with no inline
 * `* xyz` block (never defaults charge to 0), or an emitted child whose (charge, mult)
 * or atom order does not match — a bad child never reaches the create boundary.
 */
export function buildConnectivityChildren(
  tsInput: string,
  tsGeometry: Geometry,
  imaginaryMode: number[],
  deltaAngstrom: number,
): ConnectivityChildren {
  const { forward, backward } = displaceAlongImaginaryMode(
    tsGeometry,
    imaginaryMode,
    deltaAngstrom,
  );
  // Method + solvation verbatim from the TS `!` line (job-type keywords stripped),
  // carried in the composite slot; `buildReoptInput` inherits/asserts charge itself.
  const method = methodSolvationKeywords(tsInput).join(" ");
  const build = (g: Geometry) =>
    buildReoptInput(tsInput, geometryToConformer(g), { method, freq: false });
  return { forwardInput: build(forward), backwardInput: build(backward) };
}
