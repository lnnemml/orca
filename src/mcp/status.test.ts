// The read count's display (ADR-027 M4 amendment item 2): this instance only, and no 0 shown as a
// fact while it does not serve.
import { describe, expect, it } from "vitest";
import type { McpStatus } from "../types";
import { connectionReadsText, readsLine } from "./status";

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

describe("readsLine", () => {
  it("shows the total while this instance serves, 0 included", () => {
    expect(readsLine({ ...OFF, serving: true, reads: { total: 0, connections: [] } })).toEqual({
      text: "Claude's reads through this instance since the app started: 0.",
      tone: "ok",
    });
    expect(readsLine({ ...OFF, serving: true, reads: { total: 42, connections: [] } }).text).toContain(": 42.");
  });

  it("says this instance does not serve instead of showing 0 as a fact", () => {
    const line = readsLine({ ...OFF, enabled: true, reason: "another OrcaStudio instance holds the MCP lock" });
    expect(line.text).toContain("does not serve");
    expect(line.text).not.toMatch(/\b0\b/);
    expect(line.tone).toBe("muted");
  });

  it("keeps an earlier count, marked as from when it served", () => {
    const line = readsLine({ ...OFF, reads: { total: 3, connections: [] } });
    expect(line.text).toContain("does not serve");
    expect(line.text).toContain("It counted 3 reads since the app started, while it served.");
  });
});

describe("connectionReadsText", () => {
  it("shows the id, a closing state and the breakdown by tool", () => {
    expect(connectionReadsText({ id: 3, state: "closing", reads: 12, by_tool: { list_jobs: 4, get_job: 8 } })).toBe(
      "#3 (closing): 12 reads — get_job 8, list_jobs 4",
    );
    expect(connectionReadsText({ id: 1, state: "open", reads: 0, by_tool: {} })).toBe("#1: 0 reads");
  });
});
