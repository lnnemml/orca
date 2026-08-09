import { useMemo, useState } from "react";
import { CartesianGrid, Line, LineChart, ReferenceDot, Tooltip, XAxis, YAxis } from "recharts";

import type { ScanProfileJson } from "../types";
import { useContainerWidth } from "../charts/useContainerWidth";
import { energyEh, maxIndex, HARTREE_TO_KCAL, type EnergyChoice } from "../scan/scanProfile";
import {
  intrinsicBarrierKcal,
  minEnergyEh,
  maxEnergyEh,
  deltaDeltaEKcal,
  pathwaysComparable,
  referenceComparable,
  absoluteBarrierCell,
  methodSignature,
} from "./compare";

/** A pathway ready to compare: its label, the attached job's scan profile (B1), and
 * that job's input (for the method-comparability guard). */
export interface ComparePathway {
  id: string;
  label: string;
  scan: ScanProfileJson;
  input: string;
}

/** The reactant reference for absolute barriers (Phase 4.5 C2b-2b, ADR-018), read from
 * the C2b-2a `reaction_reference_energy` command. `energyEh` is the summed Σ E(ref) in
 * Eh ONLY when complete (every reference job parsed), else `null` (incomplete). `inputs`
 * are the reference jobs' `input_content` for the method guard; `jobCount` distinguishes
 * "no reference set" (0 → the C2b-1 state) from "incomplete" (>0 with `energyEh` null). */
export interface CompareReference {
  inputs: string[];
  energyEh: number | null;
  jobCount: number;
}

type ZeroMode = "min" | "reactants";

const PALETTE = ["#4f8cff", "#ff8c42", "#42c98a", "#c96ee0", "#e0c74e", "#e0576e"];
const CHART_HEIGHT = 230;

/**
 * Comparative overlay + ΔΔE‡ (Phase 4.5 C2b-1) — the mission "done-when": two scan
 * pathways → two profiles overlaid on a shared zero → ΔΔE‡. **Reference-free** (ADR-018):
 * ΔΔE‡ = E(max_A) − E(max_B) needs no reactant reference (it cancels). The numbers are
 * `reactions/compare.ts` (unit-tested); this component only wires the chart. Explicit
 * width via ResizeObserver — no `ResponsiveContainer` (the WebKitGTK 0×0 class).
 */
