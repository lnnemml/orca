/**
 * "Cancel Claude's jobs" (ADR-027 M4 amendment item 3): the confirm, the in-progress label, and the
 * report's summary. Which jobs are Claude's and cancellable is **not** decided here: Rust selects
 * them (`mcp_claude_jobs_summary` counts with the same statement `mcp_cancel_claude_jobs` acts on),
 * and TypeScript does not re-implement the filter.
 */
import type { CancelResult, ClaudeCancelReport, ClaudeJobsSummary } from "../types";

/** Item 3's sentence, verbatim: what the button does not stop. */
export const NOT_CANCELLED_NOTE =
  "jobs Claude is submitting right now, and any it creates later, are not cancelled — revoke the grant to stop new ones";

const plural = (n: number, one: string, many: string): string => `${n} ${n === 1 ? one : many}`;

export type ClaudeCancelPlan =
  /** Nothing to cancel now; `text` says so (and names the drafts being submitted, if any). */
  | { kind: "nothing"; text: string }
  /** Ask Anton with `text`; then cancel. */
  | { kind: "confirm"; text: string; count: number };

/** The confirm for the counts Rust returned at the click. */
export function claudeCancelPlan(s: ClaudeJobsSummary): ClaudeCancelPlan {
  const submitting =
    s.submitting > 0 ? ` (${plural(s.submitting, "job is", "jobs are")} being submitted by Claude now.)` : "";
  if (s.to_cancel === 0) {
    return { kind: "nothing", text: `Claude has no queued or running job to cancel.${submitting}` };
  }
  return {
    kind: "confirm",
    count: s.to_cancel,
    text:
      `Cancel ${s.to_cancel} of Claude's jobs (queued, running or cancelling; queued first)? ` +
      `Note: ${NOT_CANCELLED_NOTE}.${submitting}`,
  };
}

/** The button's label while the command runs (remote cancels make ssh calls: it can take minutes). */
export function cancellingLabel(count: number): string {
  return `Cancelling ${count}…`;
}

/** A per-job result in words. */
export function cancelResultText(r: CancelResult): string {
  switch (r.result) {
    case "dispatched":
      return "cancelled";
    case "delivered":
      return "cancel delivered to the server";
    case "pending":
      return "cancel recorded; the poller delivers it";
    case "already_on_server":
      return "cancel already on the server";
    case "moved_on":
      return `already ${r.status}`;
  }
}

export interface ClaudeCancelSummary {
  /** Jobs the command answered with a result. */
  done: number;
  /** Jobs whose cancel failed, with Rust's message. */
  failed: { job_id: string; error: string }[];
  skippedSubmitting: number;
  text: string;
}

/** The report in one line, every failure named with its job. */
export function summarizeClaudeCancel(report: ClaudeCancelReport): ClaudeCancelSummary {
  const failed: { job_id: string; error: string }[] = [];
  let done = 0;
  for (const j of report.jobs) {
    if ("error" in j) failed.push({ job_id: j.job_id, error: j.error });
    else done += 1;
  }
  const parts = [`${plural(done, "job", "jobs")} handled`];
  if (failed.length > 0) {
    parts.push(`${failed.length} failed: ${failed.map((f) => `${f.job_id}: ${f.error}`).join("; ")}`);
  }
  if (report.skipped_submitting > 0) {
    parts.push(`${plural(report.skipped_submitting, "job", "jobs")} being submitted skipped`);
  }
  return { done, failed, skippedSubmitting: report.skipped_submitting, text: `${parts.join("; ")}.` };
}

/** The commands and the dialog the button runs through, injected. */
export interface ClaudeCancelDeps {
  /** `mcp_claude_jobs_summary`, read at the click. */
  summary: () => Promise<ClaudeJobsSummary>;
  confirm: (text: string) => Promise<boolean>;
  /** Shown before the call: the button reads `cancellingLabel(count)`. */
  onStart: (count: number) => void;
  /** `mcp_cancel_claude_jobs`. */
  cancel: () => Promise<ClaudeCancelReport>;
}

export type ClaudeCancelOutcome =
  | { kind: "nothing"; text: string }
  | { kind: "declined" }
  | { kind: "done"; report: ClaudeCancelReport; summary: ClaudeCancelSummary };

/** The click: fresh counts, the confirm, then the cancel. */
export async function cancelClaudeJobsFlow(deps: ClaudeCancelDeps): Promise<ClaudeCancelOutcome> {
  const plan = claudeCancelPlan(await deps.summary());
  if (plan.kind === "nothing") return plan;
  if (!(await deps.confirm(plan.text))) return { kind: "declined" };
  deps.onStart(plan.count);
  const report = await deps.cancel();
  return { kind: "done", report, summary: summarizeClaudeCancel(report) };
}
