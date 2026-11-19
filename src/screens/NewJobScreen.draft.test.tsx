// @vitest-environment jsdom
/**
 * The New Job draft wired into the screen (ADR-026 Part B, item 12 "Component"). Monaco, 3Dmol, the
 * Tauri bridge and the sidecar are mocked; the draft store and the scene store are real. Every
 * restore gate asserts one field, so moving that field back to local `useState` turns exactly its
 * test red.
 */
import { StrictMode, type ReactElement } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";

const invokeMock = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (cmd: string, args?: Record<string, unknown>) => invokeMock(cmd, args),
}));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async () => () => {}) }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ confirm: vi.fn(), message: vi.fn() }));
vi.mock("../sidecar-client", () => ({ postSidecar: vi.fn(() => new Promise(() => {})) }));
vi.mock("../editor/InputEditor", () => ({
  InputEditor: ({ value, onChange }: { value: string; onChange: (v: string) => void }) => (
    <textarea aria-label="input editor" value={value} onChange={(e) => onChange(e.currentTarget.value)} />
  ),
}));
/** The props New Job last gave the viewer (selection, the pick callback). */
interface ViewerProps {
  selection?: readonly number[];
  onAtomPick?: (pick: { atomId: number; viewerIndex: number }) => void;
}
let viewerProps: ViewerProps = {};
vi.mock("../viewer/MoleculeViewer", () => ({
  MoleculeViewer: (props: ViewerProps) => {
    viewerProps = props;
    return <div data-testid="viewer" />;
  },
}));
const importMock = vi.fn();
vi.mock("../viewer/import-file", () => ({
  IMPORT_ACCEPT: ".xyz",
  importStructureFile: (file: File) => importMock(file),
}));

import { NewJobScreen } from "./NewJobScreen";
import {
  initialPreferences,
  initialWork,
  mintSeedId,
  useNewJobDraft,
  type DraftSeed,
} from "./new-job-draft";
import { useSceneStore } from "../scene/store";
import { emptyLog } from "../scene/oplog";
import { injectConstraints } from "../scene/constraints";
import { sceneFromOrcaInput, serializeScene } from "../scene/scene";
import { LAST_RUN_TARGET_SETTING } from "../jobs/remote";
import { setLocalMaxcore } from "../memory/local-maxcore";
import type { Group, Job, Molecule, ParsedResults, ServerProfile } from "../types";

// ── The fake bridge ────────────────────────────────────────────────────────────

