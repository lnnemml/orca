import { useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

import {
  MoleculeViewer,
  type Representation,
  type MoleculeViewerHandle,
} from "../viewer/MoleculeViewer";
import { RepresentationToggle } from "../viewer/RepresentationToggle";
import { saveBytes, exportName } from "../export/save";
import { orbitalRows, defaultOrbital, coreOrbitals } from "./orbitalList";

/** 1 Hartree in eV (CODATA 2018) — chemists read orbital energies in eV. */
const EH_TO_EV = 27.211_386_245_988;

/** Grid intervals for the cube. **Measured** (`wiki/orca/orca-plot.md`): 80³ ≈ 6.9 MB at
 * ~0.2 Å, sub-second — the moderate default domain rule #5 recommends, verified by number.
 * A display/quality choice; kept fixed here (the isovalue is the interactive control). */
const GRID_INTERVALS = 80;

/** Isosurface level — a DISPLAY choice (like FWHM / the mode amplitude), a slider with a
 * named default. Lower = a larger, fuzzier lobe; higher = a tight core. */
const DEFAULT_ISOVALUE = 0.05;
const MIN_ISOVALUE = 0.005;
const MAX_ISOVALUE = 0.1;
const ISOVALUE_STEP = 0.005;

/**
 * Orbital isosurfaces (unit 3.15) — the last Phase-3 visualization. The MO energies +
 * occupancies are already parsed (`orca_2json`); this adds the orbitals as VOLUME. A cube
 * is generated on demand per (orbital, grid) by `orca_plot`, cached in the job dir, and
 * read once (capped) — never stored in the DB (ADR-012 / overview.md).
 *
 * **State ownership (ADR-011):** the selected orbital, the isovalue and visibility are
 * app state here; the viewer is handed the cube text + isovalue and draws. There is no
 * mode animation in this scene — one scene, one mode (a section separate from the IR
 * panel's animator).
 *
 * **Absence is normal:** an xTB/GOAT `.gbw` yields no JSON MOs (measured) → this whole
 * section is not rendered (guarded in `ResultsCard`); and if `orca_plot` produces nothing
 * the viewer simply shows the molecule with a note, never crashes.
 */
export function OrbitalPanel({
  jobId,
  orbitals,
  elements,
  jobTitle,
}: {
  jobId: string;
  /** `[energyEh, occupancy]` per MO, ascending (from `results.orbitals.orbitals`). */
  orbitals: [number, number][];
  /** The molecule's element sequence (`final_geometry.elements`) — used to DERIVE the
   * core-orbital marking (per-element table + energy-gap cross-check). */
  elements: string[];
  /** For export filenames (unit 3.16). */
  jobTitle: string;
}) {
  const viewerRef = useRef<MoleculeViewerHandle | null>(null);
  const rows = useMemo(() => orbitalRows(orbitals, elements), [orbitals, elements]);
  const core = useMemo(() => coreOrbitals(orbitals, elements), [orbitals, elements]);
  const [selected, setSelected] = useState(() => defaultOrbital(orbitals));
  const [isoValue, setIsoValue] = useState(DEFAULT_ISOVALUE);
  const [representation, setRepresentation] = useState<Representation>("stick");
  const [cube, setCube] = useState<string | null>(null);
  const [state, setState] = useState<"loading" | "ready" | "none" | "error">("loading");
  const [errorMsg, setErrorMsg] = useState("");

  // Fetch (generate-then-read, lazily cached in Rust) the cube for the selected MO.
  useEffect(() => {
    let live = true;
    setState("loading");
    setCube(null);
    invoke<string | null>("read_orbital_cube", {
      id: jobId,
      moIndex: selected,
      grid: GRID_INTERVALS,
    })
      .then((text) => {
        if (!live) return;
        if (text == null) {
          setState("none");
        } else {
          setCube(text);
          setState("ready");
        }
      })
      .catch((e) => {
        if (!live) return;
        setErrorMsg(String(e));
        setState("error");
      });
    return () => {
      live = false;
    };
  }, [jobId, selected]);

  const selectedRow = rows.find((r) => r.index === selected);

  return (
    <section className="orbital-panel">
      <div className="section-title" style={{ fontSize: 12 }}>
        Molecular orbitals
      </div>

      <div className="orbital-layout">
        {/* The picker — MO number, energy (Eh + eV), occupancy; HOMO/LUMO marked. */}
        <OrbitalPicker rows={rows} selected={selected} onSelect={setSelected} />

        <div className="orbital-view">
          <div className="viewer-panel orbital-viewer">
            {state === "ready" && cube ? (
              <MoleculeViewer
                ref={viewerRef}
                orbitalCube={cube}
                orbitalIsoValue={isoValue}
                representation={representation}
              />
            ) : (
              <div className="orbital-placeholder muted mono">
                {state === "loading"
                  ? "generating cube…"
                  : state === "none"
                    ? "no orbital cube for this job (xTB/GOAT gbw has no MOs)"
                    : `could not generate cube: ${errorMsg}`}
              </div>
            )}
          </div>

          {selectedRow ? (
            <div className="mono" style={{ fontSize: 12, marginTop: 4 }}>
              MO {selectedRow.index}
              {selectedRow.kind === "HOMO"
                ? " (HOMO)"
                : selectedRow.kind === "LUMO"
                  ? " (LUMO)"
                  : selectedRow.kind === "core"
                    ? " (core)"
                    : ""}{" "}
              — {selectedRow.energyEh.toFixed(4)} Eh{" "}
              <span className="muted">({(selectedRow.energyEh * EH_TO_EV).toFixed(2)} eV)</span>
              , occ {selectedRow.occupancy.toFixed(2)}
              {selectedRow.kind === "core" ? (
                <span className="muted"> — a core 1s (deep, hides inside the atom; try “lines”)</span>
              ) : null}
            </div>
          ) : null}

          <div className="orbital-controls">
            <label className="ir-fwhm">
              isovalue {isoValue.toFixed(3)}
              <input
                type="range"
                min={MIN_ISOVALUE}
                max={MAX_ISOVALUE}
                step={ISOVALUE_STEP}
                value={isoValue}
                onChange={(e) => setIsoValue(Number(e.target.value))}
                aria-label="isosurface level"
              />
            </label>
            <RepresentationToggle value={representation} onChange={setRepresentation} />
            <span className="orbital-legend">
              <span className="orbital-swatch pos" /> +phase
              <span className="orbital-swatch neg" /> −phase
            </span>
            {state === "ready" ? (
              <button
                className="btn btn-sm"
                title="save a PNG snapshot of the 3D scene"
                onClick={() => {
                  try {
                    const bytes = viewerRef.current?.toPngBytes();
                    if (bytes)
                      saveBytes(exportName(jobTitle, `mo${selected}`, "png"), bytes).catch((e) =>
                        console.error("[export]", e),
                      );
                  } catch (e) {
                    console.error("[export]", e);
                  }
                }}
              >
                PNG
              </button>
            ) : null}
          </div>

          <div className="muted" style={{ fontSize: 11, marginTop: 2 }}>
            The two colours are the wavefunction's two <strong>phases</strong> (sign of ψ),
            NOT charge. The isovalue is a viewing choice (grid {GRID_INTERVALS}³, cube from{" "}
            <code>orca_plot</code>, cached on disk). HOMO/LUMO are the frontier orbitals —
            the electrons most easily removed / the first empty level, where reactivity lives.
          </div>

          {/* Core marking is DERIVED (per-element table + energy-gap cross-check), not
              read from the artifact — named as such, with the discrepancy when it disagrees. */}
          <div className="muted" style={{ fontSize: 11, marginTop: 2 }}>
            {core.note}
            {core.count != null && core.count > 0
              ? ` — MOs 0–${core.count - 1} are the deep 1s-type core (empty-looking, occluded).`
              : ""}
          </div>
        </div>
      </div>
    </section>
  );
}

function OrbitalPicker({
  rows,
  selected,
  onSelect,
}: {
  rows: ReturnType<typeof orbitalRows>;
  selected: number;
  onSelect: (index: number) => void;
}) {
  return (
    <div className="orbital-picker mono">
      {rows.map((r) => (
        <button
          key={r.index}
          type="button"
          className={
            "orbital-row" +
            (r.index === selected ? " selected" : "") +
            (r.kind === "HOMO" || r.kind === "LUMO" ? " frontier" : "") +
            (r.kind === "core" ? " core" : "")
          }
          onClick={() => onSelect(r.index)}
        >
          <span className="orbital-no">{r.index}</span>
          <span className="orbital-tag">
            {r.kind === "HOMO" ? "HOMO" : r.kind === "LUMO" ? "LUMO" : r.kind === "core" ? "core" : ""}
          </span>
          <span className="orbital-e">{r.energyEh.toFixed(3)}</span>
          <span className="orbital-occ muted">{r.occupancy.toFixed(1)}</span>
        </button>
      ))}
    </div>
  );
}
