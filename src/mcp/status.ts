/**
 * Pure display mapping for Settings → MCP (ADR-027 M2a). Every decision was made in Rust — whether
 * this instance serves, and why not; this module only turns `McpStatus` into words. It never infers
 * "serves" from anything but `serving`.
 */
import type { McpConnectionReads, McpStatus } from "../types";

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

/**
 * The read count's line (ADR-027 M4 amendment item 2). The count covers **this instance only**: while
 * it does not serve, another instance may be serving Claude, so a total here is never shown as "how
 * much Claude read" — the line says this instance does not serve, and shows no 0 as a fact.
 */
export function readsLine(s: McpStatus): McpLine {
  const { total } = s.reads;
  if (s.serving) {
    return { text: `Claude's reads through this instance since the app started: ${total}.`, tone: "ok" };
  }
  const earlier =
    total > 0 ? ` It counted ${plural(total, "read", "reads")} since the app started, while it served.` : "";
  return {
    text: `This instance does not serve, so it counts no reads; another instance may be serving.${earlier}`,
    tone: "muted",
  };
}

/** One connection's entry: "#3 (closing): 12 reads — get_job 8, list_jobs 4". */
export function connectionReadsText(c: McpConnectionReads): string {
  const tools = Object.entries(c.by_tool)
    .sort(([a], [b]) => a.localeCompare(b))
    .map(([tool, n]) => `${tool} ${n}`)
    .join(", ");
  const state = c.state === "closing" ? " (closing)" : "";
  return `#${c.id}${state}: ${plural(c.reads, "read", "reads")}${tools ? ` — ${tools}` : ""}`;
}
