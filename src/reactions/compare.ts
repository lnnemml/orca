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

/** The minimum absolute energy (Eh) over a scan's points — the scan's own reference
 * for its intrinsic barrier (the pre-reaction complex, when the scan starts far enough). */
export function minEnergyEh(scan: ScanProfileJson, which: EnergyChoice = "act"): number {
  return Math.min(...scan.points.map((p: ScanPoint) => energyEh(p, which)));
}

/** Intrinsic barrier (kcal/mol) = E(max) − E(min) of one scan, self-contained: the
 * barrier relative to that pathway's own encounter complex. Needs no reference. */
export function intrinsicBarrierKcal(scan: ScanProfileJson, which: EnergyChoice = "act"): number {
  return (maxEnergyEh(scan, which) - minEnergyEh(scan, which)) * HARTREE_TO_KCAL;
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

// --- Comparability guard ----------------------------------------------------

/** Keywords on the `!` line that are NOT part of the electronic-structure method
 * identity: run types, geometry/opt control, SCF-convergence thresholds, print level,
 * and parallelism. Two scans that differ ONLY in these are still comparable. */
const NON_METHOD = new Set([
  // run types / job control
  "OPT", "OPTTS", "TIGHTOPT", "VERYTIGHTOPT", "COPT", "OPTH",
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
