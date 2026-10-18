/**
 * The order in which a job view attaches to Rust's events and then asks Rust for state
 * (ADR-024 o16.7, o19.6). Both rules say the same thing: **listen first, then ask** — an event
 * emitted between the ask and the listener's registration would otherwise be lost. And both views
 * (`JobDetailScreen`, `JobsScreen`) must drop every listener on unmount, including one whose
 * `listen` promise resolves only after it.
 *
 * The functions here take the Tauri bridge as an argument so the order is testable without a
 * WebView (`live-wiring.test.ts`); the screens pass `tauriBridge`. The seq rule itself stays in
 * `remote.ts` (`mergeRemoteState`); this module only routes the read and the events through it.
 */
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { confirm } from "@tauri-apps/plugin-dialog";

import type { ConvergenceEvent, ConvergencePayload } from "../convergence/types";
import type {
  CancelResult,
  CheckError,
  Job,
  JobStatus,
  RemoteState,
  RemoteStateEvent,
  RemoteStateRead,
  ServerProfile,
  UndoResult,
} from "../types";
import {
  backfillsLog,
  cancelConfirmText,
  cancelNotice,
  cancelSendOf,
  coordinateState,
  mergeRemoteState,
  NOTHING_HELD,
  resolveRunTarget,
  rowActions,
  runTargetOptions,
  undoNotice,
  type CancelBusy,
  type CancelFamilyAction,
  type CancelSend,
  type HeldRemoteState,
} from "./remote";

/** The two Tauri calls the views make, injectable for tests. */
export interface Bridge {
  invoke: <T>(cmd: string, args?: Record<string, unknown>) => Promise<T>;
  listen: <T>(event: string, handler: (event: { payload: T }) => void) => Promise<UnlistenFn>;
}

export const tauriBridge: Bridge = { invoke, listen };

// --- Listener scope ------------------------------------------------------------------------------

/**
 * A set of listeners that live and die together. `listen` resolves `false` when the scope was
 * disposed while the registration was in flight — that listener is dropped at once, so a late
 * resolve after unmount leaks nothing — and the caller stops its sequence there.
 */
export interface ListenerScope {
  listen<T>(event: string, handler: (payload: T) => void): Promise<boolean>;
  readonly disposed: boolean;
  dispose(): void;
}

export function createListenerScope(bridge: Bridge): ListenerScope {
  let disposed = false;
  const attached: UnlistenFn[] = [];
  return {
    async listen<T>(event: string, handler: (payload: T) => void) {
      const unlisten = await bridge.listen<T>(event, (e) => {
        if (!disposed) handler(e.payload);
      });
      if (disposed) {
        unlisten();
        return false;
      }
      attached.push(unlisten);
      return true;
    },
    get disposed() {
      return disposed;
    },
    dispose() {
      disposed = true;
      for (const unlisten of attached.splice(0)) unlisten();
    },
  };
}

// --- The remote record (o19.6) ---------------------------------------------------------------------

/** Rust's `job:remote-state` event name (`poller/app.rs` `REMOTE_STATE_EVENT`). */
export const REMOTE_STATE_EVENT = "job:remote-state";

/**
 * What a view holds of the remote records: per job, the greatest `seq` seen and its record. One
 * `job:remote-state` listener for the view's life; `read(ids)` asks `remote_job_state` for each id
 * only after that listener has resolved, and every value — read or event — goes through
 * `mergeRemoteState`, so neither a late older read nor a late older event overwrites a newer state.
 */
export interface RemoteStateTracker {
  read(ids: readonly string[]): Promise<void>;
  dispose(): void;
}

export function trackRemoteStates(
  bridge: Bridge,
  onChange: (held: ReadonlyMap<string, HeldRemoteState>) => void,
  onError: (message: string) => void,
): RemoteStateTracker {
  const scope = createListenerScope(bridge);
  const held = new Map<string, HeldRemoteState>();

  const apply = (jobId: string, incoming: RemoteStateRead) => {
    const before = held.get(jobId) ?? NOTHING_HELD;
    const after = mergeRemoteState(before, incoming);
    if (after === before) return;
    held.set(jobId, after);
    onChange(new Map(held));
  };

  const ready = scope.listen<RemoteStateEvent>(REMOTE_STATE_EVENT, (event) => apply(event.job_id, event));

  return {
    async read(ids) {
      if (!(await ready)) return;
      await Promise.all(
        ids.map(async (id) => {
          try {
            const read = await bridge.invoke<RemoteStateRead>("remote_job_state", { id });
            if (!scope.disposed) apply(id, read);
          } catch (e) {
            if (!scope.disposed) onError(String(e));
          }
        }),
      );
    },
    dispose: () => scope.dispose(),
  };
}