export function CompareView({
  pathways,
  reference,
}: {
  pathways: ComparePathway[];
  reference?: CompareReference;
}) {
  const [which, setWhich] = useState<EnergyChoice>("act");
  const [baselineIdx, setBaselineIdx] = useState(0);
  const [zeroMode, setZeroMode] = useState<ZeroMode>("min");
  const { ref, width } = useContainerWidth();

  const refEnergyEh = reference?.energyEh ?? null;
  const refJobCount = reference?.jobCount ?? 0;
  const refInputs = useMemo(() => reference?.inputs ?? [], [reference]);
  const refComplete = refEnergyEh != null;
  const refIncomplete = refJobCount > 0 && !refComplete;

  // The "separated reactants" zero is offerable only when E(ref) is complete AND every
  // overlaid pathway shares the reference's method (else re-zeroing on E(ref) would place
  // a mismatched curve on an incomparable scale). Same guard as the per-row barrier.
  const canUseReactantsZero = useMemo(
    () =>
      refComplete &&
      pathways.length > 0 &&
      pathways.every((p) => referenceComparable(refInputs, methodSignature(p.input).display).ok),
    [refComplete, pathways, refInputs],
  );
  const reactantsZeroActive = zeroMode === "reactants" && canUseReactantsZero;

  // Shared zero: by default the global minimum absolute energy across ALL overlaid
  // pathways (the C2b-1 behaviour). When "separated reactants" is chosen AND offerable,
  // the zero is E(ref) instead, so each curve's max height is the ABSOLUTE barrier.
  // ΔΔE‡ is a difference, so it never depends on this choice — the zero is only the axis.
  const series = useMemo(() => {
    const globalMinEh = Math.min(...pathways.map((p) => minEnergyEh(p.scan, which)));
    const zeroEh = reactantsZeroActive && refEnergyEh != null ? refEnergyEh : globalMinEh;
    return pathways.map((p, i) => {
      const data = p.scan.points.map((pt, index) => ({
        index,
        coordinate: pt.coordinate,
        relKcal: (energyEh(pt, which) - zeroEh) * HARTREE_TO_KCAL,
      }));
      const mi = maxIndex(p.scan.points, which);
      return {
        ...p,
        color: PALETTE[i % PALETTE.length],
        data,
        maxDatum: data[mi],
        intrinsicKcal: intrinsicBarrierKcal(p.scan, which),
        // Absolute barrier vs separated reactants — honest-or-absent (compare.ts).
        absoluteCell: absoluteBarrierCell(
          maxEnergyEh(p.scan, which),
          refEnergyEh,
          refInputs,
          methodSignature(p.input).display,
          refJobCount,
        ),
      };
    });
  }, [pathways, which, reactantsZeroActive, refEnergyEh, refInputs, refJobCount]);

  const baseline = series[Math.min(baselineIdx, series.length - 1)];
  const unit = pathways[0]?.scan.coordinate_unit ?? "Å";

  // ΔΔE‡ of every other pathway vs the baseline — each GUARDED independently: a method
  // or coordinate mismatch shows the reason, never a faked number.
  const comparisons = series
    .filter((s) => s.id !== baseline.id)
    .map((s) => {
      const cmp = pathwaysComparable(s.input, baseline.input, s.scan, baseline.scan);
      return {
        id: s.id,
        label: s.label,
        color: s.color,
        guard: cmp,
        ddeKcal: cmp.ok ? deltaDeltaEKcal(s.scan, baseline.scan, which) : null,
      };
    });

  return (
    <div ref={ref} style={{ marginTop: 8 }}>
      <div className="scan-controls" style={{ marginBottom: 8 }}>
        <label className="scan-control">
          energy
          <select
            className="select select-sm"
            value={which}
            onChange={(e) => setWhich(e.target.value as EnergyChoice)}
          >
            <option value="act">actual (composite)</option>
            <option value="scf">SCF only</option>
          </select>
        </label>
        {series.length > 2 ? (
          <label className="scan-control">
            baseline
            <select
              className="select select-sm"
              value={baseline.id}
              onChange={(e) => setBaselineIdx(series.findIndex((s) => s.id === e.target.value))}
            >
              {series.map((s) => (
                <option key={s.id} value={s.id}>
                  {s.label}
                </option>
              ))}
            </select>
          </label>
        ) : null}
        <label className="scan-control">
          ΔE relative to
          <select
            className="select select-sm"
            value={reactantsZeroActive ? "reactants" : "min"}
            onChange={(e) => setZeroMode(e.target.value as ZeroMode)}
          >
            <option value="min">shared minimum</option>
            <option value="reactants" disabled={!canUseReactantsZero}>
              separated reactants{canUseReactantsZero ? "" : " (needs a complete, matching reference)"}
            </option>
          </select>
        </label>
      </div>

      {width > 0 ? (
        <div className="conv-chart">
          <div className="conv-chart-title">
            ΔE ({which === "act" ? "actual" : "SCF"}, kcal/mol) vs coordinate ({unit}) —{" "}
            {reactantsZeroActive
              ? "zero at separated reactants (E(ref)); each max = absolute barrier"
              : "shared zero (global minimum)"}
          </div>
          <LineChart
            width={width}
            height={CHART_HEIGHT}
            margin={{ top: 8, right: 16, bottom: 18, left: 8 }}
          >
            <CartesianGrid stroke="var(--border)" strokeDasharray="3 3" />
            <XAxis
              dataKey="coordinate"
              type="number"
              domain={["dataMin", "dataMax"]}
              stroke="var(--muted)"
              fontSize={11}
              tickLine={false}
              tickFormatter={(v: number) => v.toFixed(2)}
              label={{
                value: `coordinate (${unit})`,
                position: "insideBottom",
                offset: -6,
                fontSize: 11,
                fill: "var(--muted-2)",
              }}
            />
            <YAxis
              stroke="var(--muted)"
              fontSize={11}
              width={70}
              tickLine={false}
              domain={["auto", "auto"]}
              tickFormatter={(v: number) => v.toFixed(1)}
              label={{
                value: "ΔE (kcal/mol)",
                angle: -90,
                position: "insideLeft",
                fontSize: 11,
                fill: "var(--muted-2)",
              }}
            />
            <Tooltip
              isAnimationActive={false}
              formatter={(v) => (typeof v === "number" ? `${v.toFixed(2)} kcal/mol` : String(v))}
              labelFormatter={(l) => `coordinate ${Number(l).toFixed(3)} ${unit}`}
              contentStyle={{
                background: "var(--panel-2)",
                border: "1px solid var(--border)",
                borderRadius: 6,
                fontSize: 11,
              }}
            />
            {series.map((s) => (
              <Line
                key={s.id}
                data={s.data}
                dataKey="relKcal"
                name={s.label}
                stroke={s.color}
                strokeWidth={1.5}
                dot={{ r: 2 }}
                isAnimationActive={false}
              />
            ))}
            {/* Each pathway's maximum — the approximate TS, in that pathway's colour. */}
            {series.map((s) =>
              s.maxDatum ? (
                <ReferenceDot
                  key={`ts-${s.id}`}
                  x={s.maxDatum.coordinate}
                  y={s.maxDatum.relKcal}
                  r={5}
                  fill={s.color}
                  stroke="var(--panel)"
                  strokeWidth={1}
                />
              ) : null,
            )}
          </LineChart>
        </div>
      ) : null}

      {/* Legend + per-pathway intrinsic barrier (self-contained, always shown) and — when
          a reactant reference is configured — the absolute barrier vs separated reactants
          (honest-or-absent: a number only where E(ref) is complete AND method-matching). */}
      <table className="jobs-table" style={{ marginTop: 10 }}>
        <thead>
          <tr>
            <th>Pathway</th>
            <th style={{ textAlign: "right" }}>Intrinsic barrier</th>
            {refJobCount > 0 ? (
              <th style={{ textAlign: "right" }}>Absolute barrier (vs separated reactants)</th>
            ) : null}
          </tr>
        </thead>
        <tbody>
          {series.map((s) => (
            <tr key={s.id}>
              <td>
                <span
                  style={{
                    display: "inline-block",
                    width: 10,
                    height: 10,
                    borderRadius: 2,
                    background: s.color,
                    marginRight: 8,
                  }}
                />
                {s.label}
                {s.id === baseline.id && series.length > 2 ? (
                  <span className="muted"> (baseline)</span>
                ) : null}
              </td>
              <td
                className="mono"
                style={{ textAlign: "right" }}
                title="E(max) − E(reactant-side minimum): forward barrier vs this pathway's own encounter complex (the pre-barrier branch, not the global/product minimum)"
              >
                +{s.intrinsicKcal.toFixed(2)} kcal/mol
              </td>
              {refJobCount > 0 ? (
                "kcal" in s.absoluteCell ? (
                  <td
                    className="mono"
                    style={{ textAlign: "right" }}
                    title="E(max) − Σ E(reactant jobs): screening ΔE‡ vs separated reactants, not ΔG‡"
                  >
                    +{s.absoluteCell.kcal.toFixed(2)} kcal/mol
                  </td>
                ) : (
                  <td className="muted" style={{ textAlign: "right", fontSize: 12 }}>
                    {s.absoluteCell.reason}
                  </td>
                )
              ) : null}
            </tr>
          ))}
        </tbody>
      </table>

      {/* ΔΔE‡ — the mission number. Shown ONLY where the guard passes; a mismatch shows
          the reason, never a faked number. */}
      <div style={{ marginTop: 12 }}>
        <h4 style={{ margin: "0 0 6px" }}>
          ΔΔE‡ {series.length > 2 ? <span className="muted">(vs {baseline.label})</span> : null}
        </h4>
        {series.length < 2 ? (
          // ΔΔE‡ is a difference of two pathways' maxima — undefined for one. Show a note,
          // never a NaN/undefined number. The per-pathway barriers above still stand.
          <p className="muted" style={{ margin: 0 }}>
            Attach a second pathway (e.g. the other face) to compute ΔΔE‡.
          </p>
        ) : (
          comparisons.map((c) => (
            <div key={c.id} style={{ marginBottom: 6 }}>
              {c.guard.ok ? (
                <div className="mono" style={{ fontSize: 15 }}>
                  ΔΔE‡({c.label} − {baseline.label}) ={" "}
                  <strong style={{ color: c.color }}>
                    {c.ddeKcal! >= 0 ? "+" : ""}
                    {c.ddeKcal!.toFixed(2)} kcal/mol
                  </strong>
                </div>
              ) : (
                <div className="banner warn" style={{ margin: 0 }}>
                  {c.label} vs {baseline.label}: {c.guard.reason}
                </div>
              )}
            </div>
          ))
        )}
      </div>

      {refIncomplete ? (
        <div className="banner warn" style={{ marginTop: 10 }}>
          Reactant reference <strong>incomplete</strong> — a reference job has no parsed energy, so no
          absolute barrier is shown. (See the Reactant reference section above for which job is missing.)
        </div>
      ) : null}

      <p className="muted scan-ts-note" style={{ marginTop: 10 }}>
        Each maximum is an <strong>approximate TS (scan maximum)</strong> — a ΔE‡ estimate on the
        relaxed surface, not a located saddle and not ΔG‡ (refine with OptTS, Stage E). ΔΔE‡ is a{" "}
        <strong>screening</strong> value; it is <strong>reference-free</strong> (the shared reactant
        reference cancels).{" "}
        {refJobCount === 0 ? (
          <>
            Absolute (vs separated reactants) barriers need a reactant reference — add optimized reactant
            jobs in the <strong>Reactant reference</strong> section above.
          </>
        ) : (
          <>
            The <strong>absolute barrier</strong> = E(max) − Σ E(reactant jobs) is also a{" "}
            <strong>screening ΔE‡</strong> vs separated reactants (barrier 3), shown only where the
            reference is complete and on the same method as the pathway.
          </>
        )}
      </p>
    </div>
  );
}
