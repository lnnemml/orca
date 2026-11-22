// @vitest-environment jsdom
/**
 * Settings → MCP's panels (ADR-027 M4a Part B), rendered with `invoke`, `listen` and the dialog
 * mocked: the read count, the grant card and editor, Revoke, "Cancel Claude's jobs" and the audit view.
 * What each says and what each calls is pinned here; how it looks is Anton's live WebKitGTK gate.
 *
 * The zone is Europe/Kyiv (the `expiry.test.ts` pattern: set in `beforeAll`, restored in `afterAll`),
 * so the local times shown and the DST refusal are checked under a zone that is not UTC.
 *
 * Controls (`m4a_partb_controls.py`) are named in the comments; each is applied to the component and
 * must turn its test red.
 */
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";

const invokeMock = vi.fn<(cmd: string, args?: Record<string, unknown>) => Promise<unknown>>();
vi.mock("@tauri-apps/api/core", () => ({ invoke: (cmd: string, args?: Record<string, unknown>) => invokeMock(cmd, args) }));
const jobStatusHandlers = new Set<() => void>();
vi.mock("@tauri-apps/api/event", () => ({
  listen: async (name: string, handler: () => void) => {
    if (name === "job:status") jobStatusHandlers.add(handler);
    return () => {
      jobStatusHandlers.delete(handler);
    };
  },
}));
const confirmMock = vi.fn<(text: string, options?: unknown) => Promise<boolean>>();
vi.mock("@tauri-apps/plugin-dialog", () => ({
  confirm: (text: string, options?: unknown) => confirmMock(text, options),
  message: async () => {},
  open: async () => null,
}));

import type {
  AuditPage,
  AuditRowView,
  ClaudeCancelReport,
  ClaudeJobsSummary,
  GrantView,
  Job,
  McpStatus,
  ServerProfile,
} from "../types";
import { replaceConfirmText, revokeConfirmText } from "./grant";
import { McpSection, MCP_STATUS_POLL_MS } from "./McpSection";

let outerTz: string | undefined;
beforeAll(() => {
  outerTz = process.env.TZ;
  process.env.TZ = "Europe/Kyiv";
});
afterAll(() => {
  if (outerTz === undefined) delete process.env.TZ;
  else process.env.TZ = outerTz;
});

// ---- fixtures -------------------------------------------------------------------------------------

const OFF: McpStatus = {
  enabled: false,
  holds_lock: false,
  serving: false,
  stopping_tasks: null,
  connections: 0,
  socket_path: "/run/user/1000/orcastudio/mcp.sock",
  reason: "MCP is off",
  reads: { total: 0, connections: [] },
};
const SERVING: McpStatus = { ...OFF, enabled: true, holds_lock: true, serving: true, reason: null, connections: 1 };

const ACTIVE: GrantView = {
  id: "g1",
  state: "active",
  created_at: "2026-11-21 08:00:00",
  expires_at: "2026-11-21 16:00:00", // 18:00 in Kyiv (EET, UTC+2)
  revoked_at: null,
  targets: ["local", "srv1"],
  levels: ["! B3LYP D4 def2-SVP"],
  max_jobs: 5,
  max_in_flight: 2,
  charged_jobs: 2,
  in_flight: 1,
};
const REVOKED: GrantView = { ...ACTIVE, state: "revoked", revoked_at: "2026-11-21 09:00:00" };

const PROFILE = { id: "srv1", name: "university cluster", host: "uni" } as ServerProfile;
const job = (id: string, title: string, status: Job["status"], created_at: string) =>
  ({ id, title, status, created_at }) as Job;
const JOBS = [
  job("j-old", "old sp", "completed", "2026-11-01 10:00:00"),
  job("j-new", "new opt", "parsed", "2026-11-20 10:00:00"),
  job("j-failed", "a failed one", "failed", "2026-11-21 10:00:00"),
  job("j-running", "a running one", "running", "2026-11-21 10:30:00"),
];

const auditRow = (id: number, over: Partial<AuditRowView> = {}): AuditRowView => ({
  id,
  at: "2026-11-21 08:00:00",
  tool: "create_job",
  args: `{"n":${id}}`,
  args_clipped: false,
  args_chars: `{"n":${id}}`.length,
  disposition: "proposed",
  reasons: { kind: "read", list: [] },
  job_id: null,
  grant_id: null,
  ...over,
});

// ---- the fake backend -----------------------------------------------------------------------------

let status: McpStatus;
let grantNow: GrantView | null;
let grantError: string | null;
let summary: ClaudeJobsSummary;
let cancelGate: Promise<ClaudeCancelReport> | null;
let auditRows: AuditRowView[]; // every row, in any order
let auditWhole: Record<number, AuditRowView>;

const auditPage = (beforeId: number | null, limit: number): AuditPage => {
  const older = auditRows.filter((r) => beforeId === null || r.id < beforeId).sort((a, b) => b.id - a.id);
  const rows = older.slice(0, limit);
  return { rows, next_before_id: older.length > limit ? rows[rows.length - 1].id : null };
};

