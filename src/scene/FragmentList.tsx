import { useSceneStore } from "./store";
import { atomCount, totalCharge } from "./scene";
import { fragmentColor } from "../viewer/fragment-colors";

/** Charge with an explicit sign: `0`, `-1`, `+1`. */
function signed(n: number): string {
  return n > 0 ? `+${n}` : String(n);
}

/**
 * Fragment sidebar for the New Job screen — one row per scene fragment, with the
 * same palette swatch the viewer uses (`fragmentColor`), an inline-editable name,
 * atom count + signed charge, and a remove button; a totals line underneath.
 * Multiplicity is edited in the input builder, not here.
 */
export function FragmentList() {
  const scene = useSceneStore((s) => s.scene);
  const removeFragment = useSceneStore((s) => s.removeFragment);
  const renameFragment = useSceneStore((s) => s.renameFragment);

  if (!scene || scene.fragments.length === 0) {
    return (
      <div className="fragment-list empty muted">
        Fragments appear here after Import, SMILES, a template, or Add Fragment.
      </div>
    );
  }

  const multi = scene.fragments.length > 1;

  return (
    <div className="fragment-list">
      {scene.fragments.map((f, i) => {
        const color = fragmentColor(i); // undefined for fragment 0 (CPK)
        return (
          <div className="fragment-row" key={f.id}>
            <span
              className="fragment-swatch"
              style={{
                background: color ?? "transparent",
                borderColor: color ?? "var(--muted-2)",
              }}
              title={
                color
                  ? `Fragment colour ${color}`
                  : "CPK element colours (substrate — fragment 0)"
              }
            />
            {color ? null : <span className="fragment-cpk muted">CPK</span>}
            <input
              // Uncommitted edits stay local; commit once on blur / Enter so we
              // don't write to the store (and redraw the viewer) per keystroke.
              // Keyed by name so an external rename resets the field.
              key={`${f.id}:${f.name}`}
              className="input fragment-name"
              defaultValue={f.name}
              spellCheck={false}
              onBlur={(e) => {
                const v = e.currentTarget.value.trim();
                if (v && v !== f.name) renameFragment(f.id, v);
              }}
              onKeyDown={(e) => {
                if (e.key === "Enter") e.currentTarget.blur();
              }}
            />
            <span className="fragment-meta muted">
              {f.atoms.length} atom{f.atoms.length === 1 ? "" : "s"} ·{" "}
              {signed(f.charge)}
            </span>
            <button
              className="btn btn-sm"
              onClick={() => removeFragment(f.id)}
              title="Remove fragment"
            >
              ✕
            </button>
          </div>
        );
      })}

      <div className="fragment-summary muted">
        {scene.fragments.length} fragment{multi ? "s" : ""} · {atomCount(scene)}{" "}
        atoms · total charge {signed(totalCharge(scene))}
      </div>
      {multi ? (
        <div className="fragment-note muted">
          Fragment 0 (substrate) keeps CPK colours; removing it recolours the
          next fragment.
        </div>
      ) : null}
    </div>
  );
}
