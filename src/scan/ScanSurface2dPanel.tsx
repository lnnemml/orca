import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

import type { Job, ScanSurface2d } from "../types";
import { buildOptTSInput } from "../scene/optts";
import {
  OptTSMethodPicker,
  useSourceIsXtb,
  type OptTSMethodOverride,
} from "../input-builder/OptTSMethodPicker";
import { HARTREE_TO_KCAL } from "../units";
import { GroupSelect } from "../groups/GroupSelect";
import { useGroupPicker, useJobGroupId } from "../groups/useGroupPicker";
import { parseScanSurface2d } from "./scanSurface2d";
import { ContourPlot } from "./ContourPlot";

/**
 * The 2D relaxed-surface-scan viewer (Phase 4.5 Stage 4b) — a filled contour / heatmap of the
 * PES over the two scanned coordinates, with clickable grid nodes that hand THAT node's geometry
 * to OptTS (the source-agnostic refine engine, reusing `ScanProfilePanel`'s handoff verbatim).
 * Routed (from `JobDetailScreen`) when the job's INPUT is a 2-coordinate scan — a 2D scan has no
 * `results.scan` (the 1D reader stands down on the 3-column `.dat`), so this reads its own data
 * via the file-gated `read_scan_surface`, independent of the results row.
 *
 * **No auto-pick.** The global maximum of a stepwise-capable surface is a stepwise CORNER, not the
 * concerted transition state — so nothing is preselected; the researcher reads the surface and
 * clicks the col. The corner labels are orientation facts (bond lengths), never a TS claim.
 *
 * **Identity seam (the MAIN RISK):** a clicked node `(i1, i2)` → `grid.nodeRow` (1-based, row-major
 * outer=coord1) → `geometries[row-1]`. A **count assert** (`geometries.length === N₁×N₂`) gates the
 * handoff off a partial run — a missing point file disables click-to-refine rather than seeding
 * OptTS from the wrong geometry.
 */
