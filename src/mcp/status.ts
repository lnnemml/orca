/**
 * Pure display mapping for Settings → MCP (ADR-027 M2a). Every decision was made in Rust — whether
 * this instance serves, and why not; this module only turns `McpStatus` into words. It never infers
 * "serves" from anything but `serving`.
 */
import type { McpStatus } from "../types";

export type McpTone = "ok" | "warn" | "muted";

export interface McpLine {
  text: string;
  tone: McpTone;
}

/** Decision 9's one line on what leaves the machine, shown beside the switch. */
export const MCP_DATA_LINE =
  "When MCP is on, research data (geometries, input headers, energies and output excerpts) goes to the AI model's provider.";

function plural(n: number, one: string, many: string): string {
  return `${n} ${n === 1 ? one : many}`;
}

/** Whether this instance serves, with the socket and the clients; or the reason it does not. */
export function mcpStatusLine(s: McpStatus): McpLine {
  if (s.serving) {
    const where = s.socket_path ?? "a socket whose path is unknown";
    return {
      text: `This instance serves on ${where} — ${plural(s.connections, "client", "clients")} connected.`,
      tone: "ok",
    };
  }
  const reason = s.reason ?? "no reason was reported";
  const stopping =
    s.stopping_tasks !== null ? ` Stopping (${plural(s.stopping_tasks, "task", "tasks")}).` : "";
  return {
    text: `This instance does not serve: ${reason}.${stopping}`,
    tone: s.enabled === false && s.stopping_tasks === null ? "muted" : "warn",
  };
}

/** Said when the switch's row could not be read: the checkbox then shows "off", which is not known. */
export const MCP_ENABLED_UNKNOWN =
  "The switch's setting could not be read, so whether MCP is on is unknown.";
