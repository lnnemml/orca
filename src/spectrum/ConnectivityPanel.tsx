import { useCallback, useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

import type { Job, JobStatus, ParsedResults, Pathway, Reaction } from "../types";
import { buildConnectivityChildren } from "../scene/connectivity";
import { reactantHint } from "../reactions/compare";
import { GroupSelect } from "../groups/GroupSelect";
import { useGroupPicker, useJobGroupId } from "../groups/useGroupPicker";
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
  // ONE destination-group picker governs BOTH children (forward + backward): defaults to the
  // TS job's group (the source), overridable. Not the active sidebar group (unit 2b).
  const picker = useGroupPicker(useJobGroupId(tsJobId));
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

  // "Study from this OptTS" (Stage F3): once BOTH connectivity children are parsed, turn the TS +
  // its two children into a reaction study. The user DESIGNATES which child is the reactant basin
  // (default = the higher-energy endpoint HINT — an early-TS reading; the user chooses). `""` =
  // use the hint. Isolated in its own fresh reaction so the reactant-reference (a Σ of one) never
  // collides with a scan pathway's fragment-sum reference.
  const [reactantSel, setReactantSel] = useState<"forward" | "backward" | "">("");
  const [studyBusy, setStudyBusy] = useState(false);
  const [studyMsg, setStudyMsg] = useState<string | null>(null);
  const [studyError, setStudyError] = useState<string | null>(null);

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
      // ONE picker → BOTH children land in the one picked group (default = the TS's group)
      // before submitting (unit 2b). No-op each when untouched + ungrouped source.
      await picker.assignPicked(forward.id);
      await picker.assignPicked(backward.id);
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
  }, [disabled, creating, imaginaryMode, tsJobId, tsJobTitle, results, delta, storageKey, picker]);

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
          <label
            className="scan-control"
            title="Destination group for BOTH connectivity children (forward + backward)"
          >
            group
            <GroupSelect
              className="select select-sm"
              groups={picker.groups}
              value={picker.pickedGroupId}
              onChange={picker.onChange}
              aria-label="Destination group for the connectivity children"
            />
          </label>
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

      {/* Stage F3 — a reaction STUDY from this OptTS: shown once both children are parsed. */}
      {children && fwd?.results && bwd?.results ? (
        <StudyFromOptTs
          tsJobId={tsJobId}
          tsJobTitle={tsJobTitle}
          forwardId={children.forwardId}
          backwardId={children.backwardId}
          forwardEnergy={fwd.results.final_energy_eh}
          backwardEnergy={bwd.results.final_energy_eh}
          reactantSel={reactantSel}
          setReactantSel={setReactantSel}
          busy={studyBusy}
          setBusy={setStudyBusy}
          msg={studyMsg}
          setMsg={setStudyMsg}
          error={studyError}
          setError={setStudyError}
        />
      ) : null}
    </section>
  );
}

/**
 * "Study from this OptTS" (Stage F3): create a FRESH, isolated reaction from the located TS + its
 * two connectivity children, attach all three to one pathway, and add the user-designated reactant
 * child as the reactant reference (a Σ of one — the connectivity reactant BASIN). Reuses the
 * existing reaction/pathway/reference commands (no new command). The reactant defaults to the
 * higher-energy endpoint (a hint) but is the user's explicit choice, changeable later in the
 * Reactions screen's reactant-reference section.
 */
function StudyFromOptTs({
  tsJobId,
  tsJobTitle,
  forwardId,
  backwardId,
  forwardEnergy,
  backwardEnergy,
  reactantSel,
  setReactantSel,
  busy,
  setBusy,
  msg,
  setMsg,
  error,
  setError,
}: {
  tsJobId: string;
  tsJobTitle: string;
  forwardId: string;
  backwardId: string;
  forwardEnergy: number | null;
  backwardEnergy: number | null;
  reactantSel: "forward" | "backward" | "";
  setReactantSel: (v: "forward" | "backward" | "") => void;
  busy: boolean;
  setBusy: (b: boolean) => void;
  msg: string | null;
  setMsg: (m: string | null) => void;
  error: string | null;
  setError: (e: string | null) => void;
}) {
  // The default reactant = the higher-energy endpoint (hint); null if energies can't be compared.
  const hint = reactantHint(forwardEnergy, backwardEnergy);
  const effectiveReactant = reactantSel || (hint === "b" ? "backward" : "forward");

  const createStudy = async () => {
    if (busy || msg) return;
    setBusy(true);
    setError(null);
    try {
      const reactantId = effectiveReactant === "backward" ? backwardId : forwardId;
      const reaction = await invoke<Reaction>("create_reaction", {
        name: `Study: ${tsJobTitle}`,
        description: "OptTS TS + connectivity children (located ΔE‡/ΔG‡ vs the reactant basin)",
      });
      const pathway = await invoke<Pathway>("create_pathway", {
        reactionId: reaction.id,
        label: "located TS",
      });
      // Attach the TS + both children to the one pathway (the compare builder gathers them).
      await invoke("attach_job_to_pathway", { jobId: tsJobId, pathwayId: pathway.id });
      await invoke("attach_job_to_pathway", { jobId: forwardId, pathwayId: pathway.id });
      await invoke("attach_job_to_pathway", { jobId: backwardId, pathwayId: pathway.id });
      // The designated reactant child = the reactant reference (a Σ of one — the basin).
      await invoke("add_reference_job", { reactionId: reaction.id, jobId: reactantId });
      setMsg(
        `Created reaction “${reaction.name}” with the ${effectiveReactant} child as the reactant ` +
          `basin. Open the Reactions screen for the located ΔE‡/ΔG‡ (change the reactant there anytime).`,
      );
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  const fmt = (e: number | null) => (e != null ? `${e.toFixed(6)} Eh` : "no energy");

  return (
    <div style={{ marginTop: 12, borderTop: "1px solid var(--border)", paddingTop: 10 }}>
      <div className="section-title" style={{ fontSize: 12 }}>
        Reaction study from this TS (located ΔE‡/ΔG‡)
      </div>
      {msg ? (
        <div className="banner ok" style={{ marginTop: 6 }}>
          {msg}
        </div>
      ) : (
        <>
          <div className="mono" style={{ fontSize: 12, color: "var(--muted)", marginBottom: 6 }}>
            Turn this TS + its two basins into a reaction study. Designate which basin is the{" "}
            <strong>reactant</strong> (the barrier is measured vs it — the associated complex, not
            separated fragments). Default = the higher-energy endpoint.
          </div>
          <div style={{ display: "flex", gap: 10, alignItems: "center", flexWrap: "wrap" }}>
            <label className="scan-control" style={{ fontSize: 12 }}>
              reactant basin
              <select
                className="select select-sm"
                value={effectiveReactant}
                onChange={(e) => setReactantSel(e.target.value as "forward" | "backward")}
              >
                <option value="forward">forward ({fmt(forwardEnergy)})</option>
                <option value="backward">backward ({fmt(backwardEnergy)})</option>
              </select>
            </label>
            <button className="btn btn-sm btn-primary" disabled={busy} onClick={createStudy}>
              {busy ? "Creating…" : "Study from this OptTS"}
            </button>
          </div>
        </>
      )}
      {error ? (
        <div className="banner err" style={{ marginTop: 6 }}>
          Could not create the study: {error}
        </div>
      ) : null}
    </div>
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
