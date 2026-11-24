import { beforeEach, describe, expect, it, vi } from "vitest";
import type { Job, JobStatus, RemoteState } from "../types";
import { rowActions } from "./remote";
import {
  bulkDeleteConfirmText,
  bulkDeleteFlow,
  bulkDeleteNothingText,
  bulkDeleteSummary,
  planBulkDelete,
  runBulkDelete,
  selectionAfterBulkDelete,
  type BulkDeleteDeps,
  type BulkDeletePlan,
  type BulkDeleteResult,
} from "./bulk-delete";

// The real `rowActions`, wrapped in a spy: the F1 tests pin that the plan ASKS it (and obeys its
// answer) instead of keeping a status list of its own.
vi.mock("./remote", async (importOriginal) => {
  const actual = await importOriginal<typeof import("./remote")>();
  return { ...actual, rowActions: vi.fn(actual.rowActions) };
});

const COORDS = { remote_host: "uni", remote_job_dir: "/home/anton/.orcastudio/jobs/j1", remote_socket: "/tmp/ts-slot-0" };
const LOCAL = { remote_host: null, remote_job_dir: null, remote_socket: null };

function job(over: Partial<Job> = {}): Job {
  return {
    id: "j1",
    title: "a job",
    input_content: "! HF",
    status: "completed",
    job_dir: null,
    energy: null,
    wall_time: null,
    error_message: null,
    created_at: "2026-10-06",
    started_at: null,
    completed_at: null,
    scene_json: null,
    scene_log_json: null,
    pathway_id: null,
    group_id: null,
    backend_id: "p1",
    ...LOCAL,
    cancel_requested_at: null,
    cancel_sent_at: null,
    submit_pending_since: null,
    origin: "user",
    mcp_reasons: null,
    grant_id: null,
    mcp_state: null,
    mcp_target: null,
    ...over,
  };
}

function record(over: Partial<RemoteState> = {}): RemoteState {
  return {
    known: null,
    check_error: null,
    fetch: { strikes: 0, stopped: false },
    last_contact: null,
    cancel_sends: { failures: 0, stopped: false },
    sweeps: { count: 0, stopped: false },
    pending_submit: null,
    undecided: null,
    re_enqueue_waiting: null,
    ...over,
  };
}

const noRecord = () => null;
const ids = (js: readonly Job[]) => js.map((j) => j.id);
const all = (...js: Job[]) => new Set(js.map((j) => j.id));
const plan = (js: Job[], selected: ReadonlySet<string> = all(...js), visible: ReadonlySet<string> = all(...js)) =>
  planBulkDelete(selected, js, visible, noRecord);

