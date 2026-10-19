/**
 * Pure view logic for remote jobs (ADR-024 o19, unit 5.3 B3 Part A). Every decision is made in
 * Rust — whether a server profile is a run target, what the server's facts mean, whether automatic
 * fetches stopped — and this module only arranges those values for the view:
 *
 * - `coordinateState`: local, remote, or corrupt coordinates (o19.8);
 * - `mergeRemoteState` / `applyRemoteStateEvent`: the seq rule that orders the `remote_job_state`
 *   read and the `job:remote-state` events whatever order they arrive in (o19.6);
 * - `remoteView`: the badges and offered actions of a job from its row and its record (o19.2,
 *   o19.7, o22.14): the cancel family with a per-action confirm, the hidden and disabled retries,
 *   the pending-cancel and bound badges, the check error prefixed by whether the server answered.
 *   The words of a check stay in the row's `error_message`; this module never copies Rust's
 *   outcome texts (`shown_message`) or its list of outcomes that need a later unit's action;
 * - `rowActions`: the Jobs table's row actions (o22.14.8);
 * - `cancelButtons`: the cancel family's buttons and their labels, from any offered actions
 *   (o22.14.1–2; B2);
 * - `cancelNotice` / `undoNotice`: what `cancel_job` / `undo_cancel` answered, in words (o22.14.7);
 * - `backfillsLog`: whether the view reads the log already on disk (o16.7);
 * - `runTargetOptions`: the "Run on" choices next to Submit (ADR-024 o5, n2);
 * - `resolveRunTarget`: the remembered "Run on" choice, checked against those choices (o20.1).
 */
import { formatLaptopUtc } from "../format";
import type {
  CancelResult,
  CheckError,
  Job,
  Outcome,
  RemoteState,
  RemoteStateEvent,
  RemoteStateRead,
  ServerProfile,
  StatusStep,
  UndoResult,
} from "../types";

// --- Coordinates (o19.8) ------------------------------------------------------------------------

export type CoordinateState = "local" | "remote" | "corrupt";

/**
 * A job is remote iff its three coordinates are set (o item 1) and local iff none is. Some but not
 * all set is a corrupt row (the v20 CHECK forbids it; the poller sets such a row aside, so it has no
 * record) — never guessed to be either.
 */
export function coordinateState(job: Pick<Job, "remote_host" | "remote_job_dir" | "remote_socket">): CoordinateState {
  // `!= null`: a fixture or an older payload may lack the field entirely.
  const set = [job.remote_host, job.remote_job_dir, job.remote_socket].filter((v) => v != null).length;
  if (set === 0) return "local";
  if (set === 3) return "remote";
  return "corrupt";
}

// --- The seq rule (o19.6) -----------------------------------------------------------------------

/** What the view holds for one job: the greatest `seq` seen so far and the record it carried. */
export interface HeldRemoteState {
  seq: number;
  state: RemoteState | null;
}

/** Before any read or event: every real `seq` (≥ 0) is greater. */
export const NOTHING_HELD: HeldRemoteState = { seq: -1, state: null };

/**
 * Apply a `remote_job_state` read or a `job:remote-state` event only if its `seq` is greater than
 * the one held. So neither an older read nor an older event delivered late overwrites a newer
 * state, whichever order Tauri delivers them in (unmeasured, ADR-024 Open questions o19).
 */
export function mergeRemoteState(held: HeldRemoteState, incoming: RemoteStateRead): HeldRemoteState {
  return incoming.seq > held.seq ? { seq: incoming.seq, state: incoming.state } : held;
}

/** `mergeRemoteState` for an event, which is applied only to the job it names. */
export function applyRemoteStateEvent(held: HeldRemoteState, jobId: string, event: RemoteStateEvent): HeldRemoteState {
  return event.job_id === jobId ? mergeRemoteState(held, event) : held;
}

// --- Badges and actions (o19.2, o19.7, o22.14) --------------------------------------------------

export type Tone = "ok" | "warn" | "err" | "muted";

