// @vitest-environment jsdom
/**
 * Settings → MCP, the switch row (ADR-027 M2a Part B) at screen level: the screen is rendered whole
 * with `invoke` mocked. The row's look is Anton's live WebKitGTK gate; these pin what it says and
 * what it calls.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";

const invokeMock = vi.fn<(cmd: string, args?: Record<string, unknown>) => Promise<unknown>>();
vi.mock("@tauri-apps/api/core", () => ({ invoke: (cmd: string, args?: Record<string, unknown>) => invokeMock(cmd, args) }));
vi.mock("@tauri-apps/api/event", () => ({ listen: async () => () => {} }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ confirm: async () => true, message: async () => {}, open: async () => null }));

import type { McpStatus } from "../types";
import { MCP_STATUS_POLL_MS } from "../mcp/McpSection";
import { SettingsScreen } from "./SettingsScreen";

const SOCKET = "/run/user/1000/orcastudio/mcp.sock";
const ANOTHER = "another OrcaStudio instance holds the MCP lock";

const OFF: McpStatus = {
  enabled: false,
  holds_lock: false,
  serving: false,
  stopping_tasks: null,
  connections: 0,
  socket_path: SOCKET,
  reason: "MCP is off",
  reads: { total: 0, connections: [] },
};
const SERVING: McpStatus = { ...OFF, enabled: true, holds_lock: true, serving: true, reason: null, connections: 2 };

/** What `mcp_status` answers next; `mcp_set_enabled` answers `afterToggle` and moves it there. */
let current: McpStatus;
let afterToggle: McpStatus;

beforeEach(() => {
  invokeMock.mockReset();
  current = OFF;
  afterToggle = SERVING;
  invokeMock.mockImplementation(async (cmd) => {
    switch (cmd) {
      case "get_settings":
        return {};
      case "get_cpu_presets":
      case "list_server_profiles":
        return [];
      case "api_key_status":
        return { state: "absent" };
      case "mcp_status":
        return current;
      case "mcp_set_enabled":
        current = afterToggle;
        return afterToggle;
      default:
        return null;
    }
  });
});
afterEach(() => {
  cleanup();
  vi.useRealTimers();
});

const mcpCalls = (cmd: string) => invokeMock.mock.calls.filter(([c]) => c === cmd);
const status = () => screen.findByTestId("mcp-status");
const toggle = () => screen.getByRole("checkbox", { name: /Serve OrcaStudio to an AI client over MCP/ });

describe("SettingsScreen — the MCP switch row (ADR-027 M2a)", () => {
  // CONTROL (`m2b_controls.py`, `ui-no-data-line`): the data line removed → red.
  it("states, beside the switch, what leaves the machine", async () => {
    render(<SettingsScreen />);
    await status();
    expect(
      screen.getByText(
        "When MCP is on, research data (geometries, input headers, energies and output excerpts) goes to the AI model's provider.",
      ),
    ).toBeTruthy();
    expect(toggle()).toBeTruthy();
  });

  it("serving: says this instance serves, with the socket path and the clients", async () => {
    current = SERVING;
    render(<SettingsScreen />);
    expect((await status()).textContent).toBe(`This instance serves on ${SOCKET} — 2 clients connected.`);
    expect((toggle() as HTMLInputElement).checked).toBe(true);
  });

  // CONTROL (`ui-serves-when-not-serving`): "serves" keyed on anything but `serving` → red.
  // CONTROL (`ui-reason-dropped`): the reason left out → red.
  it("not serving: shows the reason and never says it serves", async () => {
    current = { ...OFF, enabled: true, reason: ANOTHER };
    render(<SettingsScreen />);
    const line = await status();
    expect(line.textContent).toBe(`This instance does not serve: ${ANOTHER}.`);
    expect(line.textContent).not.toMatch(/serves on/);
    expect((toggle() as HTMLInputElement).checked).toBe(true);
  });

  it("shows exiting, and a stop with its task count", async () => {
    current = { ...OFF, enabled: true, holds_lock: true, reason: "exiting" };
    const { unmount } = render(<SettingsScreen />);
    expect((await status()).textContent).toBe("This instance does not serve: exiting.");
    unmount();
    current = { ...OFF, holds_lock: true, stopping_tasks: 3 };
    render(<SettingsScreen />);
    expect((await status()).textContent).toBe("This instance does not serve: MCP is off. Stopping (3 tasks).");
  });

  it("says when the switch's state is unknown, rather than showing off as fact", async () => {
    current = { ...OFF, enabled: null, reason: "MCP has not been checked yet" };
    render(<SettingsScreen />);
    await status();
    expect(screen.getByText(/whether MCP is on is unknown/)).toBeTruthy();
  });

  // CONTROL (`ui-toggle-no-call`): the toggle not calling `mcp_set_enabled` → red.
  it("the toggle calls mcp_set_enabled and re-reads the status", async () => {
    render(<SettingsScreen />);
    expect((await status()).textContent).toBe("This instance does not serve: MCP is off.");
    const readsBefore = mcpCalls("mcp_status").length;
    fireEvent.click(toggle());
    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("mcp_set_enabled", { on: true }));
    await waitFor(() => expect(mcpCalls("mcp_status").length).toBeGreaterThan(readsBefore));
    await waitFor(async () => expect((await status()).textContent).toMatch(/^This instance serves on /));

    afterToggle = OFF;
    fireEvent.click(toggle());
    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("mcp_set_enabled", { on: false }));
    await waitFor(async () => expect((await status()).textContent).toBe("This instance does not serve: MCP is off."));
  });

  it("a failed read shows the error, not the last status as if current", async () => {
    current = SERVING;
    render(<SettingsScreen />);
    await status();
    invokeMock.mockImplementation(async (cmd) => {
      if (cmd === "mcp_set_enabled") throw new Error("database is locked");
      if (cmd === "mcp_status") throw new Error("the MCP lifecycle is not set up");
      return null;
    });
    fireEvent.click(toggle());
    expect(await screen.findByText(/The switch failed: .*database is locked/)).toBeTruthy();
    expect(screen.getByText(/Could not read the MCP status: .*not set up/)).toBeTruthy();
    expect(screen.queryByTestId("mcp-status")).toBeNull();
  });

  it("re-reads the status while the screen is open, and stops when it closes", async () => {
    vi.useFakeTimers();
    const { unmount } = render(<SettingsScreen />);
    await act(async () => {});
    const first = mcpCalls("mcp_status").length;
    expect(first).toBe(1);
    current = SERVING;
    await act(async () => {
      vi.advanceTimersByTime(MCP_STATUS_POLL_MS);
    });
    expect(mcpCalls("mcp_status").length).toBe(first + 1);
    expect(screen.getByTestId("mcp-status").textContent).toMatch(/^This instance serves on /);
    unmount();
    vi.advanceTimersByTime(3 * MCP_STATUS_POLL_MS);
    expect(mcpCalls("mcp_status").length).toBe(first + 1);
  });
});
