/**
 * The New Job **draft store** (ADR-026): what New Job holds across a tab switch. `App` mounts only
 * the active screen, so `NewJobScreen` unmounts on every tab switch; its work lives here instead of
 * in component state, and the mount *reads* the draft rather than resetting it.
 *
 * ── The draft is a PAIR ─────────────────────────────────────────────────────
 * Geometry stays in `useSceneStore` (ADR-017: one source of truth, a fold over its log). This store
 * holds **no scene and no log**. The draft is the pair (this store, the scene store), and the
 * functions below are the only places that change both together.
 *
 * ── Two slices, cleared differently (ADR-026 items 3, 5) ────────────────────
 *   • the **work** slice — what this job is (title, input text, iteration context, NEB payload, …);
 *   • the **preferences** slice — how the user likes to run (the builder, Run On, the group), each
 *     with its "touched" flag.
 * `resetDraft` is the only clearing path: it resets the work slice **and** the scene store, and
 * keeps the preferences with their touched flags. An untouched preference keeps following its live
 * default (the screen's effects own that), so nothing derived is ever reset to a constant.
 *
 * ── Seeds are consumed once (item 7) ────────────────────────────────────────
 * A navigation that brings a molecule, a job to iterate, or a conformer scene carries a `seedId`
 * minted here (`mintSeedId`). `applySeed` ignores an id already consumed, so a StrictMode double
 * mount, a tab return and a re-render with the same `screen` object are all harmless.
 *
 * ── Async results are bound to a generation (item 10) ───────────────────────
 * `generation` is bumped by `resetDraft` (and so by every applied seed). An async path that writes
 * the draft or the scene after an `await` captures it first and drops its result on a mismatch.
 *
 * Session only (item 2): nothing here is persisted.
 */

import { useCallback } from "react";
import { create } from "zustand";

import { DEFAULT_BUILDER_STATE, type BuilderState } from "../input-builder/build-input";
import { resolveRunTarget, runTargetOptions } from "../jobs/remote";
import { isSinglePoint } from "../reactions/compare";
import type { NebPayload } from "../reactions/NebBuilderSection";
import { finalGeometryXyz } from "../export/exporters";
import {
  frameProvenanceComment,
  iterationFrames,
  withProvenanceComment,
  type FrameChoice,
  type IterationFrames,
} from "../scene/carryForward";
import type { ConformerApply } from "../scene/ensemble";
import type { AtomId } from "../scene/ids";
import type { FragmentGeometryVia, SceneLog } from "../scene/oplog";
import { restoreSceneLog, type LogRejection } from "../scene/restore";
import { sceneFromOrcaInput, sceneFromXyz, totalCharge } from "../scene/scene";
import { useSceneStore } from "../scene/store";
import type { Scene } from "../scene/types";
import type { BondKey } from "../viewer/bond-display";
import type { Group, Job, Molecule, ParsedResults, ServerProfile } from "../types";

// ── The two slices ─────────────────────────────────────────────────────────────

/**
 * Where the iteration frame seed stands (ADR-026 item 8). A tracked state, not a once-per-mount
 * event: the frame-seed effect runs on every mount while it is `pending`, and only a resolved
 * result moves it on — so leaving the tab mid-read cannot strand the parent's input seed with no
 * banner and no refusal (the silent bug of debugging/022).
 */
export type IterFrameSeed = "none" | "pending" | "landed" | "refused";

/** The work slice: what this job is. Cleared by `resetDraft`. */
export interface DraftWork {
  title: string;
  content: string;
  /** The template last picked (highlights it in the Templates list). */
  selectedTemplateId: string | null;
  /** A generated NEB-TS input's product.xyz + source jobs; routes Create to `create_neb_job`. */
  pendingNeb: NebPayload | null;
  smiles: string;
  /** RDKit's formula from the last SMILES generation (library metadata, not geometry). */
  formula: string;
  /** `NebBuilderSection`'s endpoint picks (a job id, `""` = none). */
  nebReactantId: string;
  nebProductId: string;

