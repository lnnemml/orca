import { useMemo, useState } from "react";
import { CartesianGrid, Line, LineChart, ReferenceDot, Tooltip, XAxis, YAxis } from "recharts";

import type { ScanProfileJson, NebResults } from "../types";
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
  absoluteBarrierKcal,
  methodSignature,
  locatedBarrierEKcal,
  locatedTsBarrierFromSp,
  deltaGDoubleDaggerKcal,
  deltaDeltaGKcal,
  nebMepCurve,
  normalizedScanCurve,
  type BarrierCell,
  type Comparability,
  type OptTsStudyResult,
} from "./compare";

/** A pathway's LOCATED transition state (Stage E1b): an OptTS child of this pathway,
 * parsed. `eEh` is its electronic energy; `gEh` its Gibbs G (null unless Freq parsed —
 * the honest-or-absent signal for ΔG‡). `input` is the TS job's input (for the guards). */
export interface LocatedTs {
  input: string;
  eEh: number | null;
  gEh: number | null;
  /** True for the **G1 unrefined NEB-TS estimate** (N4): `eEh` = the converged
   * `neb.ts_energy_eh` used as a first-pass located-TS ΔE‡, `gEh` = null (no Freq → ΔG‡
   * refused for free). Drives the "NEB TS (unrefined estimate)" label. Absent for a real
   * OptTS refinement (which always wins over the estimate). */
  isEstimate?: boolean;
  /** SP-on-an-OptTS-geometry provenance (Stage F4): set when this TS-arm energy comes from a
   * standalone SINGLE-POINT (SPE) job run on an OptTS geometry — the CCSD(T)//DFT protocol (a cheap
   * DFT geometry, an accurate SP energy). `eEh` is the SP electronic energy; `gEh` is ALWAYS null
   * (an SP has no Freq → no G → ΔG‡ absent BY CONSTRUCTION, never a fabricated 0). Drives the
   * "(SP energy)" label + the geometry-provenance match line. Absent for an OptTS-native TS. */
  spOnOptTs?: {
    /** The bit-matched OptTS job's title (geometryMatchesFinal, computed at render vs the reaction's
     * OptTS jobs) when the SP ran ON one OptTS's converged geometry; `null` when it matches NONE —
     * the ⚠ case (the SP is only a valid TS barrier ON an OptTS geometry; the user is responsible). */
    matchedOptTsTitle: string | null;
  };
}

/** An OptTS-ORIGIN pathway (Stage F3): a reaction study built from a located OptTS transition
 * state + its two connectivity children (no scan coordinate — three stationary points). The
 * barrier is vs the USER-DESIGNATED reactant child (the connectivity reactant BASIN, an
 * associated complex), NOT separated fragments — rendered by the dedicated `OptTsStudyView`. */
export interface OptTsOrigin {
  /** The located barriers + 3-point profile from {@link optTsStudy}. */
  study: OptTsStudyResult;
  /** The designated reactant child's job title (the barrier's basis, for the label). */
  reactantLabel: string;
  /** The other connectivity child's job title. */
  productLabel: string;
  /** True when the reactant is the user's explicit designation (a reference job); false when it
   * is the energy-HINT default (no reference set yet) — surfaced so the label stays honest. */
  reactantDesignated: boolean;
}

/** A pathway ready to compare. **`origin` is the explicit discriminator** (never inferred from
 * which field is null): `"scan"` → `scan` set; `"neb"` → `nebMep` set; `"optts"` → `optTs` set
 * (scan & nebMep both absent). `locatedTs` is present when a scan/NEB pathway has a parsed OptTS
 * refinement (Stage E1b) — then a real ΔG‡ / located ΔE‡ replace the scan-max estimate. */
