import type { Representation } from "./MoleculeViewer";

/**
 * Ball-and-stick / lines toggle (unit 3.16). Two representations only — lines exist to
 * expose a feature (a core-1s isosurface) that hides inside an atom's drawn sphere. The
 * chosen representation is **app state** in the panel that owns the viewer (ADR-011);
 * this is just the control.
 */
export function RepresentationToggle({
  value,
  onChange,
}: {
  value: Representation;
  onChange: (r: Representation) => void;
}) {
  return (
    <div className="ir-view-toggle" role="group" aria-label="molecule representation">
      <button type="button" className={value === "stick" ? "active" : ""} onClick={() => onChange("stick")}>
        ball &amp; stick
      </button>
      <button type="button" className={value === "line" ? "active" : ""} onClick={() => onChange("line")}>
        lines
      </button>
    </div>
  );
}
