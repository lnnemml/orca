/**
 * DFT re-opt fan-out — the READ/AGGREGATE side (Phase 4.5 Stage D unit D2b).
 *
 * Takes the raw per-child facts read back from Rust (`read_conformer_reoptimization`)
 * plus the xTB ensemble (the D1 `Conformer[]`), and produces the xTB-vs-DFT
 * comparison the panel renders: a re-ranked, re-weighted view of the re-optimized
 * conformers. Pure / React-free; computed at read time, NEVER stored.
 *
 * ONE Boltzmann implementation. This reuses `boltzmannWeights` / `deltaEKcal` from
 * `ensemble.ts` — there is no second weighting here. The only new thing is honest-
 * or-absent bookkeeping (rule #9): which children may be weighted and which must be
 * shown with a reason instead.
 *
 * COMPARABILITY. The xTB and DFT populations are both computed over the SAME
 * included subset (the re-optimized, valid children) — same conformers, same
 * normalization set, only the energy LEVEL differs. So "at xTB these four were
 * ~15% each; at DFT the lowest is 60%" is an honest same-set comparison. The two
 * live in labelled columns; we never do arithmetic between an xTB and a DFT weight.
 */

import {
  boltzmannWeights,
  deltaEKcal,
  isTerminalSuccessStatus,
  type Conformer,
} from "./ensemble";

/** One DFT re-opt child, as returned by the Rust `read_conformer_reoptimization`. */
export interface ReoptChild {
  source_conformer_index: number;
  job_id: string;
  title: string;
  status: string;
  electronic_energy_eh: number | null;
  gibbs_eh: number | null;
  imaginary_count: number | null;
  freq_requested: boolean;
  element_mismatch: boolean;
}

/** The fan-out as returned by Rust (the derived set + mode-detection facts). */
export interface ReoptAggregateRaw {
  source_job_id: string;
  children: ReoptChild[];
  freq_requested_count: number;
  mode_inconsistent: boolean;
}

/** Which energy the DFT weighting uses, and how the DFT column is labelled. */
export type ReoptMode = "dG" | "dE" | "mixed";

/** An included (weighted) conformer — appears in the comparison table. */
export interface ReoptIncludedRow {
  conformerIndex: number;
  jobId: string;
  /** 1-based rank within the included set by xTB energy (its GOAT-order position). */
  xtbRank: number;
  /** xTB ΔE (kcal/mol) relative to the included set's xTB minimum. */
  xtbDeltaKcal: number;
  /** xTB Boltzmann population over the included set. */
  xtbWeight: number;
  /** 1-based rank within the included set by DFT energy (G in ΔG-mode, else E). */
  dftRank: number;
  /** DFT ΔG (ΔG-mode) or ΔE (kcal/mol) relative to the included set's DFT minimum. */
  dftDeltaKcal: number;
  /** DFT Boltzmann population over the included set. */
  dftWeight: number;
  /** Running DFT population down the DFT-energy-sorted included set. */
  dftCumulative: number;
  /** The conformer's xTB and DFT ranks differ — the teaching moment. */
  rankChanged: boolean;
}

/** An excluded conformer — listed with its reason, never given a fake weight. */
export interface ReoptExcludedRow {
  conformerIndex: number;
  jobId: string;
  status: string;
  reason: string;
}

export interface ReoptComparison {
  mode: ReoptMode;
  /** Not every child has reached a terminal success — the ranking is not final. */
  provisional: boolean;
  totalChildren: number;
  /** Children that reached a terminal success (`completed`/`parsed`). */
  terminalCount: number;
  /** Children actually weighted (terminal, valid geometry, usable energy). */
  includedCount: number;
  /** ΔE-mode (Opt-only): the geometries were NOT frequency-validated as minima. */
  notMinimumValidated: boolean;
  /** Some children requested Freq and some did not — no single mode is honest. */
  modeInconsistent: boolean;
  /** At least one included conformer's xTB rank ≠ its DFT rank. */
  reordered: boolean;
  /** Included conformers, ordered by DFT rank (best DFT first). */
  rows: ReoptIncludedRow[];
  /** Excluded conformers, in conformer-index order, each with a reason. */
  excluded: ReoptExcludedRow[];
}

/** A minimal Conformer for the energy-only helpers (atoms are unused by them). */
function energyConformer(index: number, energy: number): Conformer {
  return { atoms: [], energy, index };
}

/** Detect the set's mode from the Rust facts: all-Freq → ΔG, none-Freq → ΔE, mixed. */
export function detectReoptMode(agg: ReoptAggregateRaw): ReoptMode {
  if (agg.mode_inconsistent) return "mixed";
  if (agg.children.length > 0 && agg.freq_requested_count === agg.children.length) {
    return "dG";
  }
  return "dE";
}

