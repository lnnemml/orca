import { describe, it, expect } from "vitest";

import { aggregateReopt, type ReoptAggregateRaw, type ReoptChild } from "./reopt-aggregate";
import type { Conformer } from "./ensemble";

/** A minimal xTB ensemble whose energies ascend with index (GOAT order). */
function ensemble(energiesEh: number[]): Conformer[] {
  return energiesEh.map((energy, index) => ({ atoms: [], energy, index }));
}

/** A re-opt child with sensible defaults; override per test. */
function child(over: Partial<ReoptChild> & { source_conformer_index: number }): ReoptChild {
  return {
    job_id: `job-${over.source_conformer_index}`,
    title: `re-opt #${over.source_conformer_index}`,
    status: "completed",
    electronic_energy_eh: null,
    gibbs_eh: null,
    imaginary_count: null,
    freq_requested: true,
    element_mismatch: false,
    ...over,
  };
}

function raw(children: ReoptChild[], over: Partial<ReoptAggregateRaw> = {}): ReoptAggregateRaw {
  return {
    source_job_id: "goat-1",
    children,
    freq_requested_count: children.filter((c) => c.freq_requested).length,
    mode_inconsistent: false,
    ...over,
  };
}

describe("aggregateReopt — ΔG-mode (all Freq, all G)", () => {
  const ens = ensemble([-100.0, -99.999, -99.998]);
  const agg = raw([
    child({ source_conformer_index: 0, gibbs_eh: -100.1, imaginary_count: 0 }),
    child({ source_conformer_index: 1, gibbs_eh: -100.099, imaginary_count: 0 }),
    child({ source_conformer_index: 2, gibbs_eh: -100.05, imaginary_count: 0 }),
  ]);

  it("is ΔG-mode and weights via Gibbs G", () => {
    const c = aggregateReopt(agg, ens);
    expect(c.mode).toBe("dG");
    expect(c.notMinimumValidated).toBe(false);
    expect(c.includedCount).toBe(3);
    expect(c.excluded).toHaveLength(0);
    // Populations sum to 1 over the included set.
    const sum = c.rows.reduce((a, r) => a + r.dftWeight, 0);
    expect(sum).toBeCloseTo(1, 12);
    // Lowest-G conformer leads; cumulative reaches 100% at the last row.
    expect(c.rows[0].dftRank).toBe(1);
    expect(c.rows[c.rows.length - 1].dftCumulative).toBeCloseTo(1, 12);
  });
});

describe("aggregateReopt — ΔE-mode fallback (no Freq)", () => {
  const ens = ensemble([-100.0, -99.99]);
  const agg = raw(
    [
      child({ source_conformer_index: 0, electronic_energy_eh: -100.2, freq_requested: false }),
      child({ source_conformer_index: 1, electronic_energy_eh: -100.1, freq_requested: false }),
    ],
    { freq_requested_count: 0 },
  );

  it("weights via electronic energy and flags not-minimum-validated", () => {
    const c = aggregateReopt(agg, ens);
    expect(c.mode).toBe("dE");
    expect(c.notMinimumValidated).toBe(true);
    expect(c.includedCount).toBe(2);
    expect(c.rows.reduce((a, r) => a + r.dftWeight, 0)).toBeCloseTo(1, 12);
  });
});

