/**
 * Settings → MCP, "Cancel Claude's jobs" (ADR-027 M4 amendment item 3). The click runs
 * `cancelClaudeJobsFlow` (`claudeJobs.ts`): the count comes from `mcp_claude_jobs_summary` read at the
 * click, the confirm is Part A's text, and only then does `mcp_cancel_claude_jobs` run. While it runs
 * the button reads "Cancelling N…" (remote cancels make ssh calls, so it can take minutes). When it
 * returns, the report names every job's outcome, every failure with Rust's message, and the drafts
 * Claude was still submitting, which were skipped.
 */
import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { confirm } from "@tauri-apps/plugin-dialog";

import type { ClaudeCancelReport, ClaudeJobsSummary } from "../types";
import {
  cancelClaudeJobsFlow,
  cancelResultText,
  cancellingLabel,
  type ClaudeCancelSummary,
} from "./claudeJobs";

interface Shown {
  report: ClaudeCancelReport;
  summary: ClaudeCancelSummary;
}

export function McpClaudeJobs() {
  const [busy, setBusy] = useState(false);
  /** `N` while `mcp_cancel_claude_jobs` runs. */
  const [cancelling, setCancelling] = useState<number | null>(null);
  const [shown, setShown] = useState<Shown | null>(null);
  const [note, setNote] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  const run = async () => {
    setBusy(true);
    setShown(null);
    setNote(null);
    setError(null);
    try {
      const outcome = await cancelClaudeJobsFlow({
        summary: () => invoke<ClaudeJobsSummary>("mcp_claude_jobs_summary"),
        confirm: (text) => confirm(text, { title: "Cancel Claude's jobs", kind: "warning" }),
        onStart: (count) => setCancelling(count),
        cancel: () => invoke<ClaudeCancelReport>("mcp_cancel_claude_jobs"),
      });
      if (outcome.kind === "nothing") setNote(outcome.text);
      else if (outcome.kind === "declined") setNote("Nothing was cancelled.");
      else setShown({ report: outcome.report, summary: outcome.summary });
    } catch (e) {
      setError(String(e));
    } finally {
      setCancelling(null);
      setBusy(false);
    }
  };

  return (
    <div data-testid="claude-jobs" style={{ marginTop: 14, borderTop: "1px solid var(--border)", paddingTop: 10 }}>
      <label className="label">Claude's jobs</label>
      <div className="row" style={{ marginTop: 6 }}>
        <button className="btn btn-danger" onClick={() => void run()} disabled={busy}>
          {cancelling !== null ? cancellingLabel(cancelling) : busy ? "Checking…" : "Cancel Claude's jobs"}
        </button>
      </div>
      {error ? (
        <div data-testid="claude-jobs-error" className="banner err" style={{ marginTop: 6 }}>
          {error}
        </div>
      ) : null}
      {note ? (
        <div data-testid="claude-jobs-note" style={{ fontSize: 12, color: "var(--muted)", marginTop: 6 }}>
          {note}
        </div>
      ) : null}
      {shown ? (
        <div data-testid="claude-jobs-report" style={{ fontSize: 12, marginTop: 6 }}>
          <div style={{ color: shown.summary.failed.length > 0 ? "var(--err)" : "var(--ok)" }}>
            {shown.summary.text}
          </div>
          <ul style={{ margin: "4px 0 0 18px", padding: 0 }}>
            {shown.report.jobs.map((j) => (
              <li key={j.job_id} data-testid="claude-jobs-outcome">
                <span className="mono">{j.job_id}</span>:{" "}
                {"error" in j ? (
                  <span style={{ color: "var(--err)" }}>failed — {j.error}</span>
                ) : (
                  cancelResultText(j.result)
                )}
              </li>
            ))}
          </ul>
          {shown.report.skipped_submitting > 0 ? (
            <div data-testid="claude-jobs-skipped" style={{ color: "var(--muted)", marginTop: 4 }}>
              {shown.report.skipped_submitting} draft{shown.report.skipped_submitting === 1 ? "" : "s"} Claude was still
              submitting {shown.report.skipped_submitting === 1 ? "was" : "were"} skipped. Revoke the grant to stop new
              ones.
            </div>
          ) : null}
        </div>
      ) : null}
    </div>
  );
}