export interface ComparePathway {
  id: string;
  label: string;
  /** Which kind of pathway this is. `"scan"`/`"neb"` render the overlay; `"optts"` (F3,
   * connectivity-basin study) renders in the sibling `OptTsStudyView`; `"located-ts"` (F3+1) is a
   * standalone OptTS TS whose ABSOLUTE located ΔE‡/ΔG‡ vs the reaction's separated-reactant
   * references renders in the existing located-TS ΔΔ table (via `locatedTs`, no scan/neb primary). */
  origin: "scan" | "neb" | "optts" | "located-ts";
  /** The scan profile (a scan pathway). Set iff `origin === "scan"`. */
  scan?: ScanProfileJson;
  /** The NEB band (a NEB pathway, N4): its MEP feeds the normalized overlay curve, and
   * its converged TS feeds the located-TS ΔΔ table (via `locatedTs`, a G1 estimate or an
   * OptTS refine). Set iff `origin === "neb"`. */
  nebMep?: NebResults;
  /** The OptTS-origin study (F3). Set iff `origin === "optts"`. */
  optTs?: OptTsOrigin;
  input: string;
  locatedTs?: LocatedTs;
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
  /** Σ G(ref) in Eh (Stage E1b) — non-null ONLY when every reference job ran Freq; a partial
   * ΣG is null, never summed (ADR-018). The denominator of ΔG‡; null → ΔG‡ is refused. */
  gibbsEh: number | null;
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
  const refGibbsEh = reference?.gibbsEh ?? null;
  const refJobCount = reference?.jobCount ?? 0;
  const refInputs = useMemo(() => reference?.inputs ?? [], [reference]);
  const refComplete = refEnergyEh != null;
  const refIncomplete = refJobCount > 0 && !refComplete;

  // A NEB pathway has no physical scan coordinate — so when ANY NEB is overlaid, the whole
  // overlay drops to a NORMALIZED 0→1 reaction-coordinate axis (illustrative shape only; the
  // rigorous cross-pathway number is the located-TS ΔΔ table). All-scan → physical axis.
  const anyNeb = pathways.some((p) => p.nebMep != null);
  const normalizedAxis = anyNeb;

  // The "separated reactants" zero is offerable only on the PHYSICAL (all-scan) axis, when
  // E(ref) is complete AND every overlaid pathway shares the reference's method (else
  // re-zeroing on E(ref) would place a mismatched curve on an incomparable scale). It is a
  // physical-axis concept — meaningless on the normalized illustrative axis.
  const canUseReactantsZero = useMemo(
    () =>
      !normalizedAxis &&
      refComplete &&
      pathways.length > 0 &&
      pathways.every((p) => referenceComparable(refInputs, methodSignature(p.input).display).ok),
    [normalizedAxis, refComplete, pathways, refInputs],
  );
  const reactantsZeroActive = zeroMode === "reactants" && canUseReactantsZero;