export type RemoteBadge =
  /** No record yet this launch: the first status step has not run. */
  | { kind: "checking"; label: string; tone: Tone }
  /** The label call's verdict (o 3.4). */
  | { kind: "labelled"; label: string; tone: Tone }
  /** A fetching outcome whose download is failing (o19.11), with the strikes so far. */
  | { kind: "fetching"; strikes: number; stale: boolean; label: string; tone: Tone }
  /** The last decided outcome; `stale` while the latest check decided nothing. */
  | { kind: "outcome"; outcome: Outcome["outcome"]; stale: boolean; label: string; tone: Tone }
  /** The latest check could not be done or decided nothing, prefixed by whether the server
   *  answered (o22.14.3). */
  | { kind: "check_error"; answered: boolean; label: string; tone: Tone }
  /** Automatic fetches stopped after the strikes. */
  | { kind: "fetch_stopped"; label: string; tone: Tone }
  /** A cancel is pending (o22.14.1): since when, or — once the send is marked — that it may already
   *  be on the server. */
  | { kind: "cancel_pending"; sent: boolean; label: string; tone: Tone }
  /** A pending cancel's automatic re-sends stopped at the bound (o22.4.5, o22.14.3). */
  | { kind: "cancel_sends_stopped"; label: string; tone: Tone }
  /** A `cancelling` job's automatic sweeps stopped at the bound (o22.13.4, o22.14.3). */
  | { kind: "sweeps_stopped"; label: string; tone: Tone }
  /** Some but not all coordinates are set (o19.8). */
  | { kind: "corrupt"; label: string; tone: Tone };

/** What the view may offer — `cancel_job` (a first Cancel, or Retry cancel), `undo_cancel`,
 *  `retry_remote_submit`, `retry_remote_fetch`. */
export type RemoteAction = "cancel" | "retry_cancel" | "undo_cancel" | "retry_submit" | "retry_fetch";

/** The two retries: hidden while a cancel is pending, disabled while the server is unreachable. */
export type RetryAction = Extract<RemoteAction, "retry_submit" | "retry_fetch">;

/**
 * One offered action. `confirm`: it asks first — only a remote job's **first** Cancel does (o20.3,
 * o22.14.2); Retry cancel and Undo never do. `disabled`: why it cannot run now (a retry while the
 * last contact failed, o22.8), or null.
 */
export interface RemoteOffer {
  action: RemoteAction;
  confirm: boolean;
  disabled: string | null;
}

/** Why a retry is disabled while the server does not answer (o22.8). */
export const RETRY_UNREACHABLE = "server unreachable — retry when it answers";

export type RemoteView =
  /** No coordinates: nothing remote to show. */
  | { kind: "local" }
  /** Some but not all coordinates: shown, never acted on (o19.8). */
  | { kind: "corrupt"; badges: RemoteBadge[]; actions: RemoteOffer[] }
  /** A remote job that has finished: the ordinary job view. */
  | { kind: "terminal" }
  /** A remote `queued`/`running`/`cancelling` job. `text` is the row's `error_message`;
   *  `fromLastCheck` labels it "from the last check" whenever the record holds no decided state
   *  (o19.2) — then it may be stale across a restart and no action comes from it. */
  | {
      kind: "live";
      text: { body: string; fromLastCheck: boolean } | null;
      badges: RemoteBadge[];
      actions: RemoteOffer[];
    };

/** Short names of the classifier's outcomes for a badge. The explanation is the row's text. */
const OUTCOME_NAME: Record<Outcome["outcome"], string> = {
  queued: "queued",
  running: "running",
  completed: "completed",
  failed: "failed",
  lost: "lost",
  cancelling: "cancelling",
  cancelled: "cancelled",
  indeterminate: "undecided",
  re_enqueue: "never started",
};

const OUTCOME_TONE: Record<Outcome["outcome"], Tone> = {
  queued: "muted",
  running: "ok",
  completed: "ok",
  failed: "err",
  lost: "err",
  cancelling: "warn",
  cancelled: "muted",
  indeterminate: "warn",
  re_enqueue: "warn",
};

/** A job the server still has (o22.5): `queued`, `running` or `cancelling` — `cancelling` is live
 *  everywhere, never terminal. */
export function isLiveStatus(status: Job["status"]): boolean {
  return status === "queued" || status === "running" || status === "cancelling";
}

