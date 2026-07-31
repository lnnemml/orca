import { useMemo, useState } from "react";
import {
  CartesianGrid,
  ComposedChart,
  Line,
  ReferenceLine,
  Tooltip,
  XAxis,
  YAxis,
  useXAxisScale,
  usePlotArea,
} from "recharts";

import type { ParsedResults } from "../types";
import { useContainerWidth } from "../charts/useContainerWidth";
import { ModeAnimator } from "./ModeAnimator";
import {
  classifyModes,
  spectrum,
  type IrMode,
  DEFAULT_FWHM_CM,
  MIN_FWHM_CM,
  MAX_FWHM_CM,
} from "./ir";
import {
  scaledModes,
  fixedGrid,
  irTooltipModel,
  DEFAULT_SCALE,
  MIN_SCALE,
  MAX_SCALE,
  SCALE_STEP,
} from "./irPresentation";

type Frequencies = NonNullable<ParsedResults["frequencies"]>;

const CHART_HEIGHT = 240;
/** Stick colour (km/mol axis). The curve is `#4f8cff`; sticks are a distinct green
 * so the two representations never read as one series. */
const STICK_COLOR = "#8fb56a";

/**
 * IR spectrum + frequency table (unit 3.8, revised unit 3.10 after the first real
 * chemist review), coordinated.
 *
 * **Two honest representations, never mixed silently (unit 3.10, rules #9/#11).**
 *  - **Sticks** — a vertical line at each mode's real wavenumber, height = its IR
 *    intensity in **km/mol**. This is the physically honest object: the spectrum IS
 *    a set of lines. Drawn on the **right** axis, in km/mol.
 *  - **Curve** — the Lorentzian-broadened sum, in **km/mol·cm⁻¹** (a density whose
 *    integral over a peak is the km/mol intensity — area-normalized, `ir.ts`), on
 *    the **left** axis. It sits *on top of* the sticks to show what broadening makes
 *    of them.
 *
 * The two are genuinely different quantities (an integrated intensity vs a density),
 * so they get **two labelled axes**. Placing them on one axis would need an
 * arbitrary conversion factor (FWHM- and lineshape-dependent) — a made-up parameter,
 * exactly what rule #11 forbids. Two axes state the truth.
 *
 * **Every non-measured element of the plot is a labelled CHOICE**, not a molecular
 * property: the FWHM (slider), the display scale factor (slider, default 1.00 — NOT
 * baked-in per method, NOT the artifact's own `$frequency_scale_factor`), and the
 * inverted view (a *conventional depiction*, explicitly NOT transmittance — no
 * Beer–Lambert law is applied).
 *
 * The stick list (`.hess`, already parsed) is split by MEASURED FACT — exact-zero
 * translation/rotation modes and negative (imaginary) modes are NOT broadened into
 * the curve. Imaginary modes are shown **separately as a diagnosis**.
 *
 * Click a stick → the matching frequency row selects, and vice-versa. No mode
 * ANIMATION here — that is unit 3.9, behind the Kabsch gate.
 */
type FinalGeometry = ParsedResults["final_geometry"];

