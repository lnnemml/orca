import { useMemo, useState } from "react";
import {
  CartesianGrid,
  ComposedChart,
  Line,
  ReferenceLine,
  Scatter,
  Tooltip,
  XAxis,
  YAxis,
} from "recharts";

import type { ParsedResults } from "../types";
import { useContainerWidth } from "../charts/useContainerWidth";
import {
  classifyModes,
  autoGrid,
  spectrum,
  lorentzian,
  DEFAULT_FWHM_CM,
  MIN_FWHM_CM,
  MAX_FWHM_CM,
} from "./ir";

type Frequencies = NonNullable<ParsedResults["frequencies"]>;

const CHART_HEIGHT = 220;

/**
 * IR spectrum + frequency table (unit 3.8, Part B), coordinated.
 *
 * The stick list (`.hess`, already parsed) is split by MEASURED FACT — exact-zero
 * translation/rotation modes and negative (imaginary) modes are NOT broadened
 * into the curve (a Lorentzian would smear a peak onto/through zero). The
 * imaginary modes are shown **separately as a diagnosis** — an imaginary
 * frequency means a transition state, not noise (the teaching moment, kept).
 *
 * Three plot choices are made explicit in the UI, because they are properties of
 * the GRAPH, not the molecule: the line shape (Lorentzian), the FWHM (a slider),
 * and the x-grid (range + step, shown). The area under a peak equals its km/mol
 * intensity (area-normalized Lorentzian — `ir.ts`), cross-checked against ORCA's
 * `orca_mapspc` (`wiki/orca/parse-sources.md`).
 *
 * Click a peak → the matching frequency row selects, and vice-versa. There is no
 * mode ANIMATION here — that is unit 3.9, behind the Kabsch gate.
 */
export function IrSpectrumPanel({ f }: { f: Frequencies }) {
  const [fwhm, setFwhm] = useState(DEFAULT_FWHM_CM);
  // The selected mode's ORIGINAL index into f.frequencies_cm (shared by the
  // chart and the table); null = nothing selected.
  const [selected, setSelected] = useState<number | null>(null);
  const { ref, width } = useContainerWidth();

  const { active, imaginary, zeroCount } = useMemo(
    () => classifyModes(f.frequencies_cm, f.ir_intensity_km_mol),
    [f.frequencies_cm, f.ir_intensity_km_mol],
  );
  const grid = useMemo(() => autoGrid(active, fwhm), [active, fwhm]);
  const curve = useMemo(() => spectrum(active, grid, fwhm), [active, grid, fwhm]);

  // Clickable markers sitting ON the curve at each mode centre (so a "peak" is a
  // real target). Height = the curve value at the mode's wavenumber.
  const markers = useMemo(
    () =>
      active.map((m) => ({
        cm: m.cm,
        y: active.reduce((s, o) => s + o.kmMol * lorentzian(m.cm, o.cm, fwhm), 0),
        index: m.index,
      })),
    [active, fwhm],
  );

  const verdict = verdictFor(f.imaginary_count);
  const selectedCm =
    selected != null ? f.frequencies_cm[selected] : null;

  return (
    <section ref={ref}>
      <div className="section-title" style={{ fontSize: 12 }}>
        Vibrational spectrum
      </div>

      {/* Minimum / TS / neither — the teaching verdict, from imaginary_count. */}
      <div
        className="mono"
        style={{ fontSize: 12, marginBottom: 6, color: verdict.tone, fontWeight: 600 }}
      >
        {verdict.text}
      </div>

      {/* Imaginary modes shown SEPARATELY — a diagnosis, never dropped and never
          broadened into the spectrum. */}
      {imaginary.length > 0 ? (
        <div className="ir-imaginary">
          <span className="ir-imaginary-tag">imaginary (excluded from the spectrum):</span>{" "}
          {imaginary.map((m) => (
            <span key={m.index} className="ir-imaginary-mode mono">
              {m.cm.toFixed(2)} cm⁻¹
            </span>
          ))}
          <div className="muted" style={{ fontSize: 11, marginTop: 2 }}>
            a negative (imaginary) frequency is the signature of a transition
            state — see the verdict above.
          </div>
        </div>
      ) : null}

      {active.length > 0 && width > 0 ? (
        <div className="conv-chart" style={{ marginTop: 6 }}>
          <div className="conv-chart-title">
            IR — Lorentzian broadened (area under a peak = its km/mol intensity)
          </div>
          <ComposedChart
            width={width}
            height={CHART_HEIGHT}
            data={curve}
            margin={{ top: 8, right: 16, bottom: 16, left: 8 }}
          >
            <CartesianGrid stroke="var(--border)" strokeDasharray="3 3" />
            <XAxis
              dataKey="cm"
              type="number"
              domain={[grid.min, grid.max]}
              stroke="var(--muted)"
              fontSize={11}
              tickLine={false}
              label={{ value: "wavenumber (cm⁻¹)", position: "insideBottom", offset: -6, fontSize: 11, fill: "var(--muted-2)" }}
            />
            <YAxis
              stroke="var(--muted)"
              fontSize={11}
              width={56}
              tickLine={false}
              domain={[0, "auto"]}
              tickFormatter={(v: number) => v.toFixed(1)}
            />
            <Tooltip
              isAnimationActive={false}
              formatter={(v) => (typeof v === "number" ? v.toFixed(3) : String(v))}
              labelFormatter={(l) => `${Number(l).toFixed(0)} cm⁻¹`}
              contentStyle={{
                background: "var(--panel-2)",
                border: "1px solid var(--border)",
                borderRadius: 6,
                fontSize: 11,
              }}
            />
            {selectedCm != null && selectedCm > 0 ? (
              <ReferenceLine x={selectedCm} stroke="var(--accent)" strokeWidth={1.5} />
            ) : null}
            <Line
              type="monotone"
              dataKey="absorbance"
              stroke="#4f8cff"
              strokeWidth={1.5}
              dot={false}
              isAnimationActive={false}
            />
            {/* Clickable peak markers — selecting one highlights its table row. */}
            <Scatter
              data={markers}
              dataKey="y"
              fill="#b1eb70"
              onClick={(p: { payload?: { index?: number } }) => {
                // recharts wraps the original datum in `payload` on the point.
                const i = p?.payload?.index;
                if (i != null) setSelected(i);
              }}
              isAnimationActive={false}
            />
          </ComposedChart>

          {/* Plot construction, stated as numbers — not eyeballed. */}
          <div className="ir-controls">
            <label className="ir-fwhm">
              FWHM {fwhm} cm⁻¹
              <input
                type="range"
                min={MIN_FWHM_CM}
                max={MAX_FWHM_CM}
                step={1}
                value={fwhm}
                onChange={(e) => setFwhm(Number(e.target.value))}
                aria-label="Lorentzian FWHM"
              />
            </label>
            <span className="muted" style={{ fontSize: 11 }}>
              grid {grid.min}–{grid.max} cm⁻¹, step {grid.step} · Lorentzian ·
              FWHM is a plot choice, not a molecular property
            </span>
          </div>
        </div>
      ) : null}

      {/* The frequency table — active (real) modes, clickable rows, selection in
          sync with the chart. */}
      <FrequencyTable
        active={active}
        zeroCount={zeroCount}
        selected={selected}
        onSelect={setSelected}
      />

      {f.scale_factor != null && f.scale_factor !== 1 ? (
        <div className="muted" style={{ fontSize: 11, marginTop: 4 }}>
          frequency scale factor {f.scale_factor}
        </div>
      ) : null}
    </section>
  );
}

