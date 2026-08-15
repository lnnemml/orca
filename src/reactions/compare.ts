//! Comparative-overlay pure logic (Phase 4.5 Stage C2b-1) — ΔΔE‡ + intrinsic barriers
//! + the method/coordinate comparability guard. React-free, node-tested: the numbers
//! and the guard are unit-tested here so the chart wiring (manual-gated) carries no
//! correctness weight (the B2 lesson).
//!
//! **Reference-free by design (ADR-018).** ΔΔE‡ = E(max_A) − E(max_B); the shared
//! reactant reference cancels between two faces of the same substrate+reagent, so the
//! number needs no reference. Intrinsic barriers come from each scan's own minimum.
//! The reactant reference / absolute barriers are C2b-2 — deliberately not here.
//!
//! **The guard refuses, it does not fake (ADR-018 / chemistry/reaction-barriers.md).**
//! A ΔΔE‡ across two scans is only meaningful when they share method/basis/dispersion/
//! solvation AND scan the same coordinate. On any mismatch the UI shows the curves but
//! replaces the number with the specific reason; `pathwaysComparable` returns that reason.

import type { ScanProfileJson, NebResults } from "../types";
import { HARTREE_TO_KCAL, energyEh, type EnergyChoice } from "../scan/scanProfile";
import type { Scene } from "../scene/types";
import { sceneFromOrcaInput, totalCharge } from "../scene/scene";

type ScanPoint = ScanProfileJson["points"][number];

/** The maximum absolute energy (Eh) over a scan's points — the approximate-TS point
 * (a ΔE‡ estimate on the relaxed surface, never a located saddle, never ΔG‡). */
export function maxEnergyEh(scan: ScanProfileJson, which: EnergyChoice = "act"): number {
  return Math.max(...scan.points.map((p: ScanPoint) => energyEh(p, which)));
}

/** The minimum absolute energy (Eh) over ALL of a scan's points. **Used only for the
 * chart shared-zero** (the display axis reference) — NOT for the intrinsic barrier (see
 * `reactantSideMinEh` for why the global min is wrong there on an exothermic scan). */
export function minEnergyEh(scan: ScanProfileJson, which: EnergyChoice = "act"): number {
  return Math.min(...scan.points.map((p: ScanPoint) => energyEh(p, which)));
}

/** Index of the maximum-energy point (the approximate-TS point) — the boundary between
 * the reactant side and the product side of a relaxed scan. */
export function argMaxIndex(scan: ScanProfileJson, which: EnergyChoice = "act"): number {
  let idx = 0;
  let best = -Infinity;
  scan.points.forEach((p: ScanPoint, i: number) => {
    const e = energyEh(p, which);
    if (e > best) {
      best = e;
      idx = i;
    }
  });
  return idx;
}

/**
 * The minimum absolute energy (Eh) over the **reactant side** — `points[0 .. argMax]`
 * INCLUSIVE — i.e. the encounter complex, the correct reference for the *forward*
 * intrinsic barrier.
 *
 * **Documented assumption (rule #9):** a relaxed scan is set up **reactant → product**
 * (start at the encounter complex / far separation, scan toward the product — the
 * conventional setup this project's scans use, and what the intrinsic barrier's "starts
 * far enough" caveat already assumed). Under that convention `points[0..argMax]` is the
 * pre-barrier reactant branch and its minimum is the encounter complex. The **global**
 * minimum is wrong here: for an EXOTHERMIC reaction scanned past the barrier into a lower
 * product (Menshutkin SN2 in DMF: product ≈ 22 kcal/mol below the reactant complex), the
 * global min IS the product, so `E(max) − globalMin` yields the *reverse* barrier, not the
 * forward one. (If a scan were instead defined product → reactant this would measure the
 * reverse barrier — an acceptable, documented limitation, not a defect of this function.)
 */
export function reactantSideMinEh(scan: ScanProfileJson, which: EnergyChoice = "act"): number {
  const end = argMaxIndex(scan, which);
  let best = Infinity;
  for (let i = 0; i <= end; i++) {
    const e = energyEh(scan.points[i], which);
    if (e < best) best = e;
  }
  return best;
}