export function ScanSurface2dPanel({
  jobId,
  jobTitle,
  onOpenJob,
}: {
  jobId: string;
  jobTitle: string;
  onOpenJob?: (jobId: string) => void;
}) {
  // undefined = loading; null = no surface (absent .dat); else the payload.
  const [surface, setSurface] = useState<ScanSurface2d | null | undefined>(undefined);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [refining, setRefining] = useState(false);
  const [refineError, setRefineError] = useState<string | null>(null);
  // OptTS method override (default = inherit → `{}` → byte-identical). `sourceIsXtb` drives
  // the "pick a DFT level" note.
  const [methodOverride, setMethodOverride] = useState<OptTSMethodOverride>({});
  const sourceIsXtb = useSourceIsXtb(jobId);

  // 2b group picker — default = THIS scan job's group; rides the create_optts_job handoff.
  const picker = useGroupPicker(useJobGroupId(jobId));

  useEffect(() => {
    let live = true;
    invoke<ScanSurface2d | null>("read_scan_surface", { id: jobId })
      .then((s) => {
        if (live) setSurface(s);
      })
      .catch((e) => {
        if (live) {
          setSurface(null);
          setLoadError(String(e));
        }
      });
    return () => {
      live = false;
    };
  }, [jobId]);

  if (surface === undefined) {
    return (
      <div className="scan-surface-panel">
        <div className="scan-head">2D relaxed surface scan</div>
        <div className="muted">Loading surface…</div>
      </div>
    );
  }
  if (loadError) {
    return (
      <div className="scan-surface-panel">
        <div className="scan-head">2D relaxed surface scan</div>
        <div className="banner err">Could not read the scan surface: {loadError}</div>
      </div>
    );
  }
  if (!surface) {
    return (
      <div className="scan-surface-panel">
        <div className="scan-head">2D relaxed surface scan</div>
        <div className="muted">
          No surface yet — <code>input.relaxscanact.dat</code> is absent (run the scan).
        </div>
      </div>
    );
  }

  const grid = parseScanSurface2d(surface.dat_text);
  if (!grid) {
    return (
      <div className="scan-surface-panel">
        <div className="scan-head">2D relaxed surface scan</div>
        <div className="banner warn">
          The scan <code>.dat</code> could not be parsed as a 2D grid (not a clean N₁×N₂ surface).
        </div>
      </div>
    );
  }

  const expected = grid.axis1.length * grid.axis2.length;
  // COUNT-ASSERT (identity seam): every node must have a geometry, else the handoff is disabled.
  const complete = surface.geometries.length === expected;

  // ΔE (kcal/mol) relative to the global minimum — the colour; the reference is a labelled choice.
  const emin = Math.min(...grid.energies.flat());
  const z = grid.energies.map((row) => row.map((e) => (e - emin) * HARTREE_TO_KCAL));
  const annotations = cornerAnnotations(grid.axis1, grid.axis2);

  const refine = async (i1: number, i2: number) => {
    if (refining || !complete) return;
    setRefining(true);
    setRefineError(null);
    try {
      const row = grid.nodeRow(i1, i2); // 1-based row = point-file NNN = geometries[row-1]
      const seed = surface.geometries[row - 1];
      if (!seed) throw new Error(`no geometry for node (${i1}, ${i2}) → row ${row}`);
      // Reuse the ScanProfilePanel handoff verbatim: read THIS job's own input (method/charge),
      // seed the source-agnostic OptTS engine from the picked node, create + submit + navigate.
      const source = await invoke<Job>("get_job", { id: jobId });
      // `methodOverride` is `{}` (inherit) unless the user picked a level — default is byte-identical.
      const input = buildOptTSInput(source.input_content, seed, methodOverride);
      const child = await invoke<Job>("create_optts_job", {
        sourceJobId: jobId,
        title: `OptTS — ${jobTitle}`,
        inputContent: input,
      });
      await picker.assignPicked(child.id); // the 2b group picker rides along
      await invoke("submit_job", { id: child.id });
      onOpenJob?.(child.id);
    } catch (e) {
      // A charge/post-condition failure (buildOptTSInput) lands here — no job created.
      console.error("[optts-2d]", e);
      setRefineError(String(e));
    } finally {
      setRefining(false);
    }
  };

  return (
    <div className="scan-surface-panel">
      <div className="scan-head">
        2D relaxed surface scan
        <span className="muted">
          {" "}· {grid.axis1.length} × {grid.axis2.length} = {expected} points
        </span>
      </div>

      {!complete ? (
        <div className="banner warn">
          Incomplete scan: {surface.geometries.length} of {expected} point geometries present.
          Click-to-refine is disabled until the run finishes (never hand OptTS a wrong node).
        </div>
      ) : (
        <div className="muted scan-surface-hint">
          Click a grid node to refine THAT geometry with OptTS. The global maximum is a stepwise
          corner, not the transition state — pick the col yourself (no auto-selection).
        </div>
      )}

      {refineError ? (
        <div className="banner err">OptTS refine failed (no job created): {refineError}</div>
      ) : null}

      <div className="scan-surface-grouprow">
        <span className="muted">refined child group:</span>
        <GroupSelect
          className="select select-sm"
          groups={picker.groups}
          value={picker.pickedGroupId}
          onChange={picker.onChange}
          aria-label="Destination group for the refined child"
        />
      </div>

      {/* OptTS method for the clicked-node refine — default "Inherit from source" (byte-identical);
          pick a level to refine at (fixes XTB scan → XTB OptTS). */}
      <OptTSMethodPicker sourceIsXtb={sourceIsXtb} onChange={setMethodOverride} />

      <ContourPlot
        axis1={grid.axis1}
        axis2={grid.axis2}
        z={z}
        x1Label="coordinate 1 (Å)"
        x2Label="coordinate 2 (Å)"
        colorbarTitle="ΔE (kcal/mol)"
        annotations={annotations}
        onNodeClick={(i1, i2) => refine(i1, i2)}
      />
    </div>
  );
}

/** Label the four grid corners by bond length (facts) with a light reactant/product hint —
 * orientation only, never a TS claim. Long/short is per-axis, so it is correct whether the scan
 * steps low→high or high→low. */
function cornerAnnotations(
  axis1: number[],
  axis2: number[],
): Array<{ i1: number; i2: number; text: string }> {
  const last1 = axis1.length - 1;
  const last2 = axis2.length - 1;
  const a1max = Math.max(axis1[0], axis1[last1]);
  const a2max = Math.max(axis2[0], axis2[last2]);
  const label = (c1: number, c2: number): string => {
    const long1 = c1 === a1max;
    const long2 = c2 === a2max;
    if (long1 && long2) return "reactant (both long)";
    if (!long1 && !long2) return "product (both short)";
    return "stepwise (1 long, 1 short)";
  };
  const corners: Array<[number, number]> = [
    [0, 0],
    [0, last2],
    [last1, 0],
    [last1, last2],
  ];
  return corners.map(([i1, i2]) => ({ i1, i2, text: label(axis1[i1], axis2[i2]) }));
}
