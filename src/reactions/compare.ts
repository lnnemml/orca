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

import type { ScanProfileJson } from "../types";
import { HARTREE_TO_KCAL, energyEh, type EnergyChoice } from "../scan/scanProfile";

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
): BarrierCell {
  if (refJobCount === 0) return { reason: "no reactant reference set" };
  if (refEnergyEh === null) {
    return { reason: "reference incomplete — a reference job has no parsed energy" };
  }
  const guard = referenceComparable(referenceInputs, pathwayMethodSig);
  if (!guard.ok) return { reason: guard.reason };
  return { kcal: absoluteBarrierKcal(pathwayMaxEh, refEnergyEh) };
}