/** Intrinsic barrier (kcal/mol) = E(max) − E(reactant-side min) of one scan,
 * self-contained: the **forward** barrier relative to that pathway's own encounter
 * complex, measured over the pre-barrier branch only (`reactantSideMinEh`). Needs no
 * reference. Degenerate max-at-first-point (monotonic downhill) → reactant-side min ==
 * points[0] == the max → intrinsic 0. */
export function intrinsicBarrierKcal(scan: ScanProfileJson, which: EnergyChoice = "act"): number {
  return (maxEnergyEh(scan, which) - reactantSideMinEh(scan, which)) * HARTREE_TO_KCAL;
}

/** ΔΔE‡ (kcal/mol) = ΔE‡(A) − ΔE‡(B) = E(max_A) − E(max_B), **reference-free** (the
 * shared reactant reference cancels). Two identical/mirror profiles → ~0 by symmetry. */
export function deltaDeltaEKcal(
  a: ScanProfileJson,
  b: ScanProfileJson,
  which: EnergyChoice = "act",
): number {
  return (maxEnergyEh(a, which) - maxEnergyEh(b, which)) * HARTREE_TO_KCAL;
}

/** Absolute barrier (kcal/mol) vs a reactant reference = (E(max) − E(ref))·627.509 —
 * barrier 3 (chemistry/reaction-barriers.md): from separated reactants (Σ E(reactant
 * jobs)) to the approximate TS. Both energies in Eh. A **screening ΔE‡** on the relaxed
 * surface, never a located saddle and never ΔG‡. Only meaningful when the reference is
 * complete AND method-consistent with the pathway (see `referenceComparable`). */
export function absoluteBarrierKcal(pathwayMaxEh: number, refEnergyEh: number): number {
  return (pathwayMaxEh - refEnergyEh) * HARTREE_TO_KCAL;
}

// --- Normalized overlay curves (N4) — the mixed scan+NEB shape-illustrative axis ---
//
// A NEB has NO physical scan coordinate: its MEP rides an arc-length band, a scan rides a
// bond distance/angle/dihedral. They cannot share a physical x-axis. So when a NEB pathway
// enters the overlay, ALL series are placed on a NORMALIZED 0→1 reaction-coordinate axis —
// an ILLUSTRATIVE shape comparison only. The RIGOROUS cross-pathway number is the located-TS
// ΔΔE‡/ΔΔG‡ table (method-guarded), never the curve. (The all-scan overlay keeps its physical
// axis + `coordinateSignature` guard — this path is only taken when a NEB is present.)

/** A point on the normalized 0→1 reaction-coordinate overlay: `x` ∈ [0,1] along the path,
 * `energyKcal` = ΔE (kcal/mol) relative to the reactant side. Shape-illustrative only. */
export interface NormalizedCurvePoint {
  x: number;
  energyKcal: number;
}

/**
 * The converged NEB MEP as a normalized 0→1 curve: each mep point's `distance_angstrom`
 * divided by the LAST point's distance (arc length → fractional progress), and its
 * `energy_eh` — ALREADY relative (image 0 = 0, from `.final.interp`) — converted to
 * kcal/mol (no subtraction; point 0 is exactly 0). A degenerate band (< 2 points, or a
 * zero total arc length) yields `[]` — never a divide-by-zero or a single dot masquerading
 * as a path.
 */
export function nebMepCurve(neb: NebResults): NormalizedCurvePoint[] {
  const mep = neb.mep;
  if (mep.length < 2) return [];
  const last = mep[mep.length - 1].distance_angstrom;
  if (last === 0) return [];
  return mep.map((img) => ({
    x: img.distance_angstrom / last,
    energyKcal: img.energy_eh * HARTREE_TO_KCAL,
  }));
}

