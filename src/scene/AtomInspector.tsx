import type { Scene } from "./types";
import { describeAtom } from "./selection";
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
 * The global index is labelled **0-based** on purpose: whether ORCA's `%geom`
 * Constraints are 0- or 1-based hasn't been settled empirically, so while that's
 * open the UI must state which base it shows — otherwise nobody remembers in two
 * weeks. See wiki/orca/goat.md siblings / the 2.5.2a log entry.
 */
export function AtomInspector({
  scene,
  selection,
  onClear,
}: {
  scene: Scene;
  selection: number[];
  onClear: () => void;
}) {
  if (selection.length === 0) return null;
  const lastGlobal = selection[selection.length - 1];
  const last = describeAtom(scene, lastGlobal);
  if (!last) return null; // stale index — validateSelection normally prevents this

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
      <div className="atom-inspector-coords mono">
        x {last.x.toFixed(4)} · y {last.y.toFixed(4)} · z {last.z.toFixed(4)}
      </div>
      <div className="atom-inspector-index muted">
        local index {last.localIndex} · global index {lastGlobal} (both 0-based)
      </div>
      {selection.length > 1 ? (
        <div className="atom-inspector-list">
          {selection.map((gi) => {
            const d = describeAtom(scene, gi);
            if (!d) return null;
            return (
              <span key={gi} className="atom-chip" title={`${d.fragmentName}`}>
                <span className="fragment-swatch" style={swatch(d.fragmentIndex)} />
                {d.element}
                <span className="muted">#{gi}</span>
              </span>
            );
          })}
        </div>
      ) : null}
    </div>
  );
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
