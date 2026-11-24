// @vitest-environment jsdom
/**
 * ADR-027 M4b Part B at screen level: the Jobs table's Claude marks and its reloads. The badge, a
 * PROPOSED draft's first reason, "Claude is submitting" in Run's place with no Delete, and the two
 * events that re-read the list (`job:status`, which carries no `mcp_state`; `groups:changed`). The
 * screen is rendered whole with `invoke` and `listen` mocked.
 *
 * Render correctness (placement, spacing, colour) is Anton's live gate; these pin behaviour only.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, render, screen, waitFor } from "@testing-library/react";

import type { Job } from "../types";

const invokeMock = vi.fn<(cmd: string, args?: Record<string, unknown>) => Promise<unknown>>();
vi.mock("@tauri-apps/api/core", () => ({ invoke: (cmd: string, args?: Record<string, unknown>) => invokeMock(cmd, args) }));

/** Every registered listener, by event, with its unlisten state. */
const listeners: { event: string; handler: (e: { payload: unknown }) => void; unlistened: boolean }[] = [];
/** When set, a `listen` registration waits for it: the listeners are "not attached yet". */
let hold: Promise<void> | null = null;
vi.mock("@tauri-apps/api/event", () => ({
  listen: async (event: string, handler: (e: { payload: unknown }) => void) => {
    if (hold) await hold;
    const entry = { event, handler, unlistened: false };
    listeners.push(entry);
    return () => {
      entry.unlistened = true;
    };
  },
}));
vi.mock("@tauri-apps/plugin-dialog", () => ({ confirm: async () => true, message: async () => {}, open: async () => null }));

import { JobsScreen } from "./JobsScreen";

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
    created_at: "2026-11-23 10:00:00",
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

const REASONS = JSON.stringify([
  { row: "P5", message: "memory is over the limit" },
  { row: "P7", message: "server is full" },
]);

let rows: Job[] = [];
const calls = (cmd: string) => invokeMock.mock.calls.filter(([c]) => c === cmd).length;

function mount() {
  return render(<JobsScreen onOpenDetail={() => {}} queuePaused={false} selection={{ kind: "all" }} onSelectionChange={() => {}} />);
}

/** Emit an event to every listener of it that is still attached. */
function emit(event: string, payload: unknown = null) {
  act(() => {
    listeners.filter((l) => l.event === event && !l.unlistened).forEach((l) => l.handler({ payload }));
  });
}

beforeEach(() => {
  rows = [];
  hold = null;
  listeners.length = 0;
  invokeMock.mockReset();
  invokeMock.mockImplementation(async (cmd) => {
    if (cmd === "list_jobs") return rows;
    if (cmd === "list_groups") return [];
    if (cmd === "remote_job_state") return { seq: 0, state: null };
    return null;
  });
});
afterEach(cleanup);

describe("JobsScreen: Claude's marks", () => {
  // CONTROL: `isClaudeJob` true for every job → the user row gets the badge → red.
  it("badges Claude's jobs and no others", async () => {
    rows = [job({ id: "c", title: "from claude", origin: "mcp" }), job({ id: "u", title: "from anton" })];
    mount();
    await waitFor(() => expect(screen.getByText("from claude")).toBeTruthy());
    const badges = screen.getAllByTestId("claude-badge");
    expect(badges).toHaveLength(1);
    expect(badges[0]!.closest("tr")!.textContent).toContain("from claude");
  });

  it("shows a PROPOSED draft's first reason in its row, and none for others", async () => {
    rows = [
      job({ id: "p", title: "proposed", origin: "mcp", mcp_reasons: REASONS }),
      job({ id: "s", title: "submitting", origin: "mcp", grant_id: "g1", mcp_state: "submitting", mcp_reasons: REASONS }),
      job({ id: "u", title: "user", mcp_reasons: REASONS }),
    ];
    mount();
    await waitFor(() => expect(screen.getByText("proposed")).toBeTruthy());
    const reasons = screen.getAllByTestId("claude-reason");
    expect(reasons).toHaveLength(1);
    expect(reasons[0]!.textContent).toBe("P5: memory is over the limit (+1)");
    expect(reasons[0]!.closest("tr")!.textContent).toContain("proposed");
  });

  // CONTROL: let `parseJobReasons` throw → the whole table fails to render → red.
  it("renders the table when a row's reasons are malformed", async () => {
    rows = [job({ id: "p", title: "bad reasons", origin: "mcp", mcp_reasons: "not json {" }), job({ id: "q", title: "fine" })];
    mount();
    await waitFor(() => expect(screen.getByText("fine")).toBeTruthy());
    expect(screen.getByTestId("claude-reason").textContent).toBe("reasons unreadable");
  });
});

