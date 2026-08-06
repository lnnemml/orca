import { useEffect, useState } from "react";

import type { AtomId } from "./ids";
import type { Scene } from "./types";
import { globalIndexOfAtom, rotateFragmentInScene, rotationAxis } from "./scene";
import { describeAtomById } from "./selection";

/**
 * "Rotate about axis" — the rigid whole-fragment rotation tool (Phase 4.2 Stage 3,
 * unit 3.3). A **sibling of `EditPanel`** (pick + value + preview + apply) living in
 * the same Edit geometry section, but pure TS (no sidecar): the rigid transform is
 * `rotateFragmentInScene`, the internal-coordinate `set-internal` edits stay in the
 * sidecar (a different operation — 2.5.3 torsion).
 *
 * The **two picked atoms are the axis**: P (first pick) is the pivot on the fragment
 * that turns; Q (second pick) gives the direction (typically the substrate contact
 * atom, ADR-007). The fragment that rotates is P's fragment. The angle is **numeric**
 * (reproducible; spin-drag is deferred). Turning the angle shows an **ephemeral,
 * viewer-only preview** (frozen-topology coordinate-update, `onEphemeral`); the Scene
 * is untouched until **Apply**, which commits exactly ONE `rotate-fragment` op
 * (ADR-010). Cancel drops the preview with zero ops. A degenerate axis (P ≡ Q)
 * disables Apply with a reason.
 */
export function RotatePanel({
  scene,
  selection,
  onEphemeral,
  onAxis,
  onApply,
}: {
  scene: Scene;
  /** The pick list — the tool is active on exactly two atoms [P, Q]. */
  selection: AtomId[];
  /** Push the ephemeral preview scene up to the viewer (null = no preview). */
  onEphemeral: (previewScene: Scene | null) => void;
  /** Tell the viewer which two atoms form the drawn axis (null = none). */
  onAxis: (axis: [AtomId, AtomId] | null) => void;
  /** Commit: rotate `fragmentId` about the axis atoms by `angleRad` (one op). */
  onApply: (fragmentId: string, axisAtoms: [AtomId, AtomId], angleRad: number) => void;
}) {
  const [angleDeg, setAngleDeg] = useState("0");
  // First pick is the pivot by default; the swap flips which of the two atoms is P.
  const [swapped, setSwapped] = useState(false);

  const active = selection.length === 2;
  const selectionKey = selection.join(",");

  // A fresh pick resets the angle and the swap — a new axis, a new edit.
  useEffect(() => {
    setAngleDeg("0");
    setSwapped(false);
  }, [selectionKey]);

  const p = active ? selection[swapped ? 1 : 0] : null;
  const q = active ? selection[swapped ? 0 : 1] : null;
  const pivotFragmentId =
    p !== null ? describeAtomById(scene, p)?.fragmentId ?? null : null;
  const axis = p !== null && q !== null ? rotationAxis(scene, p, q) : null;

  const deg = Number(angleDeg);
  const validAngle = angleDeg.trim() !== "" && Number.isFinite(deg);
  const angleRad = (deg * Math.PI) / 180;

  // Push the ephemeral preview + the drawn axis whenever the inputs change. The
  // preview is a PURE recompute over the committed scene shown ONLY in the viewer
  // (the store is untouched); a zero/invalid angle or a degenerate axis shows none.
  useEffect(() => {
    if (!active || p === null || q === null) {
      onAxis(null);
      onEphemeral(null);
      return;
    }
    onAxis([p, q]);
    if (!axis || !pivotFragmentId || !validAngle || deg === 0) {
      onEphemeral(null);
      return;
    }
    onEphemeral(rotateFragmentInScene(scene, pivotFragmentId, [p, q], angleRad));
  }, [
    active, p, q, deg, validAngle, axis, pivotFragmentId, angleRad, scene,
    onAxis, onEphemeral,
  ]);

  // Clear the viewer overlay if the tool unmounts (dock section closed).
  useEffect(() => () => {
    onAxis(null);
    onEphemeral(null);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  if (!active) {
    return (
      <div className="rotate-panel muted" style={{ fontSize: 13 }}>
        Rotate about axis — pick <strong>two</strong> atoms: P on the fragment to turn,
        then Q for the axis direction (typically the substrate contact atom).
      </div>
    );
  }

  const label = (id: AtomId): string => {
    const d = describeAtomById(scene, id);
    const gi = globalIndexOfAtom(scene, id);
    return d && gi !== null ? `${d.element}#${gi}` : `#${id}`;
  };
  const pivotName = pivotFragmentId
    ? scene.fragments.find((f) => f.id === pivotFragmentId)?.name ?? "fragment"
    : "fragment";

  const degenerate = axis === null;
  const canApply = !degenerate && pivotFragmentId !== null && validAngle && deg !== 0;

  const runApply = () => {
    if (!canApply || p === null || q === null || !pivotFragmentId) return;
    onApply(pivotFragmentId, [p, q], angleRad);
    setAngleDeg("0"); // committed → drop back to the neutral preview (no double-apply)
    onEphemeral(null);
  };
  const cancel = () => {
    setAngleDeg("0");
    onEphemeral(null);
  };

  return (
    <div className="rotate-panel">
      <div className="edit-head">
        Rotate <strong>{pivotName}</strong> about axis{" "}
        <span className="muted">
          {p !== null ? label(p) : "?"} → {q !== null ? label(q) : "?"}
        </span>
      </div>
      <div className="rotate-axis-note muted">
        pivot {p !== null ? label(p) : "?"} · direction {q !== null ? label(q) : "?"}
      </div>
      <button className="btn btn-sm edit-switch" onClick={() => setSwapped((v) => !v)}>
        Swap pivot ⇄ direction
      </button>
      {degenerate ? (
        <div className="edit-error">
          The two picked atoms coincide — no axis direction is defined. Pick two
          distinct atoms.
        </div>
      ) : null}
      <div className="rotate-controls">
        <input
          className="rotate-slider"
          type="range"
          min={-180}
          max={180}
          step={1}
          value={validAngle ? Math.max(-180, Math.min(180, deg)) : 0}
          onChange={(e) => setAngleDeg(e.currentTarget.value)}
          disabled={degenerate}
          aria-label="rotation angle (degrees)"
        />
        <input
          className="input rotate-angle"
          type="number"
          step="any"
          value={angleDeg}
          onChange={(e) => setAngleDeg(e.target.value)}
          disabled={degenerate}
          aria-label="rotation angle in degrees"
        />
        <span className="edit-unit">°</span>
        <button
          className="btn btn-sm btn-primary"
          onClick={runApply}
          disabled={!canApply}
          title={degenerate ? "Axis endpoints coincide" : undefined}
        >
          Apply
        </button>
        <button className="btn btn-sm" onClick={cancel} disabled={deg === 0}>
          Cancel
        </button>
      </div>
    </div>
  );
}
