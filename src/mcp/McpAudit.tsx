/**
 * Settings → MCP, the audit view (ADR-027 M4 amendment item 4). A table of `mcp_audit`, newest first:
 * time (local), tool, disposition, job, reasons, args. Rust cuts, pages and reads; this component
 * only shows.
 *
 * - **Args** are collapsed. Expanding a row whose `args_clipped` is set fetches the whole text with
 *   `mcp_audit_row`; an unclipped row already holds it whole and makes no call.
 * - **Reasons** that could not be read show as "reasons unreadable" with the raw text, never as a
 *   failed page.
 * - **Load older** is keyset paging: the next page is asked for with the last page's `next_before_id`
 *   (`appendOlder` keeps a row that is somehow shown already from appearing twice, and says so).
 *   **Refresh** reloads the newest page and forgets the older ones.
 */
import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

import type { AuditPage, AuditRowView } from "../types";
import { AUDIT_PAGE_SIZE, appendOlder, argsCell, auditTime, dispositionLabel, reasonsText } from "./audit";

/** One expanded row's args: the row's own text, or the whole text being / having been fetched. */
type Expanded = { state: "ready"; text: string } | { state: "loading" } | { state: "error"; error: string };

/** The job column's width: the first characters of the id; the whole id is in the cell's `title`. */
const JOB_ID_SHOWN = 8;

const cell = { padding: "3px 8px", verticalAlign: "top", borderBottom: "1px solid var(--border)" } as const;

