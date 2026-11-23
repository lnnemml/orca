/**
 * "Delete selected" on the Jobs screen: the pure logic around a loop of the existing `delete_job`.
 *
 * Which rows may be deleted is NOT decided here: it is `rowActions(job, record)` offering `delete`
 * (`remote.ts`, ADR-024 o22.14.8) — the same predicate that draws a row's Delete button — so there
 * is no second list of statuses to drift. Rust's `delete_job` stays the final authority: a row that
 * turned live between the plan and the call is refused there, and comes back here as a per-job
 * failure carrying Rust's own message.
 */
import type { Job, RemoteState } from "../types";
import { rowActions, type RowAction } from "./remote";

/** What a "Delete selected" click would do, decided before anything is asked or deleted. */
export interface BulkDeletePlan {
  /** Selected jobs whose row offers Delete, in `jobs` order. */
  deletable: Job[];
  /** Selected jobs that do not (live, or corrupt coordinates): never sent to `delete_job`. */
  skipped: Job[];
  /**
   * The part of `skipped` whose row offers no cancel-family action at all (a live row with corrupt
   * coordinates, or a cancel already sent whose re-sends run): "cancel them first" would be wrong
   * advice for these.
   */
  skippedNoCancel: Job[];
  /**
   * The part of `skipped` whose row offers Retry cancel and/or Undo cancel but no first Cancel: a
   * cancel is already under way (`cancelling`, or a pending one), so "cancel them first" would be
   * wrong advice too. The rest of `skipped` (it offers `cancel`) can be cancelled.
   */
  skippedCancelUnderway: Job[];
  /**
   * The part of `skipped` that Claude holds (`rowActions` answers `submitting` for a draft, or
   * `delete_held` for a terminal row a crash left flagged): "cancel them first" is wrong advice for
   * these — they can be deleted once Claude's `create_job` ends or MCP reconciles them. Rust refuses
   * them as well.
   */
  skippedSubmitting: Job[];
  /** Selected ids no longer among the jobs (deleted elsewhere): nothing to do, dropped from the selection. */
  missing: string[];
  /** How many of `deletable` the current search/status/group filter hides. */
  hiddenDeletable: number;
}

/** The outcome of the loop: the ids `delete_job` accepted and the jobs it refused or failed on. */
export interface BulkDeleteResult {
  deleted: string[];
  failed: { job: Job; error: string }[];
}

/** Whether a row's actions offer Delete — the single authority on "deletable" (asked of `rowActions`). */
const offersDelete = (actions: readonly RowAction[]): boolean => actions.some((a) => a.action === "delete");

/** Whether Delete is held back for the row because Claude holds it (a submitting draft, or a flagged terminal row; `rowActions`). */
const offersSubmitting = (actions: readonly RowAction[]): boolean =>
  actions.some((a) => a.action === "submitting" || a.action === "delete_held");

/** Whether a row's actions offer a first Cancel. */
const offersFirstCancel = (actions: readonly RowAction[]): boolean => actions.some((a) => a.action === "cancel");

/** Whether a row's actions offer only the follow-ups of a cancel already made (Retry / Undo cancel). */
const offersCancelFollowUp = (actions: readonly RowAction[]): boolean =>
  actions.some((a) => a.action === "retry_cancel" || a.action === "undo_cancel");

/**
 * Split the whole selection (hidden rows included) into what can be deleted and what cannot.
 * Order follows `jobs`, so the confirm, the loop and the summary are deterministic.
 */