function verdictFor(imaginaryCount: number): { text: string; tone: string } {
  if (imaginaryCount === 0)
    return { text: "Minimum — 0 imaginary frequencies.", tone: "var(--muted)" };
  if (imaginaryCount === 1)
    return {
      text: "Transition state — exactly 1 imaginary frequency.",
      tone: "var(--text)",
    };
  return {
    text: `Neither a minimum nor a transition state — ${imaginaryCount} imaginary frequencies; re-optimize.`,
    tone: "var(--text)",
  };
}

function FrequencyTable({
  active,
  zeroCount,
  selected,
  onSelect,
}: {
  active: ReturnType<typeof classifyModes>["active"];
  zeroCount: number;
  selected: number | null;
  onSelect: (index: number | null) => void;
}) {
  return (
    <div style={{ marginTop: 8 }}>
      <table className="mono ir-table" style={{ fontSize: 12, borderCollapse: "collapse" }}>
        <thead>
          <tr>
            <th style={{ textAlign: "right", paddingRight: 12, color: "var(--muted)" }}>#</th>
            <th style={{ textAlign: "right", paddingRight: 12, color: "var(--muted)" }}>cm⁻¹</th>
            <th style={{ textAlign: "right", color: "var(--muted)" }}>IR km/mol</th>
          </tr>
        </thead>
        <tbody>
          {active.map((m, i) => (
            <tr
              key={m.index}
              className={"ir-row" + (m.index === selected ? " selected" : "")}
              onClick={() => onSelect(m.index === selected ? null : m.index)}
            >
              <td style={{ textAlign: "right", paddingRight: 12, color: "var(--muted)" }}>
                {i + 1}
              </td>
              <td style={{ textAlign: "right", paddingRight: 12 }}>{m.cm.toFixed(2)}</td>
              <td style={{ textAlign: "right" }}>{m.kmMol.toFixed(2)}</td>
            </tr>
          ))}
        </tbody>
      </table>
      {zeroCount > 0 ? (
        <div className="muted" style={{ fontSize: 11, marginTop: 4 }}>
          {zeroCount} translation/rotation modes at exactly 0 cm⁻¹ (excluded — not
          vibrations).
        </div>
      ) : null}
    </div>
  );
}
