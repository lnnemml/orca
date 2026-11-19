import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { beforeEach, describe, expect, it } from "vitest";

import { DEFAULT_BUILDER_STATE, type BuilderState } from "../input-builder/build-input";
import { LAST_RUN_TARGET_SETTING } from "../jobs/remote";
import { emptyLog, type SceneLog } from "../scene/oplog";
import { useSceneStore } from "../scene/store";
import type { Group, Job, Molecule, ParsedResults, ServerProfile } from "../types";
import {
  applySeed,
  builderStateOnOpen,
  captureCreateStart,
  captureGeneration,
  clearAfterCreate,
  handOffConformer,
  initialPreferences,
  initialWork,
  isCurrent,
  isDraftEmpty,
  iterFrameSeedTask,
  mintSeedId,
  resetDraft,
  resolveIterFrameSeed,
  resolveSeedPrompt,
  revalidateGroup,
  revalidateRunTarget,
  selectIterationFrame,
  useNewJobDraft,
  unlistedPick,
  type DraftSeed,
} from "./new-job-draft";

// Reset BOTH stores between tests (the actions survive — they're closures). The draft's monotonic
// counters restart too; the module-level seed-id counter does not (it never needs to).
beforeEach(() => {
  useNewJobDraft.setState({
    ...initialWork(),
    ...initialPreferences(),
    generation: 0,
    consumedSeedId: 0,
  });
  useSceneStore.setState({ log: emptyLog(), scene: null });
});

const draft = () => useNewJobDraft.getState();
const scenes = () => useSceneStore.getState();

// ── Fixtures ───────────────────────────────────────────────────────────────────

const WATER_XYZ = "3\nwater\nO 0 0 0\nH 0.96 0 0\nH -0.24 0.93 0\n";

function molecule(over: Partial<Molecule> = {}): Molecule {
  return {
    id: "mol-1",
    name: "Water",
    formula: "H2O",
    xyz: WATER_XYZ,
    charge: 0,
    multiplicity: 1,
    tags: "",
    created_at: "",
    is_reagent: false,
    ...over,
  };
}

/** A minimal Job (only the fields the draft reads). */
function job(over: Partial<Job> = {}): Job {
  return {
    id: "parent-1",
    title: "Ethene opt",
    status: "parsed",
    input_content: "! r2SCAN-3c Opt\n* xyz 0 1\nC 0 0 0\nC 0 0 1.40\n*\n",
    scene_json: null,
    scene_log_json: null,
    ...over,
  } as Job;
}

/** A minimal ParsedResults (only the fields `iterationFrames` reads). */
function results(over: Partial<ParsedResults> = {}): ParsedResults {
  return {
    converged: true,
    final_geometry: { elements: ["C", "C"], xyz_angstrom: [[0, 0, 0], [0, 0, 1.33]] },
    trajectory: null,
    scan: null,
    neb: null,
    ...over,
  } as ParsedResults;
}

/** A 2-frame trajectory: the seed (1.40 Å) and the optimized output (1.33 Å). */
const TRAJ: ParsedResults["trajectory"] = {
  n_frames: 2,
  elements: ["C", "C"],
  frames: [
    { energy_eh: -78.1, xyz_angstrom: [[0, 0, 0], [0, 0, 1.4]] },
    { energy_eh: -78.2, xyz_angstrom: [[0, 0, 0], [0, 0, 1.33]] },
  ],
};

function profile(id: string, enabled: boolean): ServerProfile {
  return {
    id,
    name: `server ${id}`,
    run_target: { is_run_target: enabled, reason: enabled ? null : "connection test not passed" },
  } as ServerProfile;
}

function group(id: string): Group {
  return { id, name: id, parent_id: null, created_at: "" };
}

/** The second C's z, the coordinate the frame fixtures differ in. */
function secondAtomZ(): number | undefined {
  return scenes().scene?.fragments[0]?.atoms[1]?.z;
}

