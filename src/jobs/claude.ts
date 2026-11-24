/**
 * Claude's jobs on the Jobs screen (ADR-027 M4 amendment, items 6 and 8): the badge, a PROPOSED job's
 * reasons, and "Claude is submitting" in place of Run; and the stored target (M4c amendment): a
 * Claude draft runs where Claude assembled it, or not at all (`submitTargetOf`, `autoRunRefusal`;
 * the row's slot is `remote.ts`'s `runSlot`). Pure: no React, no Tauri. The screens render what this
 * decides; none of them reads `origin`, `mcp_state`, `mcp_reasons` or `mcp_target` itself.
 *
 * Every predicate here is **exact** (rule #9): a job the payload does not positively name as Claude's
 * is not Claude's. A field that is missing or odd (an older payload, a fixture) never counts as a
 * match, so the badge and the PROPOSED reading err towards showing nothing, not towards claiming.
 */
import { firstReasonText, reasonsText } from "../mcp/audit";
import type { AuditReasons, Job, McpReason } from "../types";

/** The text that replaces Run on a job Claude is still submitting. */
export const SUBMITTING_LABEL = "Claude is submitting";

/** The badge's text, shown in the status cell. */
export const CLAUDE_BADGE = "Claude";

/** Claude created this job (`origin = 'mcp'`). A user job, whatever else it carries, is not. */
export function isClaudeJob(job: Pick<Job, "origin">): boolean {
  return job.origin === "mcp";
}

/**
 * PROPOSED: a draft Claude made that no grant paid for and that is not on its way anywhere —
 * `status = 'draft'` and `origin = 'mcp'` and `grant_id IS NULL` and `mcp_state IS NULL`. A charged
 * draft in step B is also a `draft` with `origin = 'mcp'`, but it is submitting, not proposed; a
 * charged draft whose step B was refused is refunded (`grant_id` cleared) and so becomes PROPOSED.
 */
export function isProposed(job: Pick<Job, "status" | "origin" | "grant_id" | "mcp_state">): boolean {
  return job.status === "draft" && job.origin === "mcp" && job.grant_id === null && job.mcp_state === null;
}

/** Claude's `create_job` is between step A and step C for this job (`mcp_state = 'submitting'`). */
export function isSubmitting(job: Pick<Job, "mcp_state">): boolean {
  return job.mcp_state === "submitting";
}

/**
 * `mcp_state` is set to anything: the job's row is the MCP server's until step C (or the
 * reconciliation) clears it, and no job-delete command takes it (Rust's `delete_refusal_for_mcp_state`
 * refuses on any non-NULL value, failing closed). Wider than {@link isSubmitting} on purpose: this is
 * what holds Delete back, that is what words Run's slot.
 */
export function isHeldByClaude(job: Pick<Job, "mcp_state">): boolean {
  return job.mcp_state != null;
}

// --- Reasons ------------------------------------------------------------------------------------

/** What `jobs.mcp_reasons` holds: nothing, a readable list, or text that is not one. */
export type JobReasons = { kind: "none" } | AuditReasons;

/** The longest raw text shown for reasons that cannot be read (as the audit view's, in characters). */
export const RAW_REASONS_SHOWN = 1_024;

const isReason = (v: unknown): v is McpReason =>
  typeof v === "object" &&
  v !== null &&
  typeof (v as { row?: unknown }).row === "string" &&
  typeof (v as { message?: unknown }).message === "string";

/**
 * `mcp_reasons` read as `[{row, message}]`. It **never throws**: null is `none`; JSON that is not an
 * array of such objects (or is not JSON at all) is `unreadable` with its raw text, cut to
 * {@link RAW_REASONS_SHOWN} characters and marked when cut. An empty array is a readable, empty list.
 */
export function parseJobReasons(job: Pick<Job, "mcp_reasons">): JobReasons {
  const raw = job.mcp_reasons;
  if (raw === null || raw === undefined) return { kind: "none" };
  const unreadable = (): JobReasons => {
    const chars = [...String(raw)];
    const clipped = chars.length > RAW_REASONS_SHOWN;
    return { kind: "unreadable", raw: clipped ? chars.slice(0, RAW_REASONS_SHOWN).join("") : chars.join(""), raw_clipped: clipped };
  };
  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch {
    return unreadable();
  }
  if (!Array.isArray(parsed) || !parsed.every(isReason)) return unreadable();
  // Keep only the two fields: a row's other keys are not ours to carry.
  return { kind: "read", list: parsed.map((r) => ({ row: r.row, message: r.message })) };
}

/**
 * The row's one-line reason of a PROPOSED job (`row: message`, with `(+n)` when there are more), or
 * "reasons unreadable"; null when the job is not PROPOSED or has no reasons to show.
 */
export function firstReason(job: Job): string | null {
  const reasons = proposedReasons(job);
  return reasons === null ? null : firstReasonText(reasons);
}

