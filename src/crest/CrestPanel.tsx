import { useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

import type { Scene } from "../scene/types";
import { mergeToXyz, totalCharge } from "../scene/scene";
import { frameToXyz } from "../trajectory/frame";
import { MoleculeViewer } from "../viewer/MoleculeViewer";
import { SOLVENT_LIBRARY } from "./solvents";
import { crestSeedNote } from "./seed-note";
import { formatCrestProgress } from "./crest-progress";

/** The `crest:done` payload — mirrors Rust `CrestGrowDone` (snake_case serde). The cluster
 * is a GEOMETRY SEED; `seed_energy_eh` is xtb-level, never a solvated result. */
interface CrestGrowDone {
  result: {
    cluster: { elements: string[]; xyz_angstrom: [number, number, number][] };
    seed_energy_eh: number | null;
    /** The charge CREST actually used (parsed from crest.out) — drives the seed banner,
     * so it reflects what ran, not what the form intended. */
    intended_charge: number | null;
    n_atoms: number;
  };
  growth: { size: number; energy_eh: number; delta: number }[];
}

/**
 * CREST/QCG microsolvation (Stage F F1c) — RUN a QCG grow from the current scene and DISPLAY
 * the grown cluster with an ALWAYS-on, charge-aware seed warning. Mirrors the xtb pre-opt
 * placement + event handling (`crest:done`/`crest:error`), but the cluster is a **transient
 * display** — nothing is committed to the scene and nothing is persisted (K3). The "Create
 * ORCA re-opt job" accept action is F2 (a disabled placeholder here).
 *
 * **The two footguns this panel is built around:**
 *  - **Charge from the SCENE, never a silent 0** (ADR-014): `opts.charge = totalCharge(scene)`
 *    — the same discipline as the xtb pre-opt (which reads the scene's charge, not a form
 *    field). The panel only renders when a scene exists.
 *  - **The seed is not the answer.** `seed_energy_eh` is labelled "xtb-ALPB seed (not
 *    solvated)", and `crestSeedNote(result.intended_charge)` ALWAYS renders — a nonzero charge
 *    is a loud warning (QCG grew the cluster neutral, so the energy is the wrong species').
 */
export function CrestPanel({ scene }: { scene: Scene }) {
  const [solventIdx, setSolventIdx] = useState(0);
  const [nsolv, setNsolv] = useState(3);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [errorDir, setErrorDir] = useState<string | null>(null);
  const [done, setDone] = useState<CrestGrowDone | null>(null);
  const [elapsed, setElapsed] = useState(0);
  const startRef = useRef(0);
  // The charge the run was launched with — the safe fallback for the seed banner if
  // crest.out's charge line was somehow unreadable (bias toward warning). CREST's own
  // parsed value (`result.intended_charge`) is preferred.
  const launchedChargeRef = useRef(0);

  // Charge comes from the SCENE (ADR-014 — never a silent 0). A scene always has a
  // total charge (Σ fragment.charge); the panel only renders when a scene exists.
  const sceneCharge = totalCharge(scene);
  const solvent = SOLVENT_LIBRARY[solventIdx];
  // -fixsolute is AUTO (matches the probe): water needs -nofix, everything else -fixsolute.
  const fixSolute = solvent.alpbName !== "water";

  const runGrow = async () => {
    setError(null);
    setErrorDir(null);
    setDone(null);
    setElapsed(0);
    startRef.current = Date.now();
    launchedChargeRef.current = sceneCharge;
    setBusy(true);
    try {
      await invoke("crest_grow", {
        soluteXyz: mergeToXyz(scene),
        solventXyz: solvent.xyz,
        opts: {
          solvent_name: solvent.alpbName,
          nsolv,
          charge: sceneCharge, // from the scene, never a form default
          uhf: scene.multiplicity - 1,
          fix_solute: fixSolute,
          threads: 4,
        },
      });
      // Started — the result/errors arrive as events (handled below).
    } catch (e) {
      // Synchronous rejection only: busy slot / bad input. No run started.
      setBusy(false);
      setError(e instanceof Error ? e.message : String(e));
    }
  };

  const cancelGrow = () => {
    invoke("crest_cancel").catch(() => {});
  };

  // Result/error events from the off-thread CREST run. Subscribed only while busy.
  useEffect(() => {
    if (!busy) return;
    let cancelled = false;
    const unlisten = Promise.all([
      listen<CrestGrowDone>("crest:done", (event) => {
        if (cancelled) return;
        setBusy(false);
        setDone(event.payload);
      }),
      listen<{ message: string; dir: string | null }>("crest:error", (event) => {
        if (cancelled) return;
        setBusy(false);
        setError(event.payload.message);
        setErrorDir(event.payload.dir);
      }),
      // The F1b runner emits no progress today; wired for a future growth-step signal.
      listen<unknown>("crest:progress", () => {}),
    ]);
    return () => {
      cancelled = true;
      unlisten.then((fns) => fns.forEach((f) => f()));
    };
  }, [busy]);

  // Tick the elapsed clock while the grow runs (the only live signal — no cycle counter).
  useEffect(() => {
    if (!busy) return;
    const id = setInterval(
      () => setElapsed(Math.round((Date.now() - startRef.current) / 1000)),
      1000,
    );
    return () => clearInterval(id);
  }, [busy]);

  // The grown cluster as xyz for the viewer — the canonical builder (no second formatter).
  const clusterXyz = useMemo(() => {
    if (!done) return null;
    return frameToXyz(done.result.cluster.elements, {
      energy_eh: done.result.seed_energy_eh,
      xyz_angstrom: done.result.cluster.xyz_angstrom,
    });
  }, [done]);

  // The seed banner reads CREST's OWN parsed charge (what ran), falling back to the
  // launched scene charge only if crest.out's line was unreadable.
  const note = done
    ? crestSeedNote(done.result.intended_charge ?? launchedChargeRef.current)
    : null;

  return (
    <div className="xtb-panel crest-panel">
      <div className="section-title" style={{ fontSize: 12 }}>
        Microsolvation (CREST/QCG) <span className="muted">— explicit solvent shell → geometry seed</span>
      </div>

      <div className="xtb-row" style={{ flexWrap: "wrap", gap: 8, alignItems: "flex-end" }}>
        <label className="mono" style={{ fontSize: 12, display: "flex", flexDirection: "column", gap: 2 }}>
          solvent
          <select
            className="select select-sm"
            value={solventIdx}
            onChange={(e) => setSolventIdx(Number(e.currentTarget.value))}
            disabled={busy}
          >
            {SOLVENT_LIBRARY.map((s, i) => (
              <option key={s.alpbName} value={i}>
                {s.display}
              </option>
            ))}
          </select>
        </label>
        <label className="mono" style={{ fontSize: 12, display: "flex", flexDirection: "column", gap: 2 }}>
          nsolv
          <input
            className="input mono"
            style={{ width: 64 }}
            type="number"
            min={1}
            value={nsolv}
            onChange={(e) => setNsolv(Math.max(1, Number(e.currentTarget.value)))}
            disabled={busy}
          />
        </label>
        <label className="mono" style={{ fontSize: 12, display: "flex", flexDirection: "column", gap: 2 }}>
          charge (from scene)
          <input className="input mono" style={{ width: 64 }} value={sceneCharge} readOnly disabled
            title="The solute charge from the scene (Σ fragment charges) — refined in ORCA at this charge (F2)" />
        </label>
        <button
          className="btn btn-sm btn-primary"
          onClick={runGrow}
          disabled={busy}
          title="Grow an explicit solvent shell around the solute with CREST/QCG (a geometry seed, ~10 s)"
        >
          {busy ? "Growing…" : "Grow microsolvation cluster"}
        </button>
        {busy ? (
          <button className="btn btn-sm" onClick={cancelGrow}>
            Cancel
          </button>
        ) : null}
      </div>

      {busy ? (
        <div className="muted xtb-note xtb-progress">{formatCrestProgress(elapsed)}</div>
      ) : null}

      {error ? (
        <div className="edit-error edit-error-severe xtb-error">
          <div className="xtb-error-msg">{error}</div>
          {errorDir ? (
            <input
              className="input mono xtb-error-dir"
              readOnly
              value={errorDir}
              onFocus={(e) => e.currentTarget.select()}
              title="Diagnostic files — select and copy this path"
            />
          ) : null}
        </div>
      ) : null}

      {done && clusterXyz ? (
        <div className="crest-result" style={{ marginTop: 8, display: "grid", gap: 8 }}>
          <div className="viewer-panel traj-viewer">
            <MoleculeViewer xyzData={clusterXyz} preserveCameraOnUpdate />
          </div>
          <div className="mono" style={{ fontSize: 12 }}>
            Grown cluster: <strong>{done.result.n_atoms}</strong> atoms
            {done.result.seed_energy_eh != null ? (
              <span
                className="muted"
                style={{ marginLeft: 8 }}
                title="xtb-ALPB energy of the grown cluster — a screening seed, NOT a solvated result"
              >
                · xtb-ALPB seed energy (not solvated): {done.result.seed_energy_eh.toFixed(6)} Eh
              </span>
            ) : null}
          </div>

          {/* ALWAYS-on charge-aware seed note — the honesty invariant. */}
          {note ? (
            <div className={note.severity === "warning" ? "banner warn" : "banner"}>
              {note.severity === "warning" ? "⚠ " : ""}
              {note.text}
            </div>
          ) : null}

          {done.growth.length > 0 ? (
            <table className="jobs-table" style={{ fontSize: 12 }}>
              <thead>
                <tr>
                  <th style={{ textAlign: "right" }}>solvent added</th>
                  <th style={{ textAlign: "right" }}>cluster E (Eh)</th>
                  <th style={{ textAlign: "right" }}>ΔEtot</th>
                </tr>
              </thead>
              <tbody>
                {done.growth.map((g) => (
                  <tr key={g.size}>
                    <td className="mono" style={{ textAlign: "right" }}>{g.size}</td>
                    <td className="mono" style={{ textAlign: "right" }}>{g.energy_eh.toFixed(6)}</td>
                    <td className="mono" style={{ textAlign: "right" }}>{g.delta.toFixed(2)}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          ) : null}

          {/* F2 — the persistence point. Disabled here so the flow reads end-to-end. */}
          <div>
            <button
              className="btn btn-sm"
              disabled
              title="F2: re-optimize this cluster in ORCA at the correct charge with SMD (the persisted result)"
            >
              Refine in ORCA (next)
            </button>
          </div>
        </div>
      ) : null}
    </div>
  );
}