describe("planBulkDelete: which rows are deletable (the rowActions predicate)", () => {
  // CONTROL: make the deletable predicate true for every row → the live-skip tests go red.
  it.each<JobStatus>(["draft", "completed", "parsed", "failed", "cancelled", "lost"])("deletes a local %s job", (status) => {
    const j = job({ id: status, status });
    expect(ids(plan([j]).deletable)).toEqual([status]);
    expect(plan([j]).skipped).toEqual([]);
  });

  it.each<JobStatus>(["completed", "parsed", "failed", "cancelled", "lost"])("deletes a remote %s job", (status) => {
    const j = job({ id: status, status, ...COORDS });
    expect(ids(plan([j]).deletable)).toEqual([status]);
  });

  it.each<JobStatus>(["queued", "running"])("skips a local %s job", (status) => {
    const j = job({ id: status, status });
    const p = plan([j]);
    expect(ids(p.skipped)).toEqual([status]);
    expect(p.deletable).toEqual([]);
  });

  it.each<JobStatus>(["queued", "running", "cancelling"])("skips a remote %s job", (status) => {
    const j = job({ id: status, status, ...COORDS });
    const p = planBulkDelete(all(j), [j], all(j), () => record());
    expect(ids(p.skipped)).toEqual([status]);
    expect(p.deletable).toEqual([]);
  });

  it("skips a live remote job whose coordinates are corrupt (no record either)", () => {
    const j = job({ id: "bad", status: "running", ...COORDS, remote_socket: null });
    const p = plan([j]);
    expect(ids(p.skipped)).toEqual(["bad"]);
    expect(p.deletable).toEqual([]);
  });

  it("uses the record of the job being planned", () => {
    const a = job({ id: "a", status: "running", ...COORDS });
    const seen: string[] = [];
    planBulkDelete(all(a), [a], all(a), (id) => (seen.push(id), record()));
    expect(seen).toEqual(["a"]);
  });

  it("follows the jobs order, not the selection's insertion order, and ignores unselected jobs", () => {
    const [a, b, c, d] = ["a", "b", "c", "d"].map((id) => job({ id }));
    const p = planBulkDelete(new Set(["c", "a", "d"]), [a, b, c, d], all(a, b, c, d), noRecord);
    expect(ids(p.deletable)).toEqual(["a", "c", "d"]);
  });

  it("splits a mixed selection into deletable and skipped, in order", () => {
    const js = [job({ id: "a", status: "running" }), job({ id: "b" }), job({ id: "c", status: "queued" }), job({ id: "d", status: "failed" })];
    const p = plan(js);
    expect(ids(p.deletable)).toEqual(["b", "d"]);
    expect(ids(p.skipped)).toEqual(["a", "c"]);
  });
});

describe("planBulkDelete: corrupt coordinates (F2)", () => {
  const CORRUPT = { ...COORDS, remote_socket: null };
  it.each<JobStatus>(["queued", "running", "cancelling"])("skips a corrupt-coordinates %s job", (status) => {
    const p = plan([job({ id: status, status, ...CORRUPT })]);
    expect(ids(p.skipped)).toEqual([status]);
    expect(p.deletable).toEqual([]);
  });

  it.each<JobStatus>(["completed", "parsed", "failed", "cancelled", "lost"])("deletes a corrupt-coordinates %s job (terminal rows are never acted on remotely)", (status) => {
    const p = plan([job({ id: status, status, ...CORRUPT })]);
    expect(ids(p.deletable)).toEqual([status]);
    expect(p.skipped).toEqual([]);
  });
});

describe("planBulkDelete asks rowActions (F1)", () => {
  beforeEach(() => {
    // Braces: a returned function would be run by vitest as a teardown hook.
    vi.mocked(rowActions).mockClear();
  });

  // CONTROL: replace the predicate with a status list (`!(queued|running|cancelling)`) → red.
  it("calls it once per selected job with that job's record, and not for unselected jobs", () => {
    const a = job({ id: "a", ...COORDS });
    const b = job({ id: "b", ...COORDS });
    const c = job({ id: "c" });
    const recA = record({ check_error: null });
    const recB = record();
    const recs: Record<string, RemoteState> = { a: recA, b: recB };
    planBulkDelete(new Set(["a", "b"]), [a, b, c], all(a, b, c), (id) => recs[id]);
    const calls = vi.mocked(rowActions).mock.calls;
    expect(calls).toHaveLength(2);
    expect(calls[0][0]).toBe(a);
    expect(calls[0][1]).toBe(recA);
    expect(calls[1][0]).toBe(b);
    expect(calls[1][1]).toBe(recB);
  });

  // CONTROL: same swap → red (a completed job is deletable by status, but rowActions says no).
  it("obeys its answer: no delete offered makes even a terminal job skipped", () => {
    const j = job({ id: "t", status: "completed" });
    vi.mocked(rowActions).mockReturnValueOnce([{ action: "open" }]);
    const p = plan([j]);
    expect(p.deletable).toEqual([]);
    expect(ids(p.skipped)).toEqual(["t"]);
  });

  it("obeys its answer the other way: a delete offered makes even a live-looking job deletable", () => {
    vi.mocked(rowActions).mockReturnValueOnce([{ action: "delete" }]);
    expect(ids(plan([job({ id: "r", status: "running" })]).deletable)).toEqual(["r"]);
  });
});