export function McpAudit() {
  const [rows, setRows] = useState<AuditRowView[]>([]);
  const [nextBeforeId, setNextBeforeId] = useState<number | null>(null);
  const [loaded, setLoaded] = useState(false);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [repeated, setRepeated] = useState(0);
  const [expanded, setExpanded] = useState<Record<number, Expanded>>({});

  const refresh = useCallback(async () => {
    setLoading(true);
    try {
      const page = await invoke<AuditPage>("mcp_list_audit", { beforeId: null, limit: AUDIT_PAGE_SIZE });
      setRows(page.rows);
      setNextBeforeId(page.next_before_id);
      setRepeated(0);
      setExpanded({});
      setError(null);
      setLoaded(true);
    } catch (e) {
      setError(String(e));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const loadOlder = async () => {
    if (nextBeforeId === null) return;
    setLoading(true);
    try {
      const page = await invoke<AuditPage>("mcp_list_audit", { beforeId: nextBeforeId, limit: AUDIT_PAGE_SIZE });
      const more = appendOlder(rows, page);
      setRows(more.rows);
      setNextBeforeId(more.nextBeforeId);
      setRepeated((n) => n + more.repeated);
      setError(null);
    } catch (e) {
      setError(String(e));
    } finally {
      setLoading(false);
    }
  };

  const toggle = async (row: AuditRowView) => {
    if (expanded[row.id]) {
      setExpanded(({ [row.id]: _, ...rest }) => rest);
      return;
    }
    if (!row.args_clipped) {
      setExpanded((m) => ({ ...m, [row.id]: { state: "ready", text: row.args } }));
      return;
    }
    setExpanded((m) => ({ ...m, [row.id]: { state: "loading" } }));
    try {
      const whole = await invoke<AuditRowView>("mcp_audit_row", { id: row.id });
      // Only a row still waiting for its text takes the answer: one hidden meanwhile, or dropped by
      // Refresh, stays as it is.
      setExpanded((m) => (m[row.id]?.state === "loading" ? { ...m, [row.id]: { state: "ready", text: whole.args } } : m));
    } catch (e) {
      setExpanded((m) => (m[row.id]?.state === "loading" ? { ...m, [row.id]: { state: "error", error: String(e) } } : m));
    }
  };

  return (
    <div data-testid="audit" style={{ marginTop: 14, borderTop: "1px solid var(--border)", paddingTop: 10 }}>
      <div className="row" style={{ justifyContent: "space-between" }}>
        <label className="label">Audit log</label>
        <button className="btn btn-sm" onClick={() => void refresh()} disabled={loading}>
          Refresh
        </button>
      </div>
      {error ? (
        <div data-testid="audit-error" className="banner err" style={{ marginTop: 6 }}>
          Could not read the audit log: {error}
        </div>
      ) : null}
      {loaded && rows.length === 0 ? (
        <div style={{ fontSize: 12, color: "var(--muted)", marginTop: 6 }}>Claude has made no write call yet.</div>
      ) : null}
      {rows.length > 0 ? (
        <div style={{ maxHeight: 420, overflow: "auto", marginTop: 6 }}>
          <table data-testid="audit-table" style={{ width: "100%", borderCollapse: "collapse", fontSize: 12 }}>
            <thead>
              <tr style={{ textAlign: "left", color: "var(--muted)" }}>
                {["Time", "Tool", "Disposition", "Job", "Reasons", "Args"].map((h) => (
                  <th key={h} style={{ ...cell, fontWeight: 500 }}>
                    {h}
                  </th>
                ))}
              </tr>
            </thead>
            <tbody>
              {rows.map((row) => {
                const open = expanded[row.id];
                const args = argsCell(row);
                return (
                  <tr key={row.id} data-testid="audit-row" data-row-id={row.id}>
                    <td style={cell}>{auditTime(row)}</td>
                    <td style={cell} className="mono">
                      {row.tool}
                    </td>
                    <td style={cell}>{dispositionLabel(row)}</td>
                    <td style={{ ...cell, whiteSpace: "nowrap" }} className="mono" data-testid="audit-job" title={row.job_id ?? undefined}>
                      {row.job_id === null ? "—" : row.job_id.slice(0, JOB_ID_SHOWN)}
                    </td>
                    <td style={{ ...cell, width: "100%", wordBreak: "break-word" }} data-testid="audit-reasons">
                      {reasonsText(row.reasons)}
                    </td>
                    <td style={{ ...cell, wordBreak: "break-word" }}>
                      <button
                        className="btn btn-sm"
                        style={{ whiteSpace: "nowrap" }}
                        aria-expanded={open !== undefined}
                        aria-label={`${open ? "Hide" : "Show"} args of row ${row.id}`}
                        onClick={() => void toggle(row)}
                      >
                        {open ? "Hide" : "Show"}
                      </button>
                      {open?.state === "loading" ? <span style={{ marginLeft: 6 }}>Reading the whole text…</span> : null}
                      {open?.state === "error" ? (
                        <div data-testid="audit-args-error" style={{ color: "var(--err)" }}>
                          Could not read the row: {open.error}
                        </div>
                      ) : null}
                      {open?.state === "ready" ? (
                        <pre
                          data-testid="audit-args"
                          className="mono"
                          style={{ margin: "4px 0 0", whiteSpace: "pre-wrap", wordBreak: "break-all" }}
                        >
                          {open.text}
                        </pre>
                      ) : null}
                      {open === undefined && args.clipped ? (
                        <span style={{ marginLeft: 6, color: "var(--muted)" }}>{args.note}</span>
                      ) : null}
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        </div>
      ) : null}
      {repeated > 0 ? (
        <div data-testid="audit-repeated" style={{ fontSize: 12, color: "var(--warn)", marginTop: 6 }}>
          {repeated} row{repeated === 1 ? "" : "s"} came back twice and {repeated === 1 ? "is" : "are"} shown once.
        </div>
      ) : null}
      {nextBeforeId !== null ? (
        <div className="row" style={{ marginTop: 6 }}>
          <button className="btn btn-sm" onClick={() => void loadOlder()} disabled={loading}>
            Load older
          </button>
        </div>
      ) : loaded && rows.length > 0 ? (
        <div style={{ fontSize: 12, color: "var(--muted)", marginTop: 6 }}>No older rows.</div>
      ) : null}
    </div>
  );
}