/** "Server answered: …" / "Server unreachable: …" — keyed on the error's own flag (o22.14.3), never on
 *  the record's `last_contact`, which a later call may have changed. */
export function checkErrorLabel(error: CheckError): string {
  return `${error.answered ? "Server answered" : "Server unreachable"}: ${error.text}`;
}

/**
 * The remote part of a job's view, from its row and its record (`null`/`undefined`: no record).
 * The record decides the badges and the actions; the row gives the words and the pending cancel.
 *
 * **The cancel offers** (o22.7, o22.14.1–2): a `cancelling` job offers Retry cancel (no confirm: the
 * cancel is on the server, the click restarts the sweeps); a `queued`/`running` job without a
 * pending cancel offers Cancel (confirmed); a pending one offers, by `cancel_sent_at` and the record's
 * `cancel_sends.stopped`:
 *
 * | sent | stopped | offered |
 * |---|---|---|
 * | no | no | Undo cancel |
 * | no | yes | Undo cancel + Retry cancel |
 * | yes | no | nothing (delivery re-sends by itself) |
 * | yes | yes | Retry cancel |
 *
 * **The retries** (o22.8, o22.14.4): hidden while a cancel is pending; disabled with
 * [`RETRY_UNREACHABLE`] while the last contact failed.
 */
export function remoteView(job: Job, record: RemoteState | null | undefined): RemoteView {
  const coords = coordinateState(job);
  if (coords === "local") return { kind: "local" };
  if (coords === "corrupt") {
    return {
      kind: "corrupt",
      badges: [{ kind: "corrupt", label: "Corrupt remote coordinates: this job is not polled", tone: "err" }],
      actions: [],
    };
  }
  if (!isLiveStatus(job.status)) return { kind: "terminal" };

  const known = record?.known ?? null;
  const badges: RemoteBadge[] = [];
  const retries: RetryAction[] = [];
  // `!= null`: a fixture or an older payload may lack the fields entirely.
  const pending = job.cancel_requested_at != null;
  const sent = job.cancel_sent_at != null;

  if (!record) {
    badges.push({ kind: "checking", label: "Checking the server…", tone: "muted" });
  }
  if (known?.kind === "labelled") {
    if (known.detail === "not_on_server") {
      badges.push({ kind: "labelled", label: "Not on the server", tone: "warn" });
      retries.push("retry_submit");
    } else if (known.detail === "submit_interrupted") {
      badges.push({ kind: "labelled", label: "Submit interrupted", tone: "warn" });
    }
    // `classifier` never reaches the record: the step goes on to collect and classify.
  } else if (known?.kind === "shown") {
    const outcome = known.detail.outcome;
    const stale = record?.check_error != null;
    badges.push({
      kind: "outcome",
      outcome,
      stale,
      label: stale ? `Last seen: ${OUTCOME_NAME[outcome]}` : `Server: ${OUTCOME_NAME[outcome]}`,
      tone: stale ? "muted" : OUTCOME_TONE[outcome],
    });
  } else if (known?.kind === "fetching") {
    const strikes = record?.fetch.strikes ?? 0;
    const stale = record?.check_error != null;
    // No count at 0 strikes (e.g. right after a manual retry cleared them): "(0 failed
    // attempts)" says nothing.
    const failed = strikes === 0 ? "" : strikes === 1 ? " (1 failed attempt)" : ` (${strikes} failed attempts)`;
    badges.push({
      kind: "fetching",
      strikes,
      stale,
      label: `${stale ? "Last seen: fetching results" : "Fetching results"}${failed}`,
      tone: stale ? "muted" : "warn",
    });
  }
  if (pending) {
    badges.push({
      kind: "cancel_pending",
      sent,
      label: sent
        ? "Cancel pending — the cancel may already be on the server"
        : `Cancel pending since ${formatLaptopUtc(job.cancel_requested_at ?? "")}`,
      tone: "warn",
    });
  }
  if (record?.check_error != null) {
    badges.push({ kind: "check_error", answered: record.check_error.answered, label: checkErrorLabel(record.check_error), tone: "warn" });
  }
  if (record?.fetch.stopped) {
    badges.push({ kind: "fetch_stopped", label: "Automatic fetches stopped", tone: "err" });
    retries.push("retry_fetch");
  }
  // No counters before a bound is reached (o22.14.3).
  const sendsStopped = record?.cancel_sends.stopped ?? false;
  if (pending && sendsStopped) {
    badges.push({ kind: "cancel_sends_stopped", label: "Cancel re-sends stopped — Retry cancel to send again", tone: "warn" });
  }
  if (job.status === "cancelling" && record?.sweeps.stopped) {
    badges.push({ kind: "sweeps_stopped", label: "Sweeps stopped — processes still run; Retry cancel to sweep again", tone: "warn" });
  }

  const actions: RemoteOffer[] = [];
  if (!pending) {
    const unreachable = record?.last_contact?.kind === "failed";
    for (const action of retries) actions.push({ action, confirm: false, disabled: unreachable ? RETRY_UNREACHABLE : null });
  }
  const offer = (action: RemoteAction, confirm: boolean) => actions.push({ action, confirm, disabled: null });
  if (job.status === "cancelling") {
    offer("retry_cancel", false);
  } else if (!pending) {
    offer("cancel", true);
  } else {
    if (!sent) offer("undo_cancel", false);
    if (sendsStopped) offer("retry_cancel", false);
  }

  const text = job.error_message != null ? { body: job.error_message, fromLastCheck: known === null } : null;
  return { kind: "live", text, badges, actions };
}

