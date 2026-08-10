import {
  CartesianGrid,
  Line,
  LineChart,
  ReferenceLine,
  Tooltip,
  XAxis,
  YAxis,
} from "recharts";

import type { JobStatus } from "../types";
import type { ConvergenceEvent, OptPoint, ScfPoint } from "./types";
import { useContainerWidth } from "../charts/useContainerWidth";
import { HARTREE_TO_KCAL } from "../units";

/** The gradient/step criteria plotted on the (log-scale) criteria chart. Energy
 * change is deliberately excluded — different sign and scale; it lives in the
 * progress indicator instead. Colours are reused for each line's tolerance
 * reference line. */
const PLOTTED_CRITERIA: { name: string; color: string }[] = [
  { name: "RMS gradient", color: "#4f8cff" },
  { name: "MAX gradient", color: "#22c55e" },
  { name: "RMS step", color: "#eab308" },
  { name: "MAX step", color: "#f472b6" },
];

const CHART_HEIGHT = 200;

interface ConvergenceDashboardProps {
  events: ConvergenceEvent[];
  status: JobStatus;
  /**
   * How to read the progress section. `"standard"` (default): the per-cycle
   * criteria are the whole job's convergence. `"goat"`: this is one **inner**
   * optimisation of one candidate in a conformer search — the cycle/criteria are
   * real but the progress BAR is hidden (a full bar with minutes of search still
   * ahead is exactly what misled the user) and a disclaimer is shown. See
   * `isGoatInput` (JobDetailScreen passes it) and wiki/orca/goat.md. The
   * pre-existing `status` prop is deliberately left alone — one prop, one meaning.
   */
  variant?: "standard" | "goat";
}

export function ConvergenceDashboard({
  events,
  variant = "standard",
}: ConvergenceDashboardProps) {
  const { ref, width } = useContainerWidth();

  const scf = events.filter((e): e is ScfPoint => e.kind === "scf");
  const opt = events.filter((e): e is OptPoint => e.kind === "opt");

  const latestScf = scf.length ? scf[scf.length - 1] : null;
  const latestOpt = opt.length ? opt[opt.length - 1] : null;

  // Nothing convergence-shaped yet (e.g. a Freq job, or a run that hasn't
  // reached its first SCF): render nothing rather than an empty frame.
  if (!latestScf && !latestOpt) return null;

  return (
    <div className="convergence-dashboard" ref={ref}>
      <ProgressIndicator
        latestScf={latestScf}
        latestOpt={latestOpt}
        variant={variant}
      />
      <EnergyChart opt={opt} width={width} />
      <CriteriaChart opt={opt} width={width} />
    </div>
  );
}

/** Section A — a real progress state, not a fake percentage. For an
 * optimization: the current cycle and how many criteria are met, with a chip
 * per criterion. For a single point: just the SCF iteration count. */
function ProgressIndicator({
  latestScf,
  latestOpt,
  variant,
}: {
  latestScf: ScfPoint | null;
  latestOpt: OptPoint | null;
  variant: "standard" | "goat";
}) {
  const isGoat = variant === "goat";
  // An optimization is in progress once we've either completed a cycle (opt
  // point) or started SCF inside cycle ≥ 1.
  const isOpt = latestOpt != null || (latestScf != null && latestScf.cycle >= 1);

  if (latestOpt) {
    const met = latestOpt.criteria.filter((c) => c.converged).length;
    const total = latestOpt.criteria.length;
    const cycle = Math.max(latestOpt.cycle, latestScf?.cycle ?? 0);
    return (
      <div className="conv-progress">
        <div className="conv-progress-head">
          {isGoat
            ? `Conformer search · inner optimisation, cycle ${cycle} · ${met}/${total} criteria met`
            : `Optimization cycle ${cycle} · ${met}/${total} criteria met`}
        </div>
        {isGoat ? (
          // The disclaimer replaces the bar: a full bar with minutes of search
          // still ahead is precisely what misled the user (wiki/orca/goat.md).
          <div className="conv-progress-sub">
            one candidate of many — overall GOAT progress is not shown
          </div>
        ) : (
          <div className="conv-progress-bar" aria-hidden>
            <div
              className="conv-progress-fill"
              style={{ width: `${total ? (met / total) * 100 : 0}%` }}
            />
          </div>
        )}
        <div className="conv-criteria-chips">
          {latestOpt.criteria.map((c) => (
            <span
              key={c.name}
              className={"conv-chip " + (c.converged ? "met" : "unmet")}
            >
              {c.name} {c.converged ? "✓" : "✗"}
            </span>
          ))}
        </div>
      </div>
    );
  }

  if (latestScf) {
    const prefix = isGoat
      ? "Conformer search · inner optimisation, "
      : isOpt
        ? `Optimization cycle ${latestScf.cycle} · `
        : "";
    return (
      <div className="conv-progress">
        <div className="conv-progress-head">
          {prefix}
          SCF iteration {latestScf.iter}
        </div>
        {isGoat ? (
          <div className="conv-progress-sub">
            one candidate of many — overall GOAT progress is not shown
          </div>
        ) : null}
      </div>
    );
  }

  return null;
}