export function IrSpectrumPanel({
  f,
  geometry,
}: {
  f: Frequencies;
  /** The final/reference geometry — the equilibrium the modes animate around, and
   * the element order the modes must match (unit 3.12). `null` disables animation. */
  geometry: FinalGeometry | null;
}) {
  const [fwhm, setFwhm] = useState(DEFAULT_FWHM_CM);
  // Display scale factor — a PLOT choice (like FWHM), default 1.00. See irPresentation.
  const [scale, setScale] = useState(DEFAULT_SCALE);
  // Inverted view (peaks drawn downward) — a conventional depiction, NOT transmittance.
  const [inverted, setInverted] = useState(false);
  // The selected mode's ORIGINAL index into f.frequencies_cm (shared by the chart
  // and the table); null = nothing selected.
  const [selected, setSelected] = useState<number | null>(null);
  const { ref, width } = useContainerWidth();

  const { active, imaginary, zeroCount } = useMemo(
    () => classifyModes(f.frequencies_cm, f.ir_intensity_km_mol),
    [f.frequencies_cm, f.ir_intensity_km_mol],
  );

  // Scaling is applied by transforming the mode list fed to the physics module, so
  // the curve and the sticks share one (scaled) x-axis. ir.ts is untouched.
  const scaledActive = useMemo(() => scaledModes(active, scale), [active, scale]);
  // The x-grid is FIXED — derived from the RAW modes and the slider's full range, NOT
  // the current scale (unit 3.11). This is what makes the slider useful: the peaks
  // slide against a stationary ruler. Depends on `active` + `fwhm`, never `scale`.
  const grid = useMemo(() => fixedGrid(active, fwhm), [active, fwhm]);
  const curve = useMemo(() => spectrum(scaledActive, grid, fwhm), [scaledActive, grid, fwhm]);

  // Right-axis (km/mol) domain for the sticks — explicit, with a little headroom so
  // the tallest peak is not flush against the top. Positioning uses this same max,
  // so the drawn sticks and the rendered axis ticks agree.
  const stickMaxRaw = useMemo(
    () => active.reduce((m, s) => Math.max(m, s.kmMol), 0),
    [active],
  );
  const stickAxisMax = stickMaxRaw > 0 ? stickMaxRaw * 1.05 : 1;

  const verdict = verdictFor(f.imaginary_count);
  const selectedCmScaled =
    selected != null && f.frequencies_cm[selected] > 0
      ? f.frequencies_cm[selected] * scale
      : null;

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
            <button
              key={m.index}
              type="button"
              className={
                "ir-imaginary-mode mono" + (m.index === selected ? " selected" : "")
              }
              onClick={() => setSelected(m.index === selected ? null : m.index)}
              title="animate this mode — the reaction coordinate"
            >
              {m.cm.toFixed(2)} cm⁻¹
            </button>
          ))}
          <div className="muted" style={{ fontSize: 11, marginTop: 2 }}>
            a negative (imaginary) frequency is the signature of a transition
            state — click it to animate the reaction coordinate.
          </div>
        </div>
      ) : null}

      {active.length > 0 && width > 0 ? (
        <div className="conv-chart" style={{ marginTop: 6 }}>
          <div className="conv-chart-title">
            IR — sticks (km/mol) with the Lorentzian-broadened curve
            {inverted ? " · inverted view (a conventional depiction, not transmittance)" : ""}
          </div>
          <ComposedChart
            width={width}
            height={CHART_HEIGHT}
            data={curve}
            margin={{ top: 8, right: 20, bottom: 18, left: 12 }}
          >
            <CartesianGrid stroke="var(--border)" strokeDasharray="3 3" />
            <XAxis
              dataKey="cm"
              type="number"
              domain={[grid.min, grid.max]}
              stroke="var(--muted)"
              fontSize={11}
              tickLine={false}
              label={{ value: "wavenumber (cm⁻¹)", position: "insideBottom", offset: -8, fontSize: 11, fill: "var(--muted-2)" }}
            />
            {/* Left axis — the broadened curve, a density in km/mol·cm⁻¹. */}
            <YAxis
              yAxisId="curve"
              stroke="#4f8cff"
              fontSize={11}
              width={62}
              tickLine={false}
              domain={[0, "auto"]}
              reversed={inverted}
              tickFormatter={(v: number) => v.toFixed(1)}
              label={{ value: "curve (km/mol·cm⁻¹)", angle: -90, position: "insideLeft", fontSize: 11, fill: "#4f8cff", style: { textAnchor: "middle" } }}
            />
            {/* Right axis — the sticks, an integrated intensity in km/mol. */}
            <YAxis
              yAxisId="stick"
              orientation="right"
              stroke={STICK_COLOR}
              fontSize={11}
              width={58}
              tickLine={false}
              type="number"
              domain={[0, stickAxisMax]}
              reversed={inverted}
              tickFormatter={(v: number) => v.toFixed(0)}
              label={{ value: "sticks (km/mol)", angle: 90, position: "insideRight", fontSize: 11, fill: STICK_COLOR, style: { textAnchor: "middle" } }}
            />
            <Tooltip
              isAnimationActive={false}
              content={<IrTooltipContent modes={scaledActive} />}
            />
            {/* Sticks drawn UNDER the curve (curve on top). Positioned from the plot
                area + the explicit km/mol max, so they do not depend on the right
                axis' internal scale being registered. */}
            <IrSticks
              modes={scaledActive}
              stickMax={stickAxisMax}
              inverted={inverted}
              selected={selected}
              onSelect={(i) => setSelected(i === selected ? null : i)}
            />
            {selectedCmScaled != null ? (
              <ReferenceLine
                yAxisId="curve"
                x={selectedCmScaled}
                stroke="var(--accent)"
                strokeWidth={1}
                strokeDasharray="4 3"
              />
            ) : null}
            <Line
              yAxisId="curve"
              type="monotone"
              dataKey="absorbance"
              stroke="#4f8cff"
              strokeWidth={1.5}
              dot={false}
              isAnimationActive={false}
            />
          </ComposedChart>

          {/* Plot construction, stated as numbers and labelled choices. */}
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

            <label className="ir-fwhm">
              display scale {scale.toFixed(3)}
              <input
                type="range"
                min={MIN_SCALE}
                max={MAX_SCALE}
                step={SCALE_STEP}
                value={scale}
                onChange={(e) => setScale(Number(e.target.value))}
                aria-label="frequency display scale factor"
              />
              {scale !== DEFAULT_SCALE ? (
                <button
                  type="button"
                  className="ir-reset"
                  onClick={() => setScale(DEFAULT_SCALE)}
                  title="reset to 1.000 (raw frequencies)"
                >
                  reset
                </button>
              ) : null}
            </label>

            <div className="ir-view-toggle" role="group" aria-label="spectrum orientation">
              <button
                type="button"
                className={!inverted ? "active" : ""}
                onClick={() => setInverted(false)}
              >
                peaks up
              </button>
              <button
                type="button"
                className={inverted ? "active" : ""}
                onClick={() => setInverted(true)}
              >
                peaks down
              </button>
            </div>
          </div>

          <div className="muted" style={{ fontSize: 11, marginTop: 4 }}>
            grid {grid.min}–{grid.max} cm⁻¹, step {grid.step} · Lorentzian · FWHM is a
            plot choice, not a molecular property. Left axis km/mol·cm⁻¹ (broadened
            density), right axis km/mol (stick intensity) — two different quantities,
            not one axis.
          </div>

          {scale !== DEFAULT_SCALE ? (
            <div className="muted" style={{ fontSize: 11, marginTop: 2 }}>
              display scale {scale.toFixed(3)} — your choice, applied to the drawing;
              it is NOT a property of the molecule. Harmonic frequencies run high, but
              we bake in no method-specific factor: the raw and scaled columns are
              both in the table below. Pick a citable value for your method.
            </div>
          ) : null}

          {inverted ? (
            <div className="muted" style={{ fontSize: 11, marginTop: 2 }}>
              inverted view — peaks drawn downward, a conventional way to depict an IR
              spectrum. This is <strong>not transmittance</strong>: converting
              intensity to %T needs the Beer–Lambert law (path length, concentration),
              which a calculation does not contain. The axes and values are unchanged;
              only the drawing direction is flipped.
            </div>
          ) : null}
        </div>
      ) : null}

      {/* Normal-mode animation (unit 3.12, Part B) — a mode is selected (peak, row,
          or imaginary click) and its atoms move. Gated behind the 3.12 Kabsch
          determiner (modes added as-is). Hidden when there is no geometry or no
          `$normal_modes` (the table still stands). */}
      {selected != null &&
      geometry != null &&
      f.n_modes > 0 &&
      f.normal_modes.length === f.n_modes * f.n_modes ? (
        <ModeAnimator
          elements={f.elements}
          equilibrium={geometry.xyz_angstrom}
          referenceElements={geometry.elements}
          normalModes={f.normal_modes}
          nModes={f.n_modes}
          modeIndex={selected}
          frequencyCm={f.frequencies_cm[selected]}
        />
      ) : null}

      {/* The frequency table — active (real) modes, clickable rows, selection in
          sync with the chart. A scaled column appears only when scale ≠ 1.00. */}
      <FrequencyTable
        active={active}
        zeroCount={zeroCount}
        scale={scale}
        selected={selected}
        onSelect={setSelected}
      />

      {/* The artifact's OWN scale factor — shown to explain why it changes nothing:
          measured 1.0 means ORCA applied none. It is not the display scale above. */}
      {f.scale_factor != null ? (
        <div className="muted" style={{ fontSize: 11, marginTop: 4 }}>
          artifact <code>$frequency_scale_factor</code> = {f.scale_factor.toFixed(6)}
          {f.scale_factor === 1
            ? " — the factor ORCA already applied to these frequencies (1.000000 = none). Not a recommended value for the method; the display scale above is the researcher's choice."
            : " — the factor ORCA already applied; the display scale above is applied on top of it."}
        </div>
      ) : null}
    </section>
  );
}

