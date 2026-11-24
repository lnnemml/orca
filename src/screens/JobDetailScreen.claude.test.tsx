// @vitest-environment jsdom
/**
 * ADR-027 M4b Part B at screen level: the job detail's Claude marks. The badge for a job Claude made,
 * a PROPOSED draft's reasons (the whole list, or the raw text when unreadable, never a crash), and the
 * auto-run submit that must not fire for a draft Claude is still submitting. Rendered whole with
 * `invoke` mocked, like `JobDetailScreen.memory.test.tsx`; the heavy children are stubs.
 *
 * Render correctness (where the badge sits, how the banner looks) is Anton's live gate; these pin
 * behaviour only.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, render, screen, waitFor } from "@testing-library/react";

import type { Job } from "../types";

const invokeMock = vi.fn<(cmd: string, args?: Record<string, unknown>) => Promise<unknown>>();
vi.mock("@tauri-apps/api/core", () => ({ invoke: (cmd: string, args?: Record<string, unknown>) => invokeMock(cmd, args) }));
vi.mock("@tauri-apps/api/event", () => ({ listen: async () => () => {} }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ confirm: async () => true, message: async () => {}, open: async () => null }));
vi.mock("../viewer/MoleculeViewer", () => ({ MoleculeViewer: () => null }));
vi.mock("./OutputViewer", () => ({ OutputViewer: () => null }));
vi.mock("../convergence/ConvergenceDashboard", () => ({ ConvergenceDashboard: () => null }));

import { JobDetailScreen } from "./JobDetailScreen";

function job(over: Partial<Job> = {}): Job {
  return {
    id: "j1",
    title: "a job",
    input_content: "! HF",
    status: "draft",
    job_dir: null,
    energy: null,
    wall_time: null,
    error_message: null,
    created_at: "2026-11-23",
    started_at: null,
    completed_at: null,
    scene_json: null,
    scene_log_json: null,
    pathway_id: null,
    group_id: null,
    backend_id: null,
    remote_host: null,
    remote_job_dir: null,
    remote_socket: null,
    cancel_requested_at: null,
    cancel_sent_at: null,
    submit_pending_since: null,
    origin: "user",
    mcp_reasons: null,
    grant_id: null,
    mcp_state: null,
    mcp_target: null,
    ...over,
  };
}

const PROPOSED = { origin: "mcp", grant_id: null, mcp_state: null } as const;
const SUBMITTING = { origin: "mcp", grant_id: "g1", mcp_state: "submitting" } as const;
const REASONS = JSON.stringify([
  { row: "P5", message: "memory is over the limit" },
  { row: "P7", message: "server is full" },
]);

function answer(row: Job) {
  invokeMock.mockImplementation(async (cmd) => {
    switch (cmd) {
      case "get_job":
        return row;
      case "remote_job_state":
        return { seq: 0, state: null };
      case "read_job_output":
      case "read_job_convergence":
        return [];
      case "submit_job":
        return null;
      default:
        return null;
    }
  });
}

const screenFor = (autoRun: boolean) => (
  <JobDetailScreen
    jobId="j1"
    autoRun={autoRun}
    runTarget={null}
    onBack={() => {}}
    onIterate={() => {}}
    onUseConformer={() => {}}
    onOpenJob={() => {}}
  />
);

const submitCalls = () => invokeMock.mock.calls.filter(([cmd]) => cmd === "submit_job");

beforeEach(() => {
  invokeMock.mockReset();
});
afterEach(cleanup);

describe("JobDetailScreen: the Claude badge", () => {
  // CONTROL: make `isClaudeJob` true for every job → the user job shows the badge → red.
  it("shows it for a job Claude created and not for a user job", async () => {
    answer(job({ ...PROPOSED }));
    const first = render(screenFor(false));
    await waitFor(() => expect(screen.getByTestId("claude-badge").textContent).toBe("Claude"));
    first.unmount();
    answer(job());
    render(screenFor(false));
    await waitFor(() => expect(screen.getByText("draft")).toBeTruthy());
    expect(screen.queryByTestId("claude-badge")).toBeNull();
  });
});

describe("JobDetailScreen: a PROPOSED draft's reasons", () => {
  // CONTROL: show the first reason only (not the list) → the second line is missing → red.
  it("lists every reason", async () => {
    answer(job({ ...PROPOSED, mcp_reasons: REASONS }));
    render(screenFor(false));
    const box = await screen.findByTestId("proposed-reasons");
    expect(Array.from(box.querySelectorAll("li")).map((li) => li.textContent)).toEqual([
      "P5: memory is over the limit",
      "P7: server is full",
    ]);
  });

  // CONTROL: let `parseJobReasons` throw on bad JSON (drop its try/catch) → the screen crashes → red.
  it.each(["not json {", '{"row":"P5"}', '[{"row":5}]', "null"])("shows malformed reasons %j as unreadable, with the raw text, and does not crash", async (raw) => {
    answer(job({ ...PROPOSED, mcp_reasons: raw }));
    render(screenFor(false));
    const box = await screen.findByTestId("proposed-reasons");
    expect(box.textContent).toContain("reasons unreadable");
    expect(box.textContent).toContain(raw);
  });

  // CONTROL: drop the `mcp_state === null` conjunct of `isProposed` → the submitting draft shows its reasons → red.
  it("shows nothing for a draft Claude is still submitting, even one that carries reasons", async () => {
    answer(job({ ...SUBMITTING, mcp_reasons: REASONS }));
    render(screenFor(false));
    await waitFor(() => expect(screen.getByTestId("claude-badge")).toBeTruthy());
    expect(screen.queryByTestId("proposed-reasons")).toBeNull();
  });

  it("shows nothing for a job that is not PROPOSED, or one with no reasons", async () => {
    answer(job({ ...PROPOSED, status: "completed", mcp_reasons: REASONS }));
    const first = render(screenFor(false));
    await waitFor(() => expect(screen.getByTestId("claude-badge")).toBeTruthy());
    expect(screen.queryByTestId("proposed-reasons")).toBeNull();
    first.unmount();
    answer(job({ ...PROPOSED, mcp_reasons: null }));
    render(screenFor(false));
    await waitFor(() => expect(screen.getByTestId("claude-badge")).toBeTruthy());
    expect(screen.queryByTestId("proposed-reasons")).toBeNull();
  });
});

describe("JobDetailScreen: the auto-run submit and a submitting draft (ADR-027 M4, item 8)", () => {
  // CONTROL: drop the `autoRunRefusal` check from `submit` → `submit_job` is sent for the submitting draft → red.
  it("does not submit a draft Claude is still submitting, and says why", async () => {
    answer(job({ ...SUBMITTING }));
    render(screenFor(true));
    await waitFor(() => expect(screen.getByText(/Claude is submitting/)).toBeTruthy());
    expect(submitCalls()).toEqual([]);
  });

  // The guard is not a blanket: a plain draft still submits on auto-run.
  it("still submits an ordinary draft, and a PROPOSED one", async () => {
    answer(job());
    const first = render(screenFor(true));
    await waitFor(() => expect(submitCalls()).toHaveLength(1));
    first.unmount();
    invokeMock.mockClear();
    // A PROPOSED draft Claude assembled for this machine (M4c: its stored target is `local`).
    answer(job({ ...PROPOSED, mcp_target: "local", mcp_reasons: REASONS }));
    render(screenFor(true));
    await waitFor(() => expect(submitCalls()).toHaveLength(1));
  });
});

describe("JobDetailScreen: the auto-run submit of a Claude draft keeps its target (ADR-027 M4c, item 3)", () => {
  const forTarget = (runTarget: string | null) => (
    <JobDetailScreen
      jobId="j1"
      autoRun
      runTarget={runTarget}
      onBack={() => {}}
      onIterate={() => {}}
      onUseConformer={() => {}}
      onOpenJob={() => {}}
    />
  );

  // CONTROL: call `autoRunRefusal` without the screen's `runTarget` (null) -> the uni draft opened
  // for uni is declined -> red; with it but the comparison dropped -> the local open submits -> red.
  it("declines a uni draft opened to run locally, and submits it on uni", async () => {
    answer(job({ ...PROPOSED, mcp_target: "p-uni" }));
    const local = render(forTarget(null));
    await waitFor(() => expect(screen.getByText(/Claude assembled this job for the server profile p-uni/)).toBeTruthy());
    expect(submitCalls()).toEqual([]);
    local.unmount();
    invokeMock.mockClear();
    answer(job({ ...PROPOSED, mcp_target: "p-uni" }));
    render(forTarget("p-uni"));
    await waitFor(() => expect(submitCalls(), "[ctl:ui_detail_target]").toHaveLength(1));
    expect(submitCalls()[0][1]).toEqual({ id: "j1", target: "p-uni" });
  });
});