/** Section B — energy per optimization cycle. Differences are ~1e-5 Eh against a
 * total of hundreds of Eh, so the Y axis auto-fits and ticks show 6 decimals;
 * the tooltip adds the far more legible ΔE-from-previous in kcal/mol. */
function EnergyChart({ opt, width }: { opt: OptPoint[]; width: number }) {
  const data = opt
    .filter((o) => o.energy != null)
    .map((o) => ({ cycle: o.cycle, energy: o.energy as number }));

  if (data.length < 2 || width <= 0) return null;

  return (
    <div className="conv-chart">
      <div className="conv-chart-title">Energy per cycle (Eh)</div>
      <LineChart
        width={width}
        height={CHART_HEIGHT}
        data={data}
        margin={{ top: 8, right: 16, bottom: 4, left: 8 }}
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
          content={<EnergyTooltip data={data} />}
        />
        <Line
          type="monotone"
          dataKey="energy"
          stroke="#4f8cff"
          strokeWidth={1.5}
          dot={{ r: 2.5 }}
          isAnimationActive={false}
        />
      </LineChart>
    </div>
  );
}

interface EnergyDatum {
  cycle: number;
  energy: number;
}

function EnergyTooltip({
  active,
  payload,
  data,
}: {
  active?: boolean;
  payload?: { payload: EnergyDatum }[];
  data: EnergyDatum[];
}) {
  if (!active || !payload || !payload.length) return null;
  const d = payload[0].payload;
  const idx = data.findIndex((p) => p.cycle === d.cycle);
  const prev = idx > 0 ? data[idx - 1] : null;
  const deltaKcal = prev ? (d.energy - prev.energy) * HARTREE_TO_KCAL : null;
  return (
    <div className="conv-tooltip">
      <div>cycle {d.cycle}</div>
      <div className="mono">{d.energy.toFixed(8)} Eh</div>
      {deltaKcal != null ? (
        <div className="mono conv-tooltip-delta">
          ΔE {deltaKcal >= 0 ? "+" : ""}
          {deltaKcal.toFixed(3)} kcal/mol
        </div>
      ) : null}
    </div>
  );
}

/** Section C — convergence criteria vs their tolerances, on a log Y axis. The
 * values fall by orders of magnitude over a run, so a linear axis would show
 * almost nothing; each criterion's tolerance is a dashed reference line in the
 * matching colour. */
function CriteriaChart({ opt, width }: { opt: OptPoint[]; width: number }) {
  // Only criteria that actually appear in the data (OptTS adds extras; a plain
  // Opt has the four gradient/step ones).
  const present = PLOTTED_CRITERIA.filter((pc) =>
    opt.some((o) => o.criteria.some((c) => c.name === pc.name)),
  );

  const data = opt.map((o) => {
    const row: Record<string, number> = { cycle: o.cycle };
    for (const pc of present) {
      const c = o.criteria.find((x) => x.name === pc.name);
      // Guard the log axis: values are magnitudes (always > 0), but skip a
      // zero/absent one rather than feed the axis a non-positive number.
      if (c && c.value > 0) row[pc.name] = c.value;
    }
    return row;
  });

  if (data.length < 2 || !present.length || width <= 0) return null;

  // Tolerances are fixed per criterion — read each from the last cycle that has
  // it — and drawn as reference lines.
  const tolerance = (name: string): number | undefined => {
    for (let i = opt.length - 1; i >= 0; i--) {
      const c = opt[i].criteria.find((x) => x.name === name);
      if (c) return c.tolerance;
    }
    return undefined;
  };

  return (
    <div className="conv-chart">
      <div className="conv-chart-title">
        Convergence criteria vs tolerance (log scale)
      </div>
      <LineChart
        width={width}
        height={CHART_HEIGHT}
        data={data}
        margin={{ top: 8, right: 16, bottom: 4, left: 8 }}
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
          width={64}
          tickLine={false}
          scale="log"
          domain={["auto", "auto"]}
          tickFormatter={(v: number) => v.toExponential(0)}
        />
        <Tooltip
          isAnimationActive={false}
          formatter={(value) =>
            typeof value === "number" ? value.toExponential(3) : String(value)
          }
          labelFormatter={(l) => `cycle ${l}`}
          contentStyle={{
            background: "var(--panel-2)",
            border: "1px solid var(--border)",
            borderRadius: 6,
            fontSize: 11,
          }}
        />
        {present.flatMap((pc) => {
          const tol = tolerance(pc.name);
          const els = [
            <Line
              key={pc.name}
              type="monotone"
              dataKey={pc.name}
              stroke={pc.color}
              strokeWidth={1.5}
              dot={{ r: 2 }}
              connectNulls
              isAnimationActive={false}
            />,
          ];
          if (tol != null) {
            els.push(
              <ReferenceLine
                key={pc.name + "-tol"}
                y={tol}
                stroke={pc.color}
                strokeDasharray="4 3"
                strokeOpacity={0.5}
              />,
            );
          }
          return els;
        })}
      </LineChart>
      <div className="conv-legend">
        {present.map((pc) => (
          <span key={pc.name} className="conv-legend-item">
            <span
              className="conv-legend-swatch"
              style={{ background: pc.color }}
            />
            {pc.name}
          </span>
        ))}
        <span className="conv-legend-item muted">— — tolerance</span>
      </div>
    </div>
  );
}
