/**
 * Settings → MCP: the switch row (ADR-027 M2a, M2 amendment item 2) — the switch, decision 9's line
 * on what leaves the machine, and whether this instance serves — and, under it, M4a's panels: the read
 * count, the grant card and editor, "Cancel Claude's jobs" and the audit view. The status is advisory
 * and the 30 s tick can change it at any time, so it is re-read on mount, after every toggle, and every
 * few seconds while Settings is open; a failed read shows the error, never the last status as if it
 * were current. The same 5 s poll drives the grant card (`pollTick`).
 */
import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

import type { McpStatus } from "../types";
import { McpAudit } from "./McpAudit";
import { McpClaudeJobs } from "./McpClaudeJobs";
import { McpGrantPanel } from "./McpGrantPanel";
import { MCP_DATA_LINE, MCP_ENABLED_UNKNOWN, connectionReadsText, mcpStatusLine, readsLine, type McpTone } from "./status";

/** How often the open screen re-reads the status. */
export const MCP_STATUS_POLL_MS = 5000;

const TONE_COLOR: Record<McpTone, string> = {
  ok: "var(--ok)",
  warn: "var(--warn)",
  muted: "var(--muted)",
};

export function McpSection() {
  const [status, setStatus] = useState<McpStatus | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [switching, setSwitching] = useState(false);
  const [switchError, setSwitchError] = useState<string | null>(null);
  /** Bumped on every poll: the grant card re-reads with it. */
  const [pollTick, setPollTick] = useState(0);

  const load = useCallback(async () => {
    try {
      setStatus(await invoke<McpStatus>("mcp_status"));
      setError(null);
    } catch (e) {
      setStatus(null);
      setError(String(e));
    }
  }, []);

  useEffect(() => {
    void load();
    const timer = window.setInterval(() => {
      void load();
      setPollTick((t) => t + 1);
    }, MCP_STATUS_POLL_MS);
    return () => window.clearInterval(timer);
  }, [load]);

  const toggle = async (on: boolean) => {
    setSwitching(true);
    setSwitchError(null);
    try {
      setStatus(await invoke<McpStatus>("mcp_set_enabled", { on }));
    } catch (e) {
      setSwitchError(String(e));
    } finally {
      setSwitching(false);
    }
    // Re-read either way: the row and the serving state are Rust's, not what was asked for.
    await load();
  };

  const line = status ? mcpStatusLine(status) : null;
  const reads = status ? readsLine(status) : null;

  return (
    <div className="card" style={{ marginTop: 12 }}>
      <div className="field">
        <label className="label">MCP</label>
        <label
          className="radio-row"
          style={{ display: "flex", gap: 8, alignItems: "flex-start", marginBottom: 4 }}
        >
          <input
            type="checkbox"
            checked={status?.enabled === true}
            disabled={switching || status === null}
            onChange={(e) => void toggle(e.currentTarget.checked)}
          />
          <span>
            <span style={{ fontWeight: 500 }}>Serve OrcaStudio to an AI client over MCP</span>
            <div style={{ fontSize: 12, color: "var(--muted)" }}>{MCP_DATA_LINE}</div>
          </span>
        </label>
        <div style={{ fontSize: 12, marginTop: 2, overflowWrap: "anywhere" }}>
          {error ? (
            <span style={{ color: "var(--err)" }}>Could not read the MCP status: {error}</span>
          ) : line ? (
            <span data-testid="mcp-status" style={{ color: TONE_COLOR[line.tone] }}>
              {line.text}
            </span>
          ) : (
            <span style={{ color: "var(--muted)" }}>Reading the MCP status…</span>
          )}
        </div>
        {status?.enabled === null ? (
          <div style={{ fontSize: 12, color: "var(--warn)", marginTop: 2 }}>{MCP_ENABLED_UNKNOWN}</div>
        ) : null}
        {switchError ? (
          <div style={{ fontSize: 12, color: "var(--err)", marginTop: 2 }}>
            The switch failed: {switchError}
          </div>
        ) : null}
        {status && reads ? (
          <div data-testid="mcp-reads" style={{ fontSize: 12, marginTop: 6, overflowWrap: "anywhere" }}>
            <div data-testid="mcp-reads-line" style={{ color: TONE_COLOR[reads.tone] }}>
              {reads.text}
            </div>
            {status.reads.connections.length > 0 ? (
              <ul style={{ margin: "2px 0 0 18px", padding: 0, color: "var(--muted)" }}>
                {status.reads.connections.map((c) => (
                  <li key={c.id} data-testid="mcp-reads-connection">
                    {connectionReadsText(c)}
                  </li>
                ))}
              </ul>
            ) : null}
          </div>
        ) : null}
      </div>
      <McpGrantPanel pollTick={pollTick} />
      <McpClaudeJobs />
      <McpAudit />
    </div>
  );
}
