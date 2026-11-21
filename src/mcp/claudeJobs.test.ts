// "Cancel Claude's jobs" (ADR-027 M4 amendment item 3): the confirm's wording, the label while it
// runs, the report's summary, and the click's order (fresh counts, confirm, then cancel).
import { describe, expect, it, vi } from "vitest";
import type { ClaudeCancelReport } from "../types";
import {
  NOT_CANCELLED_NOTE,
  cancelClaudeJobsFlow,
  cancelResultText,
  cancellingLabel,
  claudeCancelPlan,
  summarizeClaudeCancel,
  type ClaudeCancelDeps,
} from "./claudeJobs";

describe("the confirm", () => {
  it("carries item 3's sentence verbatim and the count", () => {
    expect(NOT_CANCELLED_NOTE).toBe(
      "jobs Claude is submitting right now, and any it creates later, are not cancelled — revoke the grant to stop new ones",
    );
    const plan = claudeCancelPlan({ to_cancel: 3, submitting: 0 });
    expect(plan.kind).toBe("confirm");
    expect(plan.text).toContain("Cancel 3 of Claude's jobs");
    expect(plan.text).toContain(NOT_CANCELLED_NOTE);
    expect(plan.text).not.toContain("being submitted");
  });

  it("names the drafts being submitted, and says when there is nothing to cancel", () => {
    expect(claudeCancelPlan({ to_cancel: 2, submitting: 1 }).text).toContain("1 job is being submitted by Claude now");
    const nothing = claudeCancelPlan({ to_cancel: 0, submitting: 2 });
    expect(nothing).toEqual({
      kind: "nothing",
      text: "Claude has no queued or running job to cancel. (2 jobs are being submitted by Claude now.)",
    });
  });

  it("labels the button while the call runs", () => {
    expect(cancellingLabel(4)).toBe("Cancelling 4…");
  });
});

const REPORT: ClaudeCancelReport = {
  jobs: [
    { job_id: "q1", result: { result: "dispatched" } },
    { job_id: "r1", error: "conflict: the job is busy" },
    { job_id: "r2", result: { result: "pending", already_pending: false } },
  ],
  skipped_submitting: 1,
};

describe("the report", () => {
  it("counts the results, names every failure with its job, and the skipped drafts", () => {
    const s = summarizeClaudeCancel(REPORT);
    expect(s.done).toBe(2);
    expect(s.failed).toEqual([{ job_id: "r1", error: "conflict: the job is busy" }]);
    expect(s.text).toBe("2 jobs handled; 1 failed: r1: conflict: the job is busy; 1 job being submitted skipped.");
    expect(summarizeClaudeCancel({ jobs: [], skipped_submitting: 0 }).text).toBe("0 jobs handled.");
  });

  it("says each result in words", () => {
    expect(cancelResultText({ result: "dispatched" })).toBe("cancelled");
    expect(cancelResultText({ result: "moved_on", status: "completed" })).toBe("already completed");
    expect(cancelResultText({ result: "already_on_server" })).toBe("cancel already on the server");
  });
});

function deps(toCancel: number, answer: boolean) {
  const order: string[] = [];
  const d: ClaudeCancelDeps = {
    summary: vi.fn(async () => {
      order.push("summary");
      return { to_cancel: toCancel, submitting: 0 };
    }),
    confirm: vi.fn(async () => {
      order.push("confirm");
      return answer;
    }),
    onStart: vi.fn((n: number) => order.push(`start ${n}`)),
    cancel: vi.fn(async () => {
      order.push("cancel");
      return REPORT;
    }),
  };
  return { d, order };
}

describe("the click", () => {
  it("reads the counts, confirms, shows the label, then cancels", async () => {
    const { d, order } = deps(3, true);
    const out = await cancelClaudeJobsFlow(d);
    expect(order).toEqual(["summary", "confirm", "start 3", "cancel"]);
    expect(out.kind === "done" && out.summary.done).toBe(2);
  });

  it("cancels nothing when declined or when there is nothing to cancel", async () => {
    const declined = deps(3, false);
    expect(await cancelClaudeJobsFlow(declined.d)).toEqual({ kind: "declined" });
    expect(declined.d.cancel).not.toHaveBeenCalled();
    const none = deps(0, true);
    expect((await cancelClaudeJobsFlow(none.d)).kind).toBe("nothing");
    expect(none.order).toEqual(["summary"]);
  });
});
