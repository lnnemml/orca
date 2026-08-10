import { useCallback, useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

import type { Job, JobStatus, ParsedResults } from "../types";
import { buildConnectivityChildren } from "../scene/connectivity";
import {
  connectivityVerdict,
  reactionCoordinateChanges,
  modeDisplacements,
  DEFAULT_CONNECTIVITY_DELTA_ANGSTROM,
  type Geometry,
} from "./mode";

/**
 * Connectivity check on a LOCATED transition state (Stage E2). Displaces the TS ±δ
 * along its single imaginary mode, creates TWO plain-Opt children (the app generates
 * BOTH geometries — no manual forward/backward mix-up), and, once both relax, reports
 * whether they reached two distinct basins. The imaginary mode IS the reaction
 * coordinate; see `wiki/orca/connectivity.md`.
 *
 * Rendered only for a located TS (`imaginary_count === 1`). The two child job ids are
 * persisted in `localStorage` keyed by the TS job (no schema change) so the verdict
 * survives a reload. Honest-absent throughout: while a child is running / has not
 * parsed / failed, the panel says so and never fabricates a verdict.
 */
export function ConnectivityPanel({
  tsJobId,
  tsJobTitle,
  results,
  onOpenJob,
}: {
  tsJobId: string;
  tsJobTitle: string;
  results: ParsedResults;
  onOpenJob?: (jobId: string) => void;
}) {
  const f = results.frequencies;
  const storageKey = `orcastudio.connectivity.${tsJobId}`;

  const [delta, setDelta] = useState(DEFAULT_CONNECTIVITY_DELTA_ANGSTROM);
  const [creating, setCreating] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [children, setChildren] = useState<{ forwardId: string; backwardId: string } | null>(
    () => {
      try {
        const raw = localStorage.getItem(storageKey);
        return raw ? (JSON.parse(raw) as { forwardId: string; backwardId: string }) : null;
      } catch {
        return null;
      }
    },
  );
  const [fwd, setFwd] = useState<ChildState | null>(null);
  const [bwd, setBwd] = useState<ChildState | null>(null);

  // The imaginary mode vector — extracted ONCE from the parsed .hess (reused, never
  // re-parsed): the single negative frequency's column of $normal_modes, flat 3N.
  const imaginaryMode = useMemo(() => {
    if (!f || f.imaginary_count !== 1) return null;
    const imagIdx = f.frequencies_cm.findIndex((v) => v < 0);
    if (imagIdx < 0) return null;
    try {
      return modeDisplacements(f.normal_modes, f.n_modes, imagIdx).flat();
    } catch {
      return null;
    }
  }, [f]);

  // Poll the two children until both are terminal; read results once a child is parsed.
  useEffect(() => {
    if (!children) return;
    let live = true;
    const poll = async () => {
      const read = async (id: string): Promise<ChildState> => {
        const j = await invoke<Job>("get_job", { id });
        let res: ParsedResults | null = null;
        if (j.status === "parsed") {
          res = await invoke<ParsedResults | null>("read_job_results", { id });
        }
        return { id, status: j.status, results: res };
      };
      try {
        const [a, b] = await Promise.all([read(children.forwardId), read(children.backwardId)]);
        if (!live) return;
        setFwd(a);
        setBwd(b);
      } catch {
        /* a transient read error — the next tick retries */
      }
    };
    void poll();
    const timer = setInterval(() => {
      // Stop once both are terminal (parsed/failed/cancelled).
      if (isTerminal(fwd?.status) && isTerminal(bwd?.status)) return;
      void poll();
    }, 2000);
    return () => {
      live = false;
      clearInterval(timer);
    };
  }, [children, fwd?.status, bwd?.status]);

  const disabled = !f || f.imaginary_count !== 1 || imaginaryMode == null;

  const onVerify = useCallback(async () => {
    if (disabled || creating || imaginaryMode == null) return;
    setCreating(true);
    setError(null);
    try {
      // Read the TS job's OWN input as the context (method/solvation/charge) — never
      // reconstruct it. The pure builder inherits + asserts (c, m) and method/solvation.
      const ts = await invoke<Job>("get_job", { id: tsJobId });
      const tsGeometry: Geometry = results.final_geometry;
      const { forwardInput, backwardInput } = buildConnectivityChildren(
        ts.input_content,
        tsGeometry,
        imaginaryMode,
        delta,
      );
      // Reuse the GENERIC source-agnostic child-create path (create_optts_job): source
      // = the TS job, so both children join the TS's pathway. Nothing here is
      // OptTS/scan-specific — the SAME path is E3's NEB reuse.
      const forward = await invoke<Job>("create_optts_job", {
        sourceJobId: tsJobId,
        title: `Connectivity → forward — ${tsJobTitle}`,
        inputContent: forwardInput,
      });
      const backward = await invoke<Job>("create_optts_job", {
        sourceJobId: tsJobId,
        title: `Connectivity ← backward — ${tsJobTitle}`,
        inputContent: backwardInput,
      });
      await invoke("submit_job", { id: forward.id });
      await invoke("submit_job", { id: backward.id });
      const next = { forwardId: forward.id, backwardId: backward.id };
      try {
        localStorage.setItem(storageKey, JSON.stringify(next));
      } catch {
        /* persistence is best-effort; the session still has the ids in state */
      }
      setFwd(null);
      setBwd(null);
      setChildren(next);
    } catch (e) {
      // A charge / no-`* xyz` post-condition failure (buildConnectivityChildren) lands
      // here — no job created.
      console.error("[connectivity]", e);
      setError(String(e));
    } finally {
      setCreating(false);
    }
  }, [disabled, creating, imaginaryMode, tsJobId, tsJobTitle, results, delta, storageKey]);

  const onReset = useCallback(() => {
    try {
      localStorage.removeItem(storageKey);
    } catch {
      /* ignore */
    }
    setChildren(null);
    setFwd(null);
    setBwd(null);
    setError(null);
  }, [storageKey]);

  if (!f || f.imaginary_count !== 1) return null;

  return (
    <section>
      <div className="section-title" style={{ fontSize: 12 }}>
        Connectivity check (± imaginary mode)
      </div>
      <div className="mono" style={{ fontSize: 12, color: "var(--muted)", marginBottom: 6 }}>
        The imaginary mode is the reaction coordinate — displace the TS ±δ along it and
        relax (plain Opt) to see the two basins it connects.
      </div>

      {!children ? (
        <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
          <label className="mono" style={{ fontSize: 12, display: "flex", alignItems: "center", gap: 4 }}>
            δ (Å)
            <input
              type="number"
              className="input"
              style={{ width: 72 }}
              min={0.1}
              max={1.5}
              step={0.1}
              value={delta}
              onChange={(e) => setDelta(Number(e.target.value))}
            />
          </label>
          <button
            className="btn btn-sm btn-primary"
            disabled={disabled || creating}
            title={
              disabled
                ? "Needs a located TS with exactly one imaginary mode"
                : "Create two plain-Opt children displaced ±δ along the imaginary mode"
            }
            onClick={onVerify}
          >
            {creating ? "Creating…" : "Verify connectivity (± imaginary mode)"}
          </button>
        </div>
      ) : (
        <ConnectivityVerdict
          fwd={fwd}
          bwd={bwd}
          tsGeometry={results.final_geometry}
          onOpenJob={onOpenJob}
          onReset={onReset}
        />
      )}

      {error ? (
        <div className="banner err" style={{ marginTop: 6 }}>
          Connectivity check failed (no jobs created): {error}
        </div>
      ) : null}
    </section>
  );
}

interface ChildState {
  id: string;
  status: JobStatus;
  results: ParsedResults | null;
}

function isTerminal(s: JobStatus | undefined): boolean {
  return s === "parsed" || s === "failed" || s === "cancelled";
}

/** The verdict once (or before) both children have relaxed. Honest-pending: no verdict
 * is shown until BOTH are parsed; a failed/cancelled child is reported as such. */
function ConnectivityVerdict({
  fwd,
  bwd,
  tsGeometry,
  onOpenJob,
  onReset,
}: {
  fwd: ChildState | null;
  bwd: ChildState | null;
  tsGeometry: Geometry;
  onOpenJob?: (jobId: string) => void;
  onReset: () => void;
}) {
  const bothParsed = fwd?.results != null && bwd?.results != null;

  let body: React.ReactNode;
  if (fwd?.status === "failed" || bwd?.status === "failed") {
    body = (
      <div className="banner err">
        A child job failed — connectivity is undetermined. Re-run after fixing the input.
      </div>
    );
  } else if (!bothParsed) {
    body = (
      <div className="mono" style={{ fontSize: 12, color: "var(--muted)" }}>
        pending — forward: <strong>{fwd?.status ?? "…"}</strong>, backward:{" "}
        <strong>{bwd?.status ?? "…"}</strong> (verdict appears when both parse)
      </div>
    );
  } else {
    body = (
      <VerdictBody
        forward={fwd!.results!.final_geometry}
        backward={bwd!.results!.final_geometry}
        ts={tsGeometry}
      />
    );
  }

  return (
    <div style={{ display: "grid", gap: 6 }}>
      <div style={{ display: "flex", gap: 10, alignItems: "center", flexWrap: "wrap" }}>
        <ChildBadge label="forward →" child={fwd} onOpenJob={onOpenJob} />
        <ChildBadge label="← backward" child={bwd} onOpenJob={onOpenJob} />
        <button className="btn btn-sm" onClick={onReset} title="Forget these children and start over">
          reset
        </button>
      </div>
      {body}
    </div>
  );
}

function ChildBadge({
  label,
  child,
  onOpenJob,
}: {
  label: string;
  child: ChildState | null;
  onOpenJob?: (jobId: string) => void;
}) {
  return (
    <span className="mono" style={{ fontSize: 12 }}>
      {label}{" "}
      {child ? (
        <button
          onClick={() => onOpenJob?.(child.id)}
          title="Open this job"
          style={{ background: "none", border: "none", padding: 0, cursor: "pointer" }}
        >
          <span className={`badge ${child.status}`}>{child.status}</span>
        </button>
      ) : (
        <span className="badge">…</span>
      )}
    </span>
  );
}

/** The computed verdict + the reaction-coordinate distances (which is reactant/product).
 * Wrapped so a geometry-shape mismatch (should not happen) surfaces as an honest error,
 * never a crash. */
function VerdictBody({
  forward,
  backward,
  ts,
}: {
  forward: Geometry;
  backward: Geometry;
  ts: Geometry;
}) {
  let verdict: ReturnType<typeof connectivityVerdict>;
  let coords: ReturnType<typeof reactionCoordinateChanges>;
  try {
    verdict = connectivityVerdict(forward, backward, ts);
    coords = reactionCoordinateChanges(forward, backward, ts, 3);
  } catch (e) {
    return (
      <div className="banner err">Could not compare the endpoints: {String(e)}</div>
    );
  }

  return (
    <div style={{ display: "grid", gap: 6 }}>
      <div style={{ fontSize: 13 }}>
        {verdict.distinctBasins ? (
          <span style={{ color: "var(--ok, #2a7)" }}>
            ✓ forward and backward relaxed to two distinct minima — this TS connects two
            basins
          </span>
        ) : (
          <span className="muted">
            forward and backward did not separate into two basins — they may have relaxed
            back to the TS (try a larger δ) or to the same minimum
          </span>
        )}
      </div>
      <table className="mono" style={{ fontSize: 12, borderCollapse: "collapse" }}>
        <thead>
          <tr style={{ color: "var(--muted)" }}>
            <th style={{ textAlign: "left", paddingRight: 12 }}>bond (Å)</th>
            <th style={{ textAlign: "right", paddingLeft: 12 }}>forward</th>
            <th style={{ textAlign: "right", paddingLeft: 12 }}>TS</th>
            <th style={{ textAlign: "right", paddingLeft: 12 }}>backward</th>
          </tr>
        </thead>
        <tbody>
          {coords.map((c) => (
            <tr key={`${c.i}-${c.j}`}>
              <td style={{ paddingRight: 12 }}>
                {c.elements[0]}
                {c.i}–{c.elements[1]}
                {c.j}
              </td>
              <td style={{ textAlign: "right", paddingLeft: 12 }}>
                {c.distForwardAngstrom.toFixed(3)}
              </td>
              <td style={{ textAlign: "right", paddingLeft: 12, color: "var(--muted)" }}>
                {c.distTsAngstrom.toFixed(3)}
              </td>
              <td style={{ textAlign: "right", paddingLeft: 12 }}>
                {c.distBackwardAngstrom.toFixed(3)}
              </td>
            </tr>
          ))}
        </tbody>
      </table>
      <div className="mono" style={{ fontSize: 11, color: "var(--muted)" }}>
        endpoint separation {verdict.endpointSeparation.toFixed(3)} Å · forward−TS{" "}
        {verdict.fwdShiftFromTs.toFixed(3)} Å · backward−TS {verdict.bwdShiftFromTs.toFixed(3)} Å
        (max interatomic-distance change)
      </div>
    </div>
  );
}