export function planBulkDelete(
  selected: ReadonlySet<string>,
  jobs: readonly Job[],
  visibleIds: ReadonlySet<string>,
  recordOf: (id: string) => RemoteState | null | undefined,
): BulkDeletePlan {
  const deletable: Job[] = [];
  const skipped: Job[] = [];
  const skippedNoCancel: Job[] = [];
  const skippedCancelUnderway: Job[] = [];
  const skippedSubmitting: Job[] = [];
  const known = new Set<string>();
  for (const job of jobs) {
    known.add(job.id);
    if (!selected.has(job.id)) continue;
    const actions = rowActions(job, recordOf(job.id));
    if (offersDelete(actions)) {
      deletable.push(job);
      continue;
    }
    skipped.push(job);
    if (offersSubmitting(actions)) {
      skippedSubmitting.push(job);
      continue;
    }
    if (offersFirstCancel(actions)) continue;
    (offersCancelFollowUp(actions) ? skippedCancelUnderway : skippedNoCancel).push(job);
  }
  const missing = [...selected].filter((id) => !known.has(id));
  const hiddenDeletable = deletable.filter((j) => !visibleIds.has(j.id)).length;
  return { deletable, skipped, skippedNoCancel, skippedCancelUnderway, skippedSubmitting, missing, hiddenDeletable };
}

/**
 * Delete the jobs one after another (never in parallel: each `delete_job` is a DB write plus a
 * directory removal). A failure is recorded with its message and the loop goes on — it never stops
 * early and never throws.
 */
export async function runBulkDelete(jobs: readonly Job[], del: (id: string) => Promise<void>): Promise<BulkDeleteResult> {
  const result: BulkDeleteResult = { deleted: [], failed: [] };
  for (const job of jobs) {
    try {
      await del(job.id);
      result.deleted.push(job.id);
    } catch (e) {
      result.failed.push({ job, error: String(e) });
    }
  }
  return result;
}

const jobs = (n: number) => `${n} job${n === 1 ? "" : "s"}`;

/** Skipped jobs by what the user can do: cancel them first, wait for a cancel under way, or look. */
function skipCounts(plan: BulkDeletePlan): { cancellable: number; underway: number; noCancel: number; submitting: number } {
  const noCancel = plan.skippedNoCancel.length;
  const underway = plan.skippedCancelUnderway.length;
  const submitting = plan.skippedSubmitting.length;
  return { cancellable: plan.skipped.length - noCancel - underway - submitting, underway, noCancel, submitting };
}

/** The confirm lines about skipped jobs; a zero count adds nothing. */
function skipLines(plan: BulkDeletePlan): string[] {
  const { cancellable, underway, noCancel, submitting } = skipCounts(plan);
  const lines: string[] = [];
  const verb = (n: number, one: string, many: string) => (n === 1 ? one : many);
  if (cancellable > 0) {
    lines.push(`${jobs(cancellable)} ${verb(cancellable, "is", "are")} still live and will be skipped — cancel ${verb(cancellable, "it", "them")} first.`);
  }
  if (underway > 0) {
    lines.push(`${jobs(underway)} ${verb(underway, "has", "have")} a cancel already under way and will be skipped — wait for it to finish.`);
  }
  if (noCancel > 0) {
    lines.push(`${jobs(noCancel)} ${verb(noCancel, "is", "are")} live with no cancel available and will be skipped — see the ${verb(noCancel, "job", "jobs")}.`);
  }
  if (submitting > 0) {
    lines.push(`${jobs(submitting)} held by Claude will be skipped — ${verb(submitting, "it", "they")} can be deleted once Claude's create_job ends or MCP reconciles ${verb(submitting, "it", "them")}.`);
  }
  return lines;
}

/**
 * The confirm text, or null when nothing is deletable (the screen then says so instead of asking —
 * `bulkDeleteNothingText`). The hidden and skipped clauses appear only when they are not zero.
 */
export function bulkDeleteConfirmText(plan: BulkDeletePlan): string | null {
  const n = plan.deletable.length;
  if (n === 0) return null;
  const lines = [`Permanently delete ${jobs(n)} and their local files? This cannot be undone.`];
  if (plan.hiddenDeletable > 0) {
    lines.push(`${plan.hiddenDeletable} of them ${plan.hiddenDeletable === 1 ? "is" : "are"} hidden by the current filter.`);
  }
  lines.push(...skipLines(plan));
  return lines.join("\n");
}

/** What to say instead of a confirm when no selected job can be deleted. */
export function bulkDeleteNothingText(plan: BulkDeletePlan): string {
  if (plan.skipped.length === 0) return "The selected jobs no longer exist.";
  return ["None of the selected jobs can be deleted.", ...skipLines(plan)].join("\n");
}

