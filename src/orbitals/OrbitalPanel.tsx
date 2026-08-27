import { useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

import {
  MoleculeViewer,
  type Representation,
  type MoleculeViewerHandle,
} from "../viewer/MoleculeViewer";
import { RepresentationToggle } from "../viewer/RepresentationToggle";
import { saveBytes, exportName } from "../export/save";
import { orbitalRows, defaultOrbital, coreOrbitals, homoIndex } from "./orbitalList";
import { assignPairs, toggleOrbital, MAX_ORBITALS, type AssignedOrbital } from "./orbitalPalette";

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
  // The ORDERED selection (F2) — selection order drives the palette. Starts on the HOMO
  // (single orbital → unchanged from before F2). A number[], toggled via `toggleOrbital`.
  const [selected, setSelected] = useState<number[]>(() => [defaultOrbital(orbitals)]);
  const [isoValue, setIsoValue] = useState(DEFAULT_ISOVALUE);
  const [representation, setRepresentation] = useState<Representation>("stick");
  // The cubes to draw, held in STATE with a STABLE identity — set once per fetch, never
  // rebuilt inline in render (seam 2: the viewer's `cubes` memo + isosurface effect key on
  // this array's reference, so a stray re-render — isovalue drag, representation toggle —
  // must NOT churn a new array, or 2N surfaces would re-parse/redraw every frame).
  const [orbitalCubes, setOrbitalCubes] = useState<
    Array<{ cube: string; posColor: string; negColor: string }>
  >([]);
  // MOs that were selected but whose cube could not be generated (honest-or-absent) —
  // dropped from the drawn set, surfaced here with the reason, never a blank surface.
  const [dropped, setDropped] = useState<Array<{ mo: number; reason: string }>>([]);
  const [state, setState] = useState<"loading" | "ready" | "none" | "error">("loading");
  const [errorMsg, setErrorMsg] = useState("");
  // Flashed when a toggle-add is refused at the cap; cleared on the next successful change.
  const [capHint, setCapHint] = useState(false);

  // The colour pair each selected MO gets, BY SELECTION ORDER — the single source of truth
  // for the picker swatches, the legend, AND the drawn cubes (so all three agree). Pure and
  // cheap (≤ MAX_ORBITALS); used in render for the legend/swatches. The large cube array
  // (above) is what stays in state for identity stability — this small mapping need not.
  const assigned = useMemo(() => assignPairs(selected), [selected]);

  // Fetch each selected MO's cube (generate-then-read, lazily cached in Rust — one call per
  // MO, backend unchanged). `allSettled` so one MO without a cube drops itself (honest-or-
  // absent) instead of failing the whole set. Assembles `orbitalCubes` (fulfilled + non-
  // null only) with each MO's assigned colour pair, set ONCE per fetch (stable identity).
  useEffect(() => {
    let live = true;
    setState("loading");
    setCapHint(false);
    const pairByMo = new Map(assignPairs(selected).map((a) => [a.mo, a]));
    Promise.allSettled(
      selected.map((mo) =>
        invoke<string | null>("read_orbital_cube", {
          id: jobId,
          moIndex: mo,
          grid: GRID_INTERVALS,
        }),
      ),
    ).then((results) => {
      if (!live) return;
      const cubes: Array<{ cube: string; posColor: string; negColor: string }> = [];
      const drops: Array<{ mo: number; reason: string }> = [];
      let anyError = false;
      results.forEach((res, i) => {
        const mo = selected[i];
        const pair = pairByMo.get(mo);
        if (res.status === "fulfilled") {
          if (res.value == null) {
            drops.push({ mo, reason: "no cube (xTB/GOAT gbw has no MOs)" });
          } else if (pair) {
            cubes.push({ cube: res.value, posColor: pair.posColor, negColor: pair.negColor });
          }
        } else {
          anyError = true;
          drops.push({ mo, reason: String(res.reason) });
        }
      });
      setOrbitalCubes(cubes);
      setDropped(drops);
      if (cubes.length > 0) {
        setState("ready");
      } else if (anyError) {
        setErrorMsg(drops.find((d) => d.reason !== "no cube (xTB/GOAT gbw has no MOs)")?.reason ?? "");
        setState("error");
      } else {
        setState("none"); // every selected MO legitimately has no cube
      }
    });
    return () => {
      live = false;
    };
  }, [jobId, selected]);

  // Toggle an MO in/out of the selection. Adding beyond the cap is a no-op (a "max N" hint
  // flashes); `selected` keeps its identity on a refused add so nothing downstream churns.
  function toggle(mo: number) {
    if (!selected.includes(mo) && selected.length >= MAX_ORBITALS) {
      setCapHint(true);
      return;
    }
    setCapHint(false);
    setSelected((prev) => toggleOrbital(prev, mo));
  }

  // "HOMO+LUMO" convenience — select the frontier pair (guarded when there is no LUMO).
  const homo = homoIndex(orbitals);
  const hasLumo = homo != null && homo + 1 < orbitals.length;
  function selectHomoLumo() {
    if (homo == null) return;
    setCapHint(false);
    setSelected(hasLumo ? [homo, homo + 1] : [homo]);
  }

  const droppedSet = useMemo(() => new Set(dropped.map((d) => d.mo)), [dropped]);

  return (
    <section className="orbital-panel">
      <div className="section-title" style={{ fontSize: 12 }}>
        Molecular orbitals
      </div>

      <div className="orbital-layout">
        {/* The picker — multi-select (cap {MAX_ORBITALS}); each active row shows its ± pair. */}
        <OrbitalPicker rows={rows} selected={selected} assigned={assigned} onToggle={toggle} />

        <div className="orbital-view">
          <div className="viewer-panel orbital-viewer">
            {state === "ready" && orbitalCubes.length > 0 ? (
              <MoleculeViewer
                ref={viewerRef}
                orbitalCubes={orbitalCubes}
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

          {/* Legend — one entry per SELECTED MO, with its colour pair + energy. A dropped MO
              (no generatable cube) is shown struck-through with a note (honest-or-absent). */}
          <div className="orbital-mo-legend mono" style={{ fontSize: 12, marginTop: 4 }}>
            {assigned.map((a) => {
              const row = rows.find((r) => r.index === a.mo);
              const isDropped = droppedSet.has(a.mo);
              return (
                <div key={a.mo} className={"orbital-mo" + (isDropped ? " dropped" : "")}>
                  <span className="orbital-swatch-pair" aria-hidden>
                    <span className="orbital-swatch" style={{ background: a.posColor, marginLeft: 0 }} />
                    <span className="orbital-swatch" style={{ background: a.negColor, marginLeft: 2 }} />
                  </span>{" "}
                  MO {a.mo}
                  {row?.kind === "HOMO"
                    ? " (HOMO)"
                    : row?.kind === "LUMO"
                      ? " (LUMO)"
                      : row?.kind === "core"
                        ? " (core)"
                        : ""}
                  {row ? (
                    <>
                      {" "}
                      — {row.energyEh.toFixed(4)} Eh{" "}
                      <span className="muted">({(row.energyEh * EH_TO_EV).toFixed(2)} eV)</span>
                    </>
                  ) : null}
                  {isDropped ? <span className="muted"> — no cube, not shown</span> : null}
                </div>
              );
            })}
          </div>

          <div className="orbital-controls">
            <button
              className="btn btn-sm"
              title="select the frontier pair (HOMO and LUMO) for overlap analysis"
              disabled={homo == null}
              onClick={selectHomoLumo}
            >
              HOMO+LUMO
            </button>
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
            {capHint ? (
              <span className="muted" style={{ fontSize: 11 }}>max {MAX_ORBITALS} orbitals</span>
            ) : null}
            {state === "ready" ? (
              <button
                className="btn btn-sm"
                title="save a PNG snapshot of the 3D scene"
                onClick={() => {
                  try {
                    const bytes = viewerRef.current?.toPngBytes();
                    if (bytes)
                      saveBytes(exportName(jobTitle, `mo${selected.join("-")}`, "png"), bytes).catch(
                        (e) => console.error("[export]", e),
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
            Each orbital's two colours are the wavefunction's two <strong>phases</strong> (sign
            of ψ), NOT charge; simultaneous orbitals each get their own pair (max {MAX_ORBITALS}).
            The isovalue is a viewing choice (grid {GRID_INTERVALS}³, cube from{" "}
            <code>orca_plot</code>, cached on disk). HOMO/LUMO are the frontier orbitals —
            where reactivity lives; overlapping them shows the FMO interaction.
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
  assigned,
  onToggle,
}: {
  rows: ReturnType<typeof orbitalRows>;
  /** The ordered multi-selection (F2). */
  selected: number[];
  /** The colour pair assigned to each selected MO (by selection order) — the active row's
   * swatch reads from here, so the picker, the legend and the drawn surfaces all agree. */
  assigned: AssignedOrbital[];
  onToggle: (index: number) => void;
}) {
  const pairByMo = new Map(assigned.map((a) => [a.mo, a]));
  return (
    <div className="orbital-picker mono">
      {rows.map((r) => {
        const isSel = selected.includes(r.index);
        const pair = pairByMo.get(r.index);
        return (
          <button
            key={r.index}
            type="button"
            role="checkbox"
            aria-checked={isSel}
            className={
              "orbital-row" +
              (isSel ? " selected" : "") +
              (r.kind === "HOMO" || r.kind === "LUMO" ? " frontier" : "") +
              (r.kind === "core" ? " core" : "")
            }
            onClick={() => onToggle(r.index)}
          >
            <span className="orbital-no">{r.index}</span>
            <span className="orbital-tag">
              {r.kind === "HOMO" ? "HOMO" : r.kind === "LUMO" ? "LUMO" : r.kind === "core" ? "core" : ""}
            </span>
            <span className="orbital-e">{r.energyEh.toFixed(3)}</span>
            <span className="orbital-occ muted">{r.occupancy.toFixed(1)}</span>
            <span className="orbital-swatch-pair" aria-hidden>
              {pair ? (
                <>
                  <span className="orbital-swatch" style={{ background: pair.posColor, marginLeft: 0 }} />
                  <span className="orbital-swatch" style={{ background: pair.negColor, marginLeft: 2 }} />
                </>
              ) : null}
            </span>
          </button>
        );
      })}
    </div>
  );
}