// --- The job view's log (o16.7) ------------------------------------------------------------------

interface LogPayload {
  job_id: string;
  lines: string[];
}

interface StatusPayload {
  job_id: string;
  status: JobStatus;
}

interface ResetPayload {
  job_id: string;
}

/** What the job view does with each piece of the sequence. Only events of `jobId` reach it. */
export interface JobViewHandlers {
  job(job: Job): void;
  lines(lines: string[]): void;
  convergence(events: ConvergenceEvent[]): void;
  /** `job:log-reset`: drop the lines and points held; the poller re-streams from offset 0. */
  reset(): void;
  status(status: JobStatus): void;
  /** The log and points already on disk — only for a job `backfillsLog` admits. */
  backfill(lines: string[], events: ConvergenceEvent[]): void;
  error(message: string): void;
}

/**
 * Open the job view: attach `job:log`, `job:convergence`, `job:log-reset` and `job:status`, and
 * only then open the live log (`watch_job_log`, so the poller's offset-0 chunk finds the
 * listeners) and read the row (so no status change is lost between the read and the listener),
 * backfill if `backfillsLog` admits the row, and finally run `afterOpen` (the auto-run submit).
 * Returns the close: it drops every listener and closes the watch **only if this view's open was
 * sent** (a close without an open would decrement another view's count).
 */
export function openJobView(
  bridge: Bridge,
  jobId: string,
  on: JobViewHandlers,
  afterOpen: (() => Promise<void>) | null,
): () => void {
  const scope = createListenerScope(bridge);
  let watchOpened = false;

  void (async () => {
    const mine = (p: { job_id: string }) => p.job_id === jobId;
    const attached =
      (await scope.listen<LogPayload>("job:log", (p) => mine(p) && on.lines(p.lines))) &&
      (await scope.listen<ConvergencePayload>("job:convergence", (p) => mine(p) && on.convergence(p.events))) &&
      (await scope.listen<ResetPayload>("job:log-reset", (p) => mine(p) && on.reset())) &&
      (await scope.listen<StatusPayload>("job:status", (p) => mine(p) && on.status(p.status)));
    // The view may have closed after the last listener resolved: open nothing it cannot close.
    if (!attached || scope.disposed) return;

    // Every backend: the count is per job id, and a local job's count makes no ssh call (o16.2).
    watchOpened = true;
    try {
      await bridge.invoke("watch_job_log", { id: jobId, open: true });
    } catch (e) {
      if (!scope.disposed) on.error(String(e));
    }

    // The row is read only now, with `job:status` attached: a status change before this read is in
    // the row, one after it arrives as an event — none falls in between.
    let current: Job | null = null;
    try {
      current = await bridge.invoke<Job>("get_job", { id: jobId });
      if (scope.disposed) return;
      on.job(current);
    } catch (e) {
      if (scope.disposed) return;
      on.error(String(e));
    }

    if (current && backfillsLog(current) && !scope.disposed) {
      let lines: string[] = [];
      let events: ConvergenceEvent[] = [];
      try {
        lines = await bridge.invoke<string[]>("read_job_output", { id: jobId });
      } catch (e) {
        if (!scope.disposed) on.error(String(e));
      }
      try {
        events = await bridge.invoke<ConvergenceEvent[]>("read_job_convergence", { id: jobId });
      } catch (e) {
        if (!scope.disposed) on.error(String(e));
      }
      if (!scope.disposed) on.backfill(lines, events);
    }

    if (afterOpen && !scope.disposed) await afterOpen();
  })();

  return () => {
    scope.dispose();
    if (watchOpened) {
      bridge.invoke("watch_job_log", { id: jobId, open: false }).catch(() => {
        /* the view is gone; a lost close leaks at most until the job ends (o16.3) */
      });
    }
  };
}

// --- "Run on" on load (ADR-024 o20.1) -------------------------------------------------------------

/**
 * Load the "Run on" choices and the remembered one: the profile list (a failure is an empty list
 * plus its error, shown) and `get_settings` (a failure is nothing stored), resolved by
 * `resolveRunTarget` — so a gone or disabled profile, or a failed read, preselects Local.
 * - **Reads only:** the fallback never writes the setting, so a stored profile disabled today is
 *   preselected again once it is a run target again.
 * - **Load race:** `touched()` is asked when the load resolves; a choice the user already made wins
 *   over the stored one (no `preselect`).
 * Returns the cancel (unmount): nothing is applied after it.
 */
