/**
 * What the Jobs screen and the job detail draw for Claude's jobs (ADR-027 M4 amendment, items 6 and
 * 8): the badge, a PROPOSED job's reasons, and the disabled "Claude is submitting" in Run's place.
 * Each only arranges what `claude.ts` decides; none reads `origin`, `mcp_state` or `mcp_reasons`
 * itself, and none can throw on malformed reasons (`parseJobReasons` never does).
 */
import type { Job } from "../types";
import { CLAUDE_BADGE, firstReason, isClaudeJob, reasonsList, SUBMITTING_LABEL } from "./claude";

/** "Claude" for a job Claude created (`origin = 'mcp'`); nothing for any other job. */
export function ClaudeBadge({ job }: { job: Pick<Job, "origin"> }) {
  if (!isClaudeJob(job)) return null;
  return (
    <span className="badge tone-info claude-badge" title="Created by Claude over MCP" data-testid="claude-badge">
      {CLAUDE_BADGE}
    </span>
  );
}

/** A PROPOSED job's first reason, one line, under its status in the Jobs table; nothing otherwise. */
export function RowReason({ job }: { job: Job }) {
  const text = firstReason(job);
  if (text === null) return null;
  return (
    <div className="muted claude-reason" data-testid="claude-reason" title="Why Claude's draft was not submitted">
      {text}
    </div>
  );
}

/** A PROPOSED job's whole list of reasons (or the unreadable text) in its detail; nothing otherwise. */
export function DetailReasons({ job }: { job: Job }) {
  const list = reasonsList(job);
  if (list === null) return null;
  return (
    <div className="banner warn" style={{ marginBottom: 10 }} data-testid="proposed-reasons">
      <div>Claude proposed this job but did not submit it. Why:</div>
      {list.kind === "read" ? (
        <ul style={{ margin: "4px 0 0 18px", padding: 0 }}>
          {list.lines.map((line, i) => (
            <li key={i}>{line}</li>
          ))}
        </ul>
      ) : (
        <div className="mono" style={{ marginTop: 4, whiteSpace: "pre-wrap", wordBreak: "break-all" }}>
          {list.text}
        </div>
      )}
    </div>
  );
}

/**
 * The muted note that stands where Delete would, on a row `rowActions` answers `delete_held` for: a
 * terminal job a crash left with `mcp_state` set. No button; the title says how it ends.
 */
export function DeleteHeldNote() {
  return (
    <span
      className="muted"
      title="Claude's create_job still holds this job, so it cannot be deleted yet. It can be deleted once Claude's create_job ends, or once MCP is on and reconciles it."
      data-testid="delete-held"
    >
      held by Claude
    </span>
  );
}

/** Run's replacement while Claude is submitting the draft: disabled, no click. */
export function SubmittingButton() {
  return (
    <button className="btn btn-sm" disabled title="Claude's create_job is still submitting this draft" data-testid="claude-submitting">
      {SUBMITTING_LABEL}
    </button>
  );
}