/**
 * A scan profile on the SAME normalized 0→1 axis (for the mixed scan+NEB overlay only):
 * the coordinate mapped min→max → 0→1, the energy relative to the reactant-side minimum
 * (`reactantSideMinEh` — the forward-barrier reference, consistent with the intrinsic
 * barrier) → kcal/mol. A degenerate scan (< 2 points, or all coordinates equal) → `[]`.
 * The physical-axis path (all-scan overlay) does NOT use this — it keeps the real
 * coordinate and the `coordinateSignature` guard.
 */
export function normalizedScanCurve(
  scan: ScanProfileJson,
  which: EnergyChoice = "act",
): NormalizedCurvePoint[] {
  const pts = scan.points;
  if (pts.length < 2) return [];
  const coords = pts.map((p) => p.coordinate);
  const cMin = Math.min(...coords);
  const span = Math.max(...coords) - cMin;
  if (span === 0) return [];
  const zeroEh = reactantSideMinEh(scan, which);
  return pts.map((p) => ({
    x: (p.coordinate - cMin) / span,
    energyKcal: (energyEh(p, which) - zeroEh) * HARTREE_TO_KCAL,
  }));
}

// --- Located-TS barriers (Stage E1b) — E → {E, G} over an actual saddle ------
//
// C2b's barriers estimate ΔE‡ from the SCAN MAXIMUM (an approximate TS on the relaxed
// surface). E1b GENERALIZES them to a LOCATED TS (an OptTS child, parsed with Freq → G):
// the electronic barrier from a real saddle (`locatedBarrierEKcal`, more accurate than the
// scan-max estimate) and — the point of E1b — a real **ΔG‡** (`deltaGDoubleDaggerKcal`).
// **Honest-or-absent (rule #9, ADR-018):** G is nullable (only Freq jobs have it); a null G
// anywhere → the barrier is `null`, NEVER a fabricated or partial-sum number. Raw ORCA G
// (ideal-gas RRHO, 1 atm, 298.15 K); the standard-state caveat is NAMED in the UI, never
// auto-applied (Fork A; chemistry/reaction-barriers.md).

/** True iff this input is a LOCATED transition state — its `!` line carries the `OptTS`
 * token (the role marker; a located TS is an OptTS job). Same token scan as
 * {@link methodSignature}: split each `!` line, compare case-insensitively. Matches `OptTS`
 * **specifically**, NOT a bare `Opt` substring — a plain `! … Opt` geometry optimization is
 * not a transition state. */
export function isLocatedTsInput(input: string): boolean {
  for (const raw of input.split(/\r?\n/)) {
    const line = raw.trim();
    if (!line.startsWith("!")) continue;
    for (const tok of line.slice(1).trim().split(/\s+/)) {
      if (tok.toUpperCase() === "OPTTS") return true;
    }
  }
  return false;
}

/** ΔG‡ (kcal/mol) from a located TS = (G(TS) − ΣG(reactant refs))·627.509, reusing the generic
 * {@link absoluteBarrierKcal} converter. **`null` if either G is null** — a Freq job is required
 * for G, and a missing G must NEVER be read as 0 (that fabricates a barrier). This null-guard is
 * the entire point of the function. */
export function deltaGDoubleDaggerKcal(
  gTsEh: number | null,
  gRefSumEh: number | null,
): number | null {
  if (gTsEh === null || gRefSumEh === null) return null;
  return absoluteBarrierKcal(gTsEh, gRefSumEh);
}

/** The located-TS ΔE‡ (kcal/mol) = (E(TS) − ΣE(reactant refs))·627.509 — the ELECTRONIC barrier
 * from an actual saddle, more accurate than the scan-max estimate {@link maxEnergyEh} gives.
 * Reuses {@link absoluteBarrierKcal}; same honest-absent null-guard as ΔG‡ so the contract is
 * uniform (E is present whenever a job parsed, but the guard keeps the two symmetric). */
export function locatedBarrierEKcal(
  eTsEh: number | null,
  eRefSumEh: number | null,
): number | null {
  if (eTsEh === null || eRefSumEh === null) return null;
  return absoluteBarrierKcal(eTsEh, eRefSumEh);
}