describe("planBulkDelete: the three kinds of skipped row (F3, L1)", () => {
  const CORRUPT = { ...COORDS, remote_socket: null };
  const REQUESTED = "2026-10-18 06:18:01";
  const SENT = "2026-10-18 06:18:05";
  const STOPPED = record({ cancel_sends: { failures: 3, stopped: true } });

  // The rows are real `rowActions` shapes: a first Cancel, Retry cancel / Undo cancel only, nothing.
  const localRunning = job({ id: "local-running", status: "running" });
  const remoteRunning = job({ id: "remote-running", status: "running", ...COORDS });
  const cancelling = job({ id: "cancelling", status: "cancelling", ...COORDS });
  const pendingUnsent = job({ id: "pending-unsent", status: "queued", ...COORDS, cancel_requested_at: REQUESTED });
  const pendingUnsentStopped = job({ id: "pending-unsent-stopped", status: "queued", ...COORDS, cancel_requested_at: REQUESTED });
  const pendingSentStopped = job({ id: "pending-sent-stopped", status: "running", ...COORDS, cancel_requested_at: REQUESTED, cancel_sent_at: SENT });
  const sentResending = job({ id: "sent-resending", status: "running", ...COORDS, cancel_requested_at: REQUESTED, cancel_sent_at: SENT });
  const corrupt = job({ id: "corrupt", status: "running", ...CORRUPT });
  const js = [localRunning, remoteRunning, cancelling, pendingUnsent, pendingUnsentStopped, pendingSentStopped, sentResending, corrupt];
  const recordOf = (id: string) => (id === "pending-unsent-stopped" || id === "pending-sent-stopped" ? STOPPED : record());
  const planned = () => planBulkDelete(all(...js), js, all(...js), recordOf);

  it("offers the shapes the buckets are built from", () => {
    const kinds = (j: Job) => rowActions(j, recordOf(j.id)).map((a) => a.action);
    expect(kinds(localRunning)).toEqual(["cancel"]);
    expect(kinds(remoteRunning)).toEqual(["cancel"]);
    expect(kinds(cancelling)).toEqual(["retry_cancel"]);
    expect(kinds(pendingUnsent)).toEqual(["undo_cancel"]);
    expect(kinds(pendingUnsentStopped)).toEqual(["undo_cancel", "retry_cancel"]);
    expect(kinds(pendingSentStopped)).toEqual(["retry_cancel"]);
    expect(kinds(sentResending)).toEqual([]);
    expect(kinds(corrupt)).toEqual([]);
  });

  // CONTROL: put the under-way rows into the first-Cancel bucket (never fill skippedCancelUnderway) → red.
  it("puts a cancel already under way apart from a row that can be cancelled and from one that offers nothing", () => {
    const p = planned();
    expect(ids(p.skipped)).toEqual(js.map((j) => j.id));
    expect(ids(p.skippedCancelUnderway)).toEqual(["cancelling", "pending-unsent", "pending-unsent-stopped", "pending-sent-stopped"]);
    expect(ids(p.skippedNoCancel)).toEqual(["sent-resending", "corrupt"]);
  });

  it("has the cancellable rows left over, and empty buckets when every skipped row can be cancelled or nothing is skipped", () => {
    const p = plan([job({ id: "q", status: "queued" })]);
    expect(ids(p.skipped)).toEqual(["q"]);
    expect(p.skippedNoCancel).toEqual([]);
    expect(p.skippedCancelUnderway).toEqual([]);
    expect(plan([job({ id: "d" })]).skippedCancelUnderway).toEqual([]);
  });
});