export function loadRunTarget(
  bridge: Pick<Bridge, "invoke">,
  on: { profiles(list: ServerProfile[], error: string | null): void; preselect(target: string | null): void },
  touched: () => boolean,
): () => void {
  let live = true;
  void (async () => {
    let list: ServerProfile[] = [];
    let error: string | null = null;
    try {
      list = await bridge.invoke<ServerProfile[]>("list_server_profiles");
    } catch (e) {
      error = String(e);
    }
    const settings = await bridge.invoke<Record<string, string>>("get_settings").catch(() => null);
    if (!live) return;
    on.profiles(list, error);
    if (!touched()) on.preselect(resolveRunTarget(runTargetOptions(list), settings));
  })();
  return () => {
    live = false;
  };
}

// --- Cancel (ADR-024 o20.3, o22.14.2, o22.14.8) -------------------------------------------------

/**
 * Send the cancel that was offered. An offer with `confirm` (a remote job's first Cancel) asks
 * first, and is sent only if the user agreed **and** the action is still offered once the dialog
 * has closed (`currentActions`, which `sendCancel` re-reads from the backend) — o20.3's re-check: a
 * row or record change during the dialog (the offer gone) must not fire a cancel from a stale offer. An offer without `confirm` (Retry cancel, a local
 * Cancel) is sent at once. Returns what `send` answered, or null when nothing was sent.
 */
export async function confirmThenCancel<T>(deps: {
  offer: CancelSend;
  confirm: () => Promise<boolean>;
  /** The offered actions **now**, read after the dialog (async: `sendCancel` re-reads the backend). */
  currentActions: () => readonly { action: string }[] | Promise<readonly { action: string }[]>;
  send: () => Promise<T>;
}): Promise<T | null> {
  if (deps.offer.confirm) {
    // A dialog that fails to open counts as a decline.
    const agreed = await deps.confirm().catch(() => false);
    if (!agreed) return null;
    if (!(await deps.currentActions()).some((a) => a.action === deps.offer.action)) return null;
  }
  return deps.send();
}

/** The row and the record, read from the backend — what `rowActions` derives its offers from. */
interface JobAndRecord {
  job: Job;
  record: RemoteState | null;
}

/**
 * Read the row (`get_job`) and the record (`remote_job_state`). A read that fails **throws**: with
 * no row or record read there is no offer to derive, and nothing is sent.
 */
async function readJobAndRecord(id: string): Promise<JobAndRecord> {
  const [job, read] = await Promise.all([
    tauriBridge.invoke<Job>("get_job", { id }),
    tauriBridge.invoke<RemoteStateRead>("remote_job_state", { id }),
  ]);
  return { job, record: read.state };
}

/**
 * Reload after `cancel_job` / `undo_cancel` returned (o22.14.5, o22.14.7): the row and the
 * `remote_job_state` record, read for the notice (`cancel_sent_at` and the check error's `answered`
 * flag are the Rust-side facts the notice words are keyed on). A read that fails gives `null` and
 * never throws: the answer already happened, and the notice then claims nothing it cannot know.
 */
async function reloadAfterAnswer(id: string): Promise<{ job: Job | null; checkError: CheckError | null }> {
  const [job, read] = await Promise.all([
    tauriBridge.invoke<Job>("get_job", { id }).catch(() => null),
    tauriBridge.invoke<RemoteStateRead>("remote_job_state", { id }).catch(() => null),
  ]);
  return { job, checkError: read?.state?.check_error ?? null };
}

/** What a Cancel click came to: the reloaded row (null: not reloaded or unreadable) and the notice. */
export interface CancelReport {
  result: CancelResult;
  job: Job | null;
  notice: string | null;
}

