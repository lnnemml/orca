import { useEffect, useMemo, useRef, useState } from "react";
import {
  CartesianGrid,
  Line,
  LineChart,
  ReferenceLine,
  Tooltip,
  XAxis,
  YAxis,
} from "recharts";

import { MoleculeViewer } from "../viewer/MoleculeViewer";
import { resultsBondLabel } from "./bondReadout";
import type { MayerBond } from "../types";
import { useContainerWidth } from "../charts/useContainerWidth";
import { resolveClickedIndex, type ChartClickState } from "../charts/clickIndex";
import { saveBytes, exportName } from "../export/save";
import { svgToPngBytes } from "../export/png";
import {
  frameToXyz,
  frameLabel,
  frameEnergyText,
  frameDeltaKcal,
  energySeries,
  elementsAgree,
  type Frame,
} from "./frame";

/**
 * Optimization-trajectory playback (unit 3.8, Part A).
 *
 * **Who owns the frame number:** this component (React state), NOT the viewer.
 * The viewer is a dumb renderer (ADR-011) handed ONE frame's geometry; it has no
 * frame list and no timer of its own. The play timer lives here (a `setInterval`
 * in a `useEffect`), so the current frame stays in lock-step with the label, the
 * energy readout, and the highlighted point on the E(cycle) chart — a timer
 * buried in 3Dmol could not be synchronised with any of them, and Phase 4.2 will
 * swap the renderer (ADR-011 gate), so nothing may migrate into its state.
 *
 * This is the same ownership call the geometry editor makes for `selection`
 * (view-local UI state held by the screen, not the geometry store): the frame
 * number is ephemeral view state, so it is component state, not the scene store.
 */

/** Frames/second presets for the play timer — a UI choice, not a molecule
 * property. The timer ticks in the app layer at the chosen rate. */
const SPEEDS = [
  { label: "0.5×", fps: 2 },
  { label: "1×", fps: 5 },
  { label: "2×", fps: 10 },
  { label: "4×", fps: 20 },
];
const DEFAULT_FPS = 5;
const CHART_HEIGHT = 170;

interface TrajectoryPlayerProps {
  /** The trajectory's element order (stored once, `data_json.trajectory`). */
  elements: string[];
  frames: Frame[];
  /**
   * The element order the rest of the results (the summary card / final
   * geometry) are shown in — the reference the trajectory must match before we
   * animate it. A mismatch is surfaced as an error, never a silent substitution
   * (the UI-boundary echo of the readers' element-order post-condition).
   */
  referenceElements: string[];
  /** For export filenames (unit 3.16). */
  jobTitle: string;
  /** Mayer bond orders (final structure), or null. When an entry exists for a picked
   * pair its COMPUTED order is shown as authoritative; otherwise the readout falls
   * back to the geometric estimate from the displayed frame (`bondReadout.ts`). */
  mayerBondOrders?: MayerBond[] | null;
}