beforeEach(() => {
  invokeMock.mockReset();
  confirmMock.mockReset();
  confirmMock.mockResolvedValue(true);
  jobStatusHandlers.clear();
  status = OFF;
  grantNow = null;
  grantError = null;
  summary = { to_cancel: 0, submitting: 0 };
  cancelGate = null;
  auditRows = [];
  auditWhole = {};
  invokeMock.mockImplementation(async (cmd, args) => {
    switch (cmd) {
      case "mcp_status":
        return status;
      case "mcp_set_enabled":
        return status;
      case "mcp_get_grant":
        if (grantError) throw new Error(grantError);
        return grantNow;
      case "list_server_profiles":
        return [PROFILE];
      case "list_jobs":
        return JOBS;
      case "mcp_level_from_job":
        return `! level of ${String(args?.jobId)}`;
      case "mcp_create_grant":
        grantNow = { ...ACTIVE, id: "g2", charged_jobs: 0, in_flight: 0 };
        return grantNow;
      case "mcp_revoke_grant":
        grantNow = REVOKED;
        return REVOKED;
      case "mcp_claude_jobs_summary":
        return summary;
      case "mcp_cancel_claude_jobs":
        return cancelGate ?? { jobs: [], skipped_submitting: 0 };
      case "mcp_list_audit":
        return auditPage((args?.beforeId as number | null) ?? null, (args?.limit as number) ?? 50);
      case "mcp_audit_row": {
        const whole = auditWhole[args?.id as number];
        if (!whole) throw new Error(`not found: audit row ${String(args?.id)}`);
        return whole;
      }
      default:
        throw new Error(`unexpected command ${cmd}`);
    }
  });
});
afterEach(() => {
  cleanup();
  vi.useRealTimers();
});

const calls = (cmd: string) => invokeMock.mock.calls.filter(([c]) => c === cmd);
const card = () => screen.findByTestId("grant-card");
const flush = async () => {
  await act(async () => {});
};
const click = async (el: HTMLElement) => {
  await act(async () => {
    fireEvent.click(el);
  });
};
const button = (name: RegExp | string) => screen.getByRole("button", { name });

// ---- the read count -------------------------------------------------------------------------------

