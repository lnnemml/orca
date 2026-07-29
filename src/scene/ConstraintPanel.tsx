import type { Scene } from "./types";
import { describeAtom } from "./selection";
import { measureSelection, formatMeasurementValue } from "./measure";
import { atomCount } from "./scene";
import {
  parseConstraintsBlock,
  injectConstraints,
  constraintIndexIssues,
  type Constraint,
} from "./constraints";

/**
 * The Constraints panel (2.5.4b) — a **view over the input text**, never a
 * parallel store. Its only source is `parseConstraintsBlock(content)`: whatever
 * the text says, the panel shows, including a block edited by hand in Monaco. Add
 * happens elsewhere ("Constrain selection" in the Atom section); this panel
 * lists, reports, and deletes.
 *
 * Two guards live here (without them the panel makes the app *less* safe — see
 * `wiki/orca/constraints.md`, ORCA does not range-check and segfaults):
 *  - **out-of-range rows are flagged explicitly** (not silently) — the Run path
 *    (`NewJobScreen`) blocks on the same `constraintIndexIssues`;
 *  - a **composition-change warning** (driven by `compositionSignature` in
 *    `NewJobScreen`) reminds the user that removing a fragment does NOT rewrite
 *    the `%geom` indices, and lists which atom each constraint names *now*.
 *
 * We never rewrite the text or drop a constraint automatically: the text belongs
 * to the user, and "the same atom after a fragment was removed" has no
 * operational definition (the same call we made for selection in 2.5.2a).
 */
export function ConstraintPanel({
  scene,
  content,
  onChange,
  compositionChanged,
  onDismissComposition,
}: {
  scene: Scene;
  content: string;
  onChange: (newContent: string) => void;
  /** Scene composition changed while constraints exist (signature moved). */
  compositionChanged: boolean;
  onDismissComposition: () => void;
}) {
  const constraints = parseConstraintsBlock(content) ?? [];
  if (constraints.length === 0) return null; // nothing in the text → no section

  const n = atomCount(scene);
  const issues = constraintIndexIssues(constraints, n);
  const badFor = (c: Constraint): number[] =>
    issues.find((it) => it.constraint === c)?.badIndices ?? [];

  const remove = (idx: number) => {
    const next = constraints.filter((_, i) => i !== idx);
    onChange(injectConstraints(content, next));
  };

  return (
    <div className="constraint-panel">
      <div className="constraint-head">
        Constraints <span className="muted">({constraints.length})</span>
      </div>

      {compositionChanged ? (
        <div className="banner warn constraint-warn">
          <div>
            The scene composition changed, but the <code>%geom</code> constraint
            indices were <strong>not</strong> rewritten — they still point at
            whatever atom now sits at each index. Check every constraint still
            names the atom you meant:
          </div>
          <ul className="constraint-warn-list">
            {constraints.map((c, i) => (
              <li key={i} className="mono">
                {TYPE_LETTER[c.kind]} {atomsLabel(scene, c)}
              </li>
            ))}
          </ul>
          <button className="btn btn-sm" onClick={onDismissComposition}>
            Dismiss
          </button>
        </div>
      ) : null}

      <div className="constraint-list">
        {constraints.map((c, i) => {
          const bad = badFor(c);
          const current = formatMeasurementValue(measureSelection(scene, c.atoms));
          return (
            <div
              key={i}
              className={"constraint-row" + (bad.length ? " constraint-row-bad" : "")}
            >
              <span className="constraint-badge" title={KIND_TITLE[c.kind]}>
                {TYPE_LETTER[c.kind]}
              </span>
              <span className="constraint-atoms mono">{atomsLabel(scene, c)}</span>
              <span className="constraint-values mono muted">
                {c.kind !== "cartesian" && c.value !== undefined
                  ? `set ${c.value}${unit(c)}`
                  : "frozen"}
                {current ? ` · now ${current}` : ""}
              </span>
              {bad.length ? (
                <span className="constraint-bad-note" title="ORCA segfaults on an out-of-range index">
                  out of range: {bad.map((i2) => `#${i2}`).join(", ")} (input has {n}
                  {" "}atoms, 0–{n - 1})
                </span>
              ) : null}
              <button
                className="btn btn-sm constraint-del"
                onClick={() => remove(i)}
                title="Remove this constraint"
              >
                ×
              </button>
            </div>
          );
        })}
      </div>
    </div>
  );
}

const TYPE_LETTER: Record<Constraint["kind"], string> = {
  distance: "B",
  angle: "A",
  dihedral: "D",
  cartesian: "C",
};
const KIND_TITLE: Record<Constraint["kind"], string> = {
  distance: "bond / distance",
  angle: "angle",
  dihedral: "dihedral",
  cartesian: "Cartesian (frozen position)",
};

function unit(c: Constraint): string {
  return c.kind === "distance" ? " Å" : c.kind === "cartesian" ? "" : "°";
}

/** Atoms in OrcaStudio's own terms via `locateAtom`/`describeAtom`:
 * "C#12 (Ibuprofen) ··· B#33 (BH₄⁻)". An out-of-range index reads
 * "#37 (out of range)" so a stale constraint is unmistakable. */
function atomsLabel(scene: Scene, c: Constraint): string {
  const sep = c.kind === "distance" ? " ··· " : c.kind === "cartesian" ? "" : " – ";
  return c.atoms
    .map((gi) => {
      const d = describeAtom(scene, gi);
      return d ? `${d.element}#${gi} (${d.fragmentName})` : `#${gi} (out of range)`;
    })
    .join(sep);
}