/** ΔΔG‡ (kcal/mol) = G(TS_A) − G(TS_B), **reference-free** — the shared reactants cancel (the same
 * rationale as {@link deltaDeltaEKcal}, but over located-saddle Gibbs energies). THE mission
 * headline for a two-face si/re comparison. Reuses the generic {@link absoluteBarrierKcal} converter
 * `(a−b)·627.509`; reads **no reference** (a version that subtracted a reactant sum would
 * double-count). `null` if either TS lacks G (honest-absent). The 1 atm→1 M standard-state
 * correction CANCELS here (same molecularity on both faces), so this raw number is directly
 * comparable — say so in the UI. */
export function deltaDeltaGKcal(
  gTsA: number | null,
  gTsB: number | null,
): number | null {
  if (gTsA === null || gTsB === null) return null;
  return absoluteBarrierKcal(gTsA, gTsB);
}

// --- OptTS-origin reaction study (Stage F3) ---------------------------------
//
// A THIRD pathway origin alongside scan (B1) and NEB (N4): a reaction study built FROM a located
// OptTS transition state + its two connectivity children (Stage E2). Unlike scan/NEB, there is no
// reaction coordinate — just three stationary points (reactant basin, TS, product basin). The
// barrier is the LOCATED ΔE‡/ΔG‡ vs the USER-DESIGNATED reactant child (a Σ of ONE — the associated
// complex the connectivity check reached), NOT a separated-fragment sum. Reuses the located-TS
// converters ({@link locatedBarrierEKcal} / {@link deltaGDoubleDaggerKcal}), so the honest-or-absent
// null-guard (ΔG‡ null unless the TS has Freq G AND the reactant has G) is inherited, not re-derived.

/** One stationary point's energies for an OptTS study: electronic `eEh` and Gibbs `gEh` (null
 * unless a Freq parsed — the honest-or-absent signal). */
export interface StationaryPointEnergies {
  eEh: number | null;
  gEh: number | null;
}

/** The three jobs of an OptTS-origin study: the located TS, the user-DESIGNATED reactant
 * connectivity child (the barrier's reference basin), and the other child (the product). */
export interface OptTsStudyInput {
  ts: StationaryPointEnergies;
  reactant: StationaryPointEnergies;
  product: StationaryPointEnergies;
}

/** One point of the 3-point reaction profile, relative to the reactant basin (reactant = 0),
 * placed on the illustrative 0→1 axis (no physical reaction coordinate — like the NEB overlay). */
export interface OptTsProfilePoint {
  x: number;
  role: "reactant" | "ts" | "product";
  energyKcal: number;
}

/** The result of an OptTS-origin study: the located barriers + the 3-point overlay profile. */
export interface OptTsStudyResult {
  /** Located electronic barrier ΔE‡ = (E(TS) − E(reactant child))·627.509; `null` if either E
   * is absent (honest — reuses {@link locatedBarrierEKcal}'s null-guard). The reactant is the
   * ONE designated child (Σ of one), NOT a separated-fragment sum. */
  deltaEKcal: number | null;
  /** Located ΔG‡ = (G(TS) − G(reactant child))·627.509; `null` unless BOTH have a Freq G
   * (honest — reuses {@link deltaGDoubleDaggerKcal}'s null-guard, never a fabricated 0). */
  deltaGKcal: number | null;
  /** reactant → TS → product, relative to the reactant basin (reactant = 0), on the 0→1 axis.
   * A point whose electronic energy is absent is OMITTED (never plotted as a fabricated 0); an
   * absent reactant E yields `[]` (nothing to anchor the profile to). */
  profile: OptTsProfilePoint[];
}

/**
 * Build an OptTS-origin reaction study from its TS + the user-designated reactant/product
 * connectivity children. Pure. The barrier is vs the **connectivity reactant basin** (the one
 * designated child — an associated complex), a distinct and valid quantity from a separated-
 * fragments ΔE‡ (label it so; `chemistry/reaction-barriers.md`). Swapping which child is the
 * reactant changes the barrier — that designation is the user's explicit choice, not auto-picked.
 */