describe("aggregateReopt — honest-or-absent exclusions", () => {
  const ens = ensemble([-100.0, -99.99, -99.98, -99.97]);

  it("excludes a failed child, an imaginary-freq saddle, and a running child — each with a reason", () => {
    const agg = raw([
      child({ source_conformer_index: 0, gibbs_eh: -100.1, imaginary_count: 0 }),
      child({ source_conformer_index: 1, status: "failed", gibbs_eh: null }),
      child({ source_conformer_index: 2, gibbs_eh: -100.05, imaginary_count: 2 }),
      child({ source_conformer_index: 3, status: "running", gibbs_eh: null }),
    ]);
    const c = aggregateReopt(agg, ens);

    // Only conformer 0 is weightable.
    expect(c.includedCount).toBe(1);
    expect(c.rows).toHaveLength(1);
    expect(c.rows[0].conformerIndex).toBe(0);
    expect(c.rows[0].dftWeight).toBeCloseTo(1, 12); // sole survivor → 100%

    // The other three are listed with reasons, never dropped, never weighted.
    expect(c.excluded).toHaveLength(3);
    const byIdx = Object.fromEntries(c.excluded.map((e) => [e.conformerIndex, e.reason]));
    expect(byIdx[1]).toMatch(/failed/i);
    expect(byIdx[2]).toMatch(/saddle|imaginary/i);
    expect(byIdx[3]).toMatch(/running/i);

    // Not all children terminal → provisional.
    expect(c.provisional).toBe(true);
    expect(c.terminalCount).toBe(2); // completed #0 and #2 (saddle is terminal but unweighted)
  });

  it("in ΔG-mode, a completed child with no G is excluded (Freq did not finish)", () => {
    const agg = raw([
      child({ source_conformer_index: 0, gibbs_eh: -100.1, imaginary_count: 0 }),
      child({ source_conformer_index: 1, gibbs_eh: null, electronic_energy_eh: -100.2, imaginary_count: null }),
    ]);
    const c = aggregateReopt(agg, ens);
    expect(c.mode).toBe("dG");
    expect(c.includedCount).toBe(1);
    expect(c.excluded[0].conformerIndex).toBe(1);
    expect(c.excluded[0].reason).toMatch(/gibbs|freq/i);
  });

  it("excludes a composition mismatch before anything else", () => {
    const agg = raw([
      child({ source_conformer_index: 0, gibbs_eh: -100.1, imaginary_count: 0, element_mismatch: true }),
    ]);
    const c = aggregateReopt(agg, ens);
    expect(c.includedCount).toBe(0);
    expect(c.excluded[0].reason).toMatch(/composition|atoms/i);
  });
});

describe("aggregateReopt — the teaching moment: DFT re-ranks xTB", () => {
  it("detects a rank change (xTB #1 becomes DFT #2)", () => {
    // xTB: conformer 0 lowest, conformer 1 next.
    const ens = ensemble([-100.0, -99.9995]);
    // DFT flips them: conformer 1 is now lower.
    const agg = raw([
      child({ source_conformer_index: 0, gibbs_eh: -100.10, imaginary_count: 0 }),
      child({ source_conformer_index: 1, gibbs_eh: -100.15, imaginary_count: 0 }),
    ]);
    const c = aggregateReopt(agg, ens);
    expect(c.reordered).toBe(true);
    const conf0 = c.rows.find((r) => r.conformerIndex === 0)!;
    const conf1 = c.rows.find((r) => r.conformerIndex === 1)!;
    expect(conf0.xtbRank).toBe(1);
    expect(conf0.dftRank).toBe(2); // xTB #1 → DFT #2
    expect(conf1.xtbRank).toBe(2);
    expect(conf1.dftRank).toBe(1); // xTB #2 → DFT #1
    expect(conf0.rankChanged).toBe(true);
  });

  it("no reordering when DFT preserves the xTB order", () => {
    const ens = ensemble([-100.0, -99.99]);
    const agg = raw([
      child({ source_conformer_index: 0, gibbs_eh: -100.2, imaginary_count: 0 }),
      child({ source_conformer_index: 1, gibbs_eh: -100.1, imaginary_count: 0 }),
    ]);
    const c = aggregateReopt(agg, ens);
    expect(c.reordered).toBe(false);
  });
});

describe("aggregateReopt — mixed mode is surfaced, never silently picked", () => {
  it("flags mode_inconsistent and weights on electronic E", () => {
    const ens = ensemble([-100.0, -99.99]);
    const agg = raw(
      [
        child({ source_conformer_index: 0, electronic_energy_eh: -100.2, gibbs_eh: -100.1, freq_requested: true, imaginary_count: 0 }),
        child({ source_conformer_index: 1, electronic_energy_eh: -100.15, freq_requested: false }),
      ],
      { mode_inconsistent: true, freq_requested_count: 1 },
    );
    const c = aggregateReopt(agg, ens);
    expect(c.mode).toBe("mixed");
    expect(c.modeInconsistent).toBe(true);
    // Mixed → electronic E used (the common denominator), both included.
    expect(c.includedCount).toBe(2);
  });
});
