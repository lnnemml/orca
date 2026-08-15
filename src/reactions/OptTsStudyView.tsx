import { CartesianGrid, Line, LineChart, ReferenceDot, Tooltip, XAxis, YAxis } from "recharts";

import { useContainerWidth } from "../charts/useContainerWidth";
import type { ComparePathway } from "./CompareView";

const PALETTE = ["#4f8cff", "#ff8c42", "#42c98a", "#c96ee0", "#e0c74e", "#e0576e"];
const CHART_HEIGHT = 220;

/**
 * The OptTS-ORIGIN reaction study view (Stage F3) — a THIRD pathway origin alongside scan (B1)
 * and NEB (N4). Rendered as a SIBLING of `CompareView` (not inside it) so the scan/NEB overlay
 * stays byte-identical: an OptTS-origin pathway has three stationary points (reactant basin, TS,
 * product basin), NO scan coordinate, and a barrier measured vs the **connectivity reactant
 * basin** — a different reference than the scan/NEB "vs separated reactants" columns.
 *
 * Honest-or-absent (inherited from `optTsStudy`): ΔG‡ is shown ONLY when the TS AND the reactant
 * both ran Freq (`deltaGKcal` non-null) — never a fabricated 0. The barrier's basis is labelled
 * **on the number itself** ("vs connectivity reactant basin"), so a copied value is never misread
 * as a separated-reactants ΔE‡. The ΔG‡ standard-state caveat is the same one the located-TS view
 * names (raw ORCA G = ideal-gas RRHO, 1 atm — not 1 M).
 */
export function OptTsStudyView({ pathways }: { pathways: ComparePathway[] }) {
  const { ref, width } = useContainerWidth();

  // Each OptTS-origin pathway's 3-point profile (reactant → TS → product, relative to the
  // reactant basin) + its located barriers. Colour matches the profile line.
  const studies = pathways
    .filter((p) => p.origin === "optts" && p.optTs)
    .map((p, i) => ({
      id: p.id,
      label: p.label,
      color: PALETTE[i % PALETTE.length],
      ...p.optTs!,
    }));

  if (studies.length === 0) return null;

  const sign = (v: number) => (v >= 0 ? `+${v.toFixed(2)}` : v.toFixed(2));
  const anyGibbs = studies.some((s) => s.study.deltaGKcal != null);

  return (
    <div ref={ref} style={{ marginTop: 8 }}>
      <div className="conv-chart-title" style={{ marginBottom: 4 }}>
        Located transition state — reactant → TS → product (ΔE relative to the{" "}
        <strong>connectivity reactant basin</strong>, illustrative spacing)
      </div>

      {width > 0 ? (
        <div className="conv-chart">
          <LineChart width={width} height={CHART_HEIGHT} margin={{ top: 8, right: 16, bottom: 18, left: 8 }}>
            <CartesianGrid stroke="var(--border)" strokeDasharray="3 3" />
            <XAxis
              dataKey="x"
              type="number"
              domain={[0, 1]}
              ticks={[0, 0.5, 1]}
              stroke="var(--muted)"
              fontSize={11}
              tickLine={false}
              tickFormatter={(v: number) => (v === 0 ? "reactant" : v === 1 ? "product" : "TS")}
              label={{
                value: "stationary point (illustrative spacing — no scan coordinate)",
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
                value: "ΔE vs reactant basin (kcal/mol)",
                angle: -90,
                position: "insideLeft",
                fontSize: 11,
                fill: "var(--muted-2)",
              }}
            />
            <Tooltip
              isAnimationActive={false}
              formatter={(v) => (typeof v === "number" ? `${v.toFixed(2)} kcal/mol` : String(v))}
              labelFormatter={(l) => (Number(l) === 0 ? "reactant basin" : Number(l) === 1 ? "product basin" : "transition state")}
              contentStyle={{ background: "var(--panel-2)", border: "1px solid var(--border)", borderRadius: 6, fontSize: 11 }}
            />
            {studies.map((s) => (
              <Line
                key={s.id}
                data={s.study.profile.map((pt) => ({ x: pt.x, relKcal: pt.energyKcal }))}
                dataKey="relKcal"
                name={s.label}
                stroke={s.color}
                strokeWidth={1.5}
                dot={{ r: 3 }}
                isAnimationActive={false}
              />
            ))}
            {/* The TS point of each profile — the located saddle. */}
            {studies.map((s) => {
              const ts = s.study.profile.find((pt) => pt.role === "ts");
              return ts ? (
                <ReferenceDot key={`ts-${s.id}`} x={ts.x} y={ts.energyKcal} r={5} fill={s.color} stroke="var(--panel)" strokeWidth={1} />
              ) : null;
            })}
          </LineChart>
        </div>
      ) : null}

      <table className="jobs-table" style={{ marginTop: 10 }}>
        <thead>
          <tr>
            <th>Pathway</th>
            <th style={{ textAlign: "right" }}>
              Located barrier <span className="muted">(vs connectivity reactant basin)</span>
            </th>
          </tr>
        </thead>
        <tbody>
          {studies.map((s) => (
            <tr key={s.id}>
              <td>
                <span style={{ display: "inline-block", width: 10, height: 10, borderRadius: 2, background: s.color, marginRight: 8 }} />
                {s.label}
                <div className="muted" style={{ fontSize: 11 }}>
                  reactant = <strong>{s.reactantLabel}</strong>
                  {s.reactantDesignated ? "" : " (energy-hint default — designate in the reactant reference above)"} · product = {s.productLabel}
                </div>
              </td>
              <td className="mono" style={{ textAlign: "right" }}>
                <div style={{ display: "flex", flexDirection: "column", alignItems: "flex-end" }}>
                  <span title="E(TS) − E(reactant basin): electronic barrier from the located saddle, vs the associated complex (NOT separated fragments)">
                    ΔE‡ {s.study.deltaEKcal != null ? sign(s.study.deltaEKcal) : "—"}
                  </span>
                  {s.study.deltaGKcal != null ? (
                    <strong title="G(TS) − G(reactant basin): RAW ORCA ΔG‡ (ideal-gas RRHO, 1 atm, 298.15 K)">
                      ΔG‡ {sign(s.study.deltaGKcal)}
                    </strong>
                  ) : (
                    <span className="muted" title="ΔG‡ needs a parsed Freq G on BOTH the TS and the reactant child — re-run those with Freq">
                      ΔG‡ — (needs Freq on TS + reactant)
                    </span>
                  )}
                  <span className="muted" style={{ fontSize: 10 }}>vs connectivity reactant basin</span>
                </div>
              </td>
            </tr>
          ))}
        </tbody>
      </table>

      <p className="muted" style={{ marginTop: 6, fontSize: 12 }}>
        This barrier is measured vs the <strong>connectivity reactant basin</strong> — the associated
        complex the ±imaginary-mode check relaxed to — <strong>not</strong> separated reactants (the two
        differ by the association energy). Do not read it as a separated-reactants ΔE‡.
        {anyGibbs ? (
          <>
            {" "}
            <strong>ΔG‡ is raw ORCA G</strong> (ideal-gas RRHO, 1 atm, 298.15 K); comparing an absolute ΔG‡
            to a <strong>solution</strong> experiment needs a 1 atm→1 M standard-state correction
            (~1.9 kcal/mol per molecularity change) — named here, <strong>not</strong> auto-applied.
          </>
        ) : null}
      </p>
    </div>
  );
}
