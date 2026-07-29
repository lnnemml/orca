import { useEffect, useState } from "react";

import type { Scene } from "./types";
import { postSidecar } from "../sidecar-client";
import { describeAtom } from "./selection";
import {
  applyResponseIssue,
  applyResponseToScene,
  type EditPlan,
} from "./edit-plan";
import { mergeToXyz } from "./scene";

/**
 * Edit mode UI (2.5.2d) — lives in the Atom section of the geometry rail. Given a
 * `plan` (from `planEdit`), it shows the op, the current value, a target field,
 * and Preview / Apply. When the plan is `unavailable` it shows the reason as calm
 * text with no buttons.
 *
 * **The mask is visible before Apply** (the whole point of this unit): the moving
 * fragment glows in the viewer whenever the plan is ready — `NewJobScreen` passes
 * `plan.mask` to `MoleculeViewer`. This panel names the moving fragment too.
 *
 * **Preview touches only the viewer** (the 2.5.1 decision): it POSTs to the
 * sidecar and hands the resulting Scene up as a *preview scene* that the viewer
 * renders — the store Scene and the Monaco buffer are untouched until Apply, so a
 * keystroke in the target field never runs the Scene↔Monaco sync + collapse rule.
 */

interface SidecarResponse {
  xyz: string;
  measured: number;
  max_static_displacement: number;
}

/** POST the op through the shared sidecar client (human error messages, incl.
 * "older build, restart" on a 404 — no Rust proxy; SMILES/convert go the same
 * way). */
function callSidecar(
  scene: Scene,
  plan: Extract<EditPlan, { kind: "ready" }>,
  value: number,
): Promise<SidecarResponse> {
  return postSidecar<SidecarResponse>("/geometry/set-internal", {
    xyz: mergeToXyz(scene),
    op: plan.op,
    indices: plan.indices,
    value,
    mask: plan.mask,
  });
}