// --- The Jobs table's row actions (o22.14.8) -----------------------------------------------------

/** A Jobs-table row action: one of the cancel family (with its confirm), or the terminal row's. */
export type RowAction =
  | { action: "cancel" | "retry_cancel" | "undo_cancel"; confirm: boolean }
  | { action: "run_locally" }
  | { action: "open" }
  | { action: "delete" };

/** The cancel-family actions that send `cancel_job`: a first Cancel and Retry cancel. */
export type CancelSend = { action: "cancel" | "retry_cancel"; confirm: boolean };

/**
 * A Jobs-table row's actions. A draft runs locally or is deleted; a **live** job — local
 * `queued`/`running`, remote `queued`/`running`/`cancelling` — offers only its cancel family, never
 * Delete (Rust refuses it; a `cancelling` job offers Retry cancel); a remote live job's cancel
 * family is exactly `remoteView`'s. A live job with corrupt coordinates offers nothing (o19.8).
 * Every other (terminal) row opens or is deleted.
 */
export function rowActions(job: Job, record: RemoteState | null | undefined): RowAction[] {
  if (job.status === "draft") return [{ action: "run_locally" }, { action: "delete" }];
  const coords = coordinateState(job);
  if (coords === "local") {
    if (job.status === "queued" || job.status === "running") return [{ action: "cancel", confirm: false }];
    return [{ action: "open" }, { action: "delete" }];
  }
  if (!isLiveStatus(job.status)) return [{ action: "open" }, { action: "delete" }];
  const view = remoteView(job, record);
  if (view.kind !== "live") return [];
  const family: RowAction[] = [];
  for (const o of view.actions) {
    if (o.action === "cancel" || o.action === "retry_cancel" || o.action === "undo_cancel") {
      family.push({ action: o.action, confirm: o.confirm });
    }
  }
  return family;
}

/** The row's action that sends `cancel_job` (a first Cancel or Retry cancel), if it offers one. */
export function cancelSendOf(actions: readonly RowAction[]): CancelSend | null {
  for (const a of actions) if (a.action === "cancel" || a.action === "retry_cancel") return { action: a.action, confirm: a.confirm };
  return null;
}

/** The cancel family: the three buttons a Jobs row or a detail panel shows for a live job. */
export type CancelFamilyAction = "cancel" | "retry_cancel" | "undo_cancel";

/** Which click of the family is in flight, or null: a Cancel / Retry cancel (`"cancel"`, shown
 *  "Sending cancel…" — "cancelling" is a status, o22.14.8) or an Undo (`"undo"`). */
export type CancelBusy = "cancel" | "undo" | null;

/** One button of the cancel family. */
export interface CancelButton {
  action: CancelFamilyAction;
  label: string;
  /** Every button of the family is disabled while one click is in flight. */
  disabled: boolean;
}

