import { useState } from "react";

import type { Scene } from "./types";
import type { AtomId } from "./ids";
import { describeAtom, describeAtomById } from "./selection";
import { globalIndexOfAtom } from "./scene";
import { measureSelection, type Measurement } from "./measure";
import { fragmentColor } from "../viewer/fragment-colors";

/**
 * The atom panel on New Job (2.5.2a): names the LAST picked atom, its
 * coordinates, and its global index — and, when more than one atom is picked, a
 * compact click-ordered row of all of them with fragment-colour swatches.
 *
 * Selection state lives in `NewJobScreen`, not the scene store (the store stays
 * a pure geometry wrapper). This is a display of that state: describe-only, no
 * geometry logic. `describeAtom` (pure) does the fragment lookup.
 *
 * The global index is labelled **0-based** on purpose — and that is exactly the
 * base ORCA's `%geom Constraints` use (settled by a real run, 2.5.4a;
 * `wiki/orca/constraints.md`), so the index shown here is the index a constraint
 * line will carry. "Constrain selection" (2.5.4b) writes it straight through.
 *
 * `onConstrain` (2.5.4b): when the selection is 2/3/4 atoms, offer to freeze that
 * coordinate. It calls back into `NewJobScreen`, which owns the input text — the
 * panel never touches the text directly (one data path). Value optional (empty →
 * freeze as-is, the common TS-guess case).
 */
export function AtomInspector({
  scene,
  selection,
  onClear,
  onConstrain,
  constrainDisabledReason,
}: {
  scene: Scene;
  selection: AtomId[];
  onClear: () => void;
  onConstrain?: (value?: number) => void;
  /** When set, "Constrain selection" is disabled with this as the tooltip — used
   * when the constraint block is unrecognised and must not be rewritten (2.5.5). */
  constrainDisabledReason?: string | null;
}) {
  const [constrainValue, setConstrainValue] = useState("");
  if (selection.length === 0) return null;
  const lastId = selection[selection.length - 1];
  const last = describeAtomById(scene, lastId);
  if (!last) return null; // id left the scene — filterSelection normally prevents this
  // The global index the last-picked atom occupies NOW — resolved from its id, so
  // the labelled number is the current 0-based global/ORCA index (never a stale one).
  const lastGlobal = globalIndexOfAtom(scene, lastId);

  // The measurement read off the pick list (2.5.2b), positionally: 2 → distance,
  // 3 → angle (middle pick = vertex), 4 → dihedral. `none` for 1 atom or a
  // degenerate pick — the panel then shows only the atom description.
  const measurement = measureSelection(scene, selection);
  const readout = describeMeasurement(scene, measurement);

  // Fragment 0 keeps CPK colours (no palette entry) — show a hollow swatch, the
  // same convention FragmentList uses.
  const swatch = (fragmentIndex: number) => {
    const color = fragmentColor(fragmentIndex);
    return {
      background: color ?? "transparent",
      borderColor: color ?? "var(--muted-2)",
    };
  };

  return (
    <div className="atom-inspector">
      <div className="atom-inspector-head">
        <span className="fragment-swatch" style={swatch(last.fragmentIndex)} />
        <span className="atom-inspector-title">
          atom {last.localIndex} of {last.fragmentName} ({last.element})
        </span>
        <button
          className="btn btn-sm"
          onClick={onClear}
          style={{ marginLeft: "auto" }}
          title="Clear selection (Esc)"
        >
          Clear
        </button>
      </div>
      {readout ? (
        <div className="atom-inspector-readout">
          <span className="atom-inspector-measure mono">
            {readout.chain}
            {"  "}
            {readout.value}
          </span>
          {readout.interFragment ? (
            <span
              className="atom-inspector-badge"
              title="The two atoms are in different fragments — this distance is a candidate reaction coordinate."
            >
              inter-fragment
            </span>
          ) : null}
        </div>
      ) : null}
      {onConstrain && selection.length >= 2 && selection.length <= 4 ? (
        <div className="atom-inspector-constrain">
          <button
            className="btn btn-sm"
            disabled={!!constrainDisabledReason}
            onClick={() => {
              const v = constrainValue.trim();
              onConstrain(v === "" ? undefined : Number(v));
              setConstrainValue("");
            }}
            title={
              constrainDisabledReason ??
              "Add a %geom constraint on this selection (frozen during the optimization)"
            }
          >
            Constrain {constrainKindLabel(selection.length)}
          </button>
          <input
            className="input atom-inspector-constrain-value"
            type="number"
            step="any"
            placeholder="freeze as-is"
            value={constrainValue}
            onChange={(e) => setConstrainValue(e.target.value)}
            disabled={!!constrainDisabledReason}
            aria-label="constraint target value (optional)"
          />
          {constrainDisabledReason ? (
            <span className="muted atom-inspector-constrain-note">
              {constrainDisabledReason}
            </span>
          ) : null}
        </div>
      ) : null}
      <div className="atom-inspector-coords mono">
        x {last.x.toFixed(4)} · y {last.y.toFixed(4)} · z {last.z.toFixed(4)}
      </div>
      <div className="atom-inspector-index muted">
        local index {last.localIndex} · global index {lastGlobal} (both 0-based)
      </div>
      {selection.length > 1 ? (
        <div className="atom-inspector-list">
          {selection.map((id) => {
            const d = describeAtomById(scene, id);
            if (!d) return null;
            const gi = globalIndexOfAtom(scene, id);
            return (
              <span key={id} className="atom-chip" title={`${d.fragmentName}`}>
                <span className="fragment-swatch" style={swatch(d.fragmentIndex)} />
                {d.element}
                <span className="muted" title="global index (0-based)">
                  {" "}global #{gi}
                </span>
              </span>
            );
          })}
        </div>
      ) : null}
    </div>
  );
}

/** The coordinate a selection of N atoms constrains — same length→kind rule as
 * `measureSelection` / `constraintFromSelection`. */
function constrainKindLabel(len: number): string {
  return len === 2 ? "distance" : len === 3 ? "angle" : "dihedral";
}

/** A rendered measurement line: the atom chain in click order, the value, and
 * whether it crosses fragments. Null when there is no measurement (0/1 atoms or
 * a degenerate pick). Distance uses `···` (a through-space contact), angle and
 * dihedral use `–` (a bonded chain), matching how a chemist writes them. */
function describeMeasurement(
  scene: Scene,
  m: Measurement,
): { chain: string; value: string; interFragment: boolean } | null {
  if (m.kind === "none") return null;
  const symbols = m.atoms.map((gi) => describeAtom(scene, gi)?.element ?? "?");
  const sep = m.kind === "distance" ? "···" : "–";
  const chain = symbols.join(sep);
  if (m.kind === "distance") {
    return {
      chain,
      value: `${m.value.toFixed(3)} ${m.unit}`,
      interFragment: !m.sameFragment,
    };
  }
  const label = m.kind === "dihedral" ? "dihedral " : "";
  return {
    chain,
    value: `${label}${m.value.toFixed(1)}${m.unit}`,
    interFragment: !m.sameFragment,
  };
}