/** Non-default preferences, every touched flag set. */
const TOUCHED_PREFS = {
  builder: { ...DEFAULT_BUILDER_STATE, methodFamily: "dft", functional: "PBE0", basis: "def2-TZVP" },
  maxcoreTouched: true,
  nprocsText: "8",
  runTarget: "srv-a",
  runTargetTouched: true,
  pickedGroupId: "grp-a",
  groupTouched: true,
} satisfies Partial<ReturnType<typeof initialPreferences>>;

/** Put work in both stores: a title, an input, a scene and some work-slice state. */
function fillDraft(): void {
  useNewJobDraft.setState({
    title: "My TS",
    content: "! B3LYP\n* xyz 0 1\nO 0 0 0\n*\n",
    pendingNeb: { productXyz: "1\n\nH 0 0 0\n", reactantJobId: "r", productJobId: "p" },
    carryRefusal: "an old refusal",
    iterFrameSeed: "refused",
    constraintCompWarn: true,
    smiles: "CCO",
  });
  putFragmentInScene();
}

function putFragmentInScene(): void {
  scenes().addFragment({
    id: "f1",
    name: "O",
    charge: 0,
    source: "editor",
    atoms: [{ element: "O", x: 0, y: 0, z: 0 }],
  });
}

// ── setField ───────────────────────────────────────────────────────────────────

describe("setField — one field of either slice", () => {
  it("takes a value", () => {
    draft().setField("title", "Diels–Alder TS");
    draft().setField("runTarget", "srv-a");
    expect(draft().title).toBe("Diels–Alder TS");
    expect(draft().runTarget).toBe("srv-a");
  });

  it("takes an updater of the previous value", () => {
    draft().setField("content", "! HF");
    draft().setField("content", (c) => c + "\n* xyz 0 1\n*");
    expect(draft().content).toBe("! HF\n* xyz 0 1\n*");
  });

  it("a no-op keeps the state reference (no re-render)", () => {
    draft().setField("title", "same");
    const before = useNewJobDraft.getState();
    draft().setField("title", "same");
    draft().setField("title", (t) => t);
    expect(useNewJobDraft.getState()).toBe(before);
  });
});

// ── resetDraft ─────────────────────────────────────────────────────────────────

describe("resetDraft — clears the work slice and the scene, keeps the preferences", () => {
  it("empties the work slice and the scene store", () => {
    fillDraft();
    resetDraft();
    const d = draft();
    for (const [k, v] of Object.entries(initialWork())) {
      expect(d[k as keyof typeof d], k).toEqual(v);
    }
    expect(scenes().scene).toBeNull();
    expect(scenes().log.entries).toHaveLength(0);
  });

  it("keeps the preferences slice with its touched flags", () => {
    useNewJobDraft.setState(TOUCHED_PREFS);
    fillDraft();
    resetDraft();
    const d = draft();
    for (const [k, v] of Object.entries(TOUCHED_PREFS)) {
      expect(d[k as keyof typeof d], k).toEqual(v);
    }
  });

  it("bumps the generation and keeps consumedSeedId", () => {
    useNewJobDraft.setState({ generation: 4, consumedSeedId: 7 });
    resetDraft();
    expect(draft().generation).toBe(5);
    expect(draft().consumedSeedId).toBe(7);
  });

  it("gives fresh Map/Set instances, so an annotation never leaks into the next draft", () => {
    const before = draft().formalCharges;
    resetDraft();
    expect(draft().formalCharges).not.toBe(before);
  });
});

// ── isDraftEmpty ───────────────────────────────────────────────────────────────

describe("isDraftEmpty — work counts, preferences never do", () => {
  const blank = { title: "", content: "" };
  const noScene = { scene: null, log: emptyLog() };

  it("the truth table", () => {
    expect(isDraftEmpty(blank, noScene)).toBe(true);
    expect(isDraftEmpty({ title: "  \n", content: " \t" }, noScene)).toBe(true);
    expect(isDraftEmpty({ title: "x", content: "" }, noScene)).toBe(false);
    expect(isDraftEmpty({ title: "", content: "! HF" }, noScene)).toBe(false);
    putFragmentInScene();
    expect(isDraftEmpty(blank, scenes())).toBe(false);
  });

  it("a null scene with a non-empty log (undone to nothing, redo history) is work", () => {
    putFragmentInScene();
    scenes().jumpTo(-1);
    expect(scenes().scene).toBeNull();
    const log: SceneLog = scenes().log;
    expect(log.entries.length).toBeGreaterThan(0);
    expect(isDraftEmpty(blank, scenes())).toBe(false);
  });

  it("touched preferences alone leave the draft empty", () => {
    useNewJobDraft.setState(TOUCHED_PREFS);
    expect(isDraftEmpty(draft(), scenes())).toBe(true);
  });
});