  // The iteration context ("New iteration" of a job).
  /** The job being iterated; null when the draft is not an iteration. */
  iterSource: Job | null;
  iterFrames: Extract<IterationFrames, { ok: true }> | null;
  selectedFrame: number | null;
  iterFrameSeed: IterFrameSeed;
  carryBanner: string | null;
  carryRefusal: string | null;
  snapshotRejected: boolean;
  logRejected: LogRejection | null;

  // Replace input (unit 2d): the buffer unlocked, and its content before the unlock.
  replaceMode: boolean;
  replaceSnapshot: string;

  // The constraint composition warning (ЗАХИСТ 2): kept so a remount neither drops a raised warning
  // nor mistakes the restored composition for a change.
  constraintCompWarn: boolean;
  lastConstraintSig: string | null;

  // AtomId-keyed display annotations — valid across a remount, because the scene and its ids are kept.
  formalCharges: ReadonlyMap<AtomId, number>;
  hiddenBonds: ReadonlySet<BondKey>;
  showCationBonds: boolean;
}

/** The preferences slice: how the user likes to run. Survives `resetDraft`, touched flags included. */
export interface DraftPreferences {
  /** The Input Builder's state. Charge and multiplicity are re-seeded on open (`builderStateOnOpen`). */
  builder: BuilderState;
  /** `%maxcore` was edited by hand; until then it follows the Run On default (ADR-024 o26.13.1). */
  maxcoreTouched: boolean;
  /** The nprocs field's raw text (ADR-025 item 2). */
  nprocsText: string;
  /** The Run On target: a profile id, null = this machine. */
  runTarget: string | null;
  runTargetTouched: boolean;
  /** The destination group; follows the active sidebar group until touched. */
  pickedGroupId: string | null;
  groupTouched: boolean;
}

export type DraftFields = DraftWork & DraftPreferences;

/** A value or an updater of the previous value — the shape of React's `setState` argument. */
export type Update<T> = T | ((prev: T) => T);

export interface NewJobDraftStore extends DraftFields {
  /** Bumped by `resetDraft`; async results captured under an older value are dropped. Survives reset. */
  generation: number;
  /** The newest seed id handled (applied, replaced or kept). Survives reset. 0 = none yet. */
  consumedSeedId: number;
  /** Set one field of either slice. */
  setField<K extends keyof DraftFields>(key: K, value: Update<DraftFields[K]>): void;
}

/** The work slice's initial value. A function, so every reset gets fresh `Map`/`Set` instances. */
export function initialWork(): DraftWork {
  return {
    title: "",
    content: "",
    selectedTemplateId: null,
    pendingNeb: null,
    smiles: "",
    formula: "",
    nebReactantId: "",
    nebProductId: "",
    iterSource: null,
    iterFrames: null,
    selectedFrame: null,
    iterFrameSeed: "none",
    carryBanner: null,
    carryRefusal: null,
    snapshotRejected: false,
    logRejected: null,
    replaceMode: false,
    replaceSnapshot: "",
    constraintCompWarn: false,
    lastConstraintSig: null,
    formalCharges: new Map(),
    hiddenBonds: new Set(),
    showCationBonds: false,
  };
}

/** The preferences slice at app start (only — `resetDraft` never returns to it). */
export function initialPreferences(): DraftPreferences {
  return {
    builder: DEFAULT_BUILDER_STATE,
    maxcoreTouched: false,
    nprocsText: "",
    runTarget: null,
    runTargetTouched: false,
    pickedGroupId: null,
    groupTouched: false,
  };
}

export const useNewJobDraft = create<NewJobDraftStore>((set) => ({
  ...initialWork(),
  ...initialPreferences(),
  generation: 0,
  consumedSeedId: 0,

  setField: (key, value) =>
    set((s) => {
      const next =
        typeof value === "function"
          ? (value as (prev: DraftFields[typeof key]) => DraftFields[typeof key])(s[key])
          : value;
      return next === s[key] ? s : ({ [key]: next } as Partial<NewJobDraftStore>);
    }),
}));

