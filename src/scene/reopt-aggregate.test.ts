import { describe, it, expect } from "vitest";

import { aggregateReopt, type ReoptAggregateRaw, type ReoptChild } from "./reopt-aggregate";
import { conformerMatchesFragment, type Conformer } from "./ensemble";
import type { RawFragment } from "./types";

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
    method: "r2SCAN-3c",
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
    // The energies differ by ~31 kcal/mol — a genuine minimum change, not a tie.
    expect(c.minimumChanged).toBe(true);
  });

  it("no reordering when DFT preserves the xTB order", () => {
    const ens = ensemble([-100.0, -99.99]);
    const agg = raw([
      child({ source_conformer_index: 0, gibbs_eh: -100.2, imaginary_count: 0 }),
      child({ source_conformer_index: 1, gibbs_eh: -100.1, imaginary_count: 0 }),
    ]);
    const c = aggregateReopt(agg, ens);
    expect(c.reordered).toBe(false);
    expect(c.minimumChanged).toBe(false);
    expect(c.dismissedRoseToTop).toBe(false);
  });
});

describe("aggregateReopt — caption scope (minimum held vs minimum changed vs tie)", () => {
  it("minimum HELD, tail reordered (the ibuprofen shape): xTB#1 == DFT#1, dismissed #4 tied", () => {
    // xTB order: conf0 < conf1 < conf2 < conf3 (conf3 dismissed at xTB rank 4).
    const ens = ensemble([-100.0, -99.99, -99.98, -99.97]);
    const agg = raw([
      child({ source_conformer_index: 0, gibbs_eh: -200.1, imaginary_count: 0 }),
      child({ source_conformer_index: 1, gibbs_eh: -200.098, imaginary_count: 0 }),
      child({ source_conformer_index: 2, gibbs_eh: -200.097, imaginary_count: 0 }),
      // conf3 ties conf0 at DFT (identical G) — a dismissed conformer rose to co-min.
      child({ source_conformer_index: 3, gibbs_eh: -200.1, imaginary_count: 0 }),
    ]);
    const c = aggregateReopt(agg, ens);
    // The xTB minimum (conf0) is among the DFT co-minima → the minimum did NOT change,
    // so the strong "wrong minimum" caption must NOT fire.
    expect(c.minimumChanged).toBe(false);
    // ...but the dismissed conf3 joined the DFT top — the honest teaching point.
    expect(c.dismissedRoseToTop).toBe(true);
    expect(c.reordered).toBe(true); // conf3 moved from xTB#4 into the DFT top
  });

  it("minimum CHANGED: the xTB best is not a DFT co-minimum", () => {
    const ens = ensemble([-100.0, -99.99]);
    const agg = raw([
      // xTB min conf0, but DFT puts conf1 clearly (>0.05 kcal/mol) below it.
      child({ source_conformer_index: 0, gibbs_eh: -200.10, imaginary_count: 0 }),
      child({ source_conformer_index: 1, gibbs_eh: -200.15, imaginary_count: 0 }),
    ]);
    const c = aggregateReopt(agg, ens);
    expect(c.minimumChanged).toBe(true);
  });

  it("TIE within tolerance: xTB best is a DFT co-minimum → minimum NOT changed", () => {
    const ens = ensemble([-100.0, -99.99]);
    // conf1 is DFT-lowest by only 5e-5 Eh (~0.03 kcal/mol < 0.05) over conf0 (xTB min).
    const agg = raw([
      child({ source_conformer_index: 0, gibbs_eh: -200.10, imaginary_count: 0 }),
      child({ source_conformer_index: 1, gibbs_eh: -200.10005, imaginary_count: 0 }),
    ]);
    const c = aggregateReopt(agg, ens);
    // Ranks flip (conf1 is numerically lower) but they are practically degenerate,
    // so the xTB min is still a DFT co-minimum — minimum held, tail note fires.
    expect(c.reordered).toBe(true);
    expect(c.minimumChanged).toBe(false);
    expect(c.dismissedRoseToTop).toBe(true);
  });

  it("just OUTSIDE the tolerance: a real swap is a minimum change", () => {
    const ens = ensemble([-100.0, -99.99]);
    // conf1 lower by 1e-4 Eh (~0.063 kcal/mol > 0.05) — not degenerate.
    const agg = raw([
      child({ source_conformer_index: 0, gibbs_eh: -200.10, imaginary_count: 0 }),
      child({ source_conformer_index: 1, gibbs_eh: -200.1001, imaginary_count: 0 }),
    ]);
    const c = aggregateReopt(agg, ens);
    expect(c.minimumChanged).toBe(true);
  });
});

describe("carry the DFT final geometry downstream — composition post-condition (D3)", () => {
  // `useDftConformer` builds a Conformer from the child's parsed `final_geometry`
  // (elements + xyz_angstrom) and applies it via planConformerApply, whose
  // `conformerMatchesFragment` gate is the rule-#9 post-condition: the carried
  // geometry must share the source fragment's atom count AND element order, or it is
  // refused. Coordinates here are arbitrary (99…) to prove it's composition, not
  // coordinates, that is checked.
  const fragment: RawFragment = {
    id: "f",
    name: "butane",
    charge: 0,
    source: "smiles",
    atoms: [
      { element: "C", x: 0, y: 0, z: 0 },
      { element: "C", x: 1, y: 0, z: 0 },
      { element: "H", x: 2, y: 0, z: 0 },
    ],
  };
  const dftConformer = (elements: string[]): Conformer => ({
    atoms: elements.map((element, i) => ({ element, x: 99 + i, y: 99, z: 99 })),
    energy: NaN,
    index: 0,
  });

  it("accepts a DFT geometry with the same composition + order", () => {
    expect(conformerMatchesFragment(fragment, dftConformer(["C", "C", "H"]))).toBe(true);
  });
  it("refuses a DFT geometry with a different element ORDER", () => {
    expect(conformerMatchesFragment(fragment, dftConformer(["C", "H", "C"]))).toBe(false);
  });
  it("refuses a DFT geometry with a different atom COUNT", () => {
    expect(conformerMatchesFragment(fragment, dftConformer(["C", "C"]))).toBe(false);
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
