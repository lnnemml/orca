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

import type { ConvergenceEvent, ConvergencePayload } from "../convergence/types";
import type { CancelResult, Job, JobStatus, RemoteStateEvent, RemoteStateRead, ServerProfile } from "../types";
import {
  backfillsLog,
  cancelConfirmText,
  coordinateState,
  mergeRemoteState,
  NOTHING_HELD,
  resolveRunTarget,
  runTargetOptions,
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
 * Send the cancel the view offered. An offer with `confirm` (a remote job's first Cancel) asks
 * first, and is sent only if the user agreed **and** the view still offers the same action once the
 * dialog has closed — o20.3's re-check: a row or record change during the dialog (the offer gone)
 * must not fire a cancel from a stale offer. An offer without `confirm` (Retry cancel, a local
 * Cancel) is sent at once. Returns what `send` answered, or null when nothing was sent.
 */
export async function confirmThenCancel<T>(deps: {
  offer: CancelSend;
  confirm: () => Promise<boolean>;
  /** The view's offered actions **now** (read after the dialog, e.g. from a ref). */
  currentActions: () => readonly { action: string }[];
  send: () => Promise<T>;
}): Promise<T | null> {
  if (deps.offer.confirm) {
    // A dialog that fails to open counts as a decline.
    const agreed = await deps.confirm().catch(() => false);
    if (!agreed) return null;
    if (!deps.currentActions().some((a) => a.action === deps.offer.action)) return null;
  }
  return deps.send();
}

/** The plugin-dialog `confirm`, as the screens pass it. */
export type ConfirmDialog = (message: string, options: { title: string; kind: "warning" }) => Promise<boolean>;

/**
 * **Every Cancel click of both screens** (o22.14.10): the one place that invokes `cancel_job`, behind
 * `confirmThenCancel` — so no screen cancels a remote job without the confirm its offer asks for.
 * `offer` is what `rowActions` offered at the click. The in-flight state (`sending`, shown "Sending
 * cancel…") starts once the cancel is really sent, and **for a remote job ends when `cancel_job`
 * returns** (o22.14.8: its answer is the outcome; there is no terminal `job:status` to wait for); a
 * local job keeps today's behaviour — it stays until the terminal `job:status` unmounts the button,
 * and ends only if the call fails. Returns the answer (null: not sent); a failed call rethrows.
 */
export async function sendCancel(deps: {
  bridge: Pick<Bridge, "invoke">;
  job: Pick<Job, "id" | "title" | "remote_host" | "remote_job_dir" | "remote_socket">;
  offer: CancelSend;
  confirm: ConfirmDialog;
  currentActions: () => readonly { action: string }[];
  sending: (on: boolean) => void;
}): Promise<CancelResult | null> {
  const remote = coordinateState(deps.job) !== "local";
  return confirmThenCancel({
    offer: deps.offer,
    confirm: () => deps.confirm(cancelConfirmText(deps.job), { title: "Cancel job", kind: "warning" }),
    currentActions: deps.currentActions,
    send: async () => {
      deps.sending(true);
      let answered = false;
      try {
        const result = await deps.bridge.invoke<CancelResult>("cancel_job", { id: deps.job.id });
        answered = true;
        return result;
      } finally {
        if (remote || !answered) deps.sending(false);
      }
    },
  });
}