describe("planBulkDelete: a job Claude holds (ADR-027 M4, item 6)", () => {
  const SUBMITTING = { origin: "mcp", grant_id: "g1", mcp_state: "submitting" } as const;

  // CONTROL: make rowActions offer Delete for a submitting draft → it is deletable, and this goes red.
  it("skips it in its own bucket, not as live or as cancel-less", () => {
    const sub = job({ id: "sub", status: "draft", ...SUBMITTING });
    const plain = job({ id: "plain", status: "draft" });
    const p = plan([sub, plain]);
    expect(ids(p.deletable)).toEqual(["plain"]);
    expect(ids(p.skipped)).toEqual(["sub"]);
    expect(ids(p.skippedSubmitting)).toEqual(["sub"]);
    expect(p.skippedNoCancel).toEqual([]);
    expect(p.skippedCancelUnderway).toEqual([]);
  });

  // A terminal row a crash left flagged: `rowActions` answers delete_held, the plan skips it the same way.
  // CONTROL: let `offersSubmitting` ignore `delete_held` -> it lands in the no-cancel bucket -> red.
  it("skips a terminal job that still carries mcp_state in the same bucket", () => {
    const stuck = job({ id: "stuck", status: "completed", origin: "mcp", grant_id: "g1", mcp_state: "submitting" });
    const p = plan([stuck, job({ id: "plain" })]);
    expect(ids(p.deletable)).toEqual(["plain"]);
    expect(ids(p.skippedSubmitting)).toEqual(["stuck"]);
    expect(p.skippedNoCancel).toEqual([]);
  });

  it("words it as Claude's, never as 'cancel them first'", () => {
    const sub = [job({ id: "sub", status: "draft", ...SUBMITTING })];
    const t = bulkDeleteNothingText(mkPlan({ skipped: sub, skippedSubmitting: sub }));
    expect(t.split("\n")).toEqual([
      "None of the selected jobs can be deleted.",
      "1 job held by Claude will be skipped — it can be deleted once Claude's create_job ends or MCP reconciles it.",
    ]);
    const summary = bulkDeleteSummary(mkPlan({ deletable: [], skipped: sub, skippedSubmitting: sub }), { deleted: [], failed: [] });
    expect(summary).toBe("Deleted 0; skipped 1 (held by Claude)");
    // The same words for a flagged terminal row (DeleteHeldNote, Rust's refusal), and the plural.
    const two = [job({ id: "a", status: "completed", mcp_state: "submitting" }), job({ id: "b", status: "draft", mcp_state: "submitting" })];
    expect(bulkDeleteNothingText(mkPlan({ skipped: two, skippedSubmitting: two })).split("\n")[1]).toBe(
      "2 jobs held by Claude will be skipped — they can be deleted once Claude's create_job ends or MCP reconciles them.",
    );
  });
});

describe("planBulkDelete: hidden and missing", () => {
  it("counts the deletable jobs the filter hides, not the skipped or visible ones", () => {
    const js = [job({ id: "a" }), job({ id: "b" }), job({ id: "c", status: "running" }), job({ id: "d" })];
    const p = planBulkDelete(all(...js), js, new Set(["a", "d"]), noRecord);
    expect(ids(p.deletable)).toEqual(["a", "b", "d"]);
    expect(p.hiddenDeletable).toBe(1); // b; c is hidden but not deletable
  });

  it("reports selected ids that are no longer among the jobs", () => {
    const js = [job({ id: "a" })];
    const p = planBulkDelete(new Set(["a", "gone1", "gone2"]), js, all(...js), noRecord);
    expect(p.missing).toEqual(["gone1", "gone2"]);
    expect(ids(p.deletable)).toEqual(["a"]);
  });

  it("plans nothing for an empty selection", () => {
    const p = planBulkDelete(new Set(), [job()], new Set(), noRecord);
    expect(p).toEqual({ deletable: [], skipped: [], skippedNoCancel: [], skippedCancelUnderway: [], skippedSubmitting: [], missing: [], hiddenDeletable: 0 });
  });
});

