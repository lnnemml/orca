import { useEffect, useMemo, useState } from "react";

import type { Scene } from "./types";
import type { AtomId } from "./ids";
import type { Op } from "./oplog";
import { globalIndexOfAtom } from "./scene";
import { describeAtomById } from "./selection";
import { callSetInternal } from "./EditPanel";
import {
  planGuidedPlacement,
  runGuidedPlacement,
  type GuidedStep,
  type GuidedTargets,
} from "./guided-placement";

/**
 * Guided fragment placement (Phase 4.2 tail-1) — add a reagent at a target
 * distance / angle / dihedral in ONE flow. A sibling of `EditPanel` / `RotatePanel`
 * living in the **Fragments** section: the reagent has already been added roughly
 * (`placeFragment` + an `add-fragment` op); this panel drives it to the approach
 * geometry by REUSING the set-internal edit path (`planGuidedPlacement`), one
 * `replace-fragment-atoms` op per given coordinate (see `guided-placement.ts`).
 *
 * The pick list (the shared `selection`) is split by fragment membership: the single
 * atom on the reagent fragment is the mover R; the others are substrate anchors
 * A, B, C (click order). d needs R + 1 anchor, θ needs 2, φ needs 3 — a field is
 * disabled with a reason until enough anchors are picked (never a silent 0).
 *
 * **Preview is view-only** (mirrors `EditPanel`): it POSTs the whole sequence and
 * hands the final Scene up as a preview; the store Scene and Monaco are untouched
 * until **Apply**, which commits the op sequence (via `onApplied`). Cancel abandons
 * the guided flow — the roughly-placed reagent stays (its add-fragment op is
 * already committed).
 */
