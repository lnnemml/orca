import { useCallback, useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

import type { Job, ParsedResults } from "../types";
import { buildNebInput } from "../scene/neb";
import type { BuilderState } from "../input-builder/build-input";

/**
 * The payload a NEB "Generate" hands back alongside the `.inp` text: the product image
 * (a separate aux file `create_neb_job` materializes into the isolated job dir) and the
 * two source job ids (`create_neb_job` attaches the reactant's pathway). Carried through
 * `NewJobScreen`'s `pendingNeb` until Create / Create & Run.
 */
export interface NebPayload {
  productXyz: string;
  reactantJobId: string;
  productJobId: string;
}

/**
 * NEB-TS setup INSIDE the Input Builder (N2): pick a reactant job + a product job (both
 * parsed minima), then "Generate NEB input" builds the `.inp` (+ product.xyz) INTO the
 * editor for review/edit — it does NOT submit. The standard Create / Create & Run then
 * creates the job via `create_neb_job` (deferred run).
 *
 * The NEB LEVEL is the builder's `state` (method/family/basis/solvation/SCF), so NEB runs
 * at whatever the user picked above — including GFN2-xTB from DFT-optimized endpoints. The
 * (charge, multiplicity) still come from the REACTANT input (the footgun, in `buildNebInput`).
 *
 * The SAME-ORDER requirement is enforced by `buildNebInput`, which THROWS on a mismatch —
 * surfaced here as an honest refusal, so no mismatched pair ever reaches the editor.
 */
export function NebBuilderSection({
  state,
  onGenerateNeb,
}: {
  state: BuilderState;
  onGenerateNeb: (inp: string, payload: NebPayload) => void;
}) {
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

  const canGenerate =
    reactantId !== "" && productId !== "" && reactantId !== productId && !busy;

  const generate = useCallback(async () => {
    if (!canGenerate) return;
    setBusy(true);
    setError(null);
    try {
      // The reactant JOB (for its input `* xyz` charge/mult) + BOTH results (final
      // geometries). The product's job record is not needed — only its geometry.
      const [rj, rr, pr] = await Promise.all([
        invoke<Job>("get_job", { id: reactantId }),
        invoke<ParsedResults | null>("read_job_results", { id: reactantId }),
        invoke<ParsedResults | null>("read_job_results", { id: productId }),
      ]);
      if (!rr || !pr) {
        throw new Error("both jobs must be parsed with a final geometry");
      }
      // buildNebInput emits at the BUILDER's level, inherits (charge, mult) from the
      // reactant, and THROWS if the two geometries do not share atom order — the honest
      // refusal below (no generation into the editor).
      const { inp, productXyz } = buildNebInput(
        state,
        rj.input_content,
        rr.final_geometry,
        pr.final_geometry,
      );
      onGenerateNeb(inp, {
        productXyz,
        reactantJobId: reactantId,
        productJobId: productId,
      });
    } catch (e) {
      // A same-order / charge / no-`* xyz` failure lands here — NOTHING was generated.
      console.error("[neb]", e);
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }, [canGenerate, reactantId, productId, state, onGenerateNeb]);

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
    <div className="builder-row" style={{ flexDirection: "column", alignItems: "stretch", gap: 8 }}>
      <div className="builder-note muted">
        NEB-TS finds a saddle between two optimized minima. Pick a reactant + product
        (both must share atom order); the method/basis above set the NEB level. Generate
        builds the input into the editor — review it, then Create &amp; Run.
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
        <button className="btn btn-sm btn-primary" disabled={!canGenerate} onClick={generate}>
          {busy ? "Generating…" : "Generate NEB input"}
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
      {error ? <div className="banner err">NEB input not generated: {error}</div> : null}
    </div>
  );
}
