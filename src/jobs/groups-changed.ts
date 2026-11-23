/**
 * The Jobs screen's `groups:changed` listener (ADR-027 M4 amendment, item 7). Claude's `create_group`
 * tool makes a group behind the screen's back; Rust emits a payload-free `groups:changed` once the
 * group is committed, and the screen reloads. The listener is injectable, like `live-wiring.ts`'s
 * bridge, so the subscription and its teardown are testable without a WebView.
 */
import { createListenerScope, type Bridge } from "./live-wiring";

/** Rust's `server::wiring::GROUPS_CHANGED`. Pinned there by `the_app_emits_groups_changed_with_no_payload`. */
export const GROUPS_CHANGED_EVENT = "groups:changed";

export interface GroupsChangedSubscription {
  /**
   * Resolves once the listener is attached (`false`: it was disposed first, so nothing is attached).
   * A view that wants "listen first, then ask" (ADR-024 o16.7) reads its groups after this.
   */
  ready: Promise<boolean>;
  /** Detach. Safe before `ready` resolves: a registration that completes late is dropped at once. */
  dispose(): void;
}

/** Call `onChanged` on every `groups:changed`, until `dispose`. The payload is not read. */
export function subscribeGroupsChanged(bridge: Pick<Bridge, "listen">, onChanged: () => void): GroupsChangedSubscription {
  const scope = createListenerScope(bridge);
  const ready = scope.listen<unknown>(GROUPS_CHANGED_EVENT, () => onChanged());
  return { ready, dispose: () => scope.dispose() };
}