describe("the read count", () => {
  // CONTROL (`reads-ignore-serving`): the line shown without regard to `serving` → red.
  it("serving: the total since the app started and each connection with its per-tool counts", async () => {
    status = {
      ...SERVING,
      reads: {
        total: 12,
        connections: [{ id: 3, state: "closing", reads: 12, by_tool: { list_jobs: 4, get_job: 8 } }],
      },
    };
    render(<McpSection />);
    expect((await screen.findByTestId("mcp-reads-line")).textContent).toBe(
      "Claude's reads through this instance since the app started: 12.",
    );
    expect(screen.getByTestId("mcp-reads-connection").textContent).toBe("#3 (closing): 12 reads — get_job 8, list_jobs 4");
  });

  it("not serving: says this instance counts no reads, and shows no total as how much Claude read", async () => {
    status = { ...OFF, reads: { total: 0, connections: [] } };
    render(<McpSection />);
    const line = (await screen.findByTestId("mcp-reads-line")).textContent ?? "";
    expect(line).toBe("This instance does not serve, so it counts no reads; another instance may be serving.");
    expect(line).not.toMatch(/Claude's reads through this instance/);
    expect(screen.queryByTestId("mcp-reads-connection")).toBeNull();
  });
});

// ---- the grant card -------------------------------------------------------------------------------

describe("the grant card", () => {
  // CONTROL (`card-inflight-label`): "in flight" without "(all grants)" → red.
  it("shows the state, charged of max_jobs, in flight (all grants), the local expiry, targets and levels", async () => {
    grantNow = ACTIVE;
    render(<McpSection />);
    await waitFor(() => expect(screen.getByTestId("grant-state").textContent).toBe("Active"));
    expect(screen.getByTestId("grant-charged").textContent).toBe("2 of 5 jobs charged");
    expect(screen.getByTestId("grant-in-flight").textContent).toBe("1 of 2 in flight (all grants)");
    expect(screen.getByTestId("grant-time").textContent).toBe("expires 2026-11-21 18:00:00");
    expect(screen.getByTestId("grant-targets").textContent).toBe("Targets: local, university cluster");
    expect(screen.getAllByTestId("grant-card-level").map((e) => e.textContent)).toEqual(["! B3LYP D4 def2-SVP"]);
  });

  it("says so when no grant was ever made, and when the read failed (no stale card)", async () => {
    render(<McpSection />);
    expect((await card()).textContent).toMatch(/No grant has been made/);
    cleanup();
    grantNow = ACTIVE;
    render(<McpSection />);
    await waitFor(() => expect(screen.getByTestId("grant-state")).toBeTruthy());
    // A later failed read drops the card it showed.
    grantError = "database is locked";
    await act(async () => {
      for (const h of jobStatusHandlers) h();
    });
    expect((await card()).textContent).toMatch(/Could not read the grant: .*database is locked/);
    expect(screen.queryByTestId("grant-state")).toBeNull();
  });

  // CONTROL (`card-no-poll`): the card not re-reading on the section's poll tick → red.
  it("re-reads on the section's 5 s poll", async () => {
    vi.useFakeTimers();
    grantNow = ACTIVE;
    render(<McpSection />);
    await flush();
    expect(screen.getByTestId("grant-charged").textContent).toBe("2 of 5 jobs charged");
    const before = calls("mcp_get_grant").length;
    grantNow = { ...ACTIVE, charged_jobs: 4 };
    await act(async () => {
      vi.advanceTimersByTime(MCP_STATUS_POLL_MS);
    });
    expect(calls("mcp_get_grant").length).toBeGreaterThan(before);
    expect(screen.getByTestId("grant-charged").textContent).toBe("4 of 5 jobs charged");
  });

  // CONTROL (`card-no-job-status`): the card not re-reading on `job:status` → red.
  it("re-reads on job:status", async () => {
    grantNow = ACTIVE;
    render(<McpSection />);
    await waitFor(() => expect(screen.getByTestId("grant-in-flight").textContent).toBe("1 of 2 in flight (all grants)"));
    expect(jobStatusHandlers.size).toBeGreaterThan(0);
    grantNow = { ...ACTIVE, in_flight: 2 };
    await act(async () => {
      for (const h of jobStatusHandlers) h();
    });
    await waitFor(() => expect(screen.getByTestId("grant-in-flight").textContent).toBe("2 of 2 in flight (all grants)"));
  });

  // CONTROL (`card-late-read`): a read overtaken by a later one still applied → red.
  it("discards a read that a later read has overtaken", async () => {
    grantNow = ACTIVE;
    render(<McpSection />);
    await waitFor(() => expect(screen.getByTestId("grant-charged").textContent).toBe("2 of 5 jobs charged"));
    // The next read is slow and old; the one after it is quick and new.
    let releaseSlow: (g: GrantView | null) => void = () => {};
    const slow = new Promise<GrantView | null>((resolve) => {
      releaseSlow = resolve;
    });
    const quick = { ...ACTIVE, charged_jobs: 4 };
    let n = 0;
    const previous = invokeMock.getMockImplementation()!;
    invokeMock.mockImplementation(async (cmd, args) => {
      if (cmd === "mcp_get_grant") return ++n === 1 ? slow : quick;
      return previous(cmd, args);
    });
    await act(async () => {
      for (const h of jobStatusHandlers) h();
      for (const h of jobStatusHandlers) h();
    });
    await waitFor(() => expect(screen.getByTestId("grant-charged").textContent).toBe("4 of 5 jobs charged"));
    await act(async () => releaseSlow({ ...ACTIVE, charged_jobs: 1 }));
    expect(screen.getByTestId("grant-charged").textContent).toBe("4 of 5 jobs charged");
  });
});

// ---- Revoke ---------------------------------------------------------------------------------------

describe("Revoke", () => {
  // CONTROL (`revoke-no-confirm`): the revoke called without the confirm → red.
  it("confirms with the grant's own text, then revokes it by id", async () => {
    grantNow = ACTIVE;
    render(<McpSection />);
    await waitFor(() => expect(screen.getByTestId("grant-state")).toBeTruthy());
    confirmMock.mockResolvedValueOnce(false);
    await click(button("Revoke"));
    expect(confirmMock).toHaveBeenCalledTimes(1);
    expect(confirmMock.mock.calls[0][0]).toBe(revokeConfirmText(ACTIVE));
    expect(calls("mcp_revoke_grant")).toHaveLength(0); // declined: nothing revoked

    await click(button("Revoke"));
    expect(confirmMock).toHaveBeenCalledTimes(2);
    expect(calls("mcp_revoke_grant").map(([, a]) => a)).toEqual([{ id: "g1" }]);
    await waitFor(() => expect(screen.getByTestId("grant-state").textContent).toBe("Revoked"));
    expect(screen.queryByRole("button", { name: "Revoke" })).toBeNull();
  });

  // CONTROL (`revoke-from-card`): the revoke decided from the card's grant, not a fresh read → red.
  it("reads the grant afresh at the click: a grant already gone is not revoked, and is not confirmed", async () => {
    grantNow = ACTIVE;
    render(<McpSection />);
    await waitFor(() => expect(screen.getByTestId("grant-state")).toBeTruthy());
    grantNow = REVOKED; // changed since the card's last read
    await click(button("Revoke"));
    expect(confirmMock).not.toHaveBeenCalled();
    expect(calls("mcp_revoke_grant")).toHaveLength(0);
    expect(screen.getByTestId("grant-notice").textContent).toBe("There is no active grant to revoke.");
    expect(screen.getByTestId("grant-state").textContent).toBe("Revoked");
  });
});

// ---- the editor -----------------------------------------------------------------------------------

const createButton = () => screen.getByRole("button", { name: /^Create grant|^Creating/ }) as HTMLButtonElement;
const checkTarget = async (name: RegExp) => click(screen.getByRole("checkbox", { name }));

describe("the grant editor", () => {
  it("offers local and each server profile, the levels, the counts and the expiry presets", async () => {
    render(<McpSection />);
    await screen.findByLabelText("Level to add");
    expect(screen.getByRole("checkbox", { name: /^local$/ })).toBeTruthy();
    await screen.findByRole("checkbox", { name: /university cluster/ });
    expect((screen.getByLabelText("max_jobs") as HTMLInputElement).value).toBe("5");
    expect((screen.getByLabelText("max_in_flight") as HTMLInputElement).value).toBe("1");
    for (const label of ["1 h", "8 h", "24 h", "7 days"]) expect(button(label)).toBeTruthy();
    expect(button("8 h").getAttribute("aria-pressed")).toBe("true"); // the default
  });

  it("sends what the form holds, with the expiry as UTC counted from the click", async () => {
    vi.useFakeTimers({ toFake: ["Date"] });
    vi.setSystemTime(new Date("2026-11-21T10:00:00Z")); // 12:00 in Kyiv
    render(<McpSection />);
    await screen.findByRole("checkbox", { name: /university cluster/ });
    await checkTarget(/^local$/);
    await checkTarget(/university cluster/);
    fireEvent.change(screen.getByLabelText("Level to add"), { target: { value: "! HF def2-SVP\n%cpcm\nend" } });
    await click(button("Add level"));
    fireEvent.change(screen.getByLabelText("max_jobs"), { target: { value: "7" } });
    expect(screen.getByTestId("expiry-line").textContent).toBe("8 h from Create — about 2026-11-21 20:00 (UTC+02:00)");
    await click(createButton());
    expect(confirmMock).not.toHaveBeenCalled(); // no active grant: nothing to replace
    expect(calls("mcp_create_grant").map(([, a]) => a)).toEqual([
      {
        targets: ["local", "srv1"],
        levels: ["! HF def2-SVP\n%cpcm\nend"],
        maxJobs: 7,
        maxInFlight: 1,
        expiresAt: "2026-11-21 18:00:00",
      },
    ]);
    expect((await screen.findByTestId("create-info")).textContent).toMatch(/^Grant created, expires /);
  });

  // CONTROL (`create-no-confirm`): the replace confirm answered without asking → red.
  // CONTROL (`create-bypass-flow`): the button calling `mcp_create_grant` itself → red.
  // CONTROL (`create-read-shown`): the flow's read answered from the card's grant → red.
  it("Create reads the grant afresh and confirms a replacement; a decline creates nothing", async () => {
    render(<McpSection />);
    await screen.findByRole("checkbox", { name: /university cluster/ });
    await waitFor(() => expect(screen.getByTestId("grant-card").textContent).toMatch(/No grant has been made/));
    await checkTarget(/^local$/);
    fireEvent.change(screen.getByLabelText("Level to add"), { target: { value: "! HF def2-SVP" } });
    await click(button("Add level"));
    // A grant became active after the card's last read: only a fresh read can know.
    grantNow = ACTIVE;
    confirmMock.mockResolvedValueOnce(false);
    await click(createButton());
    expect(confirmMock).toHaveBeenCalledTimes(1);
    expect(confirmMock.mock.calls[0][0]).toBe(replaceConfirmText(ACTIVE, true));
    expect(calls("mcp_create_grant")).toHaveLength(0);
    expect((await screen.findByTestId("create-info")).textContent).toBe("The active grant was not replaced.");

    await click(createButton()); // now accepted
    expect(confirmMock).toHaveBeenCalledTimes(2);
    expect(calls("mcp_create_grant")).toHaveLength(1);
  });

  it("shows the core's refusal as returned", async () => {
    render(<McpSection />);
    await screen.findByLabelText("Level to add");
    invokeMock.mockImplementation(async (cmd) => {
      if (cmd === "mcp_create_grant") throw "invalid: level 1: `! nonsense` is not a method line";
      if (cmd === "mcp_get_grant") return null;
      if (cmd === "list_server_profiles" || cmd === "list_jobs") return [];
      return status;
    });
    await click(createButton());
    expect((await screen.findByTestId("create-error")).textContent).toBe(
      "invalid: level 1: `! nonsense` is not a method line",
    );
  });

  it("this job's level: only finished jobs are offered; the core's answer is appended, its refusal shown", async () => {
    render(<McpSection />);
    const select = (await screen.findByLabelText("Finished job")) as HTMLSelectElement;
    await waitFor(() => expect(select.options.length).toBe(3)); // the prompt + 2 finished jobs
    const offered = [...select.options].map((o) => o.value);
    expect(offered).toEqual(["", "j-new", "j-old"]); // newest first; no failed, no running job
    fireEvent.change(select, { target: { value: "j-new" } });
    await click(button("Add this job's level"));
    expect(calls("mcp_level_from_job").map(([, a]) => a)).toEqual([{ jobId: "j-new" }]);
    expect(screen.getAllByTestId("grant-level").map((e) => e.textContent)).toEqual(["! level of j-new"]);

    const previous = invokeMock.getMockImplementation()!;
    invokeMock.mockImplementation(async (cmd, args) => {
      if (cmd === "mcp_level_from_job") throw "invalid: job j-old's header gives no level: no ! line";
      return previous(cmd, args);
    });
    fireEvent.change(select, { target: { value: "j-old" } });
    await click(button("Add this job's level"));
    expect((await screen.findByTestId("level-error")).textContent).toBe("invalid: job j-old's header gives no level: no ! line");
    expect(screen.getAllByTestId("grant-level")).toHaveLength(1);
  });

  // CONTROL (`l2-double-add`): the button not disabled while `mcp_level_from_job` is pending → red.
  it("this job's level: a double-click while the core is answering adds the level once", async () => {
    render(<McpSection />);
    const select = (await screen.findByLabelText("Finished job")) as HTMLSelectElement;
    await waitFor(() => expect(select.options.length).toBe(3));
    fireEvent.change(select, { target: { value: "j-new" } });
    let answer: (text: string) => void = () => {};
    const pending = new Promise<string>((resolve) => {
      answer = resolve;
    });
    const previous = invokeMock.getMockImplementation()!;
    invokeMock.mockImplementation(async (cmd, args) => (cmd === "mcp_level_from_job" ? pending : previous(cmd, args)));
    await click(button("Add this job's level"));
    expect((button("Add this job's level") as HTMLButtonElement).disabled).toBe(true);
    await click(button("Add this job's level")); // the second click of the double-click
    await act(async () => answer("! level of j-new"));
    expect(calls("mcp_level_from_job")).toHaveLength(1);
    expect(screen.getAllByTestId("grant-level").map((e) => e.textContent)).toEqual(["! level of j-new"]);
    expect((button("Add this job's level") as HTMLButtonElement).disabled).toBe(false);
  });

  // CONTROL (`l4-jobs-error-silent`): a failed `list_jobs` read not said → red.
  it("says when the jobs could not be read, rather than offering an empty picker as if there were none", async () => {
    const previous = invokeMock.getMockImplementation()!;
    invokeMock.mockImplementation(async (cmd, args) => {
      if (cmd === "list_jobs") throw new Error("database is locked");
      return previous(cmd, args);
    });
    render(<McpSection />);
    expect((await screen.findByTestId("jobs-error")).textContent).toMatch(/Could not read the jobs, so the picker offers none: .*database is locked/);
    cleanup();
    invokeMock.mockImplementation(async (cmd, args) => (cmd === "list_jobs" ? null : previous(cmd, args)));
    render(<McpSection />);
    expect((await screen.findByTestId("jobs-error")).textContent).toMatch(/list_jobs did not return a list/);
    cleanup();
    invokeMock.mockImplementation(previous);
    render(<McpSection />);
    await screen.findByLabelText("Finished job");
    expect(screen.queryByTestId("jobs-error")).toBeNull();
  });

  it("a count that is not a whole number at least 1 disables Create", async () => {
    render(<McpSection />);
    await screen.findByLabelText("max_jobs");
    expect(createButton().disabled).toBe(false);
    for (const bad of ["0", "-1", "2.5", "", "abc"]) {
      fireEvent.change(screen.getByLabelText("max_jobs"), { target: { value: bad } });
      expect(createButton().disabled, `max_jobs ${JSON.stringify(bad)}`).toBe(true);
    }
    fireEvent.change(screen.getByLabelText("max_jobs"), { target: { value: "3" } });
    fireEvent.change(screen.getByLabelText("max_in_flight"), { target: { value: "0" } });
    expect(createButton().disabled).toBe(true);
  });

  // The custom time is a text date plus hour and minute selects (WebKitGTK's `datetime-local` picker hangs).
  const setCustom = (date: string, hour?: string, minute?: string) => {
    fireEvent.change(screen.getByLabelText("date (local)"), { target: { value: date } });
    if (hour !== undefined) fireEvent.change(screen.getByLabelText("Expiry hour"), { target: { value: hour } });
    if (minute !== undefined) fireEvent.change(screen.getByLabelText("Expiry minute"), { target: { value: minute } });
  };
  const expiryText = () => screen.getByTestId("expiry-line").textContent;

  it("offers a date field with a placeholder, hours 00-23 and minutes 00-55 in steps of 5", async () => {
    render(<McpSection />);
    const date = (await screen.findByLabelText("date (local)")) as HTMLInputElement;
    expect(date.type).toBe("text");
    expect(date.placeholder).toBe("YYYY-MM-DD");
    const hours = [...(screen.getByLabelText("Expiry hour") as HTMLSelectElement).options].map((o) => o.value);
    expect(hours).toEqual(Array.from({ length: 24 }, (_, i) => String(i).padStart(2, "0")));
    const minutes = [...(screen.getByLabelText("Expiry minute") as HTMLSelectElement).options].map((o) => o.value);
    expect(minutes).toEqual(["00", "05", "10", "15", "20", "25", "30", "35", "40", "45", "50", "55"]);
  });

  // CONTROL (`custom-minute-ignored`): the minute select not composed into the time → red.
  it("a full date, hour and minute give the expiry line and the UTC sent; choosing it un-presses the presets", async () => {
    render(<McpSection />);
    await screen.findByLabelText("date (local)");
    expect(button("8 h").getAttribute("aria-pressed")).toBe("true");
    setCustom("2026-12-01", "14", "35");
    expect(expiryText()).toBe("2026-12-01 14:35 (UTC+02:00)");
    for (const label of ["1 h", "8 h", "24 h", "7 days"]) expect(button(label).getAttribute("aria-pressed")).toBe("false");
    expect(createButton().disabled).toBe(false);
    await click(createButton());
    expect(calls("mcp_create_grant").map(([, a]) => (a as { expiresAt: string }).expiresAt)).toEqual(["2026-12-01 12:35:00"]);
  });

  it("an empty date is no custom time: the preset rules; pressing a preset clears the date", async () => {
    render(<McpSection />);
    await screen.findByLabelText("date (local)");
    setCustom("", "14", "35"); // the selects alone choose nothing
    expect(button("8 h").getAttribute("aria-pressed")).toBe("true");
    expect(expiryText()).toMatch(/^8 h from Create — about /);
    setCustom("2026-12-01");
    expect(button("8 h").getAttribute("aria-pressed")).toBe("false");
    await click(button("24 h"));
    expect((screen.getByLabelText("date (local)") as HTMLInputElement).value).toBe("");
    expect(button("24 h").getAttribute("aria-pressed")).toBe("true");
    expect(expiryText()).toMatch(/^24 h from Create — about /);
  });

  it("what is typed survives the section's poll", async () => {
    render(<McpSection />);
    await screen.findByLabelText("date (local)");
    setCustom("2026-12-01", "09", "15");
    const before = calls("mcp_get_grant").length;
    await act(async () => {
      for (const h of [...jobStatusHandlers]) h(); // a job event re-reads the card
    });
    await waitFor(() => expect(calls("mcp_get_grant").length).toBeGreaterThan(before));
    expect((screen.getByLabelText("date (local)") as HTMLInputElement).value).toBe("2026-12-01");
    expect((screen.getByLabelText("Expiry hour") as HTMLSelectElement).value).toBe("09");
    expect((screen.getByLabelText("Expiry minute") as HTMLSelectElement).value).toBe("15");
    expect(expiryText()).toBe("2026-12-01 09:15 (UTC+02:00)");
  });

  it("a malformed date is refused inline and Create stays disabled", async () => {
    render(<McpSection />);
    await screen.findByLabelText("date (local)");
    for (const bad of ["2027-13-40", "2026-02-30", "tomorrow", "2027-1"]) {
      setCustom(bad, "10", "00");
      expect(expiryText(), bad).toMatch(/not a valid date and time|not a calendar date|not a date and time/);
      expect(createButton().disabled, bad).toBe(true);
    }
    setCustom("2027-01-15");
    expect(createButton().disabled).toBe(false);
  });

  // CONTROL (`dst-gap-enables-create`): Create enabled while the custom time is refused → red.
  it("a custom local time inside a DST gap is refused inline and Create stays disabled", async () => {
    render(<McpSection />);
    await screen.findByLabelText("date (local)");
    // 2026-03-29 03:30 does not exist in Kyiv: the clocks go 03:00 → 04:00.
    setCustom("2026-03-29", "03", "30");
    expect(expiryText()).toMatch(/does not exist — DST/);
    expect(createButton().disabled).toBe(true);
    // A time that exists is shown with its offset and enables Create.
    setCustom("2026-03-29", "04", "30");
    expect(expiryText()).toBe("2026-03-29 04:30 (UTC+03:00)");
    expect(createButton().disabled).toBe(false);
    // The repeated hour (25 Oct 03:30 happens twice) says which one is meant.
    setCustom("2026-10-25", "03", "30");
    expect(expiryText()).toMatch(/this hour occurs twice/);
  });

  // CONTROL (`datetime-local-back`): the native input put back in the editor → red.
  it("the editor's source holds no datetime-local input", () => {
    const src = readFileSync(resolve(process.cwd(), "src/mcp/McpGrantEditor.tsx"), "utf8")
      .replace(/\/\*[\s\S]*?\*\//g, "")
      .replace(/(^|[^:])\/\/.*$/gm, "$1");
    expect(src.length).toBeGreaterThan(1000); // the strip did not eat the file
    expect(src).not.toMatch(/datetime-local/);
  });
});

// ---- "Cancel Claude's jobs" -----------------------------------------------------------------------

describe("Cancel Claude's jobs", () => {
  // CONTROL (`cancel-no-confirm`): the command called without asking → red.
  it("confirms with the count, calls the command only on yes, and says nothing was cancelled on no", async () => {
    summary = { to_cancel: 3, submitting: 1 };
    render(<McpSection />);
    confirmMock.mockResolvedValueOnce(false);
    await click(button("Cancel Claude's jobs"));
    expect(calls("mcp_claude_jobs_summary")).toHaveLength(1);
    expect(confirmMock).toHaveBeenCalledTimes(1);
    expect(confirmMock.mock.calls[0][0]).toMatch(/^Cancel 3 of Claude's jobs/);
    expect(calls("mcp_cancel_claude_jobs")).toHaveLength(0);
    expect(screen.getByTestId("claude-jobs-note").textContent).toBe("Nothing was cancelled.");
  });

  it("with nothing to cancel: no confirm, no command, and the drafts being submitted named", async () => {
    summary = { to_cancel: 0, submitting: 2 };
    render(<McpSection />);
    await click(button("Cancel Claude's jobs"));
    expect(confirmMock).not.toHaveBeenCalled();
    expect(calls("mcp_cancel_claude_jobs")).toHaveLength(0);
    expect(screen.getByTestId("claude-jobs-note").textContent).toMatch(/no queued or running job to cancel.*2 drafts are being submitted/);
  });

  // CONTROL (`cancel-no-label`): "Cancelling N…" not shown while the call is pending → red.
  it("reads Cancelling N… while the command runs, then reports every outcome and the skipped drafts", async () => {
    summary = { to_cancel: 2, submitting: 1 };
    let finish: (r: ClaudeCancelReport) => void = () => {};
    cancelGate = new Promise<ClaudeCancelReport>((resolve) => {
      finish = resolve;
    });
    render(<McpSection />);
    await click(button("Cancel Claude's jobs"));
    expect(calls("mcp_cancel_claude_jobs")).toHaveLength(1);
    const pending = button("Cancelling 2…") as HTMLButtonElement;
    expect(pending.disabled).toBe(true);
    await act(async () =>
      finish({
        jobs: [
          { job_id: "q1", result: { result: "dispatched" } },
          { job_id: "r1", error: "conflict: the job is busy" },
        ],
        skipped_submitting: 1,
      }),
    );
    const report = await screen.findByTestId("claude-jobs-report");
    const outcomes = within(report)
      .getAllByTestId("claude-jobs-outcome")
      .map((e) => e.textContent);
    expect(outcomes).toEqual(["q1: cancelled", "r1: failed — conflict: the job is busy"]);
    expect(within(report).getByTestId("claude-jobs-skipped").textContent).toMatch(/1 draft Claude was still submitting was skipped/);
    expect(screen.getByRole("button", { name: "Cancel Claude's jobs" })).toBeTruthy(); // the label is back
  });
});

// ---- the audit view -------------------------------------------------------------------------------

const auditIds = () => screen.getAllByTestId("audit-row").map((r) => Number(r.getAttribute("data-row-id")));
const range = (from: number, to: number) => Array.from({ length: from - to + 1 }, (_, i) => from - i);

describe("the audit view", () => {
  beforeEach(() => {
    auditRows = range(120, 1).map((id) => auditRow(id));
  });

  // CONTROL (`audit-skip`): "Load older" asking one id too low → a row skipped → red.
  // CONTROL (`audit-repeat`): "Load older" asking one id too high → the boundary row returned twice → red.
  it("pages with the keyset: every row once, newest first, nothing skipped or repeated", async () => {
    render(<McpSection />);
    await screen.findByTestId("audit-table");
    expect(auditIds()).toEqual(range(120, 71));
    await click(button("Load older"));
    expect(auditIds()).toEqual(range(120, 21));
    await click(button("Load older"));
    expect(auditIds()).toEqual(range(120, 1));
    expect(screen.queryByRole("button", { name: "Load older" })).toBeNull();
    expect(screen.getByText("No older rows.")).toBeTruthy();
    expect(screen.queryByTestId("audit-repeated")).toBeNull();
    expect(calls("mcp_list_audit").map(([, a]) => a)).toEqual([
      { beforeId: null, limit: 50 },
      { beforeId: 71, limit: 50 },
      { beforeId: 21, limit: 50 },
    ]);
  });

  it("Refresh reloads the newest page, with rows that arrived, and drops the older ones", async () => {
    render(<McpSection />);
    await screen.findByTestId("audit-table");
    await click(button("Load older"));
    expect(auditIds()).toHaveLength(100);
    auditRows.push(auditRow(121));
    await click(button("Refresh"));
    expect(auditIds()).toEqual(range(121, 72));
  });

  // CONTROL (`audit-no-fetch`): a clipped row expanded without fetching the whole text → red.
  it("expanding a clipped row fetches its whole args; an unclipped row makes no call", async () => {
    const whole = JSON.stringify({ input: "x".repeat(3000) });
    auditRows = [
      auditRow(8, { args: '{"n":8}' }),
      auditRow(7, { args: whole.slice(0, 1024), args_clipped: true, args_chars: whole.length }),
    ];
    auditWhole = { 7: auditRow(7, { args: whole, args_chars: whole.length }) };
    render(<McpSection />);
    await screen.findByTestId("audit-table");
    expect(screen.getByText(`first 1,024 of ${whole.length.toLocaleString("en-US")} characters`)).toBeTruthy();
    await click(button("Show args of row 8"));
    expect(calls("mcp_audit_row")).toHaveLength(0);
    expect(screen.getAllByTestId("audit-args")[0].textContent).toBe('{"n":8}');

    await click(button("Show args of row 7"));
    expect(calls("mcp_audit_row").map(([, a]) => a)).toEqual([{ id: 7 }]);
    await waitFor(() => expect(screen.getAllByTestId("audit-args")).toHaveLength(2));
    expect(screen.getAllByTestId("audit-args")[1].textContent).toBe(whole);
    await click(button("Hide args of row 7"));
    expect(screen.getAllByTestId("audit-args")).toHaveLength(1);
  });

  // CONTROL (`l3-late-answer-reopens`): a late answer applied to a row no longer loading → red.
  it("a late answer does not re-open a row hidden meanwhile, or dropped by Refresh", async () => {
    const whole = JSON.stringify({ input: "x".repeat(3000) });
    auditRows = [auditRow(7, { args: whole.slice(0, 1024), args_clipped: true, args_chars: whole.length })];
    const answers: Array<() => void> = [];
    const previous = invokeMock.getMockImplementation()!;
    invokeMock.mockImplementation(async (cmd, args) => {
      if (cmd !== "mcp_audit_row") return previous(cmd, args);
      await new Promise<void>((resolve) => answers.push(resolve));
      return auditRow(7, { args: whole, args_chars: whole.length });
    });
    render(<McpSection />);
    await screen.findByTestId("audit-table");

    // Hidden while the whole text is being read.
    await click(button("Show args of row 7"));
    expect(screen.getByText("Reading the whole text…")).toBeTruthy();
    await click(button("Hide args of row 7"));
    await act(async () => answers.shift()!());
    expect(screen.queryByTestId("audit-args")).toBeNull();
    expect(screen.getByRole("button", { name: "Show args of row 7" })).toBeTruthy();

    // Refresh while it is being read.
    await click(button("Show args of row 7"));
    await click(button("Refresh"));
    await act(async () => answers.shift()!());
    expect(screen.queryByTestId("audit-args")).toBeNull();
    expect(screen.getByRole("button", { name: "Show args of row 7" })).toBeTruthy();
  });

  // CONTROL (`audit-unreadable-crash`): the reasons cell reading `.list` without the kind check → red.
  it("a row whose reasons are unreadable shows so, with the raw text, and the table still renders", async () => {
    auditRows = [
      auditRow(3, { reasons: { kind: "unreadable", raw: "not json {", raw_clipped: false }, disposition: "refused" }),
      auditRow(2, { reasons: { kind: "read", list: [{ row: "level", message: "not granted" }] }, disposition: "refused" }),
      auditRow(1, { disposition: null, tool: "cancel_job" }),
    ];
    render(<McpSection />);
    await screen.findByTestId("audit-table");
    const reasons = screen.getAllByTestId("audit-reasons").map((e) => e.textContent);
    expect(reasons).toEqual(["reasons unreadable: not json {", "level: not granted", "—"]);
    const rows = screen.getAllByTestId("audit-row");
    expect(within(rows[2]).getByText("outcome")).toBeTruthy(); // no disposition
  });

  // CONTROL (`audit-job-full-id`): the job cell showing the whole id → the 8-character check is red.
  it("the job cell shows the first 8 characters of the id, the whole id in its title, and — for no job", async () => {
    const id = "0b1c2d3e-4f50-6172-8394-a5b6c7d8e9f0";
    auditRows = [auditRow(2, { job_id: id }), auditRow(1, { job_id: null })];
    render(<McpSection />);
    await screen.findByTestId("audit-table");
    const [withJob, noJob] = screen.getAllByTestId("audit-job");
    expect(withJob.textContent).toBe("0b1c2d3e");
    expect(withJob.getAttribute("title")).toBe(id);
    expect(noJob.textContent).toBe("—");
    expect(noJob.getAttribute("title")).toBeNull();
  });

  it("an empty log, and a failed read, are said as such", async () => {
    auditRows = [];
    render(<McpSection />);
    expect(await screen.findByText("Claude has made no write call yet.")).toBeTruthy();
    cleanup();
    const previous = invokeMock.getMockImplementation()!;
    invokeMock.mockImplementation(async (cmd, args) => {
      if (cmd === "mcp_list_audit") throw new Error("database is locked");
      return previous(cmd, args);
    });
    render(<McpSection />);
    expect((await screen.findByTestId("audit-error")).textContent).toMatch(/Could not read the audit log: .*database is locked/);
  });
});