export function TrajectoryPlayer({
  elements,
  frames,
  referenceElements,
  jobTitle,
  mayerBondOrders,
}: TrajectoryPlayerProps) {
  const [frame, setFrame] = useState(0);
  const [playing, setPlaying] = useState(false);
  const [fps, setFps] = useState(DEFAULT_FPS);
  // A 2-atom pick (0-based indices, == final_geometry/frame order) for the bond-order
  // readout. Clicking an already-picked atom removes it; a third pick drops the oldest.
  const [picked, setPicked] = useState<number[]>([]);
  const pickAtom = (idx: number) =>
    setPicked((p) => (p.includes(idx) ? p.filter((x) => x !== idx) : [...p, idx].slice(-2)));
  const { ref, width } = useContainerWidth();
  // The energy chart container — its recharts `<svg>` is grabbed for the PNG export.
  const energyChartRef = useRef<HTMLDivElement | null>(null);

  const total = frames.length;
  const last = total - 1;

  // Identity check BEFORE anything is drawn: the trajectory's atom sequence must
  // equal the one the static scene/card is drawn in. Discrepancy → error, stop.
  const identityOk = elementsAgree(elements, referenceElements);

  // The one frame handed to the viewer. Guarded so a stale index (frames
  // shrank — shouldn't happen, but the boundary is cheap) never throws.
  const clamped = Math.min(Math.max(frame, 0), last);
  const frameXyz = useMemo(() => {
    const f = frames[clamped];
    return f ? frameToXyz(elements, f) : "";
  }, [frames, clamped, elements]);

  const series = useMemo(() => energySeries(frames), [frames]);

  // The bond-order readout for the current 2-atom pick: geometric estimate from THIS
  // frame's geometry, or the authoritative Mayer value when the run computed one for
  // the pair. Rendered under whichever viewer is shown (single-frame or animated).
  const bondReadout = () => {
    if (picked.length !== 2) return null;
    const [i, j] = picked;
    const label = resultsBondLabel(
      elements,
      frames[clamped]?.xyz_angstrom ?? [],
      mayerBondOrders ?? null,
      i,
      j,
    );
    const chip = (k: number) => `${elements[k] ?? "?"}#${k}`;
    return (
      <div className="traj-bond-readout mono">
        <span className="traj-bond-pair">
          {chip(i)} ··· {chip(j)}
        </span>
        <span className="traj-bond-order">{label ?? "not a bond (no computed order)"}</span>
        <button className="btn btn-sm" onClick={() => setPicked([])} title="Clear bond selection">
          Clear
        </button>
      </div>
    );
  };

  // The play timer — advance one frame per tick, stop at the last (a natural
  // "play once"; pressing play again from the end restarts). Lives here, not in
  // the viewer, so pause/scrub/label all stay in sync.
  useEffect(() => {
    if (!playing || total <= 1) return;
    const id = setInterval(() => {
      setFrame((f) => {
        if (f >= last) {
          setPlaying(false);
          return f;
        }
        return f + 1;
      });
    }, 1000 / fps);
    return () => clearInterval(id);
  }, [playing, fps, total, last]);

  const togglePlay = () => {
    if (frame >= last) setFrame(0); // restart from the top if parked at the end
    setPlaying((p) => !p);
  };
  const step = (delta: number) => {
    setPlaying(false);
    setFrame((f) => Math.min(Math.max(f + delta, 0), last));
  };

  if (!identityOk) {
    return (
      <section className="traj-player">
        <div className="section-title" style={{ fontSize: 12 }}>
          Optimization trajectory
        </div>
        <div className="banner err" style={{ marginTop: 6 }}>
          Trajectory atom order does not match the result geometry (
          {elements.length} vs {referenceElements.length} atoms / different
          sequence). Not animating — this would draw the wrong molecule.
        </div>
      </section>
    );
  }

  // One frame → nothing to play: show the static geometry, no controls.
  if (total <= 1) {
    return (
      <section className="traj-player">
        <div className="section-title" style={{ fontSize: 12 }}>
          Geometry
        </div>
        <div className="viewer-panel traj-viewer">
          <MoleculeViewer xyzData={frameXyz} preserveCameraOnUpdate onXyzAtomPick={pickAtom} />
        </div>
        {bondReadout()}
      </section>
    );
  }

  const cur = frames[clamped];
  const delta = frameDeltaKcal(frames, clamped);

  return (
    <section className="traj-player" ref={ref}>
      <div className="section-title" style={{ fontSize: 12 }}>
        Optimization trajectory
      </div>

      <div className="viewer-panel traj-viewer">
        <MoleculeViewer xyzData={frameXyz} preserveCameraOnUpdate onXyzAtomPick={pickAtom} />
      </div>
      {bondReadout()}

      {/* Honest label — optimization CYCLES, never "scan steps". */}
      <div className="traj-readout mono">
        <span className="traj-cycle">{frameLabel(clamped, total)}</span>
        <span className="traj-energy">{frameEnergyText(cur)}</span>
        {delta != null ? (
          <span className="traj-delta muted">
            ΔE {delta >= 0 ? "+" : ""}
            {delta.toFixed(3)} kcal/mol vs cycle 1
          </span>
        ) : null}
      </div>

      <div className="traj-transport">
        <button className="btn btn-sm" onClick={() => step(-last)} title="First cycle">
          ⏮
        </button>
        <button className="btn btn-sm" onClick={() => step(-1)} title="Previous">
          ◀
        </button>
        <button className="btn btn-sm" onClick={togglePlay} title={playing ? "Pause" : "Play"}>
          {playing ? "⏸" : "▶"}
        </button>
        <button className="btn btn-sm" onClick={() => step(1)} title="Next">
          ▶
        </button>
        <button className="btn btn-sm" onClick={() => step(last)} title="Last cycle">
          ⏭
        </button>
        <input
          className="traj-slider"
          type="range"
          min={0}
          max={last}
          value={clamped}
          onChange={(e) => {
            setPlaying(false);
            setFrame(Number(e.target.value));
          }}
          aria-label="trajectory cycle"
        />
        <select
          className="select select-sm"
          value={fps}
          onChange={(e) => setFps(Number(e.target.value))}
          aria-label="playback speed"
        >
          {SPEEDS.map((s) => (
            <option key={s.fps} value={s.fps}>
              {s.label}
            </option>
          ))}
        </select>
      </div>

      {/* Energy per cycle — the core learning view: watch it descend. Click a
          point to jump to that cycle; the current cycle is a vertical marker. */}
      {series.length >= 2 && width > 0 ? (
        <div className="conv-chart" ref={energyChartRef}>
          <div className="conv-chart-title">
            Energy per cycle (Eh) — click to jump
            <button
              className="btn btn-sm"
              style={{ marginLeft: 10 }}
              onClick={async () => {
                try {
                  const svg = energyChartRef.current?.querySelector("svg");
                  if (svg)
                    await saveBytes(exportName(jobTitle, "energy-per-cycle", "png"), await svgToPngBytes(svg));
                } catch (e) {
                  console.error("[export]", e);
                }
              }}
            >
              PNG
            </button>
          </div>
          <LineChart
            width={width}
            height={CHART_HEIGHT}
            data={series}
            margin={{ top: 8, right: 16, bottom: 4, left: 8 }}
            onClick={(state: ChartClickState) => {
              // recharts v3 hands the clicked category index back as a STRING — resolve
              // through the shared helper (number/string index, then activeLabel=cycle
              // fallback), then map the array position to the ORIGINAL frame index (the
              // series drops null-energy frames, so position ≠ frame index).
              const pos = resolveClickedIndex(state, series, (d) => d.cycle);
              if (pos != null) setFrame(series[pos].index);
              else if (import.meta.env.DEV) console.warn("[chart click] trajectory unresolved", state);
            }}
          >
            <CartesianGrid stroke="var(--border)" strokeDasharray="3 3" />
            <XAxis
              dataKey="cycle"
              stroke="var(--muted)"
              fontSize={11}
              tickLine={false}
              label={{ value: "cycle", position: "insideBottom", offset: -2, fontSize: 11, fill: "var(--muted-2)" }}
            />
            <YAxis
              stroke="var(--muted)"
              fontSize={11}
              width={92}
              tickLine={false}
              domain={["auto", "auto"]}
              tickFormatter={(v: number) => v.toFixed(6)}
            />
            <Tooltip
              isAnimationActive={false}
              formatter={(v) => (typeof v === "number" ? `${v.toFixed(8)} Eh` : String(v))}
              labelFormatter={(l) => `cycle ${l}`}
              contentStyle={{
                background: "var(--panel-2)",
                border: "1px solid var(--border)",
                borderRadius: 6,
                fontSize: 11,
              }}
            />
            <ReferenceLine x={clamped + 1} stroke="var(--accent)" strokeWidth={1.5} />
            <Line
              type="monotone"
              dataKey="energy"
              stroke="#4f8cff"
              strokeWidth={1.5}
              dot={{ r: 2.5 }}
              // Redundant on-dot select (belt-and-suspenders, debugging/016): the FUNCTION
              // form of activeDot is the one recharts calls with the datum props; a click
              // ON a dot jumps to that cycle even if the chart-level payload regresses.
              activeDot={(props: { cx?: number; cy?: number; index?: number }) => (
                <circle
                  cx={props.cx}
                  cy={props.cy}
                  r={4}
                  fill="#4f8cff"
                  style={{ cursor: "pointer" }}
                  onClick={() => {
                    const pos = resolveClickedIndex({ activeTooltipIndex: props.index }, series);
                    if (pos != null) setFrame(series[pos].index);
                  }}
                />
              )}
              isAnimationActive={false}
            />
          </LineChart>
        </div>
      ) : null}
    </section>
  );
}