export function GuidedPlacementPanel({
  scene,
  reagentFragmentId,
  reagentName,
  selection,
  onPreview,
  onApplied,
  onCancel,
}: {
  scene: Scene;
  reagentFragmentId: string;
  reagentName: string;
  selection: AtomId[];
  onPreview: (previewScene: Scene | null) => void;
  /** Commit the op sequence (add-fragment is already committed). */
  onApplied: (ops: { op: Op; scene: Scene }[]) => void;
  onCancel: () => void;
}) {
  const [distance, setDistance] = useState("");
  const [angle, setAngle] = useState("");
  const [dihedral, setDihedral] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [previewing, setPreviewing] = useState(false);

  // Split the shared pick list by fragment: exactly one atom on the reagent is the
  // mover R; the rest (click order) are substrate anchors A, B, C.
  const { reagentAtom, substrateRefs, reagentPickCount } = useMemo(() => {
    const onReagent: AtomId[] = [];
    const onSubstrate: AtomId[] = [];
    for (const id of selection) {
      const frag = describeAtomById(scene, id)?.fragmentId;
      if (frag === reagentFragmentId) onReagent.push(id);
      else onSubstrate.push(id);
    }
    return {
      reagentAtom: onReagent.length === 1 ? onReagent[0] : null,
      substrateRefs: onSubstrate,
      reagentPickCount: onReagent.length,
    };
  }, [selection, scene, reagentFragmentId]);

  const nRefs = substrateRefs.length;
  const angleEnabled = nRefs >= 2;
  const dihedralEnabled = nRefs >= 3;

  // A new pick set drops a stale preview + error (the parent also nulls previewScene
  // on a selection change; this keeps the panel's own flags in step).
  const selectionKey = selection.join(",");
  useEffect(() => {
    setPreviewing(false);
    setError(null);
    onPreview(null);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [selectionKey]);

  const d = Number(distance);
  const validDistance = distance.trim() !== "" && Number.isFinite(d) && d > 0;
  const targets: GuidedTargets = {
    distance: d,
    angle: angleEnabled && angle.trim() !== "" && Number.isFinite(Number(angle)) ? Number(angle) : null,
    dihedral:
      dihedralEnabled && dihedral.trim() !== "" && Number.isFinite(Number(dihedral))
        ? Number(dihedral)
        : null,
  };

  const plan =
    reagentAtom !== null && nRefs >= 1 && validDistance
      ? planGuidedPlacement(scene, reagentFragmentId, reagentAtom, substrateRefs, targets)
      : null;
  const ready = plan?.kind === "ready" ? plan : null;

  const realSetInternal = (s: Scene, step: GuidedStep) =>
    callSetInternal(s, { op: step.op, indices: step.indices, mask: step.mask }, step.value);

  const runPreview = async () => {
    if (!ready) return;
    setBusy(true);
    setError(null);
    try {
      const result = await runGuidedPlacement(scene, ready, reagentName, realSetInternal, {
        checkPostCondition: false,
      });
      onPreview(result.scene);
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
    if (!ready) return;
    setBusy(true);
    setError(null);
    try {
      const result = await runGuidedPlacement(scene, ready, reagentName, realSetInternal, {
        checkPostCondition: true,
      });
      setPreviewing(false);
      onPreview(null);
      onApplied(result.ops); // parent commits each op in order (d, then θ, then φ)
    } catch (e) {
      setError(messageFor(e));
    } finally {
      setBusy(false);
    }
  };

  const label = (id: AtomId): string => {
    const desc = describeAtomById(scene, id);
    const gi = globalIndexOfAtom(scene, id);
    return desc && gi !== null ? `${desc.element}#${gi}` : `#${id}`;
  };

  const anchorList =
    nRefs > 0 ? substrateRefs.map(label).join(", ") : "— pick a substrate atom —";

  return (
    <div className="guided-panel">
      <div className="edit-head">
        Place <strong>{reagentName}</strong> by approach geometry
      </div>
      <div className="guided-picks muted">
        reagent atom:{" "}
        {reagentAtom !== null ? (
          <strong>{label(reagentAtom)}</strong>
        ) : reagentPickCount > 1 ? (
          <span className="guided-warn">pick exactly one atom on {reagentName}</span>
        ) : (
          <span>pick one atom on {reagentName}</span>
        )}
        <br />
        substrate anchors: <span className="mono">{anchorList}</span>
      </div>

      <div className="guided-fields">
        <div className="guided-field">
          <label>d — approach distance (R→anchor 1)</label>
          <div className="row">
            <input
              className="input guided-target"
              type="number"
              step="any"
              min="0"
              value={distance}
              onChange={(e) => setDistance(e.target.value)}
              disabled={busy}
              aria-label="approach distance in Ångström"
              placeholder="required"
            />
            <span className="edit-unit">Å</span>
          </div>
        </div>

        <div className="guided-field">
          <label>θ — approach angle (R–anchor 1–anchor 2)</label>
          <div className="row">
            <input
              className="input guided-target"
              type="number"
              step="any"
              value={angle}
              onChange={(e) => setAngle(e.target.value)}
              disabled={busy || !angleEnabled}
              aria-label="approach angle in degrees"
              placeholder={angleEnabled ? "optional" : "pick 2 substrate atoms"}
              title={angleEnabled ? undefined : "An angle needs two substrate anchors."}
            />
            <span className="edit-unit">°</span>
          </div>
          {!angleEnabled ? (
            <div className="guided-hint muted">needs two substrate atoms</div>
          ) : null}
        </div>

        <div className="guided-field">
          <label>φ — approach dihedral (R–anchor 1–anchor 2–anchor 3)</label>
          <div className="row">
            <input
              className="input guided-target"
              type="number"
              step="any"
              value={dihedral}
              onChange={(e) => setDihedral(e.target.value)}
              disabled={busy || !dihedralEnabled}
              aria-label="approach dihedral in degrees"
              placeholder={dihedralEnabled ? "optional" : "pick 3 substrate atoms"}
              title={dihedralEnabled ? undefined : "A dihedral needs three substrate anchors."}
            />
            <span className="edit-unit">°</span>
          </div>
          {!dihedralEnabled ? (
            <div className="guided-hint muted">needs three substrate atoms</div>
          ) : null}
        </div>
      </div>

      {plan?.kind === "unavailable" ? (
        <div className="edit-unavailable muted">{plan.reason}</div>
      ) : null}

      <div className="guided-controls">
        <button className="btn btn-sm" onClick={runPreview} disabled={!ready || busy}>
          Preview
        </button>
        <button className="btn btn-sm btn-primary" onClick={runApply} disabled={!ready || busy}>
          Apply
        </button>
        {previewing ? (
          <button
            className="btn btn-sm"
            onClick={() => {
              setPreviewing(false);
              onPreview(null);
            }}
            disabled={busy}
          >
            Cancel preview
          </button>
        ) : null}
        <button className="btn btn-sm" onClick={onCancel} disabled={busy}>
          Done
        </button>
      </div>

      {error ? <div className="edit-error edit-error-severe">{error}</div> : null}
    </div>
  );
}

function messageFor(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}
