import { useEffect, useState } from "react";

import type { Scene } from "./types";
import type { Op } from "./oplog";
import { makeAtomId } from "./ids";
import { postSidecar } from "../sidecar-client";
import { describeAtom, type AtomDescription } from "./selection";
import { locateAtom } from "./scene";
import {
  applyResponseIssue,
  applyResponseToScene,
  type EditPlan,
} from "./edit-plan";
import { mergeToXyz } from "./scene";

/**
 * Edit mode UI (2.5.2d; intra-fragment 2.5.3b) — lives in the Atom section of the
 * geometry rail. Given a `plan` (from `planEdit`), shows the op, current value, a
 * target field, Preview / Apply.
 *
 * Three plan kinds:
 * - `ready` — inter-fragment; the mask is the whole moving fragment (`plan.mask`);
 * - `needs-split` — intra-fragment torsion; the mask is a **bond-graph split** the
 *   sidecar computes. `NewJobScreen` resolves it (`/geometry/rotatable-mask`) and
 *   passes `splitMask` (or `splitError`/`splitResolving`); the SAME mask drives
 *   the viewer glow and the `set-internal` call — one source, not two;
 * - `unavailable` — the reason as calm text, no buttons.
 *
 * **Preview touches only the viewer** (2.5.1): it POSTs and hands the resulting
 * Scene up as a preview scene; the store Scene and Monaco are untouched until Apply.
 */

export interface SidecarResponse {
  xyz: string;
  measured: number;
  max_static_displacement: number;
}

/** The concrete edit to run — the mask resolved (from a `ready` fragment, or from
 * the sidecar's bond-graph split). `null` while a split is resolving/errored. */
interface ActiveEdit {
  op: "distance" | "angle" | "dihedral";
  indices: number[];
  mask: number[];
  current: number;
  unit: "Å" | "°";
  movingFragmentId: string;
}

function resolveActive(
  scene: Scene,
  plan: EditPlan,
  splitMask: number[] | null,
): ActiveEdit | null {
  if (plan.kind === "ready") {
    return {
      op: plan.op,
      indices: plan.indices,
      mask: plan.mask,
      current: plan.current,
      unit: plan.unit,
      movingFragmentId: plan.movingFragmentId,
    };
  }
  if (plan.kind === "needs-split" && splitMask) {
    // The mask is a subset of ONE fragment; the moved atoms replace that
    // fragment's rows in place (the unmoved rest come back unchanged).
    const frag = locateAtom(scene, plan.moving)?.fragment.id;
    if (!frag) return null;
    return {
      op: plan.op,
      indices: plan.indices,
      mask: splitMask,
      current: plan.current,
      unit: plan.unit,
      movingFragmentId: frag,
    };
  }
  return null;
}

/** The one set-internal client call. Takes only the three fields the endpoint
 * needs (`op`/`indices`/`mask`), so `ActiveEdit` satisfies it structurally AND the
 * guided-placement panel reuses it without a second copy of the fetch. */
export function callSetInternal(
  scene: Scene,
  active: { op: "distance" | "angle" | "dihedral"; indices: number[]; mask: number[] },
  value: number,
): Promise<SidecarResponse> {
  return postSidecar<SidecarResponse>("/geometry/set-internal", {
    xyz: mergeToXyz(scene),
    op: active.op,
    indices: active.indices,
    value,
    mask: active.mask,
  });
}