/**
 * The one summary after the run: "Deleted N; skipped M (still live — cancel them first); skipped U
 * (cancel already under way — wait for it to finish); skipped K (live, no cancel available — see the
 * job); skipped S (held by Claude); failed F", then each failure on its own line with its
 * title and error. A zero clause is left out ("already gone" is for `missing`).
 */
export function bulkDeleteSummary(plan: BulkDeletePlan, result: BulkDeleteResult): string {
  const { cancellable, underway, noCancel, submitting } = skipCounts(plan);
  const parts = [`Deleted ${result.deleted.length}`];
  if (cancellable > 0) parts.push(`skipped ${cancellable} (still live — cancel them first)`);
  if (underway > 0) parts.push(`skipped ${underway} (cancel already under way — wait for it to finish)`);
  if (noCancel > 0) parts.push(`skipped ${noCancel} (live, no cancel available — see the job)`);
  if (submitting > 0) parts.push(`skipped ${submitting} (held by Claude)`);
  if (result.failed.length > 0) parts.push(`failed ${result.failed.length}`);
  if (plan.missing.length > 0) parts.push(`${plan.missing.length} already gone`);
  const lines = [parts.join("; ")];
  for (const f of result.failed) lines.push(`${f.job.title}: ${f.error}`);
  return lines.join("\n");
}

/** The selection after the run: deleted and already-gone ids leave it; skipped and failed stay. */
export function selectionAfterBulkDelete(
  selected: ReadonlySet<string>,
  plan: BulkDeletePlan,
  result: BulkDeleteResult,
): Set<string> {
  const next = new Set(selected);
  for (const id of result.deleted) next.delete(id);
  for (const id of plan.missing) next.delete(id);
  return next;
}

/** What the screen supplies to the click's sequence: the dialogs, the command, the reload, the selection. */
export interface BulkDeleteDeps {
  /** The plugin's confirm dialog; true = go ahead. (Named `ask`, not `confirm`: a source pin keeps the literal `confirm:` out of the screens.) */
  ask: (text: string) => Promise<boolean>;
  /** The plugin's message dialog. */
  notify: (text: string, kind: "info" | "warning") => Promise<void>;
  /** One `delete_job`. */
  del: (id: string) => Promise<void>;
  /** Re-read the Jobs list. */
  reload: () => Promise<void>;
  /** A functional update of the selection (it may have changed during the run). */
  setSelection: (update: (prev: ReadonlySet<string>) => Set<string>) => void;
}

/** How one click ended. */
export type BulkDeleteOutcome = "nothing-deletable" | "declined" | "done";

/**
 * The click's sequence for an already-made plan: nothing deletable → say so (and drop the vanished
 * ids from the selection), no confirm; otherwise one confirm, a declined one deletes nothing; a
 * confirmed one runs the loop, updates the selection, reloads the list, and only THEN shows the
 * summary — so the user always hears the outcome, and the list behind the dialog is already current.
 * A failed reload does not hide the outcome: it is appended to the summary.
 */
export async function bulkDeleteFlow(plan: BulkDeletePlan, deps: BulkDeleteDeps): Promise<BulkDeleteOutcome> {
  const text = bulkDeleteConfirmText(plan);
  if (text === null) {
    if (plan.missing.length > 0) {
      deps.setSelection((prev) => selectionAfterBulkDelete(prev, plan, { deleted: [], failed: [] }));
    }
    await deps.notify(bulkDeleteNothingText(plan), "info");
    return "nothing-deletable";
  }
  if (!(await deps.ask(text))) return "declined";
  const result = await runBulkDelete(plan.deletable, deps.del);
  deps.setSelection((prev) => selectionAfterBulkDelete(prev, plan, result));
  let summary = bulkDeleteSummary(plan, result);
  try {
    await deps.reload();
  } catch (e) {
    summary += `\nThe job list could not be refreshed: ${String(e)}`;
  }
  await deps.notify(summary, result.failed.length > 0 ? "warning" : "info");
  return "done";
}