export function optTsStudy(input: OptTsStudyInput): OptTsStudyResult {
  const deltaEKcal = locatedBarrierEKcal(input.ts.eEh, input.reactant.eEh);
  const deltaGKcal = deltaGDoubleDaggerKcal(input.ts.gEh, input.reactant.gEh);

  // The 3-point profile, relative to the reactant basin. Anchored on the reactant's E; without
  // it there is nothing to zero against, so the profile is empty (honest, not a guessed axis).
  const rE = input.reactant.eEh;
  const profile: OptTsProfilePoint[] = [];
  if (rE !== null) {
    profile.push({ x: 0, role: "reactant", energyKcal: 0 });
    if (input.ts.eEh !== null) {
      profile.push({ x: 0.5, role: "ts", energyKcal: (input.ts.eEh - rE) * HARTREE_TO_KCAL });
    }
    if (input.product.eEh !== null) {
      profile.push({ x: 1, role: "product", energyKcal: (input.product.eEh - rE) * HARTREE_TO_KCAL });
    }
  }
  return { deltaEKcal, deltaGKcal, profile };
}

/** A HINT (not a decision) for which connectivity child defaults to the reactant: the
 * HIGHER-energy endpoint (an early-TS / uphill-to-products reading). Returns `"a"` or `"b"`, or
 * `null` when the two energies can't be compared (either absent). The user always overrides —
 * this only seeds the default (Variant 1: the designation is the user's explicit choice). */
export function reactantHint(aEh: number | null, bEh: number | null): "a" | "b" | null {
  if (aEh === null || bEh === null) return null;
  return aEh >= bEh ? "a" : "b";
}

// --- Standalone located-TS pathway — ABSOLUTE barrier vs SEPARATED REACTANTS -----------------
//
// A FOURTH pathway origin: a standalone OptTS transition state attached to a reaction directly (no
// scan/NEB primary, no connectivity children). Its barrier is the located ΔE‡/ΔG‡ vs the reaction's
// SEPARATED-FRAGMENT references (Σ E(ref) / Σ G(ref)) — the **benchmark** quantity (literature
// reports barriers vs separated reactants). This is a DIFFERENT reference than F3's connectivity
// reactant BASIN ({@link optTsStudy}, one associated complex): both are valid barriers, but they must
// be LABELLED so they are never confused (they differ by the association energy). Reuses the GENERIC
// located converters — no new barrier math, same as the scan-refinement located barrier.

/** The ΔΔ-table cell for a standalone located-TS pathway. `origin`/`label` are baked in so the
 * value is never mistaken for F3's connectivity-basin barrier. ΔE‡ shows whenever E(TS) and
 * Σ E(ref) exist; ΔG‡ is `null` unless the TS AND every reference have a Freq G ({@link
 * deltaGDoubleDaggerKcal}'s null-guard over the already-honest-absent Σ G(ref)). */
export interface LocatedTsBarrierCell {
  origin: "located-ts";
  deltaEKcal: number | null;
  deltaGKcal: number | null;
  label: "vs separated reactants";
}

/**
 * The absolute located barrier of a standalone OptTS TS vs the reaction's separated-reactant
 * references (Σ E(ref) / Σ G(ref)). Reuses {@link locatedBarrierEKcal} (ΔE‡) and
 * {@link deltaGDoubleDaggerKcal} (ΔG‡, honest-absent) verbatim — the SAME math the scan-refinement
 * located barrier uses. NOT the connectivity-basin reference of {@link optTsStudy} (F3).
 */
export function locatedTsBarrierVsRefs(
  ts: StationaryPointEnergies,
  refs: { sumEEh: number | null; sumGEh: number | null },
): LocatedTsBarrierCell {
  return {
    origin: "located-ts",
    deltaEKcal: locatedBarrierEKcal(ts.eEh, refs.sumEEh),
    deltaGKcal: deltaGDoubleDaggerKcal(ts.gEh, refs.sumGEh),
    label: "vs separated reactants",
  };
}

// --- Comparability guard ----------------------------------------------------

/** Keywords on the `!` line that are NOT part of the electronic-structure method
 * identity: run types, geometry/opt control, SCF-convergence thresholds, print level,
 * and parallelism. Two scans that differ ONLY in these are still comparable. */