export function EditPanel({
  scene,
  plan,
  movingFragmentName,
  alternativeFragmentName,
  splitMask,
  splitError,
  splitResolving,
  onSwitchOrientation,
  onPreview,
  onApplied,
}: {
  scene: Scene;
  plan: EditPlan;
  movingFragmentName: string | null;
  alternativeFragmentName: string | null;
  /** The bond-graph mask for a `needs-split` plan, resolved by `NewJobScreen`. */
  splitMask: number[] | null;
  splitError: string | null;
  splitResolving: boolean;
  onSwitchOrientation: () => void;
  onPreview: (previewScene: Scene | null) => void;
  /** Hand the store the typed op (provenance) + its resultant snapshot. */
  onApplied: (op: Op, newScene: Scene) => void;
}) {
  const active = resolveActive(scene, plan, splitMask);
  const [target, setTarget] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [previewing, setPreviewing] = useState(false);

  // Reset field + preview when the coordinate being edited changes (op + atoms +
  // whether the split has resolved). Keyed so a coordinate-only re-render doesn't
  // stomp the user's typing.
  const editKey =
    plan.kind === "unavailable"
      ? "none"
      : `${plan.kind}:${plan.op}:${plan.indices.join(",")}:${active ? "R" : "-"}`;
  useEffect(() => {
    setError(null);
    setPreviewing(false);
    onPreview(null);
    setTarget(active ? String(round(active.current)) : "");
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [editKey]);

  if (plan.kind === "unavailable") {
    return <div className="edit-panel edit-unavailable muted">{plan.reason}</div>;
  }

  const intra = plan.kind === "needs-split";

  // Intra-fragment, mask not yet resolved → header + status, no controls.
  if (intra && !active) {
    return (
      <div className="edit-panel">
        <div className="edit-head">
          Internal edit · rotating about {bondLabel(scene, plan.cut)}
        </div>
        {splitResolving ? (
          <div className="edit-current muted">finding the rotatable atoms…</div>
        ) : null}
        {splitError ? <div className="edit-error edit-error-severe">{splitError}</div> : null}
      </div>
    );
  }
  if (!active) return null;

  const value = Number(target);
  const valid = target.trim() !== "" && Number.isFinite(value);

  const runPreview = async () => {
    if (!valid) return;
    setBusy(true);
    setError(null);
    try {
      const resp = await callSetInternal(scene, active, value);
      onPreview(applyResponseToScene(scene, active.movingFragmentId, resp.xyz));
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
      const resp = await callSetInternal(scene, active, value);
      const issue = applyResponseIssue(scene, resp.xyz, resp.max_static_displacement);
      if (issue) throw new Error(issue);
      const newScene = applyResponseToScene(scene, active.movingFragmentId, resp.xyz);
      setPreviewing(false);
      onPreview(null);
      // The op carries provenance in AtomIds (stable across a later removal): map
      // the picked global indices to their atoms' ids from the pre-edit scene.
      const atoms = active.indices.map((i) => {
        const loc = locateAtom(scene, i);
        return loc ? loc.fragment.atoms[loc.localIndex].id : makeAtomId(i);
      });
      const op: Op = {
        type: "replace-fragment-atoms",
        fragmentId: active.movingFragmentId,
        name: movingFragmentName ?? "fragment",
        edit: { via: "set-internal", kind: active.op, atoms, target: value, unit: active.unit },
      };
      onApplied(op, newScene);
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

  const header = intra ? (
    <div className="edit-head">
      Internal edit · rotating about{" "}
      {plan.kind === "needs-split" ? bondLabel(scene, plan.cut) : ""}
    </div>
  ) : (
    <div className="edit-head">
      Set {active.op} · moving <strong>{movingFragmentName ?? "fragment"}</strong>
      {pivotLabel(scene, active) ? (
        <span className="muted"> · {pivotLabel(scene, active)}</span>
      ) : null}
    </div>
  );

  return (
    <div className="edit-panel">
      {header}
      {plan.kind === "ready" && plan.reversed ? (
        <div className="edit-reversed muted">
          chain read in reverse so the reagent moves
        </div>
      ) : null}
      <div className="edit-current muted">
        current {round(active.current)} {active.unit}
      </div>
      {plan.kind === "ready" && plan.alternative ? (
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
          aria-label={`target ${active.op} in ${active.unit}`}
        />
        <span className="edit-unit">{active.unit}</span>
        <button className="btn btn-sm" onClick={runPreview} disabled={!valid || busy}>
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
            "edit-error" + (isPostConditionError(error) ? " edit-error-severe" : "")
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

/** "C#12–C#14" for the bond a torsion turns about. */
function bondLabel(scene: Scene, cut: [number, number]): string {
  const label = (i: number) => {
    const d: AtomDescription | null = describeAtom(scene, i);
    return d ? `${d.element}#${i}` : `#${i}`;
  };
  return `${label(cut[0])}–${label(cut[1])}`;
}

/** The pivot an inter-fragment angle/dihedral rotates about (vertex / axis). */
function pivotLabel(scene: Scene, active: ActiveEdit): string | null {
  if (active.op === "angle") {
    const v = active.indices[1];
    const d = describeAtom(scene, v);
    return d ? `vertex ${d.element} #${v}` : null;
  }
  if (active.op === "dihedral") {
    const [, j, k] = active.indices;
    const dj = describeAtom(scene, j);
    const dk = describeAtom(scene, k);
    return dj && dk ? `axis ${dj.element}#${j}–${dk.element}#${k}` : null;
  }
  return null;
}

function messageFor(e: unknown): string {
  if (e instanceof Error) return e.message;
  return String(e);
}

/** A 500 from the endpoint means a real post-condition breach — surface it
 * prominently, it's not a user mistake. */
function isPostConditionError(msg: string): boolean {
  return /not reached|changed|count/i.test(msg);
}