const CANCEL_LABEL: Record<CancelFamilyAction, string> = {
  cancel: "Cancel",
  retry_cancel: "Retry cancel",
  undo_cancel: "Undo cancel",
};

/**
 * The family's buttons for the given offered actions (a `remoteView`'s offers or a `rowActions`
 * list — anything with an `action`), in the order offered, each labelled by **its own offer** (o22.14.1–2):
 * Cancel / Retry cancel / Undo cancel. While a click is in flight the button that was clicked reads
 * "Sending cancel…" / "Undoing…" and all of them are disabled. `locked`: another action of the
 * view (a Retry) is in flight — the family is disabled too (o22.14.14.5), as a cancel in flight
 * already disables the retries.
 */
export function cancelButtons(actions: readonly { action: string }[], busy: CancelBusy, locked = false): CancelButton[] {
  const out: CancelButton[] = [];
  for (const { action } of actions) {
    if (action !== "cancel" && action !== "retry_cancel" && action !== "undo_cancel") continue;
    const inFlight = action === "undo_cancel" ? busy === "undo" : busy === "cancel";
    const label = inFlight ? (action === "undo_cancel" ? "Undoing…" : "Sending cancel…") : CANCEL_LABEL[action];
    out.push({ action, label, disabled: busy !== null || locked });
  }
  return out;
}

/** The confirm of a remote job's first Cancel (o20.3, o22.14.2). */
export function cancelConfirmText(job: Pick<Job, "title" | "remote_host">): string {
  return (
    `Cancel "${job.title}" on ${job.remote_host ?? "the server"}? The server stops it; files already written stay ` +
    "on the server and are downloaded for viewing. You can undo until the cancel is sent."
  );
}

// --- The cancel and undo notices (o22.14.7) ------------------------------------------------------

/** Capitalise the first letter of a notice. */
function sentence(text: string): string {
  return text.charAt(0).toUpperCase() + text.slice(1);
}

/**
 * What a delivery's step means for the user (o22.14.7). "sent" is said only for a step that
 * delivered. A `check_failed` is read with the **reloaded row** (`cancel_sent_at`: the send may have
 * happened) and the record's **check error** (whether the server answered); with no check error
 * (the record could not be read) the notice does not guess which; with no reloaded row it does not
 * guess whether the send was marked either.
 */
function deliveredNotice(step: StatusStep, row: Pick<Job, "cancel_sent_at"> | null, checkError: CheckError | null): string | null {
  switch (step.step) {
    case "cancel_delivered":
      return step.detail.outcome === "completed" && step.detail.late_cancel
        ? "The job completed before the cancel took effect; its result is kept"
        : "Cancel sent";
    case "cancelled":
    case "finalised":
      return "Cancel sent";
    case "ended_before_cancel":
      return "The job finished before the cancel reached the server";
    case "check_failed":
      // No reloaded row: whether the send was marked is unknown, so the notice claims neither.
      if (row == null) return `The cancel stays pending: ${step.detail}`;
      if (row.cancel_sent_at != null) {
        return "No answer after the cancel was sent; it may already be on the server; checked again at the next step";
      }
      if (checkError == null) return `The cancel stays pending: ${step.detail}`;
      return checkError.answered
        ? `The server's answer could not be read: ${step.detail}; the cancel stays pending`
        : "Server unreachable — the cancel stays pending and is sent when it answers";
    case "cancel_not_sent":
      return `The cancel failed on the server: ${step.detail}; it stays pending`;
    case "cancel_stopped":
      return "Cancel re-sends stopped";
    case "row_changed":
    case "not_live":
      return "The job changed meanwhile";
    // Delivery returns none of these; never "sent".
    case "busy":
    case "labelled":
    case "shown":
    case "fetch_failed":
    case "fetch_stopped":
    case "swept":
    case "submit_rechecked":
      return null;
    default: {
      const unhandled: never = step;
      return unhandled;
    }
  }
}

/**
 * The notice for `cancel_job`'s answer (o22.14.7), from the answer, the reloaded row and the
 * reloaded record's check error. `null`: nothing to say (a local cancel; a step delivery never
 * returns).
 */
