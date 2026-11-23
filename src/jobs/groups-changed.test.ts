import { describe, expect, it, vi } from "vitest";
import { GROUPS_CHANGED_EVENT, subscribeGroupsChanged } from "./groups-changed";
import type { Bridge } from "./live-wiring";

/** A `listen` held until the test releases it; every handler is recorded with its unlisten state. */
function fakeListen() {
  const entries: { event: string; handler: (e: { payload: unknown }) => void; resolve: () => void; unlistened: boolean }[] = [];
  const listen: Bridge["listen"] = <T>(event: string, handler: (e: { payload: T }) => void) =>
    new Promise((resolve) => {
      const entry = {
        event,
        handler: handler as (e: { payload: unknown }) => void,
        unlistened: false,
        resolve: () =>
          resolve(() => {
            entry.unlistened = true;
          }),
      };
      entries.push(entry);
    });
  const emit = (event: string, payload: unknown = null) =>
    entries.filter((e) => e.event === event && !e.unlistened).forEach((e) => e.handler({ payload }));
  return { listen, entries, emit };
}

describe("subscribeGroupsChanged", () => {
  it("listens for groups:changed, the name Rust emits", () => {
    expect(GROUPS_CHANGED_EVENT).toBe("groups:changed");
    const f = fakeListen();
    subscribeGroupsChanged({ listen: f.listen }, () => {});
    expect(f.entries.map((e) => e.event)).toEqual(["groups:changed"]);
  });

  it("calls onChanged once per event, without reading a payload", async () => {
    const f = fakeListen();
    const onChanged = vi.fn();
    const sub = subscribeGroupsChanged({ listen: f.listen }, onChanged);
    f.entries[0]!.resolve();
    expect(await sub.ready).toBe(true);
    f.emit("groups:changed");
    f.emit("groups:changed", { anything: 1 });
    expect(onChanged).toHaveBeenCalledTimes(2);
    expect(onChanged).toHaveBeenCalledWith();
  });

  it("ignores other events", async () => {
    const f = fakeListen();
    const onChanged = vi.fn();
    const sub = subscribeGroupsChanged({ listen: f.listen }, onChanged);
    f.entries[0]!.resolve();
    await sub.ready;
    f.emit("job:status");
    expect(onChanged).not.toHaveBeenCalled();
  });

  it("detaches on dispose, and calls nothing after it", async () => {
    const f = fakeListen();
    const onChanged = vi.fn();
    const sub = subscribeGroupsChanged({ listen: f.listen }, onChanged);
    f.entries[0]!.resolve();
    await sub.ready;
    sub.dispose();
    expect(f.entries[0]!.unlistened).toBe(true);
    f.entries[0]!.handler({ payload: null });
    expect(onChanged).not.toHaveBeenCalled();
  });

  it("drops a registration that resolves after dispose (an unmount mid-listen)", async () => {
    const f = fakeListen();
    const onChanged = vi.fn();
    const sub = subscribeGroupsChanged({ listen: f.listen }, onChanged);
    sub.dispose();
    f.entries[0]!.resolve();
    expect(await sub.ready).toBe(false);
    expect(f.entries[0]!.unlistened).toBe(true);
    f.entries[0]!.handler({ payload: null });
    expect(onChanged).not.toHaveBeenCalled();
  });
});