/**
 * The sticks: one vertical line per mode, from the km/mol baseline to its intensity,
 * plus a wide transparent hit-target for click-to-select. Positioned from the plot
 * area (`usePlotArea`) and the explicit `stickMax` rather than the right axis' own
 * scale — a linear [0, stickMax] map over the plot height matches the axis ticks
 * (same domain, same range) and does not depend on a data-less axis registering a
 * scale. `inverted` flips the baseline to the top so peaks point down.
 */
function IrSticks({
  modes,
  stickMax,
  inverted,
  selected,
  onSelect,
}: {
  modes: IrMode[];
  stickMax: number;
  inverted: boolean;
  selected: number | null;
  onSelect: (index: number) => void;
}) {
  const xScale = useXAxisScale();
  const plot = usePlotArea();
  if (!xScale || !plot || stickMax <= 0) return null;

  const baseline = inverted ? plot.y : plot.y + plot.height; // pixel of value 0
  const pixelOf = (kmMol: number) =>
    inverted
      ? plot.y + (kmMol / stickMax) * plot.height
      : plot.y + plot.height - (kmMol / stickMax) * plot.height;

  return (
    <g className="ir-sticks">
      {modes.map((m) => {
        const px = xScale(m.cm);
        if (px == null) return null;
        const py = pixelOf(m.kmMol);
        const isSel = m.index === selected;
        const top = Math.min(baseline, py);
        const h = Math.abs(baseline - py);
        return (
          <g key={m.index}>
            <line
              x1={px}
              x2={px}
              y1={baseline}
              y2={py}
              stroke={isSel ? "var(--accent)" : STICK_COLOR}
              strokeWidth={isSel ? 2 : 1}
            />
            <rect
              x={px - 4}
              y={top}
              width={8}
              height={Math.max(h, 3)}
              fill="transparent"
              style={{ cursor: "pointer" }}
              onClick={() => onSelect(m.index)}
            >
              <title>
                {m.cm.toFixed(1)} cm⁻¹ · {m.kmMol.toFixed(1)} km/mol
              </title>
            </rect>
          </g>
        );
      })}
    </g>
  );
}