  // Shared zero (physical axis only): by default the global minimum absolute energy across
  // ALL overlaid pathways (the C2b-1 behaviour). When "separated reactants" is chosen AND
  // offerable, the zero is E(ref) instead, so each curve's max height is the ABSOLUTE barrier.
  // ΔΔE‡ is a difference, so it never depends on this choice — the zero is only the axis.
  const series = useMemo(() => {
    // The shared-min zero is over the SCAN pathways only — a standalone located-TS pathway
    // (`origin === "located-ts"`) has no scan curve, so it must not be dereferenced here.
    const scanOnly = pathways.filter((p) => p.scan);
    const zeroEh = normalizedAxis
      ? 0
      : reactantsZeroActive && refEnergyEh != null
        ? refEnergyEh
        : scanOnly.length
          ? Math.min(...scanOnly.map((p) => minEnergyEh(p.scan!, which)))
          : 0;
    return pathways.map((p, i) => {
      // A standalone located-TS pathway (F3+1): NO scan/NEB curve — it contributes ONLY the
      // absolute located ΔE‡/ΔG‡ vs the reaction's separated-reactant references to the ΔΔ TABLE,
      // never a point to the overlay chart. Skip the whole scan/NEB curve machinery (which would
      // dereference the absent `p.scan`).
      if (p.origin === "located-ts") {
        const eTsEh = p.locatedTs?.eEh ?? null;
        const gTsEh = p.locatedTs?.gEh ?? null;
        const spOnOptTs = p.locatedTs?.spOnOptTs;
        const isSp = spOnOptTs != null;
        const methodSig = methodSignature(p.input).display;
        const refGuard = absoluteBarrierCell(0, refEnergyEh, refInputs, methodSig, refJobCount, p.input);
        const refUsable = "kcal" in refGuard;
        const refReason = "reason" in refGuard ? refGuard.reason : null;
        // SP-on-an-OptTS geometry (F4): the ΔE‡ comes from locatedTsBarrierFromSp (the SP electronic
        // energy vs Σ E(ref)), and ΔG‡ is ABSENT BY CONSTRUCTION — an SP has no Freq (the cell's
        // deltaGKcal is a typed `null`, never a fabricated 0). An OptTS-native TS (F3+1) keeps the
        // located ΔE‡ + a real ΔG‡ where G exists. Both gated on the reference being usable — the
        // SAME method/completeness/stoichiometry guard (referenceComparable inside absoluteBarrierCell)
        // fires the mixed-method warning (a DLPNO SP arm vs r2SCAN-3c refs is not subtractable).
        const spCell = isSp ? locatedTsBarrierFromSp(eTsEh, refEnergyEh) : null;
        const locatedEKcal = refUsable
          ? isSp
            ? spCell!.deltaEKcal
            : locatedBarrierEKcal(eTsEh, refEnergyEh)
          : null;
        const deltaGKcal = refUsable && !isSp ? deltaGDoubleDaggerKcal(gTsEh, refGibbsEh) : null;
        return {
          ...p,
          color: PALETTE[i % PALETTE.length],
          isNeb: false,
          data: [] as { x?: number; coordinate?: number; relKcal: number }[],
          maxDatum: undefined as { x?: number; coordinate?: number; relKcal: number } | undefined,
          intrinsicKcal: null as number | null, // a bare TS has no self-contained/intrinsic barrier
          absoluteCell: null as BarrierCell | null, // no scan maximum — its barrier is the located ΔE‡
          isLocated: true,
          isEstimate: false,
          locatedEKcal,
          deltaGKcal,
          gMissing: refUsable && !isSp && deltaGKcal === null,
          locatedReason: !refUsable ? refReason : null,
          eTsEh,
          gTsEh,
          isSp,
          spMethod: isSp ? methodSig : null,
          spMatchedOptTsTitle: isSp ? spOnOptTs!.matchedOptTsTitle : undefined,
        };
      }
      const isNeb = p.nebMep != null;
      const nebCurve = p.nebMep ? nebMepCurve(p.nebMep) : [];

      // The chart curve: normalized 0→1 (mixed scan+NEB) or the physical scan axis (all-scan).
      // The TS marker is the max-energy point of whichever curve.
      let data: { x?: number; coordinate?: number; relKcal: number }[];
      let maxDatum: { x?: number; coordinate?: number; relKcal: number } | undefined;
      if (normalizedAxis) {
        const pts = isNeb ? nebCurve : normalizedScanCurve(p.scan!, which);
        data = pts.map((q) => ({ x: q.x, relKcal: q.energyKcal }));
        let mi = 0;
        let mv = -Infinity;
        pts.forEach((q, k) => {
          if (q.energyKcal > mv) {
            mv = q.energyKcal;
            mi = k;
          }
        });
        maxDatum = data[mi];
      } else {
        data = p.scan!.points.map((pt, index) => ({
          index,
          coordinate: pt.coordinate,
          relKcal: (energyEh(pt, which) - zeroEh) * HARTREE_TO_KCAL,
        }));
        maxDatum = data[maxIndex(p.scan!.points, which)];
      }

      // Intrinsic (self-contained) barrier: scan → E(max) − reactant-side min; NEB → the
      // MEP's max relative energy (the forward barrier from the reactant end, image 0 = 0).
      const intrinsicKcal = isNeb
        ? nebCurve.reduce((m, q) => Math.max(m, q.energyKcal), 0)
        : intrinsicBarrierKcal(p.scan!, which);

      // Reference usability (method + completeness + stoichiometry balance) — a dummy energy
      // gets the VERDICT (the OK/reason decision ignores the pathway energy), so a NEB row
      // (no scan maximum) is gated by the SAME reference guard as a scan for its located ΔE‡.
      const methodSig = methodSignature(p.input).display;
      const refGuard = absoluteBarrierCell(0, refEnergyEh, refInputs, methodSig, refJobCount, p.input);
      const refUsable = "kcal" in refGuard;
      const refReason = "reason" in refGuard ? refGuard.reason : null;

      // Scan-max absolute barrier column — SCAN ONLY (a NEB has no scan maximum; its barrier
      // is the located-TS estimate in the next column). `p.input` threads the complex for the
      // stoichiometry guard.
      const absoluteCell: BarrierCell | null = isNeb
        ? null
        : absoluteBarrierCell(
            maxEnergyEh(p.scan!, which),
            refEnergyEh,
            refInputs,
            methodSig,
            refJobCount,
            p.input,
          );

      // Located TS (Stage E1b for a scan-refine; N4 G1 estimate for a NEB) — the ELECTRONIC
      // barrier from the actual saddle, and a real ΔG‡ where G exists. Gated on the reference
      // being usable. An estimate's `gEh` is null → ΔG‡ refused for free (honest-or-absent).
      const isLocated = !!p.locatedTs;
      const isEstimate = p.locatedTs?.isEstimate ?? false;
      const eTsEh = p.locatedTs?.eEh ?? null;
      const gTsEh = p.locatedTs?.gEh ?? null;
      const locatedEKcal = p.locatedTs && refUsable ? locatedBarrierEKcal(eTsEh, refEnergyEh) : null;
      const deltaGKcal = p.locatedTs && refUsable ? deltaGDoubleDaggerKcal(gTsEh, refGibbsEh) : null;
      const gMissing = isLocated && refUsable && deltaGKcal === null;
      const locatedReason = isLocated && !refUsable ? refReason : null;
      return {
        ...p,
        color: PALETTE[i % PALETTE.length],
        isNeb,
        data,
        maxDatum,
        intrinsicKcal,
        absoluteCell,
        isLocated,
        isEstimate,
        locatedEKcal,
        deltaGKcal,
        gMissing,
        locatedReason,
        eTsEh,
        gTsEh,
        isSp: false,
        spMethod: null as string | null,
        spMatchedOptTsTitle: undefined as string | null | undefined,
      };
    });
  }, [pathways, which, normalizedAxis, reactantsZeroActive, refEnergyEh, refGibbsEh, refInputs, refJobCount]);