/**
 * One draft field as a `useState`-shaped pair: the value (re-rendering on change) and a setter that
 * takes a value or an updater. The setter is stable, so it can be handed to children and effects.
 */
export function useDraftField<K extends keyof DraftFields>(
  key: K,
): [DraftFields[K], (value: Update<DraftFields[K]>) => void] {
  const value = useNewJobDraft((s) => s[key]);
  const setField = useNewJobDraft((s) => s.setField);
  const set = useCallback((v: Update<DraftFields[K]>) => setField(key, v), [setField, key]);
  return [value, set];
}

// ── Emptiness and reset ────────────────────────────────────────────────────────

/**
 * Is there work in the draft (ADR-026 item 6)? Work = a non-blank title or input text, a scene, or
 * a non-empty log (a scene undone to nothing still has redo history — that is work too). The
 * preferences slice never makes a draft non-empty.
 */
export function isDraftEmpty(
  draft: Pick<DraftWork, "title" | "content">,
  sceneState: { scene: Scene | null; log: SceneLog },
): boolean {
  return (
    draft.title.trim() === "" &&
    draft.content.trim() === "" &&
    sceneState.scene === null &&
    sceneState.log.entries.length === 0
  );
}

/**
 * The only clearing path (ADR-026 item 5): the work slice to its initial value, the scene store to
 * the empty log, and a new generation. The preferences slice, `consumedSeedId` and the generation's
 * monotonic count are kept.
 */
export function resetDraft(): void {
  useNewJobDraft.setState((s) => ({ ...initialWork(), generation: s.generation + 1 }));
  useSceneStore.getState().seedScene(null, "text-adopt");
}

// ── The generation guard (item 10) ─────────────────────────────────────────────

/** The draft generation now; capture it before an `await`. */
export function captureGeneration(): number {
  return useNewJobDraft.getState().generation;
}

/** True when no reset or seed happened since `gen` was captured — the result may still be written. */
export function isCurrent(gen: number): boolean {
  return useNewJobDraft.getState().generation === gen;
}

// ── Seeds (items 6, 7) ─────────────────────────────────────────────────────────

/** What a navigation into New Job can bring. */
export type DraftSeed =
  | { kind: "molecule"; molecule: Molecule }
  | { kind: "iteration"; job: Job }
  | { kind: "conformer-scene"; scene: Scene; label: string };

let lastSeedId = 0;

/** A fresh seed id, strictly greater than every one minted before (module-level, not `App`'s). */
export function mintSeedId(): number {
  lastSeedId += 1;
  return lastSeedId;
}

/** How the "Replace the current draft with ‹seed›?" prompt names a seed. */
export function seedLabel(seed: DraftSeed): string {
  switch (seed.kind) {
    case "molecule":
      return `the molecule “${seed.molecule.name}”`;
    case "iteration":
      return `a new iteration of “${seed.job.title}”`;
    case "conformer-scene":
      return seed.label;
  }
}

export type SeedOutcome = "ignored" | "applied" | "ask";

/** Ids are monotonic, so any id at or below the newest consumed one was already handled. */
function isConsumed(seedId: number): boolean {
  return seedId <= useNewJobDraft.getState().consumedSeedId;
}

function markConsumed(seedId: number): void {
  useNewJobDraft.setState((s) => ({ consumedSeedId: Math.max(s.consumedSeedId, seedId) }));
}

/**
 * Write a seed into a draft `resetDraft` has just cleared. This is the former mount
 * `useLayoutEffect` of `NewJobScreen`, one branch per kind:
 *  - molecule → a single "library" fragment, the molecule's name as the title, its formula;
 *  - iteration → the job's operation log, cross-checked against its snapshot (`restoreSceneLog`),
 *    its input text, and the frame seed `pending` (the frame-seed effect lands it, item 8);
 *  - conformer-scene → a fresh lineage from the conformer ("Use this conformer", the `new` branch).
 */