// ── Seeds ──────────────────────────────────────────────────────────────────────

describe("mintSeedId — monotonic", () => {
  it("every id is greater than the one before", () => {
    const a = mintSeedId();
    const b = mintSeedId();
    const c = mintSeedId();
    expect(b).toBeGreaterThan(a);
    expect(c).toBeGreaterThan(b);
  });
});

describe("applySeed — consumed once, asks on a non-empty draft", () => {
  const moleculeSeed: DraftSeed = { kind: "molecule", molecule: molecule() };

  it("writes a molecule seed into an empty draft (the former mount branch)", () => {
    const id = mintSeedId();
    expect(applySeed(moleculeSeed, id)).toBe("applied");
    expect(draft().title).toBe("Water");
    expect(draft().formula).toBe("H2O");
    expect(scenes().scene?.fragments).toHaveLength(1);
    expect(scenes().scene?.fragments[0].source).toBe("library");
    expect(scenes().log.entries[0].op).toMatchObject({ type: "restore-snapshot", source: "library" });
    expect(draft().consumedSeedId).toBe(id);
  });

  it("writes an iteration seed: the input, the title, the restored scene, the frame seed pending", () => {
    const j = job();
    expect(applySeed({ kind: "iteration", job: j }, mintSeedId())).toBe("applied");
    expect(draft().title).toBe("Ethene opt (iteration)");
    expect(draft().content).toBe(j.input_content);
    expect(draft().iterSource).toBe(j);
    expect(draft().iterFrameSeed).toBe("pending");
    expect(draft().logRejected).toBe("legacy"); // no persisted log on this job
    expect(scenes().scene?.fragments[0].atoms).toHaveLength(2);
  });

  it("writes a conformer scene as a fresh new-iteration lineage", () => {
    putFragmentInScene();
    const scene = scenes().scene!;
    useSceneStore.setState({ log: emptyLog(), scene: null });
    const seed: DraftSeed = { kind: "conformer-scene", scene, label: "conformer #3" };
    expect(applySeed(seed, mintSeedId())).toBe("applied");
    expect(scenes().scene).toEqual(scene);
    expect(scenes().log.entries).toHaveLength(1);
    expect(scenes().log.entries[0].op).toMatchObject({ source: "new-iteration" });
  });

  it("a consumed id is ignored and touches nothing (StrictMode's second mount)", () => {
    const id = mintSeedId();
    expect(applySeed(moleculeSeed, id)).toBe("applied");
    useNewJobDraft.setState({ title: "edited" });
    const scene = scenes().scene;
    const gen = draft().generation;
    expect(applySeed({ kind: "molecule", molecule: molecule({ name: "Other" }) }, id)).toBe("ignored");
    expect(draft().title).toBe("edited");
    expect(scenes().scene).toBe(scene);
    expect(draft().generation).toBe(gen);
  });

  it("an older id, after a newer one was consumed, is ignored", () => {
    const older = mintSeedId();
    const newer = mintSeedId();
    expect(applySeed(moleculeSeed, newer)).toBe("applied");
    resetDraft();
    expect(applySeed({ kind: "molecule", molecule: molecule({ name: "Late" }) }, older)).toBe("ignored");
    expect(draft().title).toBe("");
  });

  it("a non-empty draft → ask, and neither store is touched", () => {
    fillDraft();
    const before = { ...draft() };
    const log = scenes().log;
    expect(applySeed(moleculeSeed, mintSeedId())).toBe("ask");
    expect(draft()).toEqual(before);
    expect(scenes().log).toBe(log);
  });

  it("an empty draft is reset first: no stale iteration context, refusal or NEB payload survives", () => {
    // Empty by the predicate (no title, text or scene) but carrying a previous seed's leftovers.
    useNewJobDraft.setState({
      pendingNeb: { productXyz: "x", reactantJobId: "r", productJobId: "p" },
      carryRefusal: "stale refusal",
      snapshotRejected: true,
      iterSource: job(),
      iterFrameSeed: "refused",
    });
    expect(applySeed(moleculeSeed, mintSeedId())).toBe("applied");
    expect(draft().pendingNeb).toBeNull();
    expect(draft().carryRefusal).toBeNull();
    expect(draft().snapshotRejected).toBe(false);
    expect(draft().iterSource).toBeNull();
    expect(draft().iterFrameSeed).toBe("none");
  });

  it("Keep draft consumes the id: the same seed does not ask again", () => {
    fillDraft();
    const id = mintSeedId();
    expect(applySeed(moleculeSeed, id)).toBe("ask");
    const title = draft().title;
    expect(resolveSeedPrompt(moleculeSeed, id, "keep")).toBe("kept");
    expect(draft().title).toBe(title);
    expect(applySeed(moleculeSeed, id)).toBe("ignored");
  });

  it("Replace resets the work, writes the seed, keeps the preferences", () => {
    useNewJobDraft.setState(TOUCHED_PREFS);
    fillDraft();
    const id = mintSeedId();
    expect(applySeed(moleculeSeed, id)).toBe("ask");
    expect(resolveSeedPrompt(moleculeSeed, id, "replace")).toBe("replaced");
    expect(draft().title).toBe("Water");
    expect(draft().pendingNeb).toBeNull();
    expect(draft().runTarget).toBe("srv-a");
    expect(draft().groupTouched).toBe(true);
    expect(scenes().scene?.fragments[0].source).toBe("library");
    expect(resolveSeedPrompt(moleculeSeed, id, "replace")).toBe("ignored"); // a second click
  });
});

