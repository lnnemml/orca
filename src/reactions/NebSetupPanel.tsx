import { useCallback, useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

import type { Job, ParsedResults } from "../types";
import { buildNebInput } from "../scene/neb";

/**
 * Minimal NEB-TS setup (Stage E3a-1): pick a reactant job + a product job (both parsed
 * minima), and create a NEB-TS job that searches for the saddle between them. The band
 * VIEWER and the "Refine with OptTS from the NEB-TS" action are E3a-2 — here the job is
 * just created and submitted; its results parse into the NEB section.
 *
 * The SAME-ORDER requirement (reactant and product must share atom order) is enforced by
 * `buildNebInput`, which THROWS on a mismatch — surfaced here as an honest refusal, so no
 * NEB job with a mismatched pair is ever created.
 */
export function NebSetupPanel({ onOpenJob }: { onOpenJob: (jobId: string) => void }) {
  const [jobs, setJobs] = useState<Job[]>([]);
  const [reactantId, setReactantId] = useState("");
  const [productId, setProductId] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(async () => {
    try {
      const all = await invoke<Job[]>("list_jobs");
      // Only parsed jobs have a final geometry to interpolate between.
      setJobs(all.filter((j) => j.status === "parsed"));
    } catch (e) {
      setError(String(e));
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  const canCreate = reactantId !== "" && productId !== "" && reactantId !== productId && !busy;

  const create = useCallback(async () => {
    if (!canCreate) return;
    setBusy(true);
    setError(null);
    try {
      const [rj, pj] = await Promise.all([
        invoke<Job>("get_job", { id: reactantId }),
        invoke<Job>("get_job", { id: productId }),
      ]);
      const [rr, pr] = await Promise.all([
        invoke<ParsedResults | null>("read_job_results", { id: reactantId }),
        invoke<ParsedResults | null>("read_job_results", { id: productId }),
      ]);
      if (!rr || !pr) {
        throw new Error("both jobs must be parsed with a final geometry");
      }
      // buildNebInput inherits method/solvation/charge from the reactant and THROWS if
      // the two geometries do not share atom order — the honest refusal below.
      const { inp, productXyz } = buildNebInput(
        rj.input_content,
        rr.final_geometry,
        pr.final_geometry,
      );
      const child = await invoke<Job>("create_neb_job", {
        reactantJobId: reactantId,
        productJobId: productId,
        title: `NEB — ${rj.title} → ${pj.title}`,
        inpContent: inp,
        productXyzContent: productXyz,
      });
      await invoke("submit_job", { id: child.id });
      onOpenJob(child.id);
    } catch (e) {
      // A same-order / charge / no-`* xyz` failure lands here — NO job was created.
      console.error("[neb]", e);
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }, [canCreate, reactantId, productId, onOpenJob]);

  const options = useMemo(
    () =>
      jobs.map((j) => (
        <option key={j.id} value={j.id}>
          {j.title}
        </option>
      )),
    [jobs],
  );

  return (
    <section className="input-builder" style={{ marginBottom: 14 }}>
      <div className="section-title" style={{ padding: "8px 10px" }}>
        Create NEB-TS (reactant + product → saddle)
      </div>
      <div style={{ padding: 10, display: "grid", gap: 8 }}>
        <div className="mono" style={{ fontSize: 12, color: "var(--muted)" }}>
          Find a transition state between two optimized minima when there is no clean scan
          coordinate. Both must share atom order (element sequence + count).
        </div>
        <div className="row" style={{ gap: 8, flexWrap: "wrap", alignItems: "center" }}>
          <label className="mono" style={{ fontSize: 12, display: "flex", gap: 4, alignItems: "center" }}>
            reactant
            <select className="select" value={reactantId} onChange={(e) => setReactantId(e.target.value)}>
              <option value="">— pick a parsed job —</option>
              {options}
            </select>
          </label>
          <label className="mono" style={{ fontSize: 12, display: "flex", gap: 4, alignItems: "center" }}>
            product
            <select className="select" value={productId} onChange={(e) => setProductId(e.target.value)}>
              <option value="">— pick a parsed job —</option>
              {options}
            </select>
          </label>
          <button className="btn btn-sm btn-primary" disabled={!canCreate} onClick={create}>
            {busy ? "Creating…" : "Create NEB-TS"}
          </button>
          <button className="btn btn-sm" onClick={load} title="Refresh the parsed-job list">
            ↻
          </button>
        </div>
        {reactantId !== "" && reactantId === productId ? (
          <div className="mono" style={{ fontSize: 11, color: "var(--muted)" }}>
            reactant and product must be different jobs
          </div>
        ) : null}
        {error ? (
          <div className="banner err">NEB not created: {error}</div>
        ) : null}
      </div>
    </section>
  );
}