interface Deferred<T> {
  promise: Promise<T>;
  resolve: (v: T) => void;
  reject: (e: unknown) => void;
}
function deferred<T>(): Deferred<T> {
  let resolve!: (v: T) => void;
  let reject!: (e: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

function profile(id: string): ServerProfile {
  return { id, name: `server ${id}`, run_target: { is_run_target: true, reason: null } } as ServerProfile;
}
const group = (id: string): Group => ({ id, name: `group ${id}`, parent_id: null, created_at: "" });

let settings: Record<string, string>;
let groups: Group[];
let createJob: (args: Record<string, unknown>) => Promise<Job>;
let moveJob: (args: Record<string, unknown>) => Promise<null>;
/** One per `read_job_results` call, in order; the test resolves the one it wants. */
let resultReads: Deferred<ParsedResults | null>[];
/** The reply to `read_job_results`; by default a deferred the test resolves (`resultReads`). */
let readResults: (args: Record<string, unknown>) => Promise<ParsedResults | null>;
let profilesFail: boolean;
let groupsFail: boolean;
let parsedJobs: Job[];
let calls: { cmd: string; args?: Record<string, unknown> }[];

beforeEach(() => {
  settings = {};
  groups = [group("g1"), group("g2")];
  createJob = async (args) => ({ id: "job-1", title: String(args.title) }) as Job;
  moveJob = async () => null;
  resultReads = [];
  readResults = () => {
    const d = deferred<ParsedResults | null>();
    resultReads.push(d);
    return d.promise;
  };
  profilesFail = false;
  groupsFail = false;
  parsedJobs = [];
  calls = [];
  viewerProps = {};
  importMock.mockReset();
  invokeMock.mockReset();
  invokeMock.mockImplementation(async (cmd: string, args?: Record<string, unknown>) => {
    calls.push({ cmd, args });
    switch (cmd) {
      case "list_server_profiles":
        if (profilesFail) throw new Error("database is locked");
        return [profile("p1"), profile("p2")];
      case "get_settings":
        return settings;
      case "list_groups":
        if (groupsFail) throw new Error("database is locked");
        return groups;
      case "list_jobs":
        return parsedJobs;
      case "get_job":
        return parsedJobs.find((j) => j.id === args?.id);
      case "list_molecules":
      case "list_reagents":
        return [];
      case "read_job_results":
        return readResults(args ?? {});
      case "create_job":
      case "create_neb_job":
        return createJob(args ?? {});
      case "move_job":
        return moveJob(args ?? {});
      case "set_setting":
        return null;
      default:
        return new Promise(() => {}); // memory_estimate & co.: never answered here
    }
  });
  useNewJobDraft.setState({ ...initialWork(), ...initialPreferences(), generation: 0, consumedSeedId: 0 });
  useSceneStore.setState({ log: emptyLog(), scene: null });
});

afterEach(() => {
  cleanup();
  setLocalMaxcore(null);
});

// ── Helpers ────────────────────────────────────────────────────────────────────

const onOpenDetail = vi.fn();
const onCreatedDraft = vi.fn();
beforeEach(() => {
  onOpenDetail.mockReset();
  onCreatedDraft.mockReset();
});

function screenEl(props: { seed?: DraftSeed; seedId?: number; activeGroupId?: string | null } = {}): ReactElement {
  return (
    <NewJobScreen
      onCreatedDraft={onCreatedDraft}
      onOpenDetail={onOpenDetail}
      activeGroupId={props.activeGroupId ?? null}
      seed={props.seed}
      seedId={props.seedId}
    />
  );
}

/** Mount New Job and let its mount reads (profiles, settings, groups) resolve. */
async function mount(props: Parameters<typeof screenEl>[0] = {}, strict = false) {
  const el = screenEl(props);
  const view = render(strict ? <StrictMode>{el}</StrictMode> : el);
  await flush();
  return view;
}

async function flush(ms = 0) {
  await act(async () => {
    await new Promise((r) => setTimeout(r, ms));
  });
}

const titleInput = () => screen.getByLabelText("Job title") as HTMLInputElement;
const editor = () => screen.getByLabelText("input editor") as HTMLTextAreaElement;
const runOn = () => screen.getByLabelText("Run on") as HTMLSelectElement;
const groupSelect = () => document.getElementById("job-group") as HTMLSelectElement;
const openBuilder = () => fireEvent.click(screen.getByRole("button", { name: /Input Builder/ }));
const dftRadio = () => screen.getByRole("radio", { name: "Functional + Basis" }) as HTMLInputElement;

/** A two-fragment geometry built in the scene store (two log entries). */
function buildTwoFragments() {
  const s = useSceneStore.getState();
  s.addFragment({
    id: "a",
    name: "diene",
    charge: 0,
    source: "editor",
    atoms: [
      { element: "C", x: 0, y: 0, z: 0 },
      { element: "C", x: 0, y: 0, z: 1.34 },
    ],
  });
  s.addFragment({ id: "b", name: "Na", charge: 1, source: "editor", atoms: [{ element: "Na", x: 5, y: 0, z: 0 }] });
}

const HE = "! r2SCAN-3c SP\n* xyz 0 1\nHe 0 0 0\n*\n";

// ── Unmount → remount restores the draft, field by field ───────────────────────

describe("a tab switch keeps the draft (unmount → remount)", () => {
  it("the title", async () => {
    const v = await mount();
    fireEvent.change(titleInput(), { target: { value: "Diels–Alder TS" } });
    v.unmount();
    await mount();
    expect(titleInput().value).toBe("Diels–Alder TS");
  });

  it("the input text", async () => {
    const v = await mount();
    fireEvent.change(editor(), { target: { value: "! B3LYP def2-SVP Opt\n" } });
    v.unmount();
    await mount();
    expect(editor().value).toBe("! B3LYP def2-SVP Opt\n");
  });

  it("the scene: its fragments and its history — the mount clears nothing", async () => {
    buildTwoFragments();
    const v = await mount();
    v.unmount();
    await mount();
    expect(useSceneStore.getState().scene?.fragments).toHaveLength(2);
    expect(useSceneStore.getState().log.entries).toHaveLength(2);
  });

  it("the builder's method", async () => {
    const v = await mount();
    openBuilder();
    fireEvent.click(dftRadio());
    v.unmount();
    await mount();
    openBuilder();
    expect(dftRadio().checked).toBe(true);
  });

  it("Run on", async () => {
    const v = await mount();
    fireEvent.change(runOn(), { target: { value: "p1" } });
    v.unmount();
    await mount();
    expect(runOn().value).toBe("p1");
    // A successful list: the target is a listed option, never the "not loaded" stand-in.
    expect(screen.queryByText(/profile list not loaded/)).toBeNull();
  });

  it("the group pick, still touched (it does not follow a new active group)", async () => {
    const v = await mount({ activeGroupId: null });
    fireEvent.change(groupSelect(), { target: { value: "g1" } });
    v.unmount();
    await mount({ activeGroupId: "g2" });
    expect(groupSelect().value).toBe("g1");
  });

  it("the constraint composition warning (ЗАХИСТ 2)", async () => {
    buildTwoFragments();
    useNewJobDraft.setState({
      content: injectConstraints("! r2SCAN-3c Opt\n", [{ kind: "distance", atoms: [0, 1] }]),
    });
    const v = await mount();
    act(() => useSceneStore.getState().removeFragment("b"));
    fireEvent.click(screen.getByTitle("Constraints"));
    expect(screen.getByText(/The scene composition changed/)).toBeTruthy();
    v.unmount();
    await mount();
    fireEvent.click(screen.getByTitle("Constraints"));
    expect(screen.queryByText(/The scene composition changed/)).not.toBeNull();
  });

  it("the SMILES text", async () => {
    const v = await mount();
    fireEvent.change(screen.getByPlaceholderText("SMILES, e.g. CCO"), { target: { value: "C=CC=C" } });
    v.unmount();
    await mount();
    expect((screen.getByPlaceholderText("SMILES, e.g. CCO") as HTMLInputElement).value).toBe("C=CC=C");
  });

  it("an AtomId-keyed display choice (show cation coordinate bonds)", async () => {
    buildTwoFragments();
    const v = await mount();
    fireEvent.click(screen.getByTitle("Edit geometry"));
    fireEvent.click(screen.getByLabelText("Show cation coordinate bonds"));
    v.unmount();
    await mount();
    fireEvent.click(screen.getByTitle("Edit geometry"));
    expect((screen.getByLabelText("Show cation coordinate bonds") as HTMLInputElement).checked).toBe(true);
  });

  it("the Replace-input buffer: still unlocked, not reverted, not adopted", async () => {
    const v = await mount();
    fireEvent.click(screen.getByText("Replace input…"));
    fireEvent.click(screen.getByText("Unlock editor"));
    fireEvent.change(editor(), { target: { value: HE } });
    v.unmount();
    await mount();
    await flush(600); // past the content→scene debounce
    expect(editor().value).toBe(HE);
    expect(screen.queryByText("Adopt input")).not.toBeNull();
    expect(useSceneStore.getState().scene).toBeNull();
  });

  it("the Replace-input snapshot: Cancel after the return restores the text from before the unlock", async () => {
    useNewJobDraft.setState({ content: "! r2SCAN-3c Opt Freq\n" });
    const v = await mount();
    fireEvent.click(screen.getByText("Replace input…"));
    fireEvent.click(screen.getByText("Unlock editor"));
    fireEvent.change(editor(), { target: { value: HE } });
    v.unmount();
    await mount();
    const replaceBanner = screen.getByText("Adopt input").parentElement!;
    fireEvent.click(Array.from(replaceBanner.querySelectorAll("button")).find((b) => b.textContent === "Cancel")!);
    expect(editor().value).toBe("! r2SCAN-3c Opt Freq\n");
  });
});

// ── The builder ────────────────────────────────────────────────────────────────

describe("the builder remembers", () => {
  it("keeps the method after Generate closes the accordion", async () => {
    await mount();
    openBuilder();
    fireEvent.click(dftRadio());
    fireEvent.click(screen.getByText("Generate Input"));
    expect(screen.queryByRole("radio", { name: "Functional + Basis" })).toBeNull(); // closed
    openBuilder();
    expect(dftRadio().checked).toBe(true);
  });
});

// ── Create ─────────────────────────────────────────────────────────────────────

describe("create clears the work, keeps the preferences, uses the values it started with", () => {
  async function fillAndPick() {
    await mount();
    fireEvent.change(titleInput(), { target: { value: "He SP" } });
    fireEvent.change(editor(), { target: { value: HE } });
    fireEvent.change(runOn(), { target: { value: "p1" } });
    fireEvent.change(groupSelect(), { target: { value: "g1" } });
  }

  it("a successful Create & Run clears the work and keeps Run on and the group", async () => {
    await fillAndPick();
    fireEvent.click(screen.getByText("Create & Run"));
    await flush();
    expect(onOpenDetail).toHaveBeenCalledWith("job-1", true, "p1");
    expect(calls.find((c) => c.cmd === "move_job")?.args).toEqual({ jobId: "job-1", groupId: "g1" });
    expect(calls.find((c) => c.cmd === "set_setting")?.args).toEqual({ key: LAST_RUN_TARGET_SETTING, value: "p1" });
    expect(titleInput().value).toBe("");
    expect(editor().value).toBe("");
    expect(runOn().value).toBe("p1");
    expect(groupSelect().value).toBe("g1");
  });

  it("Run on and the group changed while the create was awaited: the job goes where it was sent", async () => {
    const pending = deferred<Job>();
    createJob = () => pending.promise;
    await fillAndPick();
    fireEvent.click(screen.getByText("Create & Run"));
    fireEvent.change(runOn(), { target: { value: "p2" } });
    fireEvent.change(groupSelect(), { target: { value: "g2" } });
    await act(async () => pending.resolve({ id: "job-1", title: "He SP" } as Job));
    await flush();
    expect(onOpenDetail).toHaveBeenCalledWith("job-1", true, "p1");
    expect(calls.find((c) => c.cmd === "move_job")?.args).toEqual({ jobId: "job-1", groupId: "g1" });
    expect(calls.find((c) => c.cmd === "set_setting")?.args).toEqual({ key: LAST_RUN_TARGET_SETTING, value: "p1" });
  });

  it("a failed create keeps the draft", async () => {
    createJob = async () => {
      throw new Error("disk full");
    };
    await fillAndPick();
    fireEvent.click(screen.getByText("Create & Run"));
    await flush();
    expect(titleInput().value).toBe("He SP");
    expect(editor().value).toContain("He 0 0 0");
    expect(onOpenDetail).not.toHaveBeenCalled();
  });

  it("a failed group move after the create names the job, says it was not run, and still clears", async () => {
    moveJob = async () => {
      throw new Error("group gone");
    };
    await fillAndPick();
    fireEvent.click(screen.getByText("Create & Run"));
    await flush();
    expect(screen.getByText(/“He SP” was created.*NOT run/)).toBeTruthy();
    expect(onOpenDetail).not.toHaveBeenCalled();
    expect(titleInput().value).toBe("");
  });
});

// ── Late results and re-validation ─────────────────────────────────────────────

describe("an async result never lands in a newer draft; a gone Run on or group is re-validated", () => {
  it("a SMILES structure arriving after New draft is dropped, with a notice", async () => {
    const reply = deferred<Response>();
    vi.stubGlobal("fetch", vi.fn(() => reply.promise));
    const base = invokeMock.getMockImplementation()!;
    invokeMock.mockImplementation(async (cmd: string, args?: Record<string, unknown>) =>
      cmd === "get_sidecar_status" ? { status: "healthy", port: 8765 } : base(cmd, args),
    );
    try {
      await mount();
      fireEvent.change(screen.getByPlaceholderText("SMILES, e.g. CCO"), { target: { value: "O" } });
      fireEvent.click(screen.getByText("Generate 3D"));
      await flush();
      fireEvent.click(screen.getByText("New draft")); // empty draft (SMILES text is not work): at once
      await act(async () =>
        reply.resolve({
          ok: true,
          json: async () => ({ xyz: "1\nwater\nO 0 0 0\n", charge: 0, formula: "H2O" }),
        } as Response),
      );
      await flush();
      expect(useSceneStore.getState().scene).toBeNull();
      expect(screen.getByText(/arrived after the draft was replaced/)).toBeTruthy();
    } finally {
      vi.unstubAllGlobals();
    }
  });

  it("a touched Run on whose profile was deleted while away falls back to Local, with a notice", async () => {
    const v = await mount();
    fireEvent.change(runOn(), { target: { value: "p2" } });
    v.unmount();
    const base = invokeMock.getMockImplementation()!;
    invokeMock.mockImplementation(async (cmd: string, args?: Record<string, unknown>) =>
      cmd === "list_server_profiles" ? [profile("p1")] : base(cmd, args),
    );
    await mount();
    expect(useNewJobDraft.getState().runTarget).toBeNull();
    expect(useNewJobDraft.getState().runTargetTouched).toBe(false);
    expect(screen.getByText(/deleted or disabled in Settings; Run on is now Local/)).toBeTruthy();
  });

  it("a touched group deleted while away follows the active group again, with a notice", async () => {
    const v = await mount({ activeGroupId: "g1" });
    fireEvent.change(groupSelect(), { target: { value: "g2" } });
    v.unmount();
    groups = [group("g1")];
    await mount({ activeGroupId: "g1" });
    expect(groupSelect().value).toBe("g1");
    expect(useNewJobDraft.getState().groupTouched).toBe(false);
    expect(screen.getByText(/group chosen for the new job was deleted/)).toBeTruthy();
  });
});

// ── "New draft" ────────────────────────────────────────────────────────────────

describe("New draft clears the work; untouched preferences keep following their defaults", () => {
  it("asks on a draft with work, then Run on = last_run_target, group = the active group, %maxcore = its default", async () => {
    settings = { [LAST_RUN_TARGET_SETTING]: "p1" };
    setLocalMaxcore(1600);
    await mount({ activeGroupId: "g2" });
    expect(runOn().value).toBe("p1");
    fireEvent.change(titleInput(), { target: { value: "scratch" } });
    fireEvent.click(screen.getByText("New draft"));
    fireEvent.click(screen.getByText("Clear the draft"));
    expect(titleInput().value).toBe("");
    expect(runOn().value).toBe("p1");
    expect(groupSelect().value).toBe("g2");
    // %maxcore stays untouched, so it follows Run on: a server's 2000, this machine's fitted value.
    openBuilder();
    const maxcore = () => (screen.getByLabelText("maxcore (MB)") as HTMLInputElement).value;
    expect(maxcore()).toBe("2000");
    fireEvent.change(runOn(), { target: { value: "" } });
    expect(maxcore()).toBe("1600");
  });

  it("clears an empty draft at once (no prompt)", async () => {
    useNewJobDraft.setState({ smiles: "CCO" }); // not work: the draft is empty
    await mount();
    fireEvent.click(screen.getByText("New draft"));
    expect(screen.queryByText("Clear the draft")).toBeNull();
    expect(useNewJobDraft.getState().smiles).toBe("");
  });
});

// ── Seeds ──────────────────────────────────────────────────────────────────────

const WATER: Molecule = {
  id: "mol-1",
  name: "Water",
  formula: "H2O",
  xyz: "3\nwater\nO 0 0 0\nH 0.96 0 0\nH -0.24 0.93 0\n",
  charge: 0,
  multiplicity: 1,
  tags: "",
  created_at: "",
  is_reagent: false,
};

describe("a seed meeting a draft with work asks first", () => {
  it("a molecule: the prompt; Keep draft leaves both stores as they were", async () => {
    buildTwoFragments();
    useNewJobDraft.setState({ title: "my TS" });
    const scene = useSceneStore.getState().scene;
    await mount({ seed: { kind: "molecule", molecule: WATER }, seedId: mintSeedId() });
    expect(screen.getByText(/Replace the current draft with the molecule “Water”/)).toBeTruthy();
    expect(useSceneStore.getState().scene).toBe(scene);
    fireEvent.click(screen.getByText("Keep draft"));
    expect(titleInput().value).toBe("my TS");
    expect(useSceneStore.getState().scene).toBe(scene);
    expect(screen.queryByText(/Replace the current draft/)).toBeNull();
  });

  it("a molecule: Replace writes the seed", async () => {
    useNewJobDraft.setState({ title: "my TS" });
    await mount({ seed: { kind: "molecule", molecule: WATER }, seedId: mintSeedId() });
    fireEvent.click(screen.getByText("Replace"));
    expect(titleInput().value).toBe("Water");
    expect(useSceneStore.getState().scene?.fragments[0].atoms).toHaveLength(3);
  });

  it("a conformer scene (the `new` branch): the prompt, the draft's scene untouched", async () => {
    buildTwoFragments();
    const scene = useSceneStore.getState().scene!;
    const conformer = { fragments: [scene.fragments[0]], multiplicity: 1, nextAtomId: 2 };
    await mount({
      seed: { kind: "conformer-scene", scene: conformer, label: "conformer 3 of “GOAT”" },
      seedId: mintSeedId(),
    });
    expect(screen.getByText(/Replace the current draft with conformer 3/)).toBeTruthy();
    expect(useSceneStore.getState().scene).toBe(scene);
  });
});

// ── StrictMode and the iteration frame seed ────────────────────────────────────

const PARENT: Job = {
  id: "parent-1",
  title: "Ethene opt",
  status: "parsed",
  input_content: "! r2SCAN-3c Opt\n* xyz 0 1\nC 0 0 0\nC 0 0 1.40\n*\n",
  scene_json: null,
  scene_log_json: null,
} as Job;

const OPT_RESULTS = {
  converged: true,
  final_geometry: { elements: ["C", "C"], xyz_angstrom: [[0, 0, 0], [0, 0, 1.33]] },
  trajectory: {
    n_frames: 2,
    elements: ["C", "C"],
    frames: [
      { energy_eh: -78.1, xyz_angstrom: [[0, 0, 0], [0, 0, 1.4]] },
      { energy_eh: -78.2, xyz_angstrom: [[0, 0, 0], [0, 0, 1.33]] },
    ],
  },
  scan: null,
  neb: null,
} as unknown as ParsedResults;

const secondAtomZ = () => useSceneStore.getState().scene?.fragments[0]?.atoms[1]?.z;

describe("inside <StrictMode> (mount effects run twice)", () => {
  it("a seed on an empty draft is applied once, with no prompt", async () => {
    await mount({ seed: { kind: "molecule", molecule: WATER }, seedId: mintSeedId() }, true);
    expect(screen.queryByText(/Replace the current draft/)).toBeNull();
    expect(titleInput().value).toBe("Water");
    expect(useSceneStore.getState().scene?.fragments).toHaveLength(1);
  });

  it("an iteration seed lands the last frame with its banner", async () => {
    await mount({ seed: { kind: "iteration", job: PARENT }, seedId: mintSeedId() }, true);
    expect(resultReads.length).toBeGreaterThan(0);
    await act(async () => resultReads.forEach((d) => d.resolve(OPT_RESULTS)));
    await flush();
    expect(secondAtomZ()).toBeCloseTo(1.33);
    expect(screen.getByText(/Seeded from final \(converged\) \(frame 1\)/)).toBeTruthy();
  });

  it("leaving before read_job_results answers, then returning, still lands the frame", async () => {
    const v = await mount({ seed: { kind: "iteration", job: PARENT }, seedId: mintSeedId() }, true);
    const before = resultReads.length;
    v.unmount();
    await act(async () => resultReads.forEach((d) => d.resolve(OPT_RESULTS))); // too late: ignored
    expect(secondAtomZ()).toBeCloseTo(1.4); // still the input seed
    await mount({}, true); // the tab button: no seed
    expect(resultReads.length).toBeGreaterThan(before);
    await act(async () => resultReads.slice(before).forEach((d) => d.resolve(OPT_RESULTS)));
    await flush();
    expect(secondAtomZ()).toBeCloseTo(1.33);
    expect(useNewJobDraft.getState().iterFrameSeed).toBe("landed");
  });
});

// ── Review fixes (Part B, round 2) ─────────────────────────────────────────────

/** The input or select of a builder field whose `<label>` is not tied to it by `htmlFor`. */
function builderField(label: string): HTMLInputElement | HTMLSelectElement {
  const el = screen.getByText(label, { selector: "label" }).parentElement!.querySelector("input, select");
  if (!el) throw new Error(`no field under the label ${label}`);
  return el as HTMLInputElement | HTMLSelectElement;
}

const METHYL = "! r2SCAN-3c Opt\n* xyz 0 2\nC 0 0 0\nH 1.08 0 0\nH -0.54 0.94 0\nH -0.54 -0.94 0\n*\n";

describe("the builder re-reads charge and multiplicity from the geometry on open (rule #9)", () => {
  it("a doublet geometry → 2 shown and generated, over a remembered singlet", async () => {
    useNewJobDraft.setState({ content: METHYL, builder: { ...useNewJobDraft.getState().builder, multiplicity: 1 } });
    useSceneStore.getState().seedScene(sceneFromOrcaInput(METHYL), "text-adopt");
    await mount();
    openBuilder();
    expect(builderField("Multiplicity").value).toBe("2");
    fireEvent.click(screen.getByText("Generate Input"));
    expect(editor().value).toContain("* xyz 0 2");
  });
});

const NO_TRAJECTORY = { ...OPT_RESULTS, trajectory: null } as ParsedResults;

describe("the iteration's refusals stay visible across a tab switch (debugging/022)", () => {
  it("an Opt parent with no readable trajectory: the INPUT SEED refusal, before and after the return", async () => {
    const v = await mount({ seed: { kind: "iteration", job: PARENT }, seedId: mintSeedId() });
    await act(async () => resultReads.forEach((d) => d.resolve(NO_TRAJECTORY)));
    await flush();
    expect(screen.getByText(/INPUT SEED/)).toBeTruthy();
    v.unmount();
    await mount();
    expect(screen.getByText(/INPUT SEED/)).toBeTruthy();
  });

  it("a rejected fragment snapshot: its banner, before and after the return", async () => {
    const otherGeometry = sceneFromOrcaInput("* xyz 0 1\nC 0 0 0\nC 0 0 2.0\n*\n")!;
    const job = { ...PARENT, scene_json: serializeScene(otherGeometry) } as Job;
    const v = await mount({ seed: { kind: "iteration", job }, seedId: mintSeedId() });
    expect(screen.getByText(/didn.t match its input/)).toBeTruthy();
    v.unmount();
    await mount();
    expect(screen.getByText(/didn.t match its input/)).toBeTruthy();
  });

  it("a frame re-pick that cannot be built keeps the frame shown and says so", async () => {
    const firstBroken = {
      ...OPT_RESULTS,
      trajectory: {
        ...OPT_RESULTS.trajectory!,
        frames: [{ energy_eh: -78.1, xyz_angstrom: [[0, 0, 0]] }, OPT_RESULTS.trajectory!.frames[1]],
      },
    } as ParsedResults;
    await mount({ seed: { kind: "iteration", job: PARENT }, seedId: mintSeedId() });
    await act(async () => resultReads.forEach((d) => d.resolve(firstBroken)));
    await flush();
    fireEvent.change(document.getElementById("iter-frame")!, { target: { value: "0" } });
    expect(screen.getByText(/Frame 0 .*could not be built/)).toBeTruthy();
    expect(secondAtomZ()).toBeCloseTo(1.33);
  });
});

describe("a kept target is shown as itself while its list is not loaded (ADR-026 item 9)", () => {
  it("Run on: a failed profile list keeps the touched target and shows it, not Local", async () => {
    useNewJobDraft.setState({ runTarget: "p1", runTargetTouched: true });
    profilesFail = true;
    await mount();
    expect(runOn().value).toBe("p1");
    expect(screen.getByText("p1 — profile list not loaded")).toBeTruthy();
    expect(useNewJobDraft.getState()).toMatchObject({ runTarget: "p1", runTargetTouched: true });
  });

  it("the group: a failed group list keeps the touched pick and shows it, not (ungrouped)", async () => {
    useNewJobDraft.setState({ pickedGroupId: "g9", groupTouched: true });
    groupsFail = true;
    await mount();
    expect(groupSelect().value).toBe("g9");
    expect(screen.getByText("g9 — group list not loaded")).toBeTruthy();
    expect(useNewJobDraft.getState()).toMatchObject({ pickedGroupId: "g9", groupTouched: true });
  });
});

describe("the remaining draft fields survive a tab switch", () => {
  const H2_INPUT = "! XTB Opt\n* xyz 0 1\nH 0 0 0\nH 0 0 0.74\n*\n";
  const h2 = (z: number) =>
    ({ converged: true, final_geometry: { elements: ["H", "H"], xyz_angstrom: [[0, 0, 0], [0, 0, z]] } }) as ParsedResults;
  const nebSelects = () =>
    Array.from(document.querySelectorAll("select")).filter((s) => s.options[0]?.text === "— pick a parsed job —");

  it("a generated NEB payload and its endpoint picks: Create after the return goes to create_neb_job", async () => {
    parsedJobs = [
      { id: "r", title: "H2 short", status: "parsed", input_content: H2_INPUT } as Job,
      { id: "p", title: "H2 long", status: "parsed", input_content: H2_INPUT } as Job,
    ];
    readResults = async (args) => (args.id === "r" ? h2(0.74) : h2(1.5));
    const v = await mount();
    openBuilder();
    fireEvent.change(builderField("Job type"), { target: { value: "NEB-TS" } });
    await flush();
    fireEvent.change(nebSelects()[0], { target: { value: "r" } });
    fireEvent.change(nebSelects()[1], { target: { value: "p" } });
    fireEvent.click(screen.getByText("Generate NEB input"));
    await flush();
    expect(editor().value).toMatch(/NEB/);
    v.unmount();
    await mount();
    openBuilder();
    await flush();
    expect(nebSelects().map((s) => s.value)).toEqual(["r", "p"]);
    fireEvent.click(screen.getByText("Create Job"));
    await flush();
    expect(calls.find((c) => c.cmd === "create_neb_job")?.args).toMatchObject({ reactantJobId: "r", productJobId: "p" });
    expect(calls.some((c) => c.cmd === "create_job")).toBe(false);
  });

  it("a typed %maxcore (touched) and the nprocs text", async () => {
    const v = await mount();
    openBuilder();
    fireEvent.change(screen.getByLabelText("maxcore (MB)"), { target: { value: "3000" } });
    fireEvent.change(screen.getByLabelText("nprocs"), { target: { value: "8" } });
    v.unmount();
    await mount();
    openBuilder();
    expect((screen.getByLabelText("maxcore (MB)") as HTMLInputElement).value).toBe("3000");
    expect((screen.getByLabelText("nprocs") as HTMLInputElement).value).toBe("8");
  });
});

describe("a structure-file import is bound to the draft it started in", () => {
  const fileInput = () => document.querySelector('input[type="file"]') as HTMLInputElement;
  const WATER_LINES = { atomLines: ["O 0 0 0", "H 0.96 0 0", "H -0.24 0.93 0"] };

  it("a file arriving after New draft is dropped, with a notice", async () => {
    const pending = deferred<{ atomLines: string[] }>();
    importMock.mockReturnValue(pending.promise);
    await mount();
    fireEvent.change(fileInput(), { target: { files: [new File(["x"], "water.xyz")] } });
    fireEvent.click(screen.getByText("New draft")); // empty draft: cleared at once
    await act(async () => pending.resolve(WATER_LINES));
    await flush();
    expect(useSceneStore.getState().scene).toBeNull();
    expect(screen.getByText(/“water.xyz” finished loading after the draft was replaced/)).toBeTruthy();
  });

  it("a title typed while the file loads is kept (read from the draft after the await)", async () => {
    const pending = deferred<{ atomLines: string[] }>();
    importMock.mockReturnValue(pending.promise);
    await mount();
    fireEvent.change(fileInput(), { target: { files: [new File(["x"], "water.xyz")] } });
    fireEvent.change(titleInput(), { target: { value: "my water" } });
    await act(async () => pending.resolve(WATER_LINES));
    await flush();
    expect(useSceneStore.getState().scene?.fragments).toHaveLength(1);
    expect(titleInput().value).toBe("my water");
  });
});

describe("a new generation clears the previous job's transient view state", () => {
  it("the atom selection does not carry into a replacing seed (its atom ids exist there too)", async () => {
    const v = await mount({ seed: { kind: "molecule", molecule: WATER }, seedId: mintSeedId() });
    act(() => viewerProps.onAtomPick?.({ atomId: 0, viewerIndex: 0 }));
    expect(viewerProps.selection).toEqual([0]);
    v.rerender(screenEl({ seed: { kind: "molecule", molecule: { ...WATER, name: "Water 2" } }, seedId: mintSeedId() }));
    fireEvent.click(screen.getByText("Replace"));
    await flush();
    expect(titleInput().value).toBe("Water 2");
    expect(viewerProps.selection).toEqual([]);
  });
});