function writeSeed(seed: DraftSeed): void {
  const scenes = useSceneStore.getState();
  switch (seed.kind) {
    case "molecule": {
      const m = seed.molecule;
      scenes.seedScene(
        sceneFromXyz(m.xyz, {
          source: "library",
          sourceLabel: m.id,
          name: m.name,
          charge: m.charge,
          multiplicity: m.multiplicity,
        }),
        "library",
      );
      useNewJobDraft.setState({ title: m.name, formula: m.formula });
      return;
    }
    case "iteration": {
      const job = seed.job;
      const restored = restoreSceneLog(job.input_content, job.scene_json, job.scene_log_json ?? null);
      scenes.installLog(restored.log);
      useNewJobDraft.setState({
        title: `${job.title} (iteration)`,
        content: job.input_content,
        iterSource: job,
        iterFrameSeed: "pending",
        snapshotRejected: restored.snapshotRejected,
        logRejected: restored.logRejected,
      });
      return;
    }
    case "conformer-scene":
      scenes.seedScene(seed.scene, "new-iteration");
      return;
  }
}

/**
 * Offer a seed to the draft (ADR-026 items 6, 7):
 *  - an id already consumed → `"ignored"`, nothing touched;
 *  - an empty draft → `resetDraft`, then the seed is written → `"applied"` (the reset is what clears
 *    a previous seed's iteration context, refusals and NEB payload);
 *  - a draft with work → `"ask"`, **nothing touched** — the screen shows the prompt and answers it
 *    with `resolveSeedPrompt`.
 */
export function applySeed(seed: DraftSeed, seedId: number): SeedOutcome {
  if (isConsumed(seedId)) return "ignored";
  if (!isDraftEmpty(useNewJobDraft.getState(), useSceneStore.getState())) return "ask";
  resetDraft();
  writeSeed(seed);
  markConsumed(seedId);
  return "applied";
}

/**
 * Answer the prompt `applySeed` asked for. *Replace* resets the draft and writes the seed; *Keep
 * draft* drops the seed and touches nothing else. Both consume the id, so the same seed never asks
 * twice. An id consumed meanwhile (a second click) → `"ignored"`.
 */
export function resolveSeedPrompt(
  seed: DraftSeed,
  seedId: number,
  choice: "replace" | "keep",
): "ignored" | "replaced" | "kept" {
  if (isConsumed(seedId)) return "ignored";
  markConsumed(seedId);
  if (choice === "keep") return "kept";
  resetDraft();
  writeSeed(seed);
  return "replaced";
}

/**
 * "Use this conformer" / the D3 DFT carry, from Job detail into the draft (ADR-026 item 6), given
 * `planConformerApply`'s decision:
 *  - `replace` → the draft's own fragment (its id is still in the scene) gets the conformer's atoms
 *    as one logged, undoable op, and New Job opens with **no seed**;
 *  - `new` → nothing is written here: the fresh single-fragment scene goes to New Job as a
 *    `conformer-scene` seed, so a draft with work gets the prompt instead of being replaced;
 *  - `refuse` → its reason.
 */
export function handOffConformer(
  plan: ConformerApply,
  provenance: FragmentGeometryVia,
  multiplicity: number,
  label: string,
): { ok: true; seed: DraftSeed | null } | { ok: false; reason: string } {
  switch (plan.action) {
    case "refuse":
      return { ok: false, reason: plan.reason };
    case "replace":
      useSceneStore.getState().replaceFragmentAtoms(plan.fragmentId, plan.atoms, provenance);
      return { ok: true, seed: null };
    case "new":
      return {
        ok: true,
        seed: {
          kind: "conformer-scene",
          scene: { fragments: [plan.fragment], multiplicity, nextAtomId: plan.nextAtomId },
          label,
        },
      };
  }
}

// ── The iteration frame seed (item 8) ──────────────────────────────────────────

/** What the frame-seed effect reads: the job to fetch results for, under a generation. */
export interface IterFrameTask {
  gen: number;
  job: Job;
}