const NON_METHOD = new Set([
  // run types / job control — incl. the full {Loose,Normal,Tight,VeryTight}Opt
  // geometry-opt CONVERGENCE-preset family: these change convergence tightness (energy
  // at a small level), NOT the electronic-structure method, so a LooseOpt scan is
  // comparable to an Opt/TightOpt reference on the same functional/basis.
  "OPT", "LOOSEOPT", "NORMALOPT", "TIGHTOPT", "VERYTIGHTOPT", "OPTTS", "COPT", "OPTH",
  "FREQ", "NUMFREQ", "ANFREQ", "SP", "ENGRAD", "GRAD", "MD", "GOAT",
  "NEB", "NEB-TS", "NEB-CI", "IRC", "SCANTS",
  // scf-convergence thresholds (affect energy only at µEh — not the method)
  "TIGHTSCF", "VERYTIGHTSCF", "EXTREMESCF", "NORMALSCF", "LOOSESCF", "SLOPPYSCF", "STRONGSCF",
  // print / misc output control
  "LARGEPRINT", "NORMALPRINT", "MINIPRINT", "PRINTBASIS", "XYZFILE", "PRINTMOS", "NOPRINTMOS",
]);

/** The method signature of an ORCA input: the identity-bearing `!`-line keywords
 * (functional/composite, basis, dispersion, RI, solvation) as a normalized, sorted,
 * uppercased token list — plus SMD detection from a `%cpcm ... smd true ... end` block
 * (SMD vs ALPB/CPCM matters, esp. for ions — chemistry/reaction-barriers.md). */
export interface MethodSig {
  tokens: string[];
  /** A human-readable one-line rendering for the mismatch reason. */
  display: string;
}

/**
 * The identity-bearing `!`-line keywords (functional/composite, basis, aux, RI, dispersion,
 * solvation) in **ORIGINAL case and order**, with the job-control / scf-conv / print tokens
 * ({@link NON_METHOD}) and `PAL*` dropped. Unlike {@link methodSignature} — which uppercases +
 * sorts these SAME tokens for *comparison* — this preserves them verbatim so a downstream job can
 * **re-emit the same method + solvation** (comparability by construction, e.g. an OptTS refine of a
 * scan point: `src/scene/optts.ts`). Shares one filter with `methodSignature` (the `NON_METHOD` set),
 * so "what counts as method" is defined in exactly one place.
 *
 * Note: solvation stated as a `%cpcm … smd true … end` BLOCK (rather than an inline `SMD(<solvent>)`
 * keyword) is NOT captured here — our own builder always emits the inline form, which this reads.
 */
export function methodSolvationKeywords(input: string): string[] {
  const kept: string[] = [];
  for (const raw of input.split(/\r?\n/)) {
    const line = raw.trim();
    if (!line.startsWith("!")) continue;
    for (const tok of line.slice(1).trim().split(/\s+/)) {
      if (!tok) continue;
      const up = tok.toUpperCase();
      if (NON_METHOD.has(up)) continue;
      if (/^PAL\d*$/.test(up)) continue; // parallelism, not method
      kept.push(tok);
    }
  }
  return kept;
}

export function methodSignature(input: string): MethodSig {
  const tokens = new Set<string>();

  for (const raw of input.split(/\r?\n/)) {
    const line = raw.trim();
    if (!line.startsWith("!")) continue;
    for (const tok of line.slice(1).trim().split(/\s+/)) {
      if (!tok) continue;
      const up = tok.toUpperCase();
      if (NON_METHOD.has(up)) continue;
      if (/^PAL\d*$/.test(up)) continue; // parallelism, not method
      tokens.add(up);
    }
  }

  // `%cpcm ... smd true ... end` promotes CPCM to SMD — a distinct solvation model.
  if (/%cpcm[\s\S]*?\bsmd\s+true\b/i.test(input)) {
    tokens.add("SMD");
  }

  const sorted = [...tokens].sort();
  return { tokens: sorted, display: sorted.join(" ") || "(no method keywords)" };
}

