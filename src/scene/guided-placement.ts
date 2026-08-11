/**
 * Guided fragment placement (Phase 4.2 tail-1) — the PURE planner + driver. Turns
 * a picked reagent atom + 1–3 substrate reference atoms + target d/θ/φ into a
 * SEQUENCE of set-internal edits, each moving the reagent fragment. It REUSES
 * `planEdit` (2.5.2d) per coordinate — there is NO new d/θ/φ math here — and the
 * existing `applyResponseToScene` / `applyResponseIssue` path drives the sidecar
 * calls. This module only decides WHICH set-internal calls to make, in which
 * order, with the reagent as the mover.
 *
 * ## One flow = a sequence of EXISTING ops (invariant 1)
 * The reagent is already added (an `add-fragment` op committed at rough placement,
 * `placeFragment` — unchanged). Placement then commits, in order, one
 * `replace-fragment-atoms` (via `set-internal`) op per GIVEN coordinate: distance
 * (required), then angle, then dihedral. Undo unwinds each step (ADR-017). It is
 * NOT one bundled op — the log reads "Add BH₄⁻", "Set distance …", "Set angle …".
 *
 * ## Z-matrix nesting (why d → θ → φ, in that order)
 * With the reagent atom R and substrate anchors A, B, C:
 *  - d = distance(R, A)        → ASE chain [A, R]
 *  - θ = angle(R, A, B)        → ASE chain [B, A, R]     (vertex A)
 *  - φ = dihedral(R, A, B, C)  → ASE chain [C, B, A, R]  (axis A–B)
 * Every later edit rotates the reagent about an axis THROUGH A, so it preserves
 * the earlier coordinate (the distance to A, then the angle at A) — the standard
 * internal-coordinate (Z-matrix) construction. `planEdit`'s chain convention puts
 * the mover LAST, so R is last in every chain; `planEdit` then selects the
 * orientation whose moving fragment IS the reagent (the mask = the reagent
 * fragment — exactly the inter-fragment case 2.5.2d).
 *
 * ## Only GIVEN coordinates apply (invariant 2)
 * d is required; θ/φ are emitted ONLY when their target is non-null AND enough
 * substrate refs were picked. An empty field is a SKIP, never a 0.
 *
 * Pure & DI-tested: the planner is synchronous and React/fetch-free; the driver
 * takes the set-internal call as a parameter, so `guided-placement.test.ts`
 * exercises the whole flow with a fake sidecar (no live server).
 */

import type { Scene } from "./types";
import type { AtomId } from "./ids";
import type { Op } from "./oplog";
import { planEdit, swapToAlternative, applyResponseIssue, applyResponseToScene } from "./edit-plan";

/** The three approach coordinates, each optional except the distance. `null` = the
 * user left the field empty → SKIP (invariant 2); never coerced to 0. */
export interface GuidedTargets {
  distance: number;
  angle: number | null;
  dihedral: number | null;
}

/** One set-internal edit to run — the reagent-fragment mask, the ASE chain (mover =
 * reagent atom, LAST), the target + unit. The same shape the sidecar call and
 * `applyResponseToScene` consume, so the driver never reinvents them. */
export interface GuidedStep {
  op: "distance" | "angle" | "dihedral";
  indices: number[];
  mask: number[];
  value: number;
  unit: "Å" | "°";
  /** The picked chain as AtomIds (mover last) — provenance for the committed op. */
  atomIds: AtomId[];
}

export type GuidedPlan =
  | { kind: "ready"; steps: GuidedStep[]; movingFragmentId: string }
  | { kind: "unavailable"; reason: string };

/**
 * Resolve one ASE chain to the orientation that moves the REAGENT fragment, reusing
 * `planEdit` (the mask, the reference-atom rule, the both-orientation search — all
 * already tested there). `planEdit` defaults to moving the smaller fragment; the
 * reagent is usually smaller, but we force the reagent regardless of size by picking
 * the primary-or-alternative orientation whose `movingFragmentId` is the reagent.
 */
function reagentOrientation(
  scene: Scene,
  reagentFragmentId: string,
  chain: AtomId[],
):
  | { op: "distance" | "angle" | "dihedral"; indices: number[]; mask: number[]; unit: "Å" | "°" }
  | { error: string } {
  // No `components` injected here → planEdit never yields `needs-component-move`
  // (that needs the sidecar's connectivity); guided placement is inter-fragment by
  // construction (one reagent atom + substrate refs).
  const plan = planEdit(scene, chain);
  if (plan.kind === "unavailable") return { error: plan.reason };
  if (plan.kind !== "ready") {
    // `needs-split` (all atoms in one fragment) — the panel's "exactly one reagent
    // atom" rule prevents it; guard anyway. (`needs-component-move` cannot arise
    // without injected connectivity.)
    return {
      error:
        "These atoms are all in one fragment — pick one atom on the reagent and the rest on the substrate.",
    };
  }
  let p = plan;
  if (p.movingFragmentId !== reagentFragmentId) {
    if (p.alternative && p.alternative.movingFragmentId === reagentFragmentId) {
      const swapped = swapToAlternative(p);
      if (swapped.kind !== "ready") return { error: "Unexpected plan state." };
      p = swapped;
    } else {
      return {
        error: "The reagent atom must be the one that moves — pick exactly one atom on the reagent.",
      };
    }
  }
  return { op: p.op, indices: p.indices, mask: p.mask, unit: p.unit };
}

