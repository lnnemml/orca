/**
 * Pure view logic for remote jobs (ADR-024 o19, unit 5.3 B3 Part A). Every decision is made in
 * Rust — whether a server profile is a run target, what the server's facts mean, whether automatic
 * fetches stopped — and this module only arranges those values for the view:
 *
 * - `coordinateState`: local, remote, or corrupt coordinates (o19.8);
 * - `mergeRemoteState` / `applyRemoteStateEvent`: the seq rule that orders the `remote_job_state`
 *   read and the `job:remote-state` events whatever order they arrive in (o19.6);
 * - `remoteView`: the badges and offered actions of a job from its row and its record (o19.2,
 *   o19.7). The words of a check stay in the row's `error_message`; this module never copies
 *   Rust's outcome texts (`shown_message`) or its list of outcomes that need a later unit's action;
 * - `backfillsLog`: whether the view reads the log already on disk (o16.7);
 * - `runTargetOptions`: the "Run on" choices next to Submit (ADR-024 o5, n2);
 * - `resolveRunTarget`: the remembered "Run on" choice, checked against those choices (o20.1).
 */
import type { Job, Outcome, RemoteState, RemoteStateEvent, RemoteStateRead, ServerProfile } from "../types";

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

// --- Badges and actions (o19.2, o19.7) ----------------------------------------------------------

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
  /** The latest check could not be done or decided nothing (the host is away, a queue unreadable). */
  | { kind: "check_error"; label: string; tone: Tone }
  /** Automatic fetches stopped after the strikes. */
  | { kind: "fetch_stopped"; label: string; tone: Tone }
  /** Some but not all coordinates are set (o19.8). */
  | { kind: "corrupt"; label: string; tone: Tone };

/** What the view may offer: `retry_remote_submit`, `withdraw_remote_job`, `retry_remote_fetch`. */
export type RemoteAction = "retry_submit" | "withdraw" | "retry_fetch";

export type RemoteView =
  /** No coordinates: nothing remote to show. */
  | { kind: "local" }
  /** Some but not all coordinates: shown, never acted on (o19.8). */
  | { kind: "corrupt"; badges: RemoteBadge[]; actions: RemoteAction[] }
  /** A remote job that has finished: the ordinary job view. */
  | { kind: "terminal" }
  /** A remote `queued`/`running` job. `text` is the row's `error_message`; `fromLastCheck` labels
   *  it "from the last check" whenever the record holds no decided state (o19.2) — then it may be
   *  stale across a restart and no action comes from it. */
  | {
      kind: "live";
      text: { body: string; fromLastCheck: boolean } | null;
      badges: RemoteBadge[];
      actions: RemoteAction[];
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

/**
 * The remote part of a job's view, from its row and its record (`null`/`undefined`: no record).
 * The record decides the badges and the actions; the row gives the words.
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
  if (job.status !== "queued" && job.status !== "running") return { kind: "terminal" };

  const known = record?.known ?? null;
  const badges: RemoteBadge[] = [];
  const actions: RemoteAction[] = [];

  if (!record) {
    badges.push({ kind: "checking", label: "Checking the server…", tone: "muted" });
  }
  if (known?.kind === "labelled") {
    if (known.detail === "not_on_server") {
      badges.push({ kind: "labelled", label: "Not on the server", tone: "warn" });
      actions.push("retry_submit", "withdraw");
    } else if (known.detail === "submit_interrupted") {
      badges.push({ kind: "labelled", label: "Submit interrupted", tone: "warn" });
      actions.push("withdraw");
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
  if (record?.check_error != null) {
    badges.push({ kind: "check_error", label: `Server unreachable / not decided: ${record.check_error}`, tone: "warn" });
  }
  if (record?.fetch.stopped) {
    badges.push({ kind: "fetch_stopped", label: "Automatic fetches stopped", tone: "err" });
    actions.push("retry_fetch");
  }

  const text = job.error_message != null ? { body: job.error_message, fromLastCheck: known === null } : null;
  return { kind: "live", text, badges, actions };
}

// --- The live log's backfill (o16.7) ------------------------------------------------------------

/**
 * Whether the job view reads the log and the convergence points already on disk
 * (`read_job_output` / `read_job_convergence`) after its listeners are attached. A draft has none.
 * A remote `queued`/`running` job never does: the poller re-streams its whole log from offset 0 on
 * every `watch_job_log` open, and during a failing fetch a local `output.out` exists while the row
 * is still live — backfill plus re-stream would show every line and point twice. Every other job
 * (local, a finished remote one, corrupt coordinates) backfills as a local job always has.
 */
export function backfillsLog(job: Pick<Job, "status" | "remote_host" | "remote_job_dir" | "remote_socket">): boolean {
  if (job.status === "draft") return false;
  const live = job.status === "queued" || job.status === "running";
  return !(live && coordinateState(job) === "remote");
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