describe("runBulkDelete", () => {
  const jobs3 = [job({ id: "a", title: "A" }), job({ id: "b", title: "B" }), job({ id: "c", title: "C" })];

  it("deletes every job and reports the ids in order", async () => {
    const called: string[] = [];
    const r = await runBulkDelete(jobs3, async (id) => void called.push(id));
    expect(called).toEqual(["a", "b", "c"]);
    expect(r).toEqual({ deleted: ["a", "b", "c"], failed: [] });
  });

  // CONTROL: stop (or rethrow) at the first failure → red.
  it("goes on after a mid-list failure and reports it with the error text", async () => {
    const called: string[] = [];
    const r = await runBulkDelete(jobs3, async (id) => {
      called.push(id);
      if (id === "b") throw "Job is running — cancel it first";
    });
    expect(called).toEqual(["a", "b", "c"]);
    expect(r.deleted).toEqual(["a", "c"]);
    expect(r.failed).toEqual([{ job: jobs3[1], error: "Job is running — cancel it first" }]);
  });

  it("turns an Error into its string and survives a synchronous throw", async () => {
    const r = await runBulkDelete(jobs3.slice(0, 2), () => {
      throw new Error("boom");
    });
    expect(r.deleted).toEqual([]);
    expect(r.failed.map((f) => f.error)).toEqual(["Error: boom", "Error: boom"]);
  });

  // CONTROL: run them with Promise.all → the second call starts before the first resolves → red.
  it("starts the next delete only after the previous one resolved", async () => {
    const log: string[] = [];
    const gates = new Map<string, () => void>();
    const run = runBulkDelete(jobs3, (id) => {
      log.push(`start ${id}`);
      return new Promise<void>((resolve) => {
        gates.set(id, () => {
          log.push(`end ${id}`);
          resolve();
        });
      });
    });
    const tick = () => new Promise((r) => setTimeout(r, 0));
    await tick();
    expect(log).toEqual(["start a"]);
    gates.get("a")!();
    await tick();
    expect(log).toEqual(["start a", "end a", "start b"]);
    gates.get("b")!();
    await tick();
    gates.get("c")!();
    const r = await run;
    expect(log).toEqual(["start a", "end a", "start b", "end b", "start c", "end c"]);
    expect(r.deleted).toEqual(["a", "b", "c"]);
  });

  it("does nothing for no jobs", async () => {
    expect(await runBulkDelete([], async () => {})).toEqual({ deleted: [], failed: [] });
  });
});

const mkPlan = (over: Partial<BulkDeletePlan> = {}): BulkDeletePlan => ({ deletable: [], skipped: [], skippedNoCancel: [], skippedCancelUnderway: [], skippedSubmitting: [], missing: [], hiddenDeletable: 0, ...over });
const some = (n: number, prefix: string, over: Partial<Job> = {}) => Array.from({ length: n }, (_, i) => job({ id: `${prefix}${i}`, ...over }));
const live = (n: number) => some(n, "live", { status: "running" });
const underway = (n: number) => some(n, "underway", { status: "cancelling", ...COORDS });
const noCancel = (n: number) => some(n, "nocancel", { status: "running", ...COORDS, remote_socket: null });