/**
 * Plan the sequence of set-internal edits that places `reagentAtom` (on the just-
 * added reagent fragment) at the target internal coordinates relative to the
 * substrate anchors — d required, θ/φ optional. Pure & synchronous.
 */
export function planGuidedPlacement(
  scene: Scene,
  reagentFragmentId: string,
  reagentAtom: AtomId,
  substrateRefs: AtomId[],
  targets: GuidedTargets,
): GuidedPlan {
  if (substrateRefs.length < 1) {
    return {
      kind: "unavailable",
      reason: "Pick one atom on the reagent and at least one substrate atom to set the approach distance.",
    };
  }
  if (!(targets.distance > 0)) {
    return { kind: "unavailable", reason: "The approach distance must be greater than 0 Å." };
  }

  const [A, B, C] = substrateRefs;

  // Build ONLY the requested coordinates (invariant 2): distance always; angle /
  // dihedral iff their target is non-null AND enough refs exist. mover = R, LAST.
  const requested: { op: "distance" | "angle" | "dihedral"; chain: AtomId[]; value: number }[] = [
    { op: "distance", chain: [A, reagentAtom], value: targets.distance },
  ];
  if (targets.angle !== null) {
    if (substrateRefs.length < 2) {
      return { kind: "unavailable", reason: "An approach angle needs two substrate atoms — pick one more." };
    }
    requested.push({ op: "angle", chain: [B, A, reagentAtom], value: targets.angle });
  }
  if (targets.dihedral !== null) {
    if (substrateRefs.length < 3) {
      return { kind: "unavailable", reason: "An approach dihedral needs three substrate atoms — pick one more." };
    }
    requested.push({ op: "dihedral", chain: [C, B, A, reagentAtom], value: targets.dihedral });
  }

  const steps: GuidedStep[] = [];
  for (const r of requested) {
    const resolved = reagentOrientation(scene, reagentFragmentId, r.chain);
    if ("error" in resolved) return { kind: "unavailable", reason: resolved.error };
    steps.push({
      op: r.op,
      indices: resolved.indices,
      mask: resolved.mask,
      value: r.value,
      unit: resolved.unit,
      atomIds: r.chain,
    });
  }
  return { kind: "ready", steps, movingFragmentId: reagentFragmentId };
}

/**
 * The legible `replace-fragment-atoms` op a single guided step commits — one per
 * coordinate, so Undo unwinds d/θ/φ individually (invariant 1; NOT one bundled,
 * opaque op). Same provenance shape `EditPanel` writes for a manual set-internal
 * edit, so the history panel renders "Set distance …" identically.
 */
export function guidedStepOp(step: GuidedStep, fragmentId: string, name: string): Op {
  return {
    type: "replace-fragment-atoms",
    fragmentId,
    name,
    edit: {
      via: "set-internal",
      kind: step.op,
      atoms: step.atomIds,
      target: step.value,
      unit: step.unit,
    },
  };
}

/** What a single set-internal call must hand back for the driver to proceed. */
export interface GuidedSetInternalResponse {
  xyz: string;
  max_static_displacement: number;
}

export interface GuidedRunResult {
  /** The final geometry after all steps (for the view-only preview). */
  scene: Scene;
  /** One (op, resultant-snapshot) pair per step — committed in order on Apply. */
  ops: { op: Op; scene: Scene }[];
}

/**
 * Run a ready plan's steps sequentially, threading the scene through each edit. The
 * sidecar call is INJECTED (`setInternal`) so tests drive the whole flow with a fake
 * server. `checkPostCondition` is the Apply guard (rule #9): a response that moved a
 * static atom or changed the atom count is REFUSED (`applyResponseIssue`) — Preview
 * skips it (view-only, mirrors `EditPanel`). Returns the final scene + the op/snapshot
 * pairs; the caller commits them (Apply) or shows `scene` (Preview).
 */
export async function runGuidedPlacement(
  scene: Scene,
  plan: Extract<GuidedPlan, { kind: "ready" }>,
  reagentName: string,
  setInternal: (scene: Scene, step: GuidedStep) => Promise<GuidedSetInternalResponse>,
  opts: { checkPostCondition: boolean },
): Promise<GuidedRunResult> {
  let s = scene;
  const ops: { op: Op; scene: Scene }[] = [];
  for (const step of plan.steps) {
    const resp = await setInternal(s, step);
    if (opts.checkPostCondition) {
      const issue = applyResponseIssue(s, resp.xyz, resp.max_static_displacement);
      if (issue) throw new Error(issue);
    }
    const next = applyResponseToScene(s, plan.movingFragmentId, resp.xyz);
    ops.push({ op: guidedStepOp(step, plan.movingFragmentId, reagentName), scene: next });
    s = next;
  }
  return { scene: s, ops };
}