// ── "Use this conformer" from Job detail ───────────────────────────────────────

describe("handOffConformer — replace in place, or a seed that asks", () => {
  const via = { via: "conformer" as const, conformerIndex: 2, deltaEKcal: null };

  it("replace: the draft's own fragment is edited as one logged op, and no seed is handed on", () => {
    fillDraft();
    const entries = scenes().log.entries.length;
    const h = handOffConformer(
      { action: "replace", fragmentId: "f1", atoms: [{ element: "O", x: 0, y: 0, z: 0.5 }] },
      via,
      1,
      "conformer #2",
    );
    expect(h).toEqual({ ok: true, seed: null });
    expect(scenes().log.entries).toHaveLength(entries + 1);
    expect(scenes().scene?.fragments[0].atoms[0].z).toBe(0.5);
  });

  it("new: the scene is not touched; the seed meets a non-empty draft with the prompt", () => {
    fillDraft();
    const scene = scenes().scene;
    const log = scenes().log;
    const fragment = { ...scene!.fragments[0], id: "goat-frag" };
    const h = handOffConformer({ action: "new", fragment, nextAtomId: 1 }, via, 2, "conformer #2");
    expect(h.ok).toBe(true);
    expect(scenes().scene).toBe(scene);
    expect(scenes().log).toBe(log);
    if (!h.ok || !h.seed) throw new Error("expected a conformer-scene seed");
    expect(h.seed).toMatchObject({ kind: "conformer-scene", label: "conformer #2" });
    expect(applySeed(h.seed, mintSeedId())).toBe("ask");
  });

  it("refuse: its reason, nothing touched", () => {
    fillDraft();
    const log = scenes().log;
    expect(handOffConformer({ action: "refuse", reason: "composition changed" }, via, 1, "x")).toEqual({
      ok: false,
      reason: "composition changed",
    });
    expect(scenes().log).toBe(log);
  });
});