/**
 * Single-source tooltip: everything derives from the ONE wavenumber under the cursor
 * (`label`) and the curve value the chart read there (`payload[0].value`). The
 * sticks are NOT a chart series (they are drawn SVG), so recharts has nothing to
 * merge — the unit-3.10 two-series mix is structurally impossible here. The nearest
 * mode is labelled as nearest, with its distance, never as "the value at this point".
 */
function IrTooltipContent(props: {
  active?: boolean;
  label?: number | string;
  payload?: ReadonlyArray<{ value?: number | string }>;
  modes: IrMode[];
}) {
  const { active, label, payload, modes } = props;
  if (!active || payload == null || payload.length === 0) return null;
  const cm = Number(label);
  const curveVal = Number(payload[0]?.value ?? 0);
  const model = irTooltipModel(cm, curveVal, modes);
  return (
    <div className="ir-tooltip mono">
      <div className="ir-tooltip-head">{model.cm.toFixed(0)} cm⁻¹</div>
      <div>
        curve {model.curve.toFixed(3)}{" "}
        <span className="muted">km/mol·cm⁻¹</span>
      </div>
      {model.nearest ? (
        <div className="ir-tooltip-mode">
          nearest mode {model.nearest.cm.toFixed(1)} cm⁻¹ ·{" "}
          {model.nearest.kmMol.toFixed(1)} km/mol{" "}
          <span className="muted">(Δ {model.nearest.deltaCm.toFixed(0)} cm⁻¹)</span>
        </div>
      ) : null}
    </div>
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

/** Number of side-by-side columns the frequency table flows into (93 modes in one
 * column is a scroll). On a narrow window the flex container wraps them (CSS). */
const FREQ_TABLE_COLUMNS = 3;

function FrequencyTable({
  active,
  zeroCount,
  scale,
  selected,
  onSelect,
}: {
  active: ReturnType<typeof classifyModes>["active"];
  zeroCount: number;
  scale: number;
  selected: number | null;
  onSelect: (index: number | null) => void;
}) {
  const showScaled = scale !== DEFAULT_SCALE;
  // Flow the modes into N balanced columns, keeping the 1-based running number
  // continuous down each column (so "#" reads 1..N across the whole table).
  const perColumn = Math.ceil(active.length / FREQ_TABLE_COLUMNS);
  const columns = Array.from({ length: FREQ_TABLE_COLUMNS }, (_, c) =>
    active
      .slice(c * perColumn, (c + 1) * perColumn)
      .map((m, j) => ({ mode: m, ordinal: c * perColumn + j + 1 })),
  ).filter((col) => col.length > 0);

  return (
    <div style={{ marginTop: 8 }}>
      <div className="ir-table-columns">
        {columns.map((col, c) => (
          <table
            key={c}
            className="mono ir-table"
            style={{ fontSize: 12, borderCollapse: "collapse" }}
          >
            <thead>
              <tr>
                <th style={{ textAlign: "right", paddingRight: 12, color: "var(--muted)" }}>#</th>
                <th style={{ textAlign: "right", paddingRight: 12, color: "var(--muted)" }}>cm⁻¹ (raw)</th>
                {showScaled ? (
                  <th style={{ textAlign: "right", paddingRight: 12, color: "var(--muted)" }}>
                    ×{scale.toFixed(3)}
                  </th>
                ) : null}
                <th style={{ textAlign: "right", color: "var(--muted)" }}>km/mol</th>
              </tr>
            </thead>
            <tbody>
              {col.map(({ mode: m, ordinal }) => (
                <tr
                  key={m.index}
                  className={"ir-row" + (m.index === selected ? " selected" : "")}
                  onClick={() => onSelect(m.index === selected ? null : m.index)}
                >
                  <td style={{ textAlign: "right", paddingRight: 12, color: "var(--muted)" }}>
                    {ordinal}
                  </td>
                  <td style={{ textAlign: "right", paddingRight: 12 }}>{m.cm.toFixed(2)}</td>
                  {showScaled ? (
                    <td style={{ textAlign: "right", paddingRight: 12, color: "var(--muted)" }}>
                      {(m.cm * scale).toFixed(2)}
                    </td>
                  ) : null}
                  <td style={{ textAlign: "right" }}>{m.kmMol.toFixed(2)}</td>
                </tr>
              ))}
            </tbody>
          </table>
        ))}
      </div>
      {showScaled ? (
        <div className="muted" style={{ fontSize: 11, marginTop: 2 }}>
          the ×{scale.toFixed(3)} column is the scaled wavenumber (derived), not a molecular property.
        </div>
      ) : null}
      {zeroCount > 0 ? (
        <div className="muted" style={{ fontSize: 11, marginTop: 4 }}>
          {zeroCount} translation/rotation modes at exactly 0 cm⁻¹ (excluded — not
          vibrations).
        </div>
      ) : null}
    </div>
  );
}
