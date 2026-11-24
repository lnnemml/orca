// @vitest-environment jsdom
/**
 * Unit 5.5 at screen level (Phase 5 follow-up, 5.5 F1): the job detail screen shows the submit's
 * memory notice beside the `%pal` notice after Create & Run (the auto-run submit) and after Retry
 * submit (ADR-024 o26.6). The screen is rendered whole with `invoke` mocked; the heavy children
 * (3Dmol viewer, Monaco output viewer, the convergence charts) are stubs.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";

import type { Job, RemoteStateRead, SubmitResponse } from "../types";

const invokeMock = vi.fn<(cmd: string, args?: Record<string, unknown>) => Promise<unknown>>();
vi.mock("@tauri-apps/api/core", () => ({ invoke: (cmd: string, args?: Record<string, unknown>) => invokeMock(cmd, args) }));
vi.mock("@tauri-apps/api/event", () => ({ listen: async () => () => {} }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ confirm: async () => true, message: async () => {}, open: async () => null }));
vi.mock("../viewer/MoleculeViewer", () => ({ MoleculeViewer: () => null }));
vi.mock("./OutputViewer", () => ({ OutputViewer: () => null }));
vi.mock("../convergence/ConvergenceDashboard", () => ({ ConvergenceDashboard: () => null }));

import { JobDetailScreen } from "./JobDetailScreen";

const PAL_NOTICE = "[OrcaStudio] %pal nprocs aligned to 24";
const MEMORY_NOTICE = "[OrcaStudio] memory: the job's estimate is 98.5 GiB, over 0.9 × 40.0 GiB available";

function job(over: Partial<Job> = {}): Job {
  return {
    id: "j1",
    title: "remote job",
    input_content: "! HF",
    status: "draft",
    job_dir: null,
    energy: null,
    wall_time: null,
    error_message: null,
    created_at: "2026-11-19",
    started_at: null,
    completed_at: null,
    scene_json: null,
    scene_log_json: null,
    pathway_id: null,
    group_id: null,
    backend_id: "p1",
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

function response(over: Partial<SubmitResponse> = {}): SubmitResponse {
  return {
    outcome: { outcome: "enqueued", tsp_id: 3 },
    pal: {} as SubmitResponse["pal"],
    notice: PAL_NOTICE,
    memory_notice: MEMORY_NOTICE,
    failure: null,
    ...over,
  } as SubmitResponse;
}

/** A remote queued row the server says is not on it: `remoteView` offers Retry submit. */
const REMOTE_NOT_ON_SERVER = job({
  status: "queued",
  remote_host: "uni",
  remote_job_dir: "/home/anton/.orcastudio/jobs/j1",
  remote_socket: "/tmp/ts-slot-0",
});
const NOT_ON_SERVER: RemoteStateRead = {
  seq: 1,
  state: {
    known: { kind: "labelled", detail: "not_on_server" },
    check_error: null,
    fetch: { strikes: 0, stopped: false },
    last_contact: null,
    cancel_sends: { failures: 0, stopped: false },
    sweeps: { count: 0, stopped: false },
    pending_submit: null,
    undecided: null,
    re_enqueue_waiting: null,
  },
};

function answer(row: Job, submit: Partial<SubmitResponse> = {}) {
  invokeMock.mockImplementation(async (cmd) => {
    switch (cmd) {
      case "get_job":
        return row;
      case "remote_job_state":
        return row.remote_host ? NOT_ON_SERVER : { seq: 0, state: null };
      case "submit_job":
      case "retry_remote_submit":
        return response(submit);
      case "read_job_results":
      case "read_job_output":
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
    runTarget="p1"
    onBack={() => {}}
    onIterate={() => {}}
    onUseConformer={() => {}}
    onOpenJob={() => {}}
  />
);

beforeEach(() => {
  invokeMock.mockReset();
});
afterEach(cleanup);

describe("JobDetailScreen — the submit's memory notice (ADR-024 o26.6)", () => {
  // CONTROL (`run_p5fu_controls.sh`, `detail-autorun-drops-memory-notice`): the auto-run submit
  // never sets the memory notice → red.
  it("shows the memory notice beside the %pal notice after Create & Run", async () => {
    answer(job());
    render(screenFor(true));
    await waitFor(() => expect(screen.getByTestId("memory-notice").textContent).toBe(MEMORY_NOTICE));
    expect(screen.getByTestId("submit-notice").textContent).toBe(PAL_NOTICE);
    expect(invokeMock).toHaveBeenCalledWith("submit_job", { id: "j1", target: "p1" });
  });

  // CONTROL (`detail-retry-drops-memory-notice`): the Retry handler never sets it → red.
  it("shows the memory notice beside the %pal notice after Retry submit", async () => {
    answer(REMOTE_NOT_ON_SERVER);
    render(screenFor(false));
    fireEvent.click(await screen.findByRole("button", { name: "Retry submit" }));
    await waitFor(() => expect(screen.getByTestId("memory-notice").textContent).toBe(MEMORY_NOTICE));
    expect(screen.getByTestId("submit-notice").textContent).toBe(PAL_NOTICE);
    expect(invokeMock).toHaveBeenCalledWith("retry_remote_submit", { id: "j1" });
  });

  it("shows no memory notice when the answer carries none", async () => {
    answer(job(), { memory_notice: null });
    render(screenFor(true));
    await waitFor(() => expect(screen.getByTestId("submit-notice").textContent).toBe(PAL_NOTICE));
    expect(screen.queryByTestId("memory-notice")).toBeNull();
  });
});