/**
 * **Every Cancel / Retry cancel click of both screens** (o22.14.10): the one place that invokes
 * `cancel_job`, behind `confirmThenCancel`.
 *
 * **It trusts nothing the screen hands it** (ADR-024 o22.14.14.1; B1 verifier VX1, B2 verifier F1).
 * It takes the job **id** and the in-flight callback, nothing else: it reads the row (`get_job`) and
 * the record (`remote_job_state`) itself and derives the offer from them with
 * `cancelSendOf(rowActions(row, record))`, and the dialog is the Tauri plugin's `confirm`, imported
 * here. A screen cannot supply an offer, a `confirm` flag, a job that looks local or `cancelling`, a
 * record, or a dialog that always agrees — those arguments do not exist. If the row offers no cancel
 * send (a pending cancel, a terminal job), it sends nothing. After the dialog it reads both **again**
 * and re-derives (o20.3's re-check, now from the backend, not from the view): the cancel is sent only
 * if the same action is still offered.
 *
 * The in-flight state (`sending`, shown "Sending cancel…") starts **before the first backend read**
 * (o22.14.14.6: a second click while the reads or the dialog are pending must find the buttons
 * disabled) and ends on every path: nothing offered, a declined or failed dialog, a failed read, a
 * failed call — and, for a remote job, **when `cancel_job` has returned and the row and record are
 * reloaded** (o22.14.8: its answer is the outcome; there is no terminal `job:status` to wait for).
 * Only a local job's answered cancel keeps it, until the terminal `job:status` unmounts the button. After a remote answer `cancelNotice` words the answer from the reloaded row and
 * record (a local `Dispatched` has no notice and no reload). A failed call rethrows.
 */
export async function sendCancel(deps: { id: string; sending: (on: boolean) => void }): Promise<CancelReport | null> {
  const { id } = deps;
  deps.sending(true);
  let answered = false;
  let remote = true;
  try {
    const first = await readJobAndRecord(id);
    const offer = cancelSendOf(rowActions(first.job, first.record));
    if (!offer) return null;
    remote = coordinateState(first.job) !== "local";
    return await confirmThenCancel({
      offer,
      confirm: () => confirm(cancelConfirmText(first.job), { title: "Cancel job", kind: "warning" }),
      // A read that fails here rejects: nothing is sent from the first read's stale offer.
      currentActions: async () => {
        const now = await readJobAndRecord(id);
        return rowActions(now.job, now.record);
      },
      send: async () => {
        const result = await tauriBridge.invoke<CancelResult>("cancel_job", { id });
        answered = true;
        if (result.result === "dispatched") return { result, job: null, notice: null };
        const { job, checkError } = await reloadAfterAnswer(id);
        return { result, job, notice: cancelNotice(result, job, checkError) };
      },
    });
  } finally {
    if (!(answered && !remote)) deps.sending(false);
  }
}

/** What an Undo click came to. */
export interface UndoReport {
  result: UndoResult;
  job: Job | null;
  notice: string;
}

/**
 * Every Undo click (o22.14.1): the one place that invokes `undo_cancel`. Like `sendCancel` it takes
 * only the job id: it reads the row and record itself and sends only if they still offer Undo, never
 * asks (no dialog: Undo takes the cancel back, it is not destructive), then reloads the row and words
 * the answer.
 */
export async function sendUndo(deps: { id: string; sending: (on: boolean) => void }): Promise<UndoReport | null> {
  const { id } = deps;
  deps.sending(true);
  try {
    const now = await readJobAndRecord(id);
    if (!rowActions(now.job, now.record).some((a) => a.action === "undo_cancel")) return null;
    const result = await tauriBridge.invoke<UndoResult>("undo_cancel", { id });
    const { job } = await reloadAfterAnswer(id);
    return { result, job, notice: undoNotice(result) };
  } finally {
    deps.sending(false);
  }
}

/** What either click of the family came to, for the screen to apply. */
export interface CancelFamilyOutcome {
  /** The reloaded row, or null (not reloaded — a local cancel — or unreadable). */
  job: Job | null;
  /** The words for the user; null: nothing to say. */
  notice: string | null;
}

/**
 * **The single entry point of the screens' cancel-family buttons** (Cancel, Retry cancel, Undo
 * cancel): the screen passes the clicked button's action, the job id and the in-flight callback —
 * nothing it could forge (o22.14.14.1). Undo goes to `sendUndo`, the other two to `sendCancel`.
 * `null`: nothing was sent (declined, or the backend's row no longer offers it).
 */
export async function clickCancelFamily(
  action: CancelFamilyAction,
  deps: { id: string; sending: (busy: CancelBusy) => void },
): Promise<CancelFamilyOutcome | null> {
  if (action === "undo_cancel") {
    const undone = await sendUndo({ id: deps.id, sending: (on) => deps.sending(on ? "undo" : null) });
    return undone && { job: undone.job, notice: undone.notice };
  }
  const sent = await sendCancel({ id: deps.id, sending: (on) => deps.sending(on ? "cancel" : null) });
  return sent && { job: sent.job, notice: sent.notice };
}