export function cancelNotice(result: CancelResult, row: Pick<Job, "cancel_sent_at"> | null, checkError: CheckError | null): string | null {
  switch (result.result) {
    case "delivered":
      return deliveredNotice(result.step, row, checkError);
    case "pending":
      return "Cancel pending — sent at the next check";
    case "already_on_server":
      return "The cancel is already on the server; sweeping again";
    case "moved_on":
      return sentence(`the job is already ${result.status}`);
    case "dispatched":
      return null;
    default: {
      const unhandled: never = result;
      return unhandled;
    }
  }
}

/**
 * The Jobs banner after a cancel-family click (o22.14.14.3): **the next answer replaces it** — a
 * click that came to an answer (`CancelFamilyOutcome`-like: its `notice`, `null` for a local cancel
 * that has none) or to an error (`"error"`: the call or a read failed, and the error banner says
 * so — an older notice must not stand beside it) — but a click that produced no answer (`null`: a
 * declined dialog, nothing offered) leaves it as it is.
 */
export function bannerAfterClick(current: string | null, answer: { notice: string | null } | null | "error"): string | null {
  if (answer === null) return current;
  if (answer === "error") return null;
  return answer.notice;
}

/** The notice for `undo_cancel`'s answer (o22.14.7). */
export function undoNotice(result: UndoResult): string {
  switch (result) {
    case "undone":
      return "Cancel undone";
    case "send_marked":
      return "Too late to undo: the cancel may already be on the server";
    case "nothing_pending":
      return "No cancel to undo";
    default: {
      const unhandled: never = result;
      return unhandled;
    }
  }
}

// --- The live log's backfill (o16.7) ------------------------------------------------------------

/**
 * Whether the job view reads the log and the convergence points already on disk
 * (`read_job_output` / `read_job_convergence`) after its listeners are attached. A draft has none.
 * A remote `queued`/`running`/`cancelling` job never does: the poller re-streams its whole log from
 * offset 0 on every `watch_job_log` open (its candidates include `cancelling`, o22.5), and during a
 * failing fetch a local `output.out` exists while the row is still live — backfill plus re-stream
 * would show every line and point twice. Every other job (local, a finished remote one, corrupt
 * coordinates) backfills as a local job always has.
 */
export function backfillsLog(job: Pick<Job, "status" | "remote_host" | "remote_job_dir" | "remote_socket">): boolean {
  if (job.status === "draft") return false;
  return !(isLiveStatus(job.status) && coordinateState(job) === "remote");
}

// --- Run on (ADR-024 o5, n2) ---------------------------------------------------------------------

export interface RunTargetOption {
  /** `submit_job`'s `target`: null = this machine, else the profile id. */
  target: string | null;
  label: string;
  enabled: boolean;
  /** Why a profile is not a run target (Rust's words); null when enabled. */
  reason: string | null;
}

/**
 * `Local` and every profile, in the given order. A profile is enabled iff Rust says it is a run
 * target; a disabled one carries Rust's reason — the rule is never re-derived here.
 */
export function runTargetOptions(profiles: ServerProfile[]): RunTargetOption[] {
  return [
    { target: null, label: "Local", enabled: true, reason: null },
    ...profiles.map((p) => ({
      target: p.id,
      label: p.name,
      enabled: p.run_target.is_run_target,
      reason: p.run_target.is_run_target ? null : (p.run_target.reason ?? "no reason reported"),
    })),
  ];
}

/** The app setting holding the target of the last Create & Run (o20.1): a profile id, `""` for
 *  Local. Named apart from `ServerProfile.run_target`. */
export const LAST_RUN_TARGET_SETTING = "last_run_target";

/**
 * The "Run on" choice to preselect from the stored settings (`get_settings`; null when the read
 * failed). The stored profile is used only while it is still among `options` **and** enabled; a
 * profile that is gone or no longer a run target, an absent value, or a failed read gives Local
 * (null) — never a stale or disabled target preselected.
 */
export function resolveRunTarget(options: RunTargetOption[], settings: Record<string, string> | null): string | null {
  const stored = settings?.[LAST_RUN_TARGET_SETTING];
  if (!stored) return null;
  return options.some((o) => o.target === stored && o.enabled) ? stored : null;
}