/**
 * The frame-seed effect's start: the task while the seed is `pending`, null otherwise (nothing to
 * do — `none`, or already `landed`/`refused`, which never re-run). Reads only: the state moves only
 * when a result is applied (`resolveIterFrameSeed`), so an unmount before that leaves it `pending`
 * and the next mount runs it again.
 */
export function iterFrameSeedTask(): IterFrameTask | null {
  const d = useNewJobDraft.getState();
  if (d.iterFrameSeed !== "pending" || d.iterSource === null) return null;
  return { gen: d.generation, job: d.iterSource };
}

/** Whether a frame could be built into a scene; `cause` says why not. */
type FrameBuild = { ok: true } | { ok: false; cause: string };

/** The refusal shown when a trajectory frame cannot be built: it names the frame and says what the
 *  viewer shows instead (ADR-026 item 8 — never a silent input seed, debugging/022). */
function frameRefusal(job: Job, choice: FrameChoice, cause: string): string {
  return `Frame ${choice.index} (${choice.label}) of “${job.title}” could not be built into a geometry (${cause}). The geometry shown is the INPUT SEED, not that frame.`;
}

/**
 * Seed the iteration scene from ONE trajectory frame (the default, or a re-pick): a fresh scene from
 * the frame — never the input text — charge and multiplicity from the parent's header, a
 * frame-exact provenance comment in the text (stripped and replaced, so re-picks never stack) and
 * the banner. The parent's edit log is not carried. A real frame supersedes the snapshot/log
 * rejection notes of the restore.
 *
 * The scene is built first, and nothing is written unless it was: `finalGeometryXyz` throws on an
 * elements/rows mismatch, and `sceneFromXyz` returns null on an unreadable block.
 */
function seedFrame(job: Job, choice: FrameChoice): FrameBuild {
  let fresh: Scene | null;
  try {
    const parent = sceneFromOrcaInput(job.input_content); // charge/mult from the header (.xyz has none)
    const xyz = finalGeometryXyz(choice.geometry, job.title, choice.energyEh);
    fresh = sceneFromXyz(xyz, {
      name: `${job.title} — ${choice.label}`,
      charge: parent ? totalCharge(parent) : 0,
      multiplicity: parent ? parent.multiplicity : 1,
      source: "editor",
    });
  } catch (e) {
    return { ok: false, cause: e instanceof Error ? e.message : String(e) };
  }
  if (!fresh) return { ok: false, cause: "the frame's coordinates do not read as a geometry" };
  useSceneStore.getState().seedScene(fresh, "text-adopt");
  useNewJobDraft.setState((s) => {
    const stripped = s.content
      .split(/\r?\n/)
      .filter((l) => !l.trim().startsWith("# geometry:"))
      .join("\n");
    return {
      content: withProvenanceComment(stripped, frameProvenanceComment(job.id, choice)),
      carryBanner: `Seeded from ${choice.label} (frame ${choice.index}) of “${job.title}”. Pick another frame below to reseed.`,
      snapshotRejected: false,
      logRejected: null,
    };
  });
  return { ok: true };
}

/** What happened to a frame-seed result. `stale` = dropped (reset/seed since, or already resolved). */
export type IterFrameOutcome = "stale" | "landed" | "refused";

/**
 * Apply the parent's parsed results to a `pending` frame seed (debugging/022):
 *  - an optimization trajectory → the LAST frame (the optimized output) seeded unconditionally,
 *    the picker filled → `landed`;
 *  - no trajectory on a genuine single point → its seed IS its geometry, kept silently → `landed`
 *    (so it is never re-read on every mount);
 *  - no trajectory on an input that asks for an optimization → a refusal: the shown geometry is the
 *    input seed → `refused`;
 *  - scan / NEB / no result → their refusal → `refused`;
 *  - a last frame that cannot be built into a geometry → a refusal naming it → `refused`. The
 *    picker (`iterFrames`, `selectedFrame`) is written only once the frame was built.
 * A result for an older generation, or for a seed no longer `pending`, changes nothing.
 */