/** A scan coordinate's identity: kind + scanned atoms (order-sensitive) + unit. Two
 * ΔΔE‡ endpoints must be the SAME coordinate to be subtractable. */
export function coordinateSignature(scan: ScanProfileJson): string {
  return `${scan.kind} ${scan.atoms.join("-")} ${scan.coordinate_unit}`;
}

export type Comparability = { ok: true } | { ok: false; reason: string };

/**
 * Whether two pathways' scans are comparable for a ΔΔE‡ number: same scan coordinate
 * AND same electronic-structure method. Returns the **specific reason** on a mismatch
 * so the UI can name it instead of faking a number. Coordinate is checked first (a
 * different coordinate is the more fundamental incomparability).
 */
export function pathwaysComparable(
  inputA: string,
  inputB: string,
  scanA: ScanProfileJson,
  scanB: ScanProfileJson,
): Comparability {
  const coordA = coordinateSignature(scanA);
  const coordB = coordinateSignature(scanB);
  if (coordA !== coordB) {
    return {
      ok: false,
      reason: `different scan coordinate (${coordA} vs ${coordB}) — ΔΔE‡ not comparable`,
    };
  }

  const sigA = methodSignature(inputA);
  const sigB = methodSignature(inputB);
  if (sigA.display !== sigB.display) {
    return {
      ok: false,
      reason: `methods differ — ΔΔE‡ not comparable (${sigA.display} vs ${sigB.display})`,
    };
  }

  return { ok: true };
}

/**
 * Whether the reactant-reference jobs are on the same electronic-structure scale as a
 * pathway, so an absolute barrier E(max) − E(ref) is subtractable (ADR-018): a B3LYP
 * reactant energy under an r2SCAN-3c scan maximum is nonsense. Reuses `methodSignature`.
 * `referenceInputs` are the reference jobs' `input_content`; `pathwayMethodSig` is the
 * pathway's `methodSignature(...).display`. **Every** reference job must match; the first
 * mismatch names the reason. An empty reference is vacuously `ok` (completeness is a
 * separate check — an incomplete reference has no number to guard).
 */
export function referenceComparable(
  referenceInputs: string[],
  pathwayMethodSig: string,
): Comparability {
  for (const input of referenceInputs) {
    const sig = methodSignature(input);
    if (sig.display !== pathwayMethodSig) {
      return {
        ok: false,
        reason: `reference method differs — absolute barrier not comparable (${sig.display} vs ${pathwayMethodSig})`,
      };
    }
  }
  return { ok: true };
}

// --- Stoichiometry guard ----------------------------------------------------

/** Element → count over a whole Scene (all fragments flattened). */
function sceneElementMultiset(scene: Scene): Map<string, number> {
  const m = new Map<string, number>();
  for (const f of scene.fragments) {
    for (const a of f.atoms) m.set(a.element, (m.get(a.element) ?? 0) + 1);
  }
  return m;
}

/** Hill-system formula for a reason string (C, then H, then the rest alphabetical). */
function hillFormula(m: Map<string, number>): string {
  const rest = [...m.keys()].filter((e) => e !== "C" && e !== "H").sort();
  const order = [...(m.has("C") ? ["C"] : []), ...(m.has("H") ? ["H"] : []), ...rest];
  const s = order.map((e) => `${e}${m.get(e)! > 1 ? m.get(e) : ""}`).join("");
  return s || "(no atoms)";
}

function multisetsEqual(a: Map<string, number>, b: Map<string, number>): boolean {
  if (a.size !== b.size) return false;
  for (const [k, v] of a) if (b.get(k) !== v) return false;
  return true;
}

function signed(n: number): string {
  return n >= 0 ? `+${n}` : `${n}`;
}