describe("JobsScreen: Run and Delete for a draft Claude is submitting (Anton, 2026-11-23)", () => {
  const rowOf = (title: string) => screen.getByText(title).closest("tr")!;
  const buttonsOf = (title: string) => Array.from(rowOf(title).querySelectorAll("button")).map((b) => b.textContent);

  // CONTROL: make `rowActions` offer the plain draft's actions for a submitting one → Run and Delete are back → red.
  it("replaces Run with a disabled 'Claude is submitting' and offers no Delete", async () => {
    rows = [
      job({ id: "s", title: "submitting", origin: "mcp", grant_id: "g1", mcp_state: "submitting" }),
      job({ id: "d", title: "ordinary draft" }),
    ];
    mount();
    await waitFor(() => expect(screen.getByText("ordinary draft")).toBeTruthy());

    const sub = buttonsOf("submitting");
    expect(sub).toContain("Claude is submitting");
    expect(sub).not.toContain("Run locally");
    expect(sub).not.toContain("Delete");
    const button = screen.getByTestId("claude-submitting") as HTMLButtonElement;
    expect(button.disabled).toBe(true);

    const plain = buttonsOf("ordinary draft");
    expect(plain).toContain("Run locally");
    expect(plain).toContain("Delete");
    expect(plain).not.toContain("Claude is submitting");
  });

  // A terminal row a crash left flagged (Anton chose (b)): Open stays, no Delete, a muted note instead.
  // CONTROL: let `rowActions` keep `delete` on a flagged terminal row -> the Delete button is back -> red.
  it("holds Delete back on a terminal job that still carries mcp_state, with a note", async () => {
    rows = [
      job({ id: "stuck", title: "stuck", status: "completed", origin: "mcp", grant_id: "g1", mcp_state: "submitting" }),
      job({ id: "done", title: "done", status: "completed" }),
    ];
    mount();
    await waitFor(() => expect(screen.getByText("done")).toBeTruthy());
    expect(buttonsOf("stuck")).toContain("Open");
    expect(buttonsOf("stuck")).not.toContain("Delete");
    expect(rowOf("stuck").querySelector("[data-testid='delete-held']")).not.toBeNull();
    expect(buttonsOf("done")).toContain("Delete");
    expect(rowOf("done").querySelector("[data-testid='delete-held']")).toBeNull();
  });

  // The PROPOSED draft (no grant, no state) is an ordinary draft: Run and Delete as any.
  it("offers a PROPOSED draft Run and Delete", async () => {
    rows = [job({ id: "p", title: "proposed", origin: "mcp", mcp_reasons: REASONS })];
    mount();
    await waitFor(() => expect(screen.getByText("proposed")).toBeTruthy());
    expect(buttonsOf("proposed")).toEqual(expect.arrayContaining(["Run locally", "Delete"]));
  });

  // The event carries no mcp_state: the row changes only because the list is read again.
  // CONTROL: drop the `job:status` subscription → the row stays "Claude is submitting" → red.
  it("lets 'Claude is submitting' clear on the next job:status, by re-reading the list", async () => {
    rows = [job({ id: "s", title: "submitting", origin: "mcp", grant_id: "g1", mcp_state: "submitting" })];
    mount();
    await waitFor(() => expect(screen.getByTestId("claude-submitting")).toBeTruthy());
    rows = [job({ id: "s", title: "submitting", origin: "mcp", grant_id: null, mcp_state: null, status: "queued" })];
    emit("job:status", { job_id: "s", status: "queued" });
    await waitFor(() => expect(screen.queryByTestId("claude-submitting")).toBeNull());
  });
});

describe("JobsScreen: the reloads", () => {
  // CONTROL: remove the `groups:changed` subscription → `list_groups` is not read again → red.
  it("re-reads jobs and groups on groups:changed", async () => {
    mount();
    await waitFor(() => expect(calls("list_groups")).toBe(1));
    emit("groups:changed");
    await waitFor(() => expect(calls("list_groups")).toBe(2));
    expect(calls("list_jobs")).toBe(2);
  });

  it("re-reads on job:status", async () => {
    mount();
    await waitFor(() => expect(calls("list_jobs")).toBe(1));
    emit("job:status", { job_id: "x", status: "running" });
    await waitFor(() => expect(calls("list_jobs")).toBe(2));
  });

  // CONTROL: skip `sub.dispose()` in the effect's cleanup → the listeners stay attached → red.
  it("detaches both listeners on unmount, and reads nothing after it", async () => {
    const view = mount();
    await waitFor(() => expect(calls("list_jobs")).toBe(1));
    view.unmount();
    const mine = listeners.filter((l) => l.event === "groups:changed" || l.event === "job:status");
    expect(mine.map((l) => l.event).sort()).toEqual(["groups:changed", "job:status"]);
    expect(mine.every((l) => l.unlistened)).toBe(true);
    emit("groups:changed");
    emit("job:status");
    expect(calls("list_jobs")).toBe(1);
  });

  // CONTROL: call `load()` at once instead of after `sub.ready` -> the list is read while the listeners are
  // still pending, and a change in that gap is lost -> red.
  it("reads its list only once the listeners are attached (listen first, then ask)", async () => {
    let release: () => void = () => {};
    hold = new Promise<void>((resolve) => {
      release = resolve;
    });
    mount();
    await act(async () => {
      await new Promise((r) => setTimeout(r, 30));
    });
    expect(listeners).toEqual([]);
    expect(calls("list_jobs")).toBe(0);
    release();
    await waitFor(() => expect(calls("list_jobs")).toBe(1));
    expect(listeners.some((l) => l.event === "groups:changed")).toBe(true);
    expect(listeners.some((l) => l.event === "job:status")).toBe(true);
  });
});