export function resolveIterFrameSeed(
  task: IterFrameTask,
  results: ParsedResults | null,
): IterFrameOutcome {
  if (!isCurrent(task.gen) || useNewJobDraft.getState().iterFrameSeed !== "pending") return "stale";
  const job = task.job;
  const fr = iterationFrames(job, results);
  if (!fr.ok) {
    if (fr.kind === "no-trajectory" && isSinglePoint(job.input_content)) {
      useNewJobDraft.setState({ iterFrameSeed: "landed" });
      return "landed";
    }
    const reason =
      fr.kind === "no-trajectory"
        ? `“${job.title}” is an optimization but its trajectory could not be read — there is no frame to pick, and the geometry shown is the INPUT SEED, not the optimized output. Re-run or re-parse before iterating.`
        : fr.reason;
    useNewJobDraft.setState({ carryRefusal: reason, iterFrameSeed: "refused" });
    return "refused";
  }
  const choice = fr.frames[fr.defaultIndex];
  const built = seedFrame(job, choice);
  if (!built.ok) {
    useNewJobDraft.setState({
      carryRefusal: frameRefusal(job, choice, built.cause),
      iterFrameSeed: "refused",
    });
    return "refused";
  }
  useNewJobDraft.setState({ iterFrames: fr, selectedFrame: fr.defaultIndex, iterFrameSeed: "landed" });
  return "landed";
}

/**
 * The frame picker's re-pick: reseed from frame `index`; the seed state stays `landed`. A frame that
 * cannot be built changes nothing — `selectedFrame` keeps the frame actually shown — and its refusal
 * is returned for a notice. Null when the re-pick landed (or there was nothing to pick).
 */
export function selectIterationFrame(index: number): string | null {
  const d = useNewJobDraft.getState();
  const choice = d.iterFrames?.frames[index];
  if (!d.iterSource || !choice) return null;
  const built = seedFrame(d.iterSource, choice);
  if (!built.ok) return frameRefusal(d.iterSource, choice, built.cause);
  useNewJobDraft.setState({ selectedFrame: index });
  return null;
}

// ── Create (item 5) ────────────────────────────────────────────────────────────

/**
 * Everything `create()` reads, captured at its start. The job is created from these, and the steps
 * after the create call (group assignment, `last_run_target`, opening the detail) use them — never
 * the store, which `clearAfterCreate` may have reset by then.
 */
export interface CreateStart {
  gen: number;
  title: string;
  content: string;
  pendingNeb: NebPayload | null;
  scene: Scene | null;
  log: SceneLog;
  runTarget: string | null;
  groupTouched: boolean;
  pickedGroupId: string | null;
}

export function captureCreateStart(): CreateStart {
  const d = useNewJobDraft.getState();
  const { scene, log } = useSceneStore.getState();
  return {
    gen: d.generation,
    title: d.title,
    content: d.content,
    pendingNeb: d.pendingNeb,
    scene,
    log,
    runTarget: d.runTarget,
    groupTouched: d.groupTouched,
    pickedGroupId: d.pickedGroupId,
  };
}

export type CreateClear = { cleared: true } | { cleared: false; reason: string };

/**
 * Clear the draft after a successful create — only when it is still the draft the job was created
 * from: the same generation, and the same `title`, `content` and `scene` **references** as at the
 * start. An edit made while the create was awaited is not in the job, so that draft is kept and the
 * reason is returned for a notice.
 */
export function clearAfterCreate(
  start: Pick<CreateStart, "gen" | "title" | "content" | "scene">,
): CreateClear {
  if (!isCurrent(start.gen)) {
    return {
      cleared: false,
      reason: "The job was created. The draft was replaced while it was being created, so the new draft is kept.",
    };
  }
  const d = useNewJobDraft.getState();
  if (
    d.title !== start.title ||
    d.content !== start.content ||
    useSceneStore.getState().scene !== start.scene
  ) {
    return {
      cleared: false,
      reason:
        "The job was created from the input as it was when you pressed Create. Your edits since then are not in it; they are kept in the draft.",
    };
  }
  resetDraft();
  return { cleared: true };
}

// ── Re-validation of the preferences on mount (item 9) ────────────────────────

