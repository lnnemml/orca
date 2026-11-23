/**
 * When the Jobs screen re-reads its list (ADR-027 M4 amendment, items 7 and 8): on `job:status`, and
 * on `groups:changed`. Both are payload-free for this purpose — the event says only that something
 * moved, and the screen's own `load()` reads the truth. In particular `job:status` carries no
 * `mcp_state`, so "Claude is submitting" clears only because the row is read again.
 *
 * Injectable like `live-wiring.ts`'s bridge, so the subscription, its failure path and its teardown
 * are testable without a WebView.
 */
import { subscribeGroupsChanged } from "./groups-changed";
import { createListenerScope, type Bridge } from "./live-wiring";

/** Rust's `job:status` (the poller's and the backends' status transitions). */
export const JOB_STATUS_EVENT = "job:status";

export interface JobsReloadSubscription {
  /**
   * Settles once both registrations have finished — attached, or failed (the failure went to
   * `onError`); never rejects. The screen reads its list after this ("listen first, then ask",
   * ADR-024 o16.7), so a group or a status that changes while it loads is not missed.
   */
  ready: Promise<void>;
  /** Detach both. Safe before `ready` settles: a registration that completes late is dropped at once. */
  dispose(): void;
}

/**
 * Call `reload` on every `job:status` and every `groups:changed`, until `dispose`. A registration
 * that fails is reported to `onError` and caught here: no unhandled rejection reaches the window.
 */
export function subscribeJobsReload(
  bridge: Pick<Bridge, "listen">,
  reload: () => void,
  onError: (message: string) => void,
): JobsReloadSubscription {
  const failed = (e: unknown) => onError(String(e));
  const status = createListenerScope(bridge);
  const statusReady = status.listen<unknown>(JOB_STATUS_EVENT, () => reload());
  const groups = subscribeGroupsChanged(bridge, reload);
  const ready = Promise.all([statusReady.catch(failed), groups.ready.catch(failed)]).then(() => undefined);
  return {
    ready,
    dispose() {
      status.dispose();
      groups.dispose();
    },
  };
}