describe("bulkDeleteConfirmText", () => {
  const plain = (n: number) => mkPlan({ deletable: some(n, "d") });

  it("is null when nothing is deletable", () => {
    expect(bulkDeleteConfirmText(mkPlan({ skipped: live(1) }))).toBeNull();
  });

  it("states the count, the local files and that it cannot be undone", () => {
    const t = bulkDeleteConfirmText(plain(3))!;
    expect(t).toContain("Permanently delete 3 jobs");
    expect(t).toContain("local files");
    expect(t).toContain("cannot be undone");
    expect(bulkDeleteConfirmText(plain(1))!).toContain("1 job ");
  });

  it("omits the hidden and skipped clauses when they are zero", () => {
    const t = bulkDeleteConfirmText(plain(2))!;
    expect(t).not.toContain("hidden");
    expect(t).not.toContain("cancel them first");
    expect(t).not.toContain("no cancel available");
  });

  it("names the hidden count and the skipped live jobs that can be cancelled", () => {
    const t = bulkDeleteConfirmText({ ...plain(4), hiddenDeletable: 2, skipped: live(2) })!;
    expect(t).toContain("2 of them are hidden by the current filter");
    expect(t).toContain("2 jobs are still live and will be skipped — cancel them first");
    expect(t).not.toContain("no cancel available");
  });

  // CONTROL: word every skipped job as "cancel them first" → red (the no-cancel row gets wrong advice).
  it("words the skipped jobs that offer no cancel apart, without telling the user to cancel them", () => {
    const nc = noCancel(1);
    const t = bulkDeleteConfirmText({ ...plain(1), skipped: [...live(2), ...nc], skippedNoCancel: nc })!;
    expect(t).toContain("2 jobs are still live and will be skipped — cancel them first");
    expect(t).toContain("1 job is live with no cancel available and will be skipped — see the job");
    const only = bulkDeleteConfirmText({ ...plain(1), skipped: nc, skippedNoCancel: nc })!;
    expect(only).not.toContain("cancel them first");
  });

  // CONTROL: word the under-way rows as "cancel them first" → red.
  it("words a cancel already under way apart: wait, do not cancel again", () => {
    const uw = underway(2);
    const t = bulkDeleteConfirmText({ ...plain(1), skipped: [...live(1), ...uw, ...noCancel(1)], skippedCancelUnderway: uw, skippedNoCancel: noCancel(1) })!;
    expect(t).toContain("1 job is still live and will be skipped — cancel it first");
    expect(t).toContain("2 jobs have a cancel already under way and will be skipped — wait for it to finish");
    const only = bulkDeleteConfirmText({ ...plain(1), skipped: uw, skippedCancelUnderway: uw })!;
    expect(only).not.toContain("cancel them first");
    expect(only).not.toContain("no cancel available");
  });
});

describe("bulkDeleteNothingText", () => {
  it("says none can be deleted and why, with the same two skip wordings", () => {
    const nc = noCancel(1);
    const t = bulkDeleteNothingText(mkPlan({ skipped: [...live(2), ...nc], skippedNoCancel: nc }));
    expect(t.split("\n")).toEqual([
      "None of the selected jobs can be deleted.",
      "2 jobs are still live and will be skipped — cancel them first.",
      "1 job is live with no cancel available and will be skipped — see the job.",
    ]);
  });

  it("words the three kinds of skipped row on their own lines, each only when non-zero", () => {
    const uw = underway(1);
    const nc = noCancel(1);
    const t = bulkDeleteNothingText(mkPlan({ skipped: [...live(1), ...uw, ...nc], skippedCancelUnderway: uw, skippedNoCancel: nc }));
    expect(t.split("\n")).toEqual([
      "None of the selected jobs can be deleted.",
      "1 job is still live and will be skipped — cancel it first.",
      "1 job has a cancel already under way and will be skipped — wait for it to finish.",
      "1 job is live with no cancel available and will be skipped — see the job.",
    ]);
    expect(bulkDeleteNothingText(mkPlan({ skipped: uw, skippedCancelUnderway: uw })).split("\n")).toEqual([
      "None of the selected jobs can be deleted.",
      "1 job has a cancel already under way and will be skipped — wait for it to finish.",
    ]);
  });

  it("agrees the verb and the object with the count in all three lines", () => {
    const nc = noCancel(2);
    const uw = underway(2);
    const t = bulkDeleteNothingText(mkPlan({ skipped: [...live(3), ...uw, ...nc], skippedCancelUnderway: uw, skippedNoCancel: nc }));
    expect(t.split("\n").slice(1)).toEqual([
      "3 jobs are still live and will be skipped — cancel them first.",
      "2 jobs have a cancel already under way and will be skipped — wait for it to finish.",
      "2 jobs are live with no cancel available and will be skipped — see the jobs.",
    ]);
  });

  it("says the jobs are gone when only vanished ids were selected", () => {
    expect(bulkDeleteNothingText(mkPlan({ missing: ["x"] }))).toBe("The selected jobs no longer exist.");
  });
});