  const baseline = series[Math.min(baselineIdx, series.length - 1)];
  const unit = pathways[0]?.scan?.coordinate_unit ?? "Å";
  // Any pathway with a located TS (scan-refine OR a NEB, incl. the G1 estimate) → show the
  // located-TS column + retire "approximate".
  const anyLocated = series.some((s) => s.isLocated);
  const sign = (v: number) => (v >= 0 ? `+${v.toFixed(2)}` : v.toFixed(2));

  // ΔΔ of every other pathway vs the baseline — each GUARDED independently: a method (and,
  // for scan↔scan, coordinate) mismatch shows the reason, never a faked number.
  const comparisons = series
    .filter((s) => s.id !== baseline.id)
    .map((s) => {
      // scan↔scan keeps the coordinate + method guard (unchanged). Any NEB involved → a
      // method-ONLY guard (there is no shared physical coordinate; the located-TS ΔΔ is
      // coordinate-agnostic — it's over saddle energies, not scan maxima).
      const bothScan = !s.isNeb && !baseline.isNeb && !!s.scan && !!baseline.scan;
      const guard: Comparability = bothScan
        ? pathwaysComparable(s.input, baseline.input, s.scan!, baseline.scan!)
        : (() => {
            const a = methodSignature(s.input);
            const b = methodSignature(baseline.input);
            return a.display === b.display
              ? { ok: true }
              : {
                  ok: false,
                  reason: `methods differ — ΔΔ‡ not comparable (${a.display} vs ${b.display})`,
                };
          })();
      // ΔΔE‡: scan↔scan → the scan-max screening value (unchanged). Any NEB involved → the
      // LOCATED ΔΔE‡ over the two TS electronic energies (reference-free — the shared
      // reactants cancel; `absoluteBarrierKcal(a,b) = (a−b)·627.509`), when both have one.
      let ddeKcal: number | null = null;
      let ddeLocated = false;
      if (guard.ok) {
        if (bothScan) {
          ddeKcal = deltaDeltaEKcal(s.scan!, baseline.scan!, which);
        } else if (s.eTsEh != null && baseline.eTsEh != null) {
          ddeKcal = absoluteBarrierKcal(s.eTsEh, baseline.eTsEh);
          ddeLocated = true;
        }
      }
      // ΔΔG‡ — reference-free, over LOCATED-saddle G, only when BOTH faces have a parsed G
      // (a NEB estimate has none → refused). The standard-state correction cancels (same
      // molecularity both faces).
      const ddgKcal =
        guard.ok && s.gTsEh != null && baseline.gTsEh != null
          ? deltaDeltaGKcal(s.gTsEh, baseline.gTsEh)
          : null;
      return {
        id: s.id,
        label: s.label,
        color: s.color,
        guard,
        ddeKcal,
        ddeLocated,
        ddgKcal,
        estimate: s.isEstimate || baseline.isEstimate,
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
        {!normalizedAxis ? (
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
        ) : null}
      </div>

      {width > 0 ? (
        <div className="conv-chart">
          <div className="conv-chart-title">
            {normalizedAxis ? (
              <>
                ΔE ({which === "act" ? "actual" : "SCF"}, kcal/mol) vs{" "}
                <strong>normalized reaction coordinate</strong> (illustrative) — a NEB pathway has no
                physical scan coordinate, so shapes are overlaid on a 0→1 axis; the rigorous number is
                the ΔΔ‡ table below.
              </>
            ) : (
              <>
                ΔE ({which === "act" ? "actual" : "SCF"}, kcal/mol) vs coordinate ({unit}) —{" "}
                {reactantsZeroActive
                  ? "zero at separated reactants (E(ref)); each max = absolute barrier"
                  : "shared zero (global minimum)"}
              </>
            )}
          </div>
          <LineChart
            width={width}
            height={CHART_HEIGHT}
            margin={{ top: 8, right: 16, bottom: 18, left: 8 }}
          >
            <CartesianGrid stroke="var(--border)" strokeDasharray="3 3" />
            <XAxis
              dataKey={normalizedAxis ? "x" : "coordinate"}
              type="number"
              domain={normalizedAxis ? [0, 1] : ["dataMin", "dataMax"]}
              stroke="var(--muted)"
              fontSize={11}
              tickLine={false}
              tickFormatter={(v: number) => v.toFixed(2)}
              label={{
                value: normalizedAxis
                  ? "normalized reaction coordinate (illustrative)"
                  : `coordinate (${unit})`,
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
              labelFormatter={(l) =>
                normalizedAxis
                  ? `reaction coordinate ${Number(l).toFixed(2)} (normalized)`
                  : `coordinate ${Number(l).toFixed(3)} ${unit}`
              }
              contentStyle={{
                background: "var(--panel-2)",
                border: "1px solid var(--border)",
                borderRadius: 6,
                fontSize: 11,
              }}
            />
            {/* A located-TS pathway has no scan/NEB curve — it appears in the ΔΔ table below, NOT
                the overlay chart, so exclude it from the plotted Lines + dots (its `data` is []). */}
            {series
              .filter((s) => s.origin !== "located-ts")
              .map((s) => (
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
            {/* Each pathway's maximum — the approximate/≈ saddle point, in that pathway's colour. */}
            {series.map((s) =>
              s.maxDatum ? (
                <ReferenceDot
                  key={`ts-${s.id}`}
                  x={normalizedAxis ? s.maxDatum.x : s.maxDatum.coordinate}
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
            {anyLocated ? (
              <th
                style={{ textAlign: "right" }}
                title="From a LOCATED transition state (OptTS+Freq), vs separated reactants — a real ΔG‡, not the scan-max estimate"
              >
                Located TS — ΔE‡ · ΔG‡
              </th>
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
                {/* A standalone located-TS pathway has no scan curve → no intrinsic barrier; its
                    barrier is the ABSOLUTE located ΔE‡/ΔG‡ in the Located-TS column. */}
                {s.intrinsicKcal != null ? `+${s.intrinsicKcal.toFixed(2)} kcal/mol` : "—"}
              </td>
              {refJobCount > 0 ? (
                s.absoluteCell === null ? (
                  // A NEB / standalone located-TS pathway has no scan maximum — its barrier is the
                  // located-TS value in the next column, not a scan-max estimate.
                  <td
                    className="muted"
                    style={{ textAlign: "right", fontSize: 12 }}
                    title="No scan-maximum estimate — the barrier is the located TS →"
                  >
                    {s.origin === "located-ts" ? "— (located TS →)" : "— (NEB → located TS)"}
                  </td>
                ) : "kcal" in s.absoluteCell ? (
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
              {anyLocated ? (
                <td className="mono" style={{ textAlign: "right", fontSize: 12 }}>
                  {!s.isLocated ? (
                    <span className="muted" title="No parsed OptTS refinement for this pathway yet">
                      — (refine with OptTS)
                    </span>
                  ) : s.isSp ? (
                    // SP-on-an-OptTS geometry (F4): the geometry-provenance ✓/⚠, the SP method, the
                    // high-accuracy electronic ΔE‡ (or the comparability/incompleteness reason), and
                    // ΔG‡ ABSENT (an SP has no Freq — never 0). All as VISIBLE text (never a tooltip) so
                    // the provenance + match + warning survive a copy into notes.
                    <div style={{ display: "flex", flexDirection: "column", alignItems: "flex-end", gap: 2 }}>
                      {s.spMatchedOptTsTitle != null ? (
                        <span
                          className="muted"
                          style={{ fontSize: 11 }}
                          title="The SP job's final geometry (≡ its input) bit-matches this OptTS job's converged saddle (geometryMatchesFinal) — the SP energy is on that OptTS geometry"
                        >
                          SP energy ({s.spMethod}) on “{s.spMatchedOptTsTitle}” geometry ✓
                        </span>
                      ) : (
                        <span
                          className="banner warn"
                          style={{ fontSize: 11, margin: 0, padding: "2px 6px", textAlign: "left" }}
                          title="The SP geometry bit-matches no OptTS job in this reaction — an SP energy is only a valid TS barrier ON an OptTS geometry. Shown, not enforced: you are responsible for the geometry."
                        >
                          ⚠ SP energy ({s.spMethod}) matches no OptTS in this reaction — you are
                          responsible for the geometry
                        </span>
                      )}
                      {s.locatedReason ? (
                        // Mixed-method (DLPNO SP vs r2SCAN-3c refs) / incomplete / unbalanced reference
                        // → the number is withheld with the specific reason (referenceComparable inside).
                        <span className="muted">{s.locatedReason}</span>
                      ) : (
                        <span title="E(SP) − Σ E(reactant jobs): the high-accuracy electronic barrier (CCSD(T)//DFT) from the SP energy on the OptTS geometry, vs separated reactants">
                          ΔE‡ {s.locatedEKcal != null ? sign(s.locatedEKcal) : "—"}
                        </span>
                      )}
                      <span
                        className="muted"
                        title="A single-point has no Freq → no Gibbs energy → ΔG‡ is ABSENT (never 0). A composite ΔG‡ (SP electronic + the OptTS thermal correction) is a separate step."
                      >
                        ΔG‡ absent (SP — no Freq)
                      </span>
                    </div>
                  ) : s.locatedReason ? (
                    <span className="muted">{s.locatedReason}</span>
                  ) : (
                    <div style={{ display: "flex", flexDirection: "column", alignItems: "flex-end" }}>
                      {s.isEstimate ? (
                        <span
                          className="muted"
                          style={{ fontSize: 11 }}
                          title="The converged NEB-TS energy used as a first-pass ΔE‡ estimate — refine with OptTS+Freq for a located saddle and a real ΔG‡"
                        >
                          NEB TS (unrefined estimate)
                        </span>
                      ) : null}
                      <span title="E(TS) − Σ E(reactant jobs): electronic barrier from the located saddle">
                        ΔE‡ {s.locatedEKcal != null ? sign(s.locatedEKcal) : "—"}
                      </span>
                      {s.deltaGKcal != null ? (
                        <strong title="G(TS) − Σ G(reactant jobs): RAW ORCA ΔG‡ (ideal-gas RRHO, 1 atm, 298.15 K)">
                          ΔG‡ {sign(s.deltaGKcal)}
                        </strong>
                      ) : s.isEstimate ? (
                        <span className="muted" title="An unrefined NEB-TS estimate has no Freq — ΔG‡ is refused. Refine with OptTS+Freq.">
                          ΔG‡ — (estimate, no Freq)
                        </span>
                      ) : s.gMissing ? (
                        <span className="muted" title="ΔG‡ needs G for the TS AND every reference — re-run those with Freq">
                          ΔG‡ — re-run w/ Freq
                        </span>
                      ) : (
                        <span className="muted">ΔG‡ —</span>
                      )}
                    </div>
                  )}
                </td>
              ) : null}
            </tr>
          ))}
        </tbody>
      </table>
      {anyLocated ? (
        <p className="muted" style={{ marginTop: 6, fontSize: 12 }}>
          <strong>ΔG‡ is raw ORCA G</strong> (ideal-gas RRHO, 1 atm, 298.15 K) — it already includes the{" "}
          <strong>association entropy</strong> (two reactants → one TS), so ΔG‡ ≫ ΔE‡ for a bimolecular
          step is <em>expected</em>. Comparing an absolute ΔG‡ to a <strong>solution</strong> experiment
          needs a 1 atm→1 M standard-state correction (~1.9 kcal/mol per molecularity change) — named
          here, <strong>not</strong> auto-applied. (For ΔΔG‡ below the correction <strong>cancels</strong>.)
        </p>
      ) : null}

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
                <>
                  {c.ddeKcal != null ? (
                    <div className="mono" style={{ fontSize: 15 }}>
                      ΔΔE‡({c.label} − {baseline.label}) ={" "}
                      <strong style={{ color: c.color }}>
                        {c.ddeKcal >= 0 ? "+" : ""}
                        {c.ddeKcal.toFixed(2)} kcal/mol
                      </strong>
                      <span className="muted">
                        {" "}
                        {c.ddeLocated
                          ? `(located TS · reference-free${c.estimate ? " · incl. NEB estimate" : ""})`
                          : "(scan-max screening)"}
                      </span>
                    </div>
                  ) : (
                    // Guard passes (same method) but a located ΔΔE‡ needs a TS energy on BOTH
                    // pathways — one side is an unrefined scan with no located TS. Honest note.
                    <div className="muted" style={{ fontSize: 13 }}>
                      ΔΔE‡({c.label} − {baseline.label}): refine both pathways&apos; TS (OptTS) to
                      compare located barriers.
                    </div>
                  )}
                  {c.ddgKcal != null ? (
                    // THE mission headline: ΔΔG‡ over two LOCATED TSs — reference-free, and the
                    // standard-state correction cancels (same molecularity both faces).
                    <div className="mono" style={{ fontSize: 15, marginTop: 2 }}>
                      ΔΔG‡({c.label} − {baseline.label}) ={" "}
                      <strong style={{ color: c.color }}>
                        {c.ddgKcal >= 0 ? "+" : ""}
                        {c.ddgKcal.toFixed(2)} kcal/mol
                      </strong>{" "}
                      <span className="muted">
                        (located TS · reference-free · standard-state cancels)
                      </span>
                    </div>
                  ) : null}
                </>
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
        {anyLocated ? (
          <>
            Pathways with a <strong>located TS</strong> (OptTS+Freq) show a real ΔE‡ / ΔG‡ from the
            actual saddle. A pathway still on the scan maximum shows an{" "}
            <strong>approximate TS (scan maximum)</strong> — a ΔE‡ estimate, not a located saddle and
            not ΔG‡ (refine with OptTS, Stage E).{" "}
          </>
        ) : (
          <>
            Each maximum is an <strong>approximate TS (scan maximum)</strong> — a ΔE‡ estimate on the
            relaxed surface, not a located saddle and not ΔG‡ (refine with OptTS, Stage E).{" "}
          </>
        )}
        ΔΔE‡ is a <strong>screening</strong> value; it is <strong>reference-free</strong> (the shared
        reactant reference cancels).{" "}
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