/** A list read from Rust: its items, or the failure. A failed read is never an empty list. */
export type ListResult<T> = { ok: true; items: readonly T[] } | { ok: false; error: string };

/** A preference change found by re-validation, with the notice that must accompany it. */
export type Revalidation<P> = { changed: false } | { changed: true; patch: P; notice: string };

/**
 * Re-validate a **touched** Run On target once the profile list has loaded. A profile deleted or
 * disabled while New Job was away → the target `resolveRunTarget` gives (`last_run_target`, else
 * Local), touched reset, and a notice — never a silent switch. A failed list changes nothing. An
 * untouched target is not this function's: `loadRunTarget` preselects it on every mount. Writes no
 * setting.
 */
export function revalidateRunTarget(
  prefs: Pick<DraftPreferences, "runTarget" | "runTargetTouched">,
  profiles: ListResult<ServerProfile>,
  settings: Record<string, string> | null,
): Revalidation<Pick<DraftPreferences, "runTarget" | "runTargetTouched">> {
  if (!profiles.ok || !prefs.runTargetTouched) return { changed: false };
  const options = runTargetOptions([...profiles.items]);
  if (options.some((o) => o.target === prefs.runTarget && o.enabled)) return { changed: false };
  const resolved = resolveRunTarget(options, settings);
  const label = options.find((o) => o.target === resolved)?.label ?? "Local";
  return {
    changed: true,
    patch: { runTarget: resolved, runTargetTouched: false },
    notice: `The server chosen for Run on was deleted or disabled in Settings; Run on is now ${label}.`,
  };
}

/**
 * Re-validate a **touched** group pick once the group list has loaded. A group deleted while New
 * Job was away (`move_job` would refuse it) → the pick is untouched again and follows the active
 * sidebar group, with a notice. "(ungrouped)" (null) is always valid. A failed list changes nothing.
 */
export function revalidateGroup(
  prefs: Pick<DraftPreferences, "pickedGroupId" | "groupTouched">,
  groups: ListResult<Group>,
  activeGroupId: string | null,
): Revalidation<Pick<DraftPreferences, "pickedGroupId" | "groupTouched">> {
  if (!groups.ok || !prefs.groupTouched || prefs.pickedGroupId === null) return { changed: false };
  if (groups.items.some((g) => g.id === prefs.pickedGroupId)) return { changed: false };
  return {
    changed: true,
    patch: { pickedGroupId: activeGroupId, groupTouched: false },
    notice:
      "The group chosen for the new job was deleted; the destination follows the active group in the Jobs sidebar again.",
  };
}

/**
 * A kept pick to show as its own option because its list is not loaded (ADR-026 item 9, Anton): while
 * the list is loading or after it failed, a non-null pick absent from it is still the choice — the
 * create will use it — so the select must show it, never Local, "(ungrouped)" or a blank. Null when
 * there is nothing to add: no pick, a successfully loaded list (re-validation owns that case), or a
 * pick the list already contains.
 */
export function unlistedPick(
  pick: string | null,
  listLoaded: boolean,
  listed: readonly (string | null)[],
): string | null {
  if (pick === null || listLoaded || listed.includes(pick)) return null;
  return pick;
}

// ── The builder on open (item 4) ───────────────────────────────────────────────

/**
 * The builder's state when it opens: every remembered field, except charge and multiplicity, which
 * are re-read from the current `* xyz c m` header — or the defaults when there is none — exactly as
 * the form's own initializer does. Rule #9: a remembered multiplicity must never be applied to a
 * geometry that changed since (the form generates with `setMultiplicity(parsed, multiplicity)`).
 */
export function builderStateOnOpen(remembered: BuilderState, currentContent: string): BuilderState {
  const seed = sceneFromOrcaInput(currentContent);
  return {
    ...remembered,
    charge: seed ? totalCharge(seed) : DEFAULT_BUILDER_STATE.charge,
    multiplicity: seed ? seed.multiplicity : DEFAULT_BUILDER_STATE.multiplicity,
  };
}