export function EditPanel({
  scene,
  plan,
  movingFragmentName,
  alternativeFragmentName,
  onSwitchOrientation,
  onPreview,
  onApplied,
}: {
  scene: Scene;
  plan: EditPlan;
  /** Display name of the moving fragment (looked up by the parent). */
  movingFragmentName: string | null;
  /** Display name of the OTHER movable fragment, if any (the alternative). */
  alternativeFragmentName: string | null;
  /** Flip to the alternative orientation (the "Move X instead" action). */
  onSwitchOrientation: () => void;
  /** Hand a preview scene to the viewer, or `null` to clear the preview. */
  onPreview: (previewScene: Scene | null) => void;
  /** Commit: the new scene + the scene before the edit (for one-step Undo). */
  onApplied: (newScene: Scene, previousScene: Scene) => void;
}) {
  const ready = plan.kind === "ready" ? plan : null;
  const [target, setTarget] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [previewing, setPreviewing] = useState(false);

  // Reset the field + preview whenever the coordinate being edited changes
  // (a different op or a different atom chain). Keyed on op+indices so a
  // coordinate-only re-render doesn't stomp the user's typing.
  const editKey = ready ? `${ready.op}:${ready.indices.join(",")}` : "none";
  useEffect(() => {
    setError(null);
    setPreviewing(false);
    onPreview(null);
    setTarget(ready ? String(round(ready.current)) : "");
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [editKey]);

  if (!ready) {
    return plan.kind === "unavailable" ? (
      <div className="edit-panel edit-unavailable muted">{plan.reason}</div>
    ) : null;
  }

  const value = Number(target);
  const valid = target.trim() !== "" && Number.isFinite(value);

  const runPreview = async () => {
    if (!valid) return;
    setBusy(true);
    setError(null);
    try {
      const resp = await callSidecar(scene, ready, value);
      onPreview(applyResponseToScene(scene, ready.movingFragmentId, resp.xyz));
      setPreviewing(true);
    } catch (e) {
      setPreviewing(false);
      onPreview(null);
      setError(messageFor(e));
    } finally {
      setBusy(false);
    }
  };

  const runApply = async () => {
    if (!valid) return;
    setBusy(true);
    setError(null);
    try {
      const resp = await callSidecar(scene, ready, value);
      // Front-of-the-boundary check (pure, shared, tested) — the server already
      // checked, this guards our side before we mutate the scene.
      const issue = applyResponseIssue(
        scene,
        resp.xyz,
        resp.max_static_displacement,
      );
      if (issue) throw new Error(issue);
      const newScene = applyResponseToScene(
        scene,
        ready.movingFragmentId,
        resp.xyz,
      );
      setPreviewing(false);
      onPreview(null);
      onApplied(newScene, scene);
    } catch (e) {
      setError(messageFor(e));
    } finally {
      setBusy(false);
    }
  };

  const cancelPreview = () => {
    setPreviewing(false);
    onPreview(null);
  };

  const pivot = pivotLabel(scene, ready);

  return (
    <div className="edit-panel">
      <div className="edit-head">
        Set {ready.op} · moving <strong>{movingFragmentName ?? "fragment"}</strong>
        {pivot ? <span className="muted"> · {pivot}</span> : null}
      </div>
      {ready.reversed ? (
        <div className="edit-reversed muted">
          chain read in reverse so the reagent moves
        </div>
      ) : null}
      <div className="edit-current muted">
        current {round(ready.current)} {ready.unit}
      </div>
      {ready.alternative ? (
        <button
          className="btn btn-sm edit-switch"
          onClick={onSwitchOrientation}
          disabled={busy}
        >
          Move {alternativeFragmentName ?? "the other fragment"} instead
        </button>
      ) : null}
      <div className="edit-controls">
        <input
          className="input edit-target"
          type="number"
          step="any"
          value={target}
          onChange={(e) => setTarget(e.target.value)}
          disabled={busy}
          aria-label={`target ${ready.op} in ${ready.unit}`}
        />
        <span className="edit-unit">{ready.unit}</span>
        <button
          className="btn btn-sm"
          onClick={runPreview}
          disabled={!valid || busy}
        >
          Preview
        </button>
        <button
          className="btn btn-sm btn-primary"
          onClick={runApply}
          disabled={!valid || busy}
        >
          Apply
        </button>
        {previewing ? (
          <button className="btn btn-sm" onClick={cancelPreview} disabled={busy}>
            Cancel preview
          </button>
        ) : null}
      </div>
      {error ? (
        <div
          className={
            "edit-error" +
            (isPostConditionError(error) ? " edit-error-severe" : "")
          }
        >
          {error}
        </div>
      ) : null}
    </div>
  );
}

function round(v: number): number {
  return Math.round(v * 1000) / 1000;
}

/** The atom(s) an edit rotates about — the immovable pivot. For an angle it's the
 * vertex (`indices[1]`); for a dihedral it's the j–k axis (`indices[1..2]`). This
 * is what tells the user what stays put; without it the panel couldn't say what
 * the rotation is around. Null for a distance. */
function pivotLabel(
  scene: Scene,
  plan: Extract<EditPlan, { kind: "ready" }>,
): string | null {
  if (plan.op === "angle") {
    const v = plan.indices[1];
    const d = describeAtom(scene, v);
    return d ? `vertex ${d.element} #${v}` : null;
  }
  if (plan.op === "dihedral") {
    const [, j, k] = plan.indices;
    const dj = describeAtom(scene, j);
    const dk = describeAtom(scene, k);
    return dj && dk ? `axis ${dj.element}#${j}–${dk.element}#${k}` : null;
  }
  return null; // distance has no pivot
}

function messageFor(e: unknown): string {
  if (e instanceof Error) return e.message;
  return String(e);
}

/** A 500 from the endpoint means a real post-condition breach (the count/order/
 * measured invariant failed) — surface it prominently, it's not a user mistake. */
function isPostConditionError(msg: string): boolean {
  return /not reached|changed|count/i.test(msg);
}