describe("bulkDeleteSummary", () => {
  const r = (deleted: string[], failed: BulkDeleteResult["failed"] = []): BulkDeleteResult => ({ deleted, failed });

  it("is just the deleted count when everything went", () => {
    expect(bulkDeleteSummary(mkPlan(), r(["a", "b"]))).toBe("Deleted 2");
  });

  it("reports deleted, skipped and failed, each failure on its own line with title and error", () => {
    const f1 = { job: job({ id: "f1", title: "Water opt" }), error: "disk locked" };
    const f2 = { job: job({ id: "f2", title: "NH3 scan" }), error: "Job is running" };
    const t = bulkDeleteSummary(mkPlan({ skipped: live(1) }), r(["a"], [f1, f2]));
    expect(t.split("\n")).toEqual([
      "Deleted 1; skipped 1 (still live — cancel them first); failed 2",
      "Water opt: disk locked",
      "NH3 scan: Job is running",
    ]);
  });

  it("words the skipped jobs with and without a cancel as two clauses, each only when non-zero", () => {
    const nc = noCancel(1);
    expect(bulkDeleteSummary(mkPlan({ skipped: [...live(2), ...nc], skippedNoCancel: nc }), r(["a"]))).toBe(
      "Deleted 1; skipped 2 (still live — cancel them first); skipped 1 (live, no cancel available — see the job)",
    );
    expect(bulkDeleteSummary(mkPlan({ skipped: nc, skippedNoCancel: nc }), r([]))).toBe("Deleted 0; skipped 1 (live, no cancel available — see the job)");
  });

  it("words a cancel already under way as its own clause", () => {
    const uw = underway(2);
    const nc = noCancel(1);
    expect(bulkDeleteSummary(mkPlan({ skipped: [...live(1), ...uw, ...nc], skippedCancelUnderway: uw, skippedNoCancel: nc }), r(["a"]))).toBe(
      "Deleted 1; skipped 1 (still live — cancel them first); skipped 2 (cancel already under way — wait for it to finish); skipped 1 (live, no cancel available — see the job)",
    );
    expect(bulkDeleteSummary(mkPlan({ skipped: uw, skippedCancelUnderway: uw }), r([]))).toBe("Deleted 0; skipped 2 (cancel already under way — wait for it to finish)");
  });

  it("mentions already-gone ids only when there are some", () => {
    expect(bulkDeleteSummary(mkPlan({ missing: ["g"] }), r([]))).toBe("Deleted 0; 1 already gone");
  });
});

describe("selectionAfterBulkDelete", () => {
  // CONTROL: return an empty Set (clear everything) → red.
  it("removes deleted and missing ids and keeps skipped and failed ones", () => {
    const liveJob = job({ id: "live", status: "running" });
    const bad = job({ id: "bad" });
    const selected = new Set(["a", "bad", "live", "gone", "keep-hidden"]);
    const pl = mkPlan({ deletable: [job({ id: "a" }), bad], skipped: [liveJob], missing: ["gone"] });
    const after = selectionAfterBulkDelete(selected, pl, { deleted: ["a"], failed: [{ job: bad, error: "x" }] });
    expect([...after].sort()).toEqual(["bad", "keep-hidden", "live"]);
  });

  it("does not mutate the selection it was given", () => {
    const selected = new Set(["a"]);
    selectionAfterBulkDelete(selected, mkPlan(), { deleted: ["a"], failed: [] });
    expect([...selected]).toEqual(["a"]);
  });
});

