import type { Scene } from "./types";
import { describeAtom } from "./selection";
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
      <div className="atom-inspector-coords mono">
        x {last.x.toFixed(4)} · y {last.y.toFixed(4)} · z {last.z.toFixed(4)}
      </div>
      <div className="atom-inspector-index muted">
        global index {lastGlobal} (0-based)
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