describe("JobDetailScreen's conformer call site goes through handOffConformer", () => {
  // A source-level guard (the `text-guard.test.ts` pattern): rendering JobDetailScreen for one
  // branch is heavy, and the bypass this prevents is a one-line edit there — Job detail writing the
  // scene store itself, so a `new` conformer replaces a draft with work without the prompt.
  const code = readFileSync(resolve(process.cwd(), "src/screens/JobDetailScreen.tsx"), "utf8")
    .replace(/\/\*[\s\S]*?\*\//g, "")
    .replace(/(^|[^:])\/\/.*$/gm, "$1");

  it("never seeds or installs the scene store itself", () => {
    expect(code).not.toMatch(/\.seedScene\(|\.installLog\(/);
  });

  it("hands the plan to handOffConformer and passes its seed on", () => {
    expect(code).toMatch(/handOffConformer\(/);
    expect(code).toMatch(/onUseConformer\(handoff\.seed\)/);
  });
});

// ── The iteration frame seed ───────────────────────────────────────────────────

describe("iterFrameSeed — a tracked state; only a resolved result moves it", () => {
  function seedIteration(j: Job = job()): void {
    expect(applySeed({ kind: "iteration", job: j }, mintSeedId())).toBe("applied");
  }

  it("pending survives an unmount before resolution; the next mount lands the last frame", () => {
    seedIteration();
    // Mount 1 starts the read and unmounts before it resolves: its result is never applied.
    expect(iterFrameSeedTask()).not.toBeNull();
    expect(draft().iterFrameSeed).toBe("pending");
    // The restore's notes, which a real frame supersedes.
    useNewJobDraft.setState({ snapshotRejected: true, logRejected: "log-diverged" });
    // Mount 2 runs the read again and it resolves.
    const task = iterFrameSeedTask();
    expect(task).not.toBeNull();
    expect(resolveIterFrameSeed(task!, results({ trajectory: TRAJ }))).toBe("landed");
    expect(draft().iterFrameSeed).toBe("landed");
    expect(secondAtomZ()).toBeCloseTo(1.33); // the optimized output, never the 1.40 seed
    expect(draft().selectedFrame).toBe(1);
    expect(draft().carryBanner).toMatch(/frame 1/);
    expect(draft().content).toMatch(/^# geometry: frame 1 /m);
    expect(draft().snapshotRejected).toBe(false);
    expect(draft().logRejected).toBeNull();
    // Landed never re-runs: a later mount has no task.
    expect(iterFrameSeedTask()).toBeNull();
  });

  it("a genuine single point lands with no banner and is not re-read", () => {
    seedIteration(job({ input_content: "! r2SCAN-3c\n* xyz 0 1\nC 0 0 0\nC 0 0 1.40\n*\n" }));
    const task = iterFrameSeedTask()!;
    expect(resolveIterFrameSeed(task, results())).toBe("landed");
    expect(draft().carryBanner).toBeNull();
    expect(draft().carryRefusal).toBeNull();
    expect(iterFrameSeedTask()).toBeNull();
  });

  it("an optimization with no readable trajectory is refused, never kept silently", () => {
    seedIteration();
    expect(resolveIterFrameSeed(iterFrameSeedTask()!, results())).toBe("refused");
    expect(draft().carryRefusal).toMatch(/INPUT SEED/);
    expect(iterFrameSeedTask()).toBeNull();
  });

  it("a scan is refused with its own reason", () => {
    seedIteration();
    expect(resolveIterFrameSeed(iterFrameSeedTask()!, results({ scan: {} as never }))).toBe("refused");
    expect(draft().carryRefusal).toMatch(/scan/i);
  });

  it("a result for an older generation is dropped", () => {
    seedIteration();
    const task = iterFrameSeedTask()!;
    resetDraft();
    expect(resolveIterFrameSeed(task, results({ trajectory: TRAJ }))).toBe("stale");
    expect(scenes().scene).toBeNull();
    expect(draft().carryBanner).toBeNull();
  });

  it("a frame result for an iteration that was replaced is dropped; the new seed stays pending", () => {
    seedIteration(); // iteration A
    const taskA = iterFrameSeedTask()!;
    const jobB = job({
      id: "parent-2",
      title: "N2 opt",
      input_content: "! r2SCAN-3c Opt\n* xyz 0 1\nN 0 0 0\nN 0 0 1.10\n*\n",
    });
    const seedB: DraftSeed = { kind: "iteration", job: jobB };
    const idB = mintSeedId();
    expect(applySeed(seedB, idB)).toBe("ask"); // A's draft has work
    expect(resolveSeedPrompt(seedB, idB, "replace")).toBe("replaced");
    // A's read resolves late.
    expect(resolveIterFrameSeed(taskA, results({ trajectory: TRAJ }))).toBe("stale");
    expect(draft().iterFrameSeed).toBe("pending");
    expect(draft().iterSource).toBe(jobB);
    expect(scenes().scene?.fragments[0].atoms.map((a) => a.element)).toEqual(["N", "N"]);
  });

  it("a last frame whose rows do not match its elements is refused, the input seed named", () => {
    seedIteration();
    const broken: ParsedResults["trajectory"] = {
      ...TRAJ!,
      frames: [TRAJ!.frames[0], { energy_eh: -78.2, xyz_angstrom: [[0, 0, 0]] }],
    };
    expect(resolveIterFrameSeed(iterFrameSeedTask()!, results({ trajectory: broken }))).toBe("refused");
    expect(draft().carryRefusal).toMatch(/Frame 1 .*INPUT SEED/);
    expect(draft().iterFrames).toBeNull();
    expect(draft().selectedFrame).toBeNull();
    expect(secondAtomZ()).toBeCloseTo(1.4); // the input seed, and the refusal says so
    expect(iterFrameSeedTask()).toBeNull();
  });

  it("a last frame that does not read as a geometry is refused, never landed silently", () => {
    seedIteration();
    const empty: ParsedResults["trajectory"] = {
      n_frames: 1,
      elements: [],
      frames: [{ energy_eh: null, xyz_angstrom: [] }],
    };
    expect(resolveIterFrameSeed(iterFrameSeedTask()!, results({ trajectory: empty }))).toBe("refused");
    expect(draft().carryRefusal).toMatch(/INPUT SEED/);
    expect(draft().carryBanner).toBeNull();
  });

  it("a re-pick of a frame that cannot be built keeps the frame shown and says so", () => {
    seedIteration();
    const firstBroken: ParsedResults["trajectory"] = {
      ...TRAJ!,
      frames: [{ energy_eh: -78.1, xyz_angstrom: [[0, 0, 0]] }, TRAJ!.frames[1]],
    };
    expect(resolveIterFrameSeed(iterFrameSeedTask()!, results({ trajectory: firstBroken }))).toBe("landed");
    const notice = selectIterationFrame(0);
    expect(notice).toMatch(/Frame 0 .*could not be built/);
    expect(draft().selectedFrame).toBe(1);
    expect(secondAtomZ()).toBeCloseTo(1.33);
    expect(draft().iterFrameSeed).toBe("landed");
  });

  it("a second resolution of the same seed is dropped (two mounts both resolved)", () => {
    seedIteration();
    const t1 = iterFrameSeedTask()!;
    const t2 = iterFrameSeedTask()!;
    expect(resolveIterFrameSeed(t1, results({ trajectory: TRAJ }))).toBe("landed");
    selectIterationFrame(0);
    expect(resolveIterFrameSeed(t2, results({ trajectory: TRAJ }))).toBe("stale");
    expect(secondAtomZ()).toBeCloseTo(1.4); // the user's re-pick stands
  });

  it("a re-pick reseeds from that frame and stays landed, without stacking provenance lines", () => {
    seedIteration();
    resolveIterFrameSeed(iterFrameSeedTask()!, results({ trajectory: TRAJ }));
    selectIterationFrame(0);
    expect(draft().iterFrameSeed).toBe("landed");
    expect(draft().selectedFrame).toBe(0);
    expect(secondAtomZ()).toBeCloseTo(1.4);
    expect(draft().content.match(/# geometry:/g)).toHaveLength(1);
  });
});

// ── The generation guard ───────────────────────────────────────────────────────

describe("the generation guard — a late result never lands in a replaced draft", () => {
  it("a SMILES fragment arriving after New draft is dropped", () => {
    const gen = captureGeneration();
    // …the sidecar call is awaited, and meanwhile the user presses "New draft".
    resetDraft();
    // The late write, guarded as `generateFromSmiles` will be:
    if (isCurrent(gen)) putFragmentInScene();
    expect(scenes().scene).toBeNull();
  });

  it("applying a seed is a new generation too", () => {
    const gen = captureGeneration();
    applySeed({ kind: "molecule", molecule: molecule() }, mintSeedId());
    expect(isCurrent(gen)).toBe(false);
  });

  it("with no reset in between the result is written", () => {
    const gen = captureGeneration();
    if (isCurrent(gen)) putFragmentInScene();
    expect(scenes().scene).not.toBeNull();
  });
});

// ── The create clear ───────────────────────────────────────────────────────────

describe("clearAfterCreate — clears only the draft the job was created from", () => {
  beforeEach(() => {
    useNewJobDraft.setState(TOUCHED_PREFS);
    fillDraft();
  });

  it("an unchanged draft is cleared; the preferences stay", () => {
    const start = captureCreateStart();
    expect(clearAfterCreate(start)).toEqual({ cleared: true });
    expect(draft().title).toBe("");
    expect(scenes().scene).toBeNull();
    expect(draft().runTarget).toBe("srv-a");
    expect(draft().pickedGroupId).toBe("grp-a");
  });

  it("the captured values outlive the clear", () => {
    const start = captureCreateStart();
    clearAfterCreate(start);
    expect(start.title).toBe("My TS");
    expect(start.runTarget).toBe("srv-a");
    expect(start.groupTouched).toBe(true);
    expect(start.scene).not.toBeNull();
  });

  it("an edit to the text during the create keeps the draft, with a reason", () => {
    const start = captureCreateStart();
    useNewJobDraft.setState({ content: start.content + "\n%pal nprocs 4 end\n" });
    const r = clearAfterCreate(start);
    expect(r.cleared).toBe(false);
    if (!r.cleared) expect(r.reason).toMatch(/kept in the draft/);
    expect(draft().title).toBe("My TS");
  });

  it("a title edit during the create keeps the draft", () => {
    const start = captureCreateStart();
    useNewJobDraft.setState({ title: "My TS, renamed" });
    expect(clearAfterCreate(start).cleared).toBe(false);
    expect(draft().title).toBe("My TS, renamed");
  });

  it("a geometry edit during the create keeps the draft", () => {
    const start = captureCreateStart();
    scenes().translateFragment("f1", 1, 0, 0);
    expect(clearAfterCreate(start).cleared).toBe(false);
    expect(scenes().scene).not.toBeNull();
  });

  it("a reset or seed during the create keeps the newer draft", () => {
    const start = captureCreateStart();
    resetDraft();
    useNewJobDraft.setState({ title: "the next one" });
    const r = clearAfterCreate(start);
    expect(r.cleared).toBe(false);
    if (!r.cleared) expect(r.reason).toMatch(/replaced/);
    expect(draft().title).toBe("the next one");
  });
});

// ── Re-validation of Run On and the group ──────────────────────────────────────

describe("revalidateRunTarget — a gone or disabled touched target falls back, with a notice", () => {
  const settings = { [LAST_RUN_TARGET_SETTING]: "srv-b" };
  const touched = { runTarget: "srv-a", runTargetTouched: true };

  it("a deleted profile → resolveRunTarget (the stored target) and touched reset", () => {
    const r = revalidateRunTarget(touched, { ok: true, items: [profile("srv-b", true)] }, settings);
    expect(r.changed).toBe(true);
    if (r.changed) {
      expect(r.patch).toEqual({ runTarget: "srv-b", runTargetTouched: false });
      expect(r.notice).toMatch(/server srv-b/);
    }
  });

  it("a disabled profile → Local when the stored target is not usable either", () => {
    const r = revalidateRunTarget(touched, { ok: true, items: [profile("srv-a", false)] }, null);
    expect(r).toMatchObject({ changed: true, patch: { runTarget: null, runTargetTouched: false } });
    if (r.changed) expect(r.notice).toMatch(/Local/);
  });

  it("a failed list changes nothing", () => {
    expect(revalidateRunTarget(touched, { ok: false, error: "db locked" }, settings)).toEqual({
      changed: false,
    });
  });

  it("a still-enabled target, Local, or an untouched target is left alone", () => {
    const list = { ok: true as const, items: [profile("srv-a", true)] };
    expect(revalidateRunTarget(touched, list, settings).changed).toBe(false);
    expect(revalidateRunTarget({ runTarget: null, runTargetTouched: true }, list, settings).changed).toBe(false);
    expect(
      revalidateRunTarget({ runTarget: "gone", runTargetTouched: false }, list, settings).changed,
    ).toBe(false);
  });
});

describe("revalidateGroup — a gone touched group follows the active group again, with a notice", () => {
  const touched = { pickedGroupId: "grp-a", groupTouched: true };

  it("a deleted group → untouched, the active group, a notice", () => {
    const r = revalidateGroup(touched, { ok: true, items: [group("grp-b")] }, "grp-b");
    expect(r).toMatchObject({ changed: true, patch: { pickedGroupId: "grp-b", groupTouched: false } });
    if (r.changed) expect(r.notice).toMatch(/deleted/);
  });

  it("a failed list changes nothing", () => {
    expect(revalidateGroup(touched, { ok: false, error: "db locked" }, null)).toEqual({ changed: false });
  });

  it("a still-present group, an explicit (ungrouped), or an untouched pick is left alone", () => {
    const list = { ok: true as const, items: [group("grp-a")] };
    expect(revalidateGroup(touched, list, null).changed).toBe(false);
    expect(revalidateGroup({ pickedGroupId: null, groupTouched: true }, list, null).changed).toBe(false);
    expect(revalidateGroup({ pickedGroupId: "gone", groupTouched: false }, list, null).changed).toBe(false);
  });
});

// ── A kept pick whose list is not loaded ──────────────────────────────────────

describe("unlistedPick — a kept pick is shown even when its list is not loaded", () => {
  it("a pick missing from a loading or failed list is returned (shown as its own option)", () => {
    expect(unlistedPick("p1", false, [null])).toBe("p1");
  });

  it("nothing extra for no pick, a loaded list, or a listed pick", () => {
    expect(unlistedPick(null, false, [null])).toBeNull();
    expect(unlistedPick("p1", true, [null])).toBeNull(); // loaded: re-validation owns it
    expect(unlistedPick("p1", false, [null, "p1"])).toBeNull();
  });
});

// ── The builder on open ────────────────────────────────────────────────────────

describe("builderStateOnOpen — remembered method, charge/multiplicity from the geometry", () => {
  const remembered: BuilderState = {
    ...DEFAULT_BUILDER_STATE,
    methodFamily: "dft",
    functional: "PBE0",
    basis: "def2-TZVP",
    solvationModel: "CPCM",
    solvent: "water",
    maxcore: 3000,
    nprocs: 8,
    charge: 0,
    multiplicity: 1,
  };

  it("a radical imported since the builder was last used → its doublet, not the remembered singlet", () => {
    const radical = "! r2SCAN-3c\n* xyz 0 2\nC 0 0 0\nH 1.08 0 0\nH -0.54 0.94 0\nH -0.54 -0.94 0\n*\n";
    const s = builderStateOnOpen(remembered, radical);
    expect(s.multiplicity).toBe(2);
    expect(s.charge).toBe(0);
    expect({ ...s, charge: remembered.charge, multiplicity: remembered.multiplicity }).toEqual(remembered);
  });

  it("an ion's charge comes from the header", () => {
    const s = builderStateOnOpen({ ...remembered, charge: 0 }, "* xyz -1 1\nCl 0 0 0\n*\n");
    expect(s.charge).toBe(-1);
  });

  it("no geometry header → the defaults, never the remembered values", () => {
    const s = builderStateOnOpen({ ...remembered, charge: 2, multiplicity: 3 }, "! HF\n");
    expect(s.charge).toBe(DEFAULT_BUILDER_STATE.charge);
    expect(s.multiplicity).toBe(DEFAULT_BUILDER_STATE.multiplicity);
    expect(s.functional).toBe("PBE0");
  });
});