/**
 * Build the xTB-vs-DFT comparison from the raw fan-out + the xTB ensemble. Reuses
 * `boltzmannWeights` for BOTH levels over the same included subset. Honest-or-absent:
 * a child that is not a terminal success, has a composition mismatch, optimized to a
 * saddle (imaginary_count > 0), or lacks the mode's usable energy is EXCLUDED from the
 * weighting and returned in `excluded` with a reason — never a fabricated weight.
 */
export function aggregateReopt(
  agg: ReoptAggregateRaw,
  ensemble: Conformer[],
): ReoptComparison {
  const mode = detectReoptMode(agg);
  const usableEnergy = (c: ReoptChild): number | null =>
    mode === "dG" ? c.gibbs_eh : c.electronic_energy_eh;

  const excluded: ReoptExcludedRow[] = [];
  const included: { child: ReoptChild; dft: number; xtb: number }[] = [];

  for (const c of agg.children) {
    const base = {
      conformerIndex: c.source_conformer_index,
      jobId: c.job_id,
      status: c.status,
    };
    // Composition mismatch first — we must not rank across different atoms.
    if (c.element_mismatch) {
      excluded.push({ ...base, reason: "composition mismatch (different atoms)" });
      continue;
    }
    if (!isTerminalSuccessStatus(c.status)) {
      const reason =
        c.status === "failed"
          ? "job failed"
          : c.status === "cancelled"
            ? "cancelled"
            : "still running";
      excluded.push({ ...base, reason });
      continue;
    }
    if (c.imaginary_count != null && c.imaginary_count > 0) {
      const n = c.imaginary_count;
      excluded.push({
        ...base,
        reason: `saddle point — ${n} imaginary frequenc${n === 1 ? "y" : "ies"}, not a minimum`,
      });
      continue;
    }
    const e = usableEnergy(c);
    if (e == null) {
      excluded.push({
        ...base,
        reason:
          mode === "dG"
            ? "no Gibbs free energy (Freq did not complete)"
            : "no DFT energy parsed",
      });
      continue;
    }
    // xTB energy of this conformer (from the D1 ensemble). Absent → exclude honestly.
    const xtb = ensemble[c.source_conformer_index]?.energy;
    if (xtb == null || Number.isNaN(xtb)) {
      excluded.push({ ...base, reason: "no xTB energy for this conformer" });
      continue;
    }
    included.push({ child: c, dft: e, xtb });
  }

  // Rank + weight over the SAME included subset, at each level independently.
  const xtbSorted = [...included].sort((a, b) => a.xtb - b.xtb);
  const dftSorted = [...included].sort((a, b) => a.dft - b.dft);
  const xtbRankOf = new Map(xtbSorted.map((it, i) => [it.child.job_id, i + 1]));
  const dftRankOf = new Map(dftSorted.map((it, i) => [it.child.job_id, i + 1]));

  const xtbConfs = xtbSorted.map((it) => energyConformer(0, it.xtb));
  const dftConfs = dftSorted.map((it) => energyConformer(0, it.dft));
  const xtbDelta = deltaEKcal(xtbConfs);
  const xtbW = boltzmannWeights(xtbConfs);
  const dftDelta = deltaEKcal(dftConfs);
  const dftW = boltzmannWeights(dftConfs);
  const xtbDeltaOf = new Map(xtbSorted.map((it, i) => [it.child.job_id, xtbDelta[i]]));
  const xtbWeightOf = new Map(xtbSorted.map((it, i) => [it.child.job_id, xtbW[i]]));

  let cum = 0;
  const rows: ReoptIncludedRow[] = dftSorted.map((it, i) => {
    cum += dftW[i];
    const xtbRank = xtbRankOf.get(it.child.job_id)!;
    const dftRank = dftRankOf.get(it.child.job_id)!;
    return {
      conformerIndex: it.child.source_conformer_index,
      jobId: it.child.job_id,
      xtbRank,
      xtbDeltaKcal: xtbDeltaOf.get(it.child.job_id)!,
      xtbWeight: xtbWeightOf.get(it.child.job_id)!,
      dftRank,
      dftDeltaKcal: dftDelta[i],
      dftWeight: dftW[i],
      dftCumulative: cum,
      rankChanged: xtbRank !== dftRank,
    };
  });

  const terminalCount = agg.children.filter((c) =>
    isTerminalSuccessStatus(c.status),
  ).length;

  return {
    mode,
    provisional: terminalCount < agg.children.length,
    totalChildren: agg.children.length,
    terminalCount,
    includedCount: included.length,
    notMinimumValidated: mode === "dE",
    modeInconsistent: agg.mode_inconsistent,
    reordered: rows.some((r) => r.rankChanged),
    rows,
    excluded: excluded.sort((a, b) => a.conformerIndex - b.conformerIndex),
  };
}