/**
 * Whether the reactant-reference jobs are **mass- and charge-balanced** with the reacting
 * complex (ADR-018 honest-or-absent): the absolute barrier E(max) − Σ E(ref) is only physical
 * when the reference accounts for EXACTLY the complex's atoms and charge — the two barrier
 * endpoints must be the same chemical system. Both compositions are read with the existing
 * `sceneFromOrcaInput` parser (NOT a hand-rolled regex), so this reuses the app's one
 * xyz-block reader and its charge handling.
 *
 * Refuses (with a specific reason) when: the complex or any reference has no readable
 * coordinate block (can't verify → don't show a number); Σ(reference atoms) ≠ complex atoms;
 * or Σ(reference charge) ≠ complex charge. **Both valid shapes pass:** two references summing
 * to the complex (e.g. 8 + 7 = 15 atoms), OR a single reference that IS the whole complex.
 *
 * Why this exists: on the real SN2, a reference set left with EtI alone (8 atoms) was subtracted
 * from the 15-atom E(max) and produced a confident **−60127 kcal/mol** (≈ −E(methylamine)) —
 * garbage. A composition/charge mismatch must be an honest refusal, never a number.
 */
export function referenceStoichiometryOk(
  complexInput: string,
  referenceInputs: string[],
): Comparability {
  const complexScene = sceneFromOrcaInput(complexInput);
  if (!complexScene) {
    return { ok: false, reason: "cannot verify complex composition (no readable coordinate block)" };
  }
  const complexAtoms = sceneElementMultiset(complexScene);
  const complexCharge = totalCharge(complexScene);

  const refAtoms = new Map<string, number>();
  let refCharge = 0;
  for (const input of referenceInputs) {
    const scene = sceneFromOrcaInput(input);
    if (!scene) {
      return {
        ok: false,
        reason: "cannot verify reference composition (a reference job has no readable coordinate block)",
      };
    }
    for (const [el, n] of sceneElementMultiset(scene)) refAtoms.set(el, (refAtoms.get(el) ?? 0) + n);
    refCharge += totalCharge(scene);
  }

  if (!multisetsEqual(refAtoms, complexAtoms)) {
    return {
      ok: false,
      reason: `reference incomplete — reactant atoms (${hillFormula(refAtoms)}) do not sum to the reacting complex (${hillFormula(complexAtoms)}); a reactant is missing or mismatched`,
    };
  }
  if (refCharge !== complexCharge) {
    return {
      ok: false,
      reason: `reference charge (${signed(refCharge)}) ≠ complex charge (${signed(complexCharge)}) — barrier endpoints are different chemical systems`,
    };
  }
  return { ok: true };
}

/** One pathway's absolute-barrier cell: a number, or the specific reason it is withheld. */
export type BarrierCell = { kcal: number } | { reason: string };

/**
 * The absolute barrier for ONE pathway, **honest-or-absent** (properties 1+2, ADR-018).
 * Returns a `kcal` number ONLY when the reference is present, **complete**
 * (`refEnergyEh` non-null — the C2b-2a command already summed only when every reference
 * job is parsed) AND **method-consistent** with the pathway; otherwise the specific
 * `reason`. Centralizing the decision here is deliberate: the chart wiring cannot then
 * accidentally treat a `null` (incomplete) reference as `0` and print a wrong number.
 */
export function absoluteBarrierCell(
  pathwayMaxEh: number,
  refEnergyEh: number | null,
  referenceInputs: string[],
  pathwayMethodSig: string,
  refJobCount: number,
  complexInput: string,
): BarrierCell {
  if (refJobCount === 0) return { reason: "no reactant reference set" };
  if (refEnergyEh === null) {
    return { reason: "reference incomplete — a reference job has no parsed energy" };
  }
  const method = referenceComparable(referenceInputs, pathwayMethodSig);
  if (!method.ok) return { reason: method.reason };
  // Composition + charge balance LAST (assumes the inputs are parseable + method-consistent):
  // the reference must account for exactly the complex's atoms and charge, or the number is
  // confident garbage (the −60127 SN2 case). Honest-or-absent: refuse, never fake.
  const stoich = referenceStoichiometryOk(complexInput, referenceInputs);
  if (!stoich.ok) return { reason: stoich.reason };
  return { kcal: absoluteBarrierKcal(pathwayMaxEh, refEnergyEh) };
}