/** What the job detail shows: every reason on its own line, or the unreadable text. */
export type ReasonsList =
  | { kind: "read"; lines: string[] }
  | { kind: "unreadable"; text: string };

/** The whole list for a PROPOSED job's detail; null when the job is not PROPOSED or has none. */
export function reasonsList(job: Job): ReasonsList | null {
  const reasons = proposedReasons(job);
  if (reasons === null) return null;
  if (reasons.kind === "unreadable") return { kind: "unreadable", text: reasonsText(reasons) };
  return { kind: "read", lines: reasons.list.map((r) => `${r.row}: ${r.message}`) };
}

/** A PROPOSED job's readable-or-not reasons; null for any other job, and for no or empty reasons. */
function proposedReasons(job: Job): AuditReasons | null {
  if (!isProposed(job)) return null;
  const reasons = parseJobReasons(job);
  if (reasons.kind === "none") return null;
  if (reasons.kind === "read" && reasons.list.length === 0) return null;
  return reasons;
}

// --- Run ----------------------------------------------------------------------------------------

/**
 * What a job's Run slot shows. `run`: the screen's own Run (its label stays the screen's). `submitting`:
 * Run is replaced by a **disabled** `label`; the `submit_job` refusal in Rust stays as the backstop.
 * `none`: not a draft, so no Run at all.
 */
export type RunAction =
  | { kind: "run" }
  | { kind: "submitting"; label: typeof SUBMITTING_LABEL; disabled: true }
  | { kind: "none" };

export function runAction(job: Pick<Job, "status" | "mcp_state">): RunAction {
  if (job.status !== "draft") return { kind: "none" };
  if (isSubmitting(job)) return { kind: "submitting", label: SUBMITTING_LABEL, disabled: true };
  return { kind: "run" };
}

// --- The stored target (ADR-027 M4c amendment) ---------------------------------------------------

/** The `mcp_target` text for this machine (Rust's `mcp::grant::LOCAL_TARGET`); any other value is a
 *  server profile id. */
export const LOCAL_MCP_TARGET = "local";

/** Why a Claude draft from before v25 offers no Run (item 4). Rust's `submit_job` refuses it with the
 *  same words (`mcp::steps::LEGACY_DRAFT_NOTE`). */
export const LEGACY_DRAFT_NOTE = "assembled before targets were stored — delete it and ask Claude again";

/** Why a Claude draft whose stored profile is gone offers no Run (item 3); Rust's
 *  `mcp::steps::DELETED_PROFILE_NOTE`. */
export const DELETED_PROFILE_NOTE = "its target profile was deleted — delete it and ask Claude again";

/**
 * `submit_job`'s `target` for a stored `mcp_target` (item 3): `local` → null (this machine); a
 * profile id → itself. Never pass `"local"` through: `Backend::for_submit(Some("local"))` would look
 * for an ssh profile named "local", and Rust's target guard refuses it.
 */
export function submitTargetOf(mcpTarget: string): string | null {
  return mcpTarget === LOCAL_MCP_TARGET ? null : mcpTarget;
}

/** How a refusal names a stored target where no profile names are loaded (the job detail). */
function storedTargetWords(mcpTarget: string): string {
  return mcpTarget === LOCAL_MCP_TARGET ? "this machine" : `the server profile ${mcpTarget}`;
}

/**
 * The text that stops an auto-run submit (a row's Run, opened as the detail with `runTarget`) on
 * `runTarget`; null when the submit may go ahead. It declines:
 * - a job Claude is still submitting (the same `runAction` answer as the row's, so the screens cannot
 *   disagree);
 * - a Claude draft with no stored target (item 4: from before v25, or a payload without the field —
 *   failing closed), with {@link LEGACY_DRAFT_NOTE};
 * - a Claude draft asked to run anywhere but its stored target (`submitTargetOf(mcp_target) !==
 *   runTarget`, item 3): `%maxcore` was fitted for that target.
 *
 * A user draft is unchanged. A row that could not be read (`null`) is let through: Rust's `submit_job`
 * refusals are the defence in depth behind this check, and the only check of a deleted profile (the
 * detail loads no profiles). This one spares the user a call that would be refused.
 */
export function autoRunRefusal(
  job: Pick<Job, "status" | "origin" | "mcp_state" | "mcp_target"> | null,
  runTarget: string | null,
): string | null {
  if (job === null) return null;
  if (runAction(job).kind === "submitting") return `${SUBMITTING_LABEL}: this job can be run once that ends.`;
  if (job.status !== "draft" || job.origin !== "mcp") return null;
  if (job.mcp_target == null) return `Not run: ${LEGACY_DRAFT_NOTE}.`;
  if (submitTargetOf(job.mcp_target) !== runTarget) {
    return `Not run: Claude assembled this job for ${storedTargetWords(job.mcp_target)}; run it there, or delete it.`;
  }
  return null;
}
