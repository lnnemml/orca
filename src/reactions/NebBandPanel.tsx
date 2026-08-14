import { useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import {
  CartesianGrid,
  Legend,
  Line,
  LineChart,
  ReferenceDot,
  ReferenceLine,
  Tooltip,
  XAxis,
  YAxis,
} from "recharts";

import type { NebResults, Job, NebImageGeometry } from "../types";
import { buildOptTSInput } from "../scene/optts";
import { GroupSelect } from "../groups/GroupSelect";
import { useGroupPicker, useJobGroupId } from "../groups/useGroupPicker";
import { MoleculeViewer } from "../viewer/MoleculeViewer";
import { finalGeometryXyz } from "../export/exporters";
import { saveText, exportName } from "../export/save";
import { useContainerWidth } from "../charts/useContainerWidth";
import {
  iterationSeries,
  mepSeries,
  barrierSeries,
  bandMaxIndex,
  imageGeometryXyz,
  imageExportXyz,
} from "./nebBand";

/**
 * NEB band viewer (Phase 4.5 Stage E3a-2) — the first time the band is *visible*.
 *
 * **The honest-units rule this panel is built around (rule #11, presentation side).**
 * The `.NEB.log` per-iteration energies are ABSOLUTE (Eh); the `.final.interp` MEP
 * energies are RELATIVE (image 0 = 0). They are NOT comparable and must never share
 * one absolute y-axis. So the band series is `iterationSeries` — each iteration
 * relativized to ITS OWN image-0 (ΔE vs that iteration's reactant end) — which puts
 * it on the same relative footing as the already-relative MEP. Both plotted curves
 * are ΔE-in-kcal/mol, each honestly relative to its own reactant; the axis label says
 * so.
 *
 * Reuses the trajectory disciplines (`results-ui.md`): the current ITERATION is
 * application state (`iter`), driven by the same transport idiom as
 * `TrajectoryPlayer` (play/step/slider); explicit chart width via `ResizeObserver`
 * (no `ResponsiveContainer` — the WebKitGTK 0×0 class). Re-parses nothing (ADR-012):
 * the band is E3a-1's `ParsedResults.neb`.
 *
 * The "Refine TS with OptTS" action MIRRORS `ScanProfilePanel`: it reads THIS NEB
 * job's own input (never reconstructed), seeds the source-agnostic OptTS engine from
 * `neb.ts_geometry` (the converged climbing image), and creates+submits a child
 * through the generic `create_optts_job` — no NEB-specific refine path (ADR-020). The
 * located TS it produces flows into the existing E1b ΔG‡ machinery with no new code.
 */
const BAND_CHART_HEIGHT = 220;
const BARRIER_CHART_HEIGHT = 150;

const SPEEDS = [
  { label: "0.5×", fps: 2 },
  { label: "1×", fps: 4 },
  { label: "2×", fps: 8 },
];
const DEFAULT_FPS = 4;

export function NebBandPanel({
  neb,
  jobId,
  jobTitle,
  onOpenJob,
}: {
  neb: NebResults;
  jobId: string;
  jobTitle: string;
  /** Navigate to a newly-created job (the OptTS-refine child). Optional so the panel
   * renders in isolation (tests) without a router. */
  onOpenJob?: (jobId: string) => void;
}) {
  const iterations = neb.iterations;
  const last = iterations.length - 1;

  // The current iteration is APPLICATION state — the same ownership call
  // TrajectoryPlayer makes for the frame number (ADR-011): ephemeral view state held
  // here, driving the label, the highlighted band, and the barrier-line marker in
  // lock-step.
  const [iter, setIter] = useState(0);
  const [playing, setPlaying] = useState(false);
  const [fps, setFps] = useState(DEFAULT_FPS);
  const { ref, width } = useContainerWidth();

  // OptTS-refine: local busy/error state; nothing persists until the child job is
  // created + submitted on click.
  const [refining, setRefining] = useState(false);
  const [refineError, setRefineError] = useState<string | null>(null);
  // Destination-group picker for the OptTS-refine child (unit 2b): defaults to THIS NEB
  // job's group (the source), overridable. Not the active sidebar group.
  const picker = useGroupPicker(useJobGroupId(jobId));

  // ── MEP band geometries (N3): the 3D image stepper + the saddle (TS) view ──────
  // The band-image geometries are loaded on demand; the selected image AND the
  // saddle-toggle are APPLICATION state (ADR-011) — the viewer is fed ONE geometry.
  const [geometries, setGeometries] = useState<NebImageGeometry[] | null>(null);
  const [selectedImage, setSelectedImage] = useState(0);
  const [showTs, setShowTs] = useState(false);

  // Fetch the band image geometries once (lazy — the panel is only mounted on a
  // completed NEB job). A failure/absence leaves the charts usable, viewer absent.
  useEffect(() => {
    let cancelled = false;
    invoke<NebImageGeometry[] | null>("read_neb_geometries", { id: jobId })
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

  const nImages = geometries?.length ?? 0;
  // The ≈ saddle image (interior max energy) — labelled honestly, never an endpoint.
  const maxImageIdx = useMemo(() => (geometries ? bandMaxIndex(geometries) : 0), [geometries]);
  const clampedImage = Math.min(Math.max(selectedImage, 0), Math.max(nImages - 1, 0));
  const selectedEnergy = geometries?.[clampedImage]?.energy_eh ?? null;
  // The one geometry handed to the image viewer — the converged TS when toggled, else
  // the selected MEP image (element-order checked at the boundary vs the TS order).
  const imageViewerState = useMemo(() => {
    if (showTs) {
      return {
        xyz: finalGeometryXyz(neb.ts_geometry, "converged TS", neb.ts_energy_eh ?? null),
      };
    }
    const g = geometries?.[clampedImage];
    if (!g) return null;
    return imageGeometryXyz(g, neb.ts_geometry.elements);
  }, [showTs, geometries, clampedImage, neb.ts_geometry, neb.ts_energy_eh]);

  const clamped = Math.min(Math.max(iter, 0), Math.max(last, 0));
  const current = iterations[clamped];

  // The two plotted curves — both ΔE in kcal/mol, each relative to its own reactant.
  const bandData = useMemo(() => (current ? iterationSeries(current) : []), [current]);
  const mepData = useMemo(() => mepSeries(neb), [neb]);
  const barrierData = useMemo(() => barrierSeries(neb), [neb]);

  // The climbing-image marker — only once climbing is active for this iteration.
  const climbing =
    current && current.climbing_image != null ? bandData[current.climbing_image] : null;

  // The play timer — advance one iteration per tick, stop at the last (play once).
  // Lives here, not in any child, so pause/scrub/label/marker stay in sync.
  useEffect(() => {
    if (!playing || iterations.length <= 1) return;
    const id = setInterval(() => {
      setIter((i) => {
        if (i >= last) {
          setPlaying(false);
          return i;
        }
        return i + 1;
      });
    }, 1000 / fps);
    return () => clearInterval(id);
  }, [playing, fps, iterations.length, last]);

  const togglePlay = () => {
    if (iter >= last) setIter(0); // restart from the top if parked at the end
    setPlaying((p) => !p);
  };
  const step = (delta: number) => {
    setPlaying(false);
    setIter((i) => Math.min(Math.max(i + delta, 0), last));
  };

  // A NEB job with no parsed iterations is a degenerate state, not a crash.
  if (iterations.length === 0) {
    return (
      <section className="neb-band" ref={ref}>
        <div className="section-title" style={{ fontSize: 12 }}>
          NEB band
        </div>
        <div className="banner muted" style={{ marginTop: 6 }}>
          No band iterations parsed for this NEB job.
        </div>
      </section>
    );
  }

  return (
    <section className="neb-band" ref={ref}>
      <div className="section-title" style={{ fontSize: 12 }}>
        NEB band — energy profile per iteration
      </div>

      {/* Honest readout: per-iteration ΔE is relative to THAT iteration's reactant end;
          the barrier is the screening NEB estimate (the honest ΔG‡ comes from Refine). */}
      <div className="neb-readout mono" style={{ fontSize: 12, marginTop: 4 }}>
        <span>
          iteration <strong>{current.index}</strong> / {last}
        </span>
        <span style={{ marginLeft: 12, color: "var(--muted)" }}>
          barrier {(barrierData[clamped]?.barrier_kcal ?? NaN).toFixed(2)} kcal/mol
        </span>
        {current.climbing_image != null ? (
          <span style={{ marginLeft: 12, color: "var(--accent)" }}>
            climbing image #{current.climbing_image}
          </span>
        ) : (
          <span style={{ marginLeft: 12, color: "var(--muted)" }}>climbing not yet active</span>
        )}
      </div>

      {/* The band chart — the per-iteration ΔE band (relativized to its own image-0)
          with the converged smooth MEP overlaid as a distinct labelled series. Two
          series with DIFFERENT x-domains (discrete band vs smooth interp), so each
          Line carries its own `data` (no shared top-level chart data). */}
      {width > 0 ? (
        <LineChart
          width={width}
          height={BAND_CHART_HEIGHT}
          margin={{ top: 28, right: 16, bottom: 18, left: 8 }}
        >
          <CartesianGrid stroke="var(--border)" strokeDasharray="3 3" />
          <XAxis
            type="number"
            dataKey="distance"
            domain={["auto", "auto"]}
            stroke="var(--muted)"
            fontSize={11}
            tickLine={false}
            tickFormatter={(v: number) => v.toFixed(2)}
            label={{
              value: "arc length along band (Å)",
              position: "insideBottom",
              offset: -8,
              fontSize: 11,
              fill: "var(--muted-2)",
            }}
          />
          <YAxis
            stroke="var(--muted)"
            fontSize={11}
            width={64}
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
            labelFormatter={(l) => `d = ${typeof l === "number" ? l.toFixed(2) : l} Å`}
            contentStyle={{
              background: "var(--panel-2)",
              border: "1px solid var(--border)",
              borderRadius: 6,
              fontSize: 11,
            }}
          />
          <Legend
            verticalAlign="top"
            align="center"
            wrapperStyle={{ fontSize: 11, paddingBottom: 4 }}
          />
          {/* The converged smooth MEP — a MUTED DASHED reference, drawn first (under the
              live band). The expected band↔MEP crossing (initial band overshoots the
              converged path) is correct physics — the muted styling keeps it reading as a
              reference, not a glitch. */}
          <Line
            data={mepData}
            dataKey="deltaE_kcal"
            name="converged MEP"
            type="monotone"
            stroke="var(--muted)"
            strokeOpacity={0.6}
            strokeWidth={1.5}
            strokeDasharray="4 3"
            dot={false}
            isAnimationActive={false}
          />
          {/* The current iteration's band — ΔE vs ITS OWN reactant end. The prominent
              SOLID accent line, drawn last so it sits on top of the MEP reference. */}
          <Line
            data={bandData}
            dataKey="deltaE_kcal"
            name={`iteration ${current.index}`}
            type="monotone"
            stroke="#4f8cff"
            strokeWidth={2.25}
            dot={{ r: 2.5 }}
            isAnimationActive={false}
          />
          {/* The climbing image (once active) — the band point being driven to the saddle. */}
          {climbing ? (
            <ReferenceDot
              x={climbing.distance}
              y={climbing.deltaE_kcal}
              r={5}
              fill="var(--accent)"
              stroke="none"
              label={{
                value: "climbing image",
                position: "top",
                fontSize: 10,
                fill: "var(--accent)",
              }}
            />
          ) : null}
        </LineChart>
      ) : null}

      <div className="traj-transport">
        <button className="btn btn-sm" onClick={() => step(-last)} title="First iteration">
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
        <button className="btn btn-sm" onClick={() => step(last)} title="Last iteration">
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
            setIter(Number(e.target.value));
          }}
          aria-label="NEB iteration"
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

      {/* Barrier convergence — barrier (kcal/mol) vs iteration; the current iteration is
          a vertical marker. Distinct from the band: this is the band TIGHTENING over
          iterations, not a path. */}
      {width > 0 && barrierData.length >= 2 ? (
        <div style={{ marginTop: 8 }}>
          <div className="conv-chart-title" style={{ fontSize: 11, color: "var(--muted)" }}>
            Barrier convergence (kcal/mol per iteration)
          </div>
          <LineChart
            width={width}
            height={BARRIER_CHART_HEIGHT}
            data={barrierData}
            margin={{ top: 8, right: 16, bottom: 18, left: 8 }}
          >
            <CartesianGrid stroke="var(--border)" strokeDasharray="3 3" />
            <XAxis
              dataKey="iteration"
              stroke="var(--muted)"
              fontSize={11}
              tickLine={false}
              label={{
                value: "iteration",
                position: "insideBottom",
                offset: -8,
                fontSize: 11,
                fill: "var(--muted-2)",
              }}
            />
            <YAxis
              stroke="var(--muted)"
              fontSize={11}
              width={64}
              tickLine={false}
              domain={["auto", "auto"]}
              tickFormatter={(v: number) => v.toFixed(1)}
            />
            <Tooltip
              isAnimationActive={false}
              formatter={(v) => (typeof v === "number" ? `${v.toFixed(2)} kcal/mol` : String(v))}
              labelFormatter={(l) => `iteration ${l}`}
              contentStyle={{
                background: "var(--panel-2)",
                border: "1px solid var(--border)",
                borderRadius: 6,
                fontSize: 11,
              }}
            />
            <ReferenceLine x={current.index} stroke="var(--accent)" strokeWidth={1.5} />
            <Line
              type="monotone"
              dataKey="barrier_kcal"
              stroke="#e0902f"
              strokeWidth={1.5}
              dot={{ r: 2 }}
              isAnimationActive={false}
            />
          </LineChart>
        </div>
      ) : null}

      {/* Band geometries (converged MEP) — the 3D image stepper + the saddle (TS) view,
          mirroring ScanProfilePanel's geometry viewer. LABELS by geometry + energy ONLY:
          "Image k — E = …", the interior max is "≈ saddle", the toggle shows the converged
          TS by ITS energy. NEVER a source endpoint job's title (the HCN/HNC mislabel lesson:
          a job title is human intent, the geometry is truth). Honest-or-absent: the whole
          block renders only when `read_neb_geometries` returned a band. */}
      {geometries && geometries.length > 0 ? (
        <div className="neb-geometries" style={{ marginTop: 12 }}>
          <div className="section-title" style={{ fontSize: 12 }}>
            Band geometries{" "}
            <span className="muted">
              — converged MEP, im0 (reactant end) → im{nImages - 1} (product end)
            </span>
          </div>

          {/* One geometry to the viewer (ADR-011): the selected image, or the TS. */}
          <div className="viewer-panel traj-viewer">
            {imageViewerState && "xyz" in imageViewerState ? (
              <MoleculeViewer xyzData={imageViewerState.xyz} preserveCameraOnUpdate />
            ) : imageViewerState && "error" in imageViewerState ? (
              <div className="banner err" style={{ margin: 8 }}>
                {imageViewerState.error}
              </div>
            ) : (
              <div className="viewer-empty muted">Loading band geometry…</div>
            )}
          </div>

          <div className="neb-readout mono" style={{ fontSize: 12 }}>
            {showTs ? (
              <span>
                Saddle (TS)
                {neb.ts_energy_eh != null ? ` — E = ${neb.ts_energy_eh.toFixed(6)} Eh` : ""}
              </span>
            ) : (
              <>
                <span>
                  Image {clampedImage} / {nImages - 1}
                  {selectedEnergy != null ? ` — E = ${selectedEnergy.toFixed(6)} Eh` : ""}
                </span>
                {clampedImage === maxImageIdx ? (
                  <span
                    className="scan-ts-badge"
                    style={{ marginLeft: 8 }}
                    title="The highest-energy MEP image — the converged saddle lives here. Toggle 'Saddle (TS)' for the located climbing image."
                  >
                    ≈ saddle
                  </span>
                ) : null}
              </>
            )}
            <button
              className="btn btn-sm"
              style={{ marginLeft: 8 }}
              title={showTs ? "Show the MEP band images" : "Show the converged TS geometry"}
              onClick={() => setShowTs((s) => !s)}
            >
              {showTs ? "Show band images" : "Saddle (TS)"}
            </button>
            {/* Export the SHOWN geometry (selected image, or the TS when toggled) — honest-or-
                absent (disabled until it renders). File name/comment carry the NEB job's own
                title + the image index/energy, never a reactant/product endpoint title. */}
            <button
              className="btn btn-sm"
              disabled={!(imageViewerState && "xyz" in imageViewerState)}
              title="Export the shown geometry (.xyz)"
              onClick={async () => {
                try {
                  if (showTs) {
                    await saveText(
                      exportName(jobTitle, "neb-ts", "xyz"),
                      finalGeometryXyz(
                        neb.ts_geometry,
                        `${jobTitle} — NEB converged TS (saddle)`,
                        neb.ts_energy_eh ?? null,
                      ),
                      "xyz",
                    );
                    return;
                  }
                  const g = geometries?.[clampedImage];
                  if (!g) return;
                  await saveText(
                    exportName(jobTitle, `neb-image-${clampedImage}`, "xyz"),
                    imageExportXyz(g, nImages, clampedImage === maxImageIdx, jobTitle),
                    "xyz",
                  );
                } catch (e) {
                  console.error("[export]", e);
                }
              }}
            >
              geometry .xyz
            </button>
          </div>

          {/* Image stepper — im0…imN. The selected image is app state (ADR-011); the
              controls are disabled while the TS is shown (a distinct, single geometry). */}
          <div className="traj-transport">
            <button
              className="btn btn-sm"
              disabled={showTs}
              onClick={() => setSelectedImage(0)}
              title="Reactant end (im0)"
            >
              ⏮
            </button>
            <button
              className="btn btn-sm"
              disabled={showTs}
              onClick={() => setSelectedImage((i) => Math.max(i - 1, 0))}
              title="Previous image"
            >
              ◀
            </button>
            <button
              className="btn btn-sm"
              disabled={showTs}
              onClick={() => setSelectedImage((i) => Math.min(i + 1, nImages - 1))}
              title="Next image"
            >
              ▶
            </button>
            <button
              className="btn btn-sm"
              disabled={showTs}
              onClick={() => setSelectedImage(nImages - 1)}
              title={`Product end (im${nImages - 1})`}
            >
              ⏭
            </button>
            <input
              className="traj-slider"
              type="range"
              min={0}
              max={Math.max(nImages - 1, 0)}
              value={clampedImage}
              disabled={showTs}
              onChange={(e) => setSelectedImage(Number(e.target.value))}
              aria-label="NEB MEP image"
            />
            <button
              className="btn btn-sm"
              disabled={showTs}
              onClick={() => setSelectedImage(maxImageIdx)}
              title="Jump to the ≈ saddle (the max-energy MEP image)"
            >
              ≈ saddle
            </button>
          </div>
        </div>
      ) : null}

      {/* Refine TS with OptTS — MIRRORS ScanProfilePanel's Stage-E1a action exactly:
          read THIS NEB job's own input (method/solvation/charge), seed the generic OptTS
          engine from the converged climbing image (neb.ts_geometry), create+submit a child,
          navigate to it. No NEB-specific path — the same source-agnostic engine (ADR-020). */}
      <div style={{ marginTop: 10 }}>
        <button
          className="btn btn-sm btn-primary"
          disabled={refining}
          title="Refine the converged NEB climbing image into a located transition state (OptTS + Freq)"
          onClick={async () => {
            if (refining) return;
            setRefining(true);
            setRefineError(null);
            try {
              // Read the NEB job's OWN input as the context — never reconstruct it. The pure
              // engine inherits + asserts (charge, multiplicity); a failure throws before create.
              const source = await invoke<Job>("get_job", { id: jobId });
              const input = buildOptTSInput(source.input_content, neb.ts_geometry);
              const child = await invoke<Job>("create_optts_job", {
                sourceJobId: jobId,
                title: `OptTS — ${jobTitle}`,
                inputContent: input,
              });
              // Move the child into the picked group (default = this NEB job's group) before
              // submitting/navigating (unit 2b). No-op when untouched + ungrouped source.
              await picker.assignPicked(child.id);
              await invoke("submit_job", { id: child.id });
              onOpenJob?.(child.id);
            } catch (e) {
              // A charge/post-condition failure (buildOptTSInput) lands here — no job created.
              console.error("[optts]", e);
              setRefineError(String(e));
            } finally {
              setRefining(false);
            }
          }}
        >
          {refining ? "Refining…" : "Refine TS with OptTS"}
        </button>
        <label
          className="scan-control"
          style={{ marginLeft: 8 }}
          title="Destination group for the refined child job"
        >
          group
          <GroupSelect
            className="select select-sm"
            groups={picker.groups}
            value={picker.pickedGroupId}
            onChange={picker.onChange}
            aria-label="Destination group for the refined child"
          />
        </label>
      </div>
      {refineError ? (
        <div className="banner err" style={{ marginTop: 6 }}>
          OptTS refine failed (no job created): {refineError}
        </div>
      ) : null}
    </section>
  );
}
