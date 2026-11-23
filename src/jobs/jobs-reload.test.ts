import { describe, expect, it, vi } from "vitest";
import { JOB_STATUS_EVENT, subscribeJobsReload } from "./jobs-reload";
import type { Bridge } from "./live-wiring";

/** A `listen` held until the test releases (or fails) it; each handler is recorded with its unlisten state. */
function fakeListen() {
  const entries: {
    event: string;
    handler: (e: { payload: unknown }) => void;
    resolve: () => void;
    reject: (e: unknown) => void;
    unlistened: boolean;
  }[] = [];
  const listen: Bridge["listen"] = <T>(event: string, handler: (e: { payload: T }) => void) =>
    new Promise((resolve, reject) => {
      const entry = {
        event,
        handler: handler as (e: { payload: unknown }) => void,
        unlistened: false,
        reject,
        resolve: () =>
          resolve(() => {
            entry.unlistened = true;
          }),
      };
      entries.push(entry);
    });
  const emit = (event: string, payload: unknown = null) =>
    entries.filter((e) => e.event === event && !e.unlistened).forEach((e) => e.handler({ payload }));
  const releaseAll = () => entries.forEach((e) => e.resolve());
  return { listen, entries, emit, releaseAll };
}

describe("subscribeJobsReload", () => {
  it("listens for job:status and groups:changed, the names Rust emits", () => {
    expect(JOB_STATUS_EVENT).toBe("job:status");
    const f = fakeListen();
    subscribeJobsReload({ listen: f.listen }, () => {}, () => {});
    expect(f.entries.map((e) => e.event).sort()).toEqual(["groups:changed", "job:status"]);
  });

  // CONTROL: drop the groups subscription → a group Claude made never appears → red.
  it("reloads on groups:changed", async () => {
    const f = fakeListen();
    const reload = vi.fn();
    const sub = subscribeJobsReload({ listen: f.listen }, reload, () => {});
    f.releaseAll();
    await sub.ready;
    f.emit("groups:changed");
    expect(reload).toHaveBeenCalledTimes(1);
  });

  // CONTROL: drop the job:status subscription → "Claude is submitting" never clears → red.
  it("reloads on job:status, whose payload carries no mcp_state", async () => {
    const f = fakeListen();
    const reload = vi.fn();
    const sub = subscribeJobsReload({ listen: f.listen }, reload, () => {});
    f.releaseAll();
    await sub.ready;
    f.emit("job:status", { job_id: "j", status: "queued" });
    f.emit("job:status", { job_id: "k", status: "completed" });
    expect(reload).toHaveBeenCalledTimes(2);
    f.emit("job:log");
    expect(reload).toHaveBeenCalledTimes(2);
  });

  // CONTROL: skip `dispose()` of the groups subscription (or of the status scope) → a handler stays attached → red.
  it("detaches both on dispose, and reloads nothing after it", async () => {
    const f = fakeListen();
    const reload = vi.fn();
    const sub = subscribeJobsReload({ listen: f.listen }, reload, () => {});
    f.releaseAll();
    await sub.ready;
    sub.dispose();
    expect(f.entries.every((e) => e.unlistened)).toBe(true);
    f.entries.forEach((e) => e.handler({ payload: null }));
    expect(reload).not.toHaveBeenCalled();
  });

  it("drops registrations that resolve after dispose (an unmount mid-listen)", async () => {
    const f = fakeListen();
    const reload = vi.fn();
    const sub = subscribeJobsReload({ listen: f.listen }, reload, () => {});
    sub.dispose();
    f.releaseAll();
    await sub.ready;
    expect(f.entries.every((e) => e.unlistened)).toBe(true);
    f.entries.forEach((e) => e.handler({ payload: null }));
    expect(reload).not.toHaveBeenCalled();
  });

  // CONTROL: remove the `.catch` on a registration → `ready` rejects (an unhandled rejection) → red.
  it("reports a failed registration and never rejects", async () => {
    const f = fakeListen();
    const onError = vi.fn();
    const sub = subscribeJobsReload({ listen: f.listen }, () => {}, onError);
    f.entries.find((e) => e.event === "groups:changed")!.reject(new Error("no bridge"));
    f.entries.find((e) => e.event === "job:status")!.resolve();
    await expect(sub.ready).resolves.toBeUndefined();
    expect(onError).toHaveBeenCalledTimes(1);
    expect(onError.mock.calls[0]![0]).toContain("no bridge");
    // The one that did attach still works.
    const reload = vi.fn();
    const g = fakeListen();
    const sub2 = subscribeJobsReload({ listen: g.listen }, reload, () => {});
    g.entries.find((e) => e.event === "groups:changed")!.reject(new Error("x"));
    g.entries.find((e) => e.event === "job:status")!.resolve();
    await sub2.ready;
    g.emit("job:status");
    expect(reload).toHaveBeenCalledTimes(1);
  });
});