describe("bulkDeleteFlow: the click's sequence", () => {
  /** Deps that log every call in order; `answers` sets the confirm's reply. */
  function deps(log: string[], over: Partial<BulkDeleteDeps> = {}, answer = true) {
    let selection: ReadonlySet<string> = new Set(["a", "b", "live", "gone"]);
    const d: BulkDeleteDeps = {
      ask: async (text) => {
        log.push(`confirm: ${text.split("\n")[0]}`);
        return answer;
      },
      notify: async (text, kind) => void log.push(`notify(${kind}): ${text.split("\n")[0]}`),
      del: async (id) => void log.push(`del ${id}`),
      reload: async () => void log.push("reload"),
      setSelection: (update) => {
        selection = update(selection);
        log.push("setSelection");
      },
      ...over,
    };
    return { d, selection: () => selection };
  }
  const flowPlan = () => mkPlan({ deletable: [job({ id: "a" }), job({ id: "b" })], skipped: [job({ id: "live", status: "running" })], missing: ["gone"] });

  // CONTROL: delete without asking (skip the confirm) → red.
  it("deletes nothing when the confirm is declined", async () => {
    const log: string[] = [];
    const { d } = deps(log, {}, false);
    expect(await bulkDeleteFlow(flowPlan(), d)).toBe("declined");
    expect(log).toEqual(["confirm: Permanently delete 2 jobs and their local files? This cannot be undone."]);
  });

  it("shows the message and no confirm when nothing is deletable, and drops vanished ids", async () => {
    const log: string[] = [];
    const { d, selection } = deps(log);
    const p = mkPlan({ skipped: [job({ id: "live", status: "running" })], missing: ["gone"] });
    expect(await bulkDeleteFlow(p, d)).toBe("nothing-deletable");
    expect(log).toEqual(["setSelection", "notify(info): None of the selected jobs can be deleted."]);
    expect([...selection()].sort()).toEqual(["a", "b", "live"]);
  });

  // CONTROL: show the summary before the reload (or never) → red.
  it("on a confirmed run: confirm, one delete per deletable job, selection, reload, then the summary", async () => {
    const log: string[] = [];
    const { d, selection } = deps(log);
    expect(await bulkDeleteFlow(flowPlan(), d)).toBe("done");
    expect(log).toEqual([
      "confirm: Permanently delete 2 jobs and their local files? This cannot be undone.",
      "del a",
      "del b",
      "setSelection",
      "reload",
      "notify(info): Deleted 2; skipped 1 (still live — cancel them first); 1 already gone",
    ]);
    expect([...selection()]).toEqual(["live"]);
  });

  it("goes on after a failure, keeps the failed job selected and warns", async () => {
    const log: string[] = [];
    const { d, selection } = deps(log, {
      del: async (id) => {
        log.push(`del ${id}`);
        if (id === "a") throw "refused: running";
      },
    });
    await bulkDeleteFlow(flowPlan(), d);
    expect(log.filter((l) => l.startsWith("del"))).toEqual(["del a", "del b"]);
    expect(log[log.length - 1]).toMatch(/^notify\(warning\): Deleted 1; skipped 1 \(still live — cancel them first\); failed 1; 1 already gone/);
    expect([...selection()].sort()).toEqual(["a", "live"]);
  });

  it("still shows the outcome when the reload fails, and says so", async () => {
    const notes: string[] = [];
    const log: string[] = [];
    const { d } = deps(log, {
      reload: async () => {
        throw "ipc down";
      },
      notify: async (text) => void notes.push(text),
    });
    await bulkDeleteFlow(flowPlan(), d);
    expect(notes).toHaveLength(1);
    expect(notes[0]).toContain("Deleted 2");
    expect(notes[0]).toContain("The job list could not be refreshed: ipc down");
  });
});
