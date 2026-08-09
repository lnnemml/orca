import { useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import {
  CartesianGrid,
  Line,
  LineChart,
  ReferenceDot,
  ReferenceLine,
  Tooltip,
  XAxis,
  YAxis,
} from "recharts";

import type { ScanProfileJson, ScanGeometry, Job } from "../types";
import { buildOptTSInput } from "../scene/optts";
import { MoleculeViewer } from "../viewer/MoleculeViewer";
import { useContainerWidth } from "../charts/useContainerWidth";
import { resolveClickedIndex, type ChartClickState } from "../charts/clickIndex";
import { saveBytes, saveText, exportName } from "../export/save";
import { svgToPngBytes } from "../export/png";
import {
  profileSeries,
  maxIndex,
  pointGeometryXyz,
  pointReadout,
  scanPointExportXyz,
  type EnergyChoice,
  type RefChoice,
} from "./scanProfile";

/**
 * Relaxed-scan energy-profile panel (Phase 4.5 Stage B2) — the first time a scan is
 * *visible*. Reuses the trajectory disciplines (`results-ui.md`):
 *  - **the current point is application state** (`selected`), never 3Dmol's frame
 *    apparatus; the viewer is fed ONE geometry (ADR-011);
 *  - **element-order identity is checked at the UI boundary** before a point renders;
 *  - **honest labels**: y is ΔE in kcal/mol against a labelled reference; the maximum
 *    is an *approximate* TS (a ΔE‡ estimate on a relaxed surface — ADR-007), never
 *    "the transition state" and never ΔG‡;
 *  - explicit chart width via `ResizeObserver` (no `ResponsiveContainer` — the
 *    WebKitGTK 0×0 class).
 *
 * Re-parses nothing (ADR-012): the profile is B1's `ParsedResults.scan`; the point
 * geometries are fetched once via `read_scan_geometries` (reads `input.NNN.xyz`).
 */
const CHART_HEIGHT = 190;

export function ScanProfilePanel({
  scan,
  referenceElements,
  jobId,
  jobTitle,
  onOpenJob,
}: {
  scan: ScanProfileJson;
  /** The element order the result geometry is drawn in — a point geometry must
   * match this before it is rendered. */
  referenceElements: string[];
  jobId: string;
  jobTitle: string;
  /** Navigate to a newly-created job (the OptTS-refine child). Optional so the panel
   * renders in isolation (tests, storybook) without a router. */
  onOpenJob?: (jobId: string) => void;
}) {
  const points = scan.points;
  const unit = scan.coordinate_unit;
  // The selected point is APPLICATION state — the viewer never owns it (ADR-011).
  // The panel OPENS on the approximate-TS maximum (act series) — its Stage-E purpose is
  // "the maximum → refine with OptTS", so that is the point to land on (an intended B2
  // behaviour change; before it opened on point 1).
  const [selected, setSelected] = useState(() => maxIndex(points, "act"));
  const [energyChoice, setEnergyChoice] = useState<EnergyChoice>("act"); // act = composite (default)
  const [refChoice, setRefChoice] = useState<RefChoice>("first");
  const [geometries, setGeometries] = useState<ScanGeometry[] | null>(null);
  // OptTS-refine (Stage E1a): local busy/error state; nothing persists until the child
  // job is created + submitted on click.
  const [refining, setRefining] = useState(false);
  const [refineError, setRefineError] = useState<string | null>(null);
  const { ref, width } = useContainerWidth();
  const chartRef = useRef<HTMLDivElement | null>(null);

  // Fetch the per-point geometries once (lazy — the panel is only mounted on a
  // completed scan job). A failure leaves the chart usable, viewer empty.
  useEffect(() => {
    let cancelled = false;
    invoke<ScanGeometry[] | null>("read_scan_geometries", { id: jobId })
      .then((g) => {
        if (!cancelled) setGeometries(g ?? null);
      })
      .catch(() => {
        if (!cancelled) setGeometries(null);
      });
    return () => {
      cancelled = true;
    };
  }, [jobId]);

  const series = useMemo(
    () => profileSeries(points, energyChoice, refChoice),
    [points, energyChoice, refChoice],
  );
  const tsIndex = useMemo(() => maxIndex(points, energyChoice), [points, energyChoice]);
  const clamped = Math.min(Math.max(selected, 0), points.length - 1);
  const readout = pointReadout(points, clamped, energyChoice, refChoice, unit);

  // The one geometry handed to the viewer — the selected point, element-order
  // checked at the boundary. `null` until geometries load.
  const viewerState = useMemo(() => {
    const g = geometries?.[clamped];
    if (!g) return null;
    return pointGeometryXyz(g, referenceElements);
  }, [geometries, clamped, referenceElements]);

  // A single point (degenerate — e.g. a scan that silently ran one point) is a
  // clear state, not a crash.
  if (points.length < 2) {
    return (
      <section className="scan-profile" ref={ref}>
        <div className="section-title" style={{ fontSize: 12 }}>
          Scan profile
        </div>
        <div className="banner muted" style={{ marginTop: 6 }}>
          This scan has only {points.length} point — no profile to plot. A relaxed
          scan needs <code>! Opt</code> and ≥ 2 points.
        </div>
      </section>
    );
  }

  const tsDatum = series[tsIndex];

  return (
    <section className="scan-profile" ref={ref}>
      <div className="section-title" style={{ fontSize: 12 }}>
        Scan profile{" "}
        <span className="muted">
          — {scan.kind === "B" ? "distance" : scan.kind === "A" ? "angle" : "dihedral"} scan on
          atoms {scan.atoms.join(", ")} (0-based)
        </span>
      </div>

      {/* The selected point's geometry — one frame to the viewer (ADR-011). */}
      <div className="viewer-panel traj-viewer">
        {viewerState && "xyz" in viewerState ? (
          <MoleculeViewer xyzData={viewerState.xyz} preserveCameraOnUpdate />
        ) : viewerState && "error" in viewerState ? (
          <div className="banner err" style={{ margin: 8 }}>
            {viewerState.error}
          </div>
        ) : (
          <div className="viewer-empty muted">Loading point geometry…</div>
        )}
      </div>

      <div className="scan-readout mono">
        <span>
          point {clamped + 1} / {points.length}
        </span>
        {readout ? (
          <>
            <span className="scan-coord">{readout.coordinate}</span>
            <span className="muted">
              ΔE {readout.delta} ({energyChoice === "act" ? "actual" : "SCF"})
            </span>
          </>
        ) : null}
        {clamped === tsIndex ? (
          <span
            className="scan-ts-badge"
            title="The scan maximum is a ΔE‡ ESTIMATE on a relaxed surface — not a located saddle and not ΔG‡. Refine with OptTS (Stage E)."
          >
            approximate TS
          </span>
        ) : null}
        {/* Export the SELECTED point's geometry (WYSIWYG with the viewer) — sourced from the
            SAME read_scan_geometries array the viewer is fed, never the last-point result
            geometry the top ExportBar exports. Honest-or-absent: enabled ONLY when the point
            actually renders (element order agrees). This is the Stage-E seam — the approx-TS
            max exported here is the OptTS-refine seed. */}
        <button
          className="btn btn-sm"
          disabled={!(viewerState && "xyz" in viewerState)}
          title={
            viewerState && "xyz" in viewerState
              ? "Export this point's geometry (.xyz) — the OptTS-refine seed (Stage E)"
              : "geometry not rendered (element order disagrees, or still loading)"
          }
          onClick={async () => {
            try {
              const g = geometries?.[clamped];
              const pt = points[clamped];
              if (!g || !pt) return;
              await saveText(
                exportName(jobTitle, `scan-point-${clamped + 1}`, "xyz"),
                scanPointExportXyz(g, pt, clamped, points.length, unit, clamped === tsIndex, jobTitle),
                "xyz",
              );
            } catch (e) {
              console.error("[export]", e);
            }
          }}
        >
          geometry .xyz
        </button>
        {/* Refine with OptTS (Stage E1a) — the SCAN entry point into the source-agnostic
            OptTS engine. Enabled ONLY on the approx-TS maximum AND when it renders: the scan
            maximum is the TS guess (a clean 1-D coordinate; `wiki/orca/optts.md`). On click it
            reads THIS scan job's own input (not reconstructed) as the context, seeds from the
            max point's geometry, creates+submits a child, and navigates to it. */}
        <button
          className="btn btn-sm btn-primary"
          disabled={
            refining || !(clamped === tsIndex && viewerState && "xyz" in viewerState)
          }
          title={
            clamped === tsIndex
              ? "Refine this approximate TS into a located transition state (OptTS + Freq, Stage E)"
              : "Select the approximate-TS maximum to refine it with OptTS"
          }
          onClick={async () => {
            if (refining) return;
            setRefining(true);
            setRefineError(null);
            try {
              const seed = geometries?.[tsIndex];
              if (!seed) return;
              // Read the scan job's OWN input as the context (method/solvation/charge) —
              // never reconstruct it. The pure engine inherits + asserts (c,m).
              const source = await invoke<Job>("get_job", { id: jobId });
              const input = buildOptTSInput(source.input_content, seed);
              const child = await invoke<Job>("create_optts_job", {
                sourceJobId: jobId,
                title: `OptTS — ${jobTitle}`,
                inputContent: input,
              });
              await invoke("submit_job", { id: child.id });
              onOpenJob?.(child.id);
            } catch (e) {
              // A charge/Scan post-condition failure (buildOptTSInput) lands here — no job created.
              console.error("[optts]", e);
              setRefineError(String(e));
            } finally {
              setRefining(false);
            }
          }}
        >
          {refining ? "Refining…" : "Refine with OptTS (Stage E)"}
        </button>
      </div>
      {refineError ? (
        <div className="banner err" style={{ marginTop: 6 }}>
          OptTS refine failed (no job created): {refineError}
        </div>
      ) : null}

      {/* Display controls — each a LABELLED choice, not a molecule property. */}
      <div className="scan-controls">
        <label className="scan-control">
          energy
          <select
            className="select select-sm"
            value={energyChoice}
            onChange={(e) => setEnergyChoice(e.target.value as EnergyChoice)}
          >
            <option value="act">actual (composite)</option>
            <option value="scf">SCF only</option>
          </select>
        </label>
        <label className="scan-control">
          ΔE relative to
          <select
            className="select select-sm"
            value={refChoice}
            onChange={(e) => setRefChoice(e.target.value as RefChoice)}
          >
            <option value="first">point 1</option>
            <option value="min">the minimum</option>
          </select>
        </label>
      </div>

      {width > 0 ? (
        <div className="conv-chart" ref={chartRef}>
          <div className="conv-chart-title">
            ΔE ({energyChoice === "act" ? "actual" : "SCF"}, kcal/mol) vs coordinate ({unit}) —
            click a point to view its geometry
            <button
              className="btn btn-sm"
              style={{ marginLeft: 10 }}
              onClick={async () => {
                try {
                  const svg = chartRef.current?.querySelector("svg");
                  if (svg)
                    await saveBytes(
                      exportName(jobTitle, "scan-profile", "png"),
                      await svgToPngBytes(svg),
                    );
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
            margin={{ top: 8, right: 16, bottom: 18, left: 8 }}
            onClick={(state: ChartClickState) => {
              // recharts v3 hands the index back as a STRING — resolve through the shared
              // helper (number/string index, then activeLabel=coordinate fallback).
              const pos = resolveClickedIndex(state, series, (d) => d.coordinate);
              if (pos != null) setSelected(series[pos].index);
              else if (import.meta.env.DEV) console.warn("[chart click] scan unresolved", state);
            }}
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
            {/* The current point — a vertical marker (app-owned index). */}
            <ReferenceLine x={series[clamped]?.coordinate} stroke="var(--accent)" strokeWidth={1.5} />
            {/* The approximate-TS point — highlighted, honestly labelled. */}
            {tsDatum ? (
              <ReferenceDot
                x={tsDatum.coordinate}
                y={tsDatum.relKcal}
                r={5}
                fill="#ff8c42"
                stroke="none"
                label={{
                  value: "approx. TS",
                  position: "top",
                  fontSize: 10,
                  fill: "#ff8c42",
                }}
              />
            ) : null}
            <Line
              type="monotone"
              dataKey="relKcal"
              stroke="#4f8cff"
              strokeWidth={1.5}
              dot={{ r: 2.5 }}
              // Redundant on-dot select (belt-and-suspenders): the FUNCTION form of
              // activeDot is the one recharts calls with the datum props (index/payload);
              // the object form's onClick receives the activeDot props object, not the
              // datum (measured, debugging/016). A click ON a dot selects even if the
              // chart-level payload ever regresses. Routed through the same resolver.
              activeDot={(props: { cx?: number; cy?: number; index?: number }) => (
                <circle
                  cx={props.cx}
                  cy={props.cy}
                  r={4}
                  fill="#4f8cff"
                  style={{ cursor: "pointer" }}
                  onClick={() => {
                    const pos = resolveClickedIndex({ activeTooltipIndex: props.index }, series);
                    if (pos != null) setSelected(series[pos].index);
                  }}
                />
              )}
              isAnimationActive={false}
            />
          </LineChart>
          <div className="muted scan-ts-note">
            The maximum is an <strong>approximate TS (scan maximum)</strong> — a ΔE‡ estimate on the
            relaxed surface, not a located saddle and not ΔG‡. Refine with OptTS (Stage E).
          </div>
        </div>
      ) : null}
    </section>
  );
}
