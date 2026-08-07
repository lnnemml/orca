import { useEffect, useMemo, useState } from "react";

import type { AtomId } from "./ids";
import type { Scene } from "./types";
import { globalIndexOfAtom, rotateFragmentInScene, rotationAxis } from "./scene";
import { describeAtomById } from "./selection";
import { DEFAULT_ROTATE_OVERLAY, type RotateOverlay } from "../viewer/rotate-overlay";

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
  overlay,
  onOverlayChange,
  onEphemeral,
  onAxis,
  onApply,
}: {
  scene: Scene;
  /** The pick list — the tool is active on exactly two atoms [P, Q]. */
  selection: AtomId[];
  /** Which overlay the viewer draws for the axis pair (unit 3.3b) — app-owned in
   * `NewJobScreen`; this panel only toggles it. */
  overlay: RotateOverlay;
  onOverlayChange: (o: RotateOverlay) => void;
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
  // MEMOIZED: `rotationAxis` returns a fresh object; if it went straight into an
  // effect's deps the effect would re-run every render → `setRotateAxis`/
  // `setRotateEphemeral` every render → an infinite update loop ("Maximum update
  // depth exceeded", caught by the unit-3.3b-fix manual gate). Keyed on the stable
  // inputs, it's the SAME object across renders that don't change scene/p/q.
  const axis = useMemo(
    () => (p !== null && q !== null ? rotationAxis(scene, p, q) : null),
    [scene, p, q],
  );

  const deg = Number(angleDeg);
  const validAngle = angleDeg.trim() !== "" && Number.isFinite(deg);
  const angleRad = (deg * Math.PI) / 180;

  // Two SEPARATE effects, so turning the angle does not churn the drawn-axis state.
  // (a) The drawn axis pair — fires ONLY when the pair / active flag changes. Were
  //     this to fire on every angle tick, `rotateAxis`'s identity would change each
  //     tick and `NewJobScreen`'s `[rotateAxis]` reset would snap the overlay toggle
  //     back to the default overlay — making Axis⇄Distance un-switchable.
  useEffect(() => {
    onAxis(active && p !== null && q !== null ? [p, q] : null);
  }, [active, p, q, onAxis]);
  // (b) The ephemeral preview — a PURE recompute over the committed scene shown ONLY
  //     in the viewer (store untouched); null for a zero/invalid angle or degenerate
  //     axis. `axis` is memoized, so this runs on real input changes, never a loop.
  useEffect(() => {
    if (
      !active || p === null || q === null ||
      !axis || !pivotFragmentId || !validAngle || deg === 0
    ) {
      onEphemeral(null);
      return;
    }
    onEphemeral(rotateFragmentInScene(scene, pivotFragmentId, [p, q], angleRad));
  }, [active, p, q, axis, pivotFragmentId, validAngle, deg, angleRad, scene, onEphemeral]);

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
    onOverlayChange(DEFAULT_ROTATE_OVERLAY); // back to the default overlay on cancel (unit 3.3b)
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
      {/* Overlay toggle (unit 3.3b): show EITHER the rotation axis OR the P→Q
          distance for this pair — never both. The Å number is shown either way. */}
      <div
        className="rotate-overlay-toggle"
        role="group"
        aria-label="axis overlay: axis or distance"
      >
        <button
          className={"seg-btn" + (overlay === "axis" ? " seg-on" : "")}
          onClick={() => onOverlayChange("axis")}
          aria-pressed={overlay === "axis"}
        >
          Axis
        </button>
        <button
          className={"seg-btn" + (overlay === "distance" ? " seg-on" : "")}
          onClick={() => onOverlayChange("distance")}
          aria-pressed={overlay === "distance"}
        >
          Distance
        </button>
      </div>
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
