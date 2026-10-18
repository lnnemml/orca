import { describe, expect, it } from "vitest";
import type { Job, RemoteState, RemoteStateEvent, RemoteStateRead } from "../types";
import { makeProfile } from "../servers/test-fixtures";
import { LAST_RUN_TARGET_SETTING, type CancelSend, type HeldRemoteState } from "./remote";
import {
  confirmThenCancel,
  loadRunTarget,
  sendCancel,
  openJobView,
  REMOTE_STATE_EVENT,
  trackRemoteStates,
  type Bridge,
  type JobViewHandlers,
} from "./live-wiring";

// --- A fake bridge: every `listen` and every `invoke` is held until the test releases it ----------

interface PendingListen {
  event: string;
  handler: (e: { payload: unknown }) => void;
  resolve: () => void;
  resolved: boolean;
  unlistened: boolean;
}

interface PendingInvoke {
  cmd: string;
  args?: Record<string, unknown>;
  resolve: (value: unknown) => void;
}

/** `autoReply`: commands answered at once with the given value (still logged). */
function fakeBridge(autoReply: Record<string, unknown> = {}) {
  const listens: PendingListen[] = [];
  const invokes: PendingInvoke[] = [];
  /** Every call in order, `listen:<event>` once registered (resolved), `invoke:<cmd>` when sent. */
  const log: string[] = [];
  const bridge: Bridge = {
    listen: <T>(event: string, handler: (e: { payload: T }) => void) =>
      new Promise((resolve) => {
        const entry: PendingListen = {
          event,
          handler: handler as (e: { payload: unknown }) => void,
          unlistened: false,
          resolved: false,
          resolve: () => {
            entry.resolved = true;
            log.push(`listen:${event}`);
            resolve(() => {
              entry.unlistened = true;
            });
          },
        };
        listens.push(entry);
      }),
    invoke: <T>(cmd: string, args?: Record<string, unknown>) =>
      new Promise<T>((resolve) => {
        log.push(`invoke:${cmd}`);
        if (cmd in autoReply) return resolve(autoReply[cmd] as T);
        invokes.push({ cmd, args, resolve: resolve as (value: unknown) => void });
      }),
  };
  const emit = (event: string, payload: unknown) =>
    listens.filter((l) => l.event === event && !l.unlistened).forEach((l) => l.handler({ payload }));
  const pendingInvoke = (cmd: string) => invokes.find((i) => i.cmd === cmd);
  return { bridge, listens, invokes, log, emit, pendingInvoke };
}

/** Let every queued microtask run (each `await` in the code under test is one hop). */
const settle = () => new Promise((r) => setTimeout(r, 0));

function record(over: Partial<RemoteState> = {}): RemoteState {
  return {
    known: null,
    check_error: null,
    fetch: { strikes: 0, stopped: false },
    last_contact: null,
    cancel_sends: { failures: 0, stopped: false },
    sweeps: { count: 0, stopped: false },
    ...over,
  };
}

const RUNNING = record({ known: { kind: "shown", detail: { outcome: "running" } } });
const QUEUED = record({ known: { kind: "shown", detail: { outcome: "queued" } } });

// --- The remote record (o19.6) -------------------------------------------------------------------

describe("trackRemoteStates (o19.6)", () => {
  function tracker() {
    const fake = fakeBridge();
    let held: ReadonlyMap<string, HeldRemoteState> = new Map();
    const t = trackRemoteStates(fake.bridge, (h) => (held = h), () => {});
    return { fake, t, held: () => held };
  }

  // N1 — CONTROL: in `read`, drop the `await ready` (ask before the listener resolved) → red.
  it("reads remote_job_state only after the job:remote-state listener has resolved", async () => {
    const { fake, t } = tracker();
    const reading = t.read(["j1"]);
    await settle();
    expect(fake.listens.map((l) => l.event)).toEqual([REMOTE_STATE_EVENT]);
    expect(fake.pendingInvoke("remote_job_state")).toBeUndefined();
    fake.listens[0].resolve();
    await settle();
    expect(fake.log).toEqual([`listen:${REMOTE_STATE_EVENT}`, "invoke:remote_job_state"]);
    expect(fake.pendingInvoke("remote_job_state")?.args).toEqual({ id: "j1" });
    fake.pendingInvoke("remote_job_state")?.resolve({ seq: 1, state: QUEUED } satisfies RemoteStateRead);
    await reading;
  });

  // N2 — CONTROL: apply the read by plain assignment (`held.set(id, {seq, state})`, bypassing
  // `mergeRemoteState`) → the older read overwrites the newer event, red.
  it("keeps a newer event when an older read arrives after it, and takes a newer read over an older event", async () => {
    const { fake, t, held } = tracker();
    const reading = t.read(["j1", "j2"]);
    fake.listens[0].resolve();
    await settle();
    // The poller changes j1 while the read is in flight: the event (seq 7) lands first.
    fake.emit(REMOTE_STATE_EVENT, { job_id: "j1", seq: 7, state: RUNNING } satisfies RemoteStateEvent);
    // An older event for j2 arrives before j2's newer read.
    fake.emit(REMOTE_STATE_EVENT, { job_id: "j2", seq: 2, state: QUEUED } satisfies RemoteStateEvent);
    const [r1, r2] = fake.invokes.filter((i) => i.cmd === "remote_job_state");
    r1.resolve({ seq: 5, state: QUEUED } satisfies RemoteStateRead);
    r2.resolve({ seq: 9, state: RUNNING } satisfies RemoteStateRead);
    await reading;
    expect(held().get("j1")).toEqual({ seq: 7, state: RUNNING });
    expect(held().get("j2")).toEqual({ seq: 9, state: RUNNING });
    // A newer removal still wins over what was held.
    fake.emit(REMOTE_STATE_EVENT, { job_id: "j1", seq: 10, state: null } satisfies RemoteStateEvent);
    expect(held().get("j1")).toEqual({ seq: 10, state: null });
  });

  // N5 (tracker) — CONTROL: in `createListenerScope.listen`, drop the `if (disposed) unlisten()`
  // branch → the late listener stays attached, red.
  it("drops a listener whose listen resolves after dispose, and asks nothing", async () => {
    const { fake, t } = tracker();
    const reading = t.read(["j1"]);
    t.dispose();
    fake.listens[0].resolve();
    await reading;
    expect(fake.listens[0].unlistened).toBe(true);
    expect(fake.pendingInvoke("remote_job_state")).toBeUndefined();
  });
});

// --- The job view's log (o16.7) ------------------------------------------------------------------

function job(over: Partial<Job> = {}): Job {
  return {
    id: "j1",
    title: "job",
    input_content: "! HF",
    status: "running",
    job_dir: null,
    energy: null,
    wall_time: null,
    error_message: null,
    created_at: "2026-10-07",
    started_at: null,
    completed_at: null,
    scene_json: null,
    scene_log_json: null,
    pathway_id: null,
    group_id: null,
    backend_id: null,
    remote_host: null,
    remote_job_dir: null,
    remote_socket: null,
    cancel_requested_at: null,
    cancel_sent_at: null,
    ...over,
  };
}

const REMOTE = { backend_id: "p1", remote_host: "uni", remote_job_dir: "/srv/jobs/j1", remote_socket: "/tmp/ts-0" };

function handlers() {
  const seen: string[] = [];
  const on: JobViewHandlers = {
    job: () => seen.push("job"),
    lines: (l) => seen.push(`lines:${l.join(",")}`),
    convergence: () => seen.push("convergence"),
    reset: () => seen.push("reset"),
    status: (s) => seen.push(`status:${s}`),
    backfill: (l) => seen.push(`backfill:${l.join(",")}`),
    error: (m) => seen.push(`error:${m}`),
  };
  return { on, seen };
}

const LOG_EVENTS = ["job:log", "job:convergence", "job:log-reset", "job:status"];

/** Open the view; `get_job` answers `row` whenever it is asked. Listeners are pending in order. */
async function opened(row: Job, afterOpen: (() => Promise<void>) | null = null) {
  const fake = fakeBridge({ get_job: row });
  const { on, seen } = handlers();
  const close = openJobView(fake.bridge, "j1", on, afterOpen);
  await settle();
  return { fake, close, seen };
}

/** Resolve the view's listeners one by one, in the order it registers them. */
async function attachAll(fake: ReturnType<typeof fakeBridge>) {
  for (let i = 0; i < LOG_EVENTS.length; i++) {
    expect(fake.listens[i]?.event).toBe(LOG_EVENTS[i]);
    fake.listens[i].resolve();
    await settle();
  }
}

describe("openJobView (o16.7)", () => {
  // N3a — CONTROL: stop awaiting any one of the three listens (e.g. `(void scope.listen(
  // "job:log-reset", …), true)`) → the open goes out while that listener is pending, red.
  it.each(["job:log", "job:convergence", "job:log-reset"])(
    "opens the watch only after job:log, job:convergence and job:log-reset have resolved (holding %s)",
    async (held) => {
      const { fake } = await opened(job(REMOTE));
      // Resolve every other listener as soon as it is registered, whatever the order.
      for (let round = 0; round < LOG_EVENTS.length + 1; round++) {
        fake.listens.filter((l) => l.event !== held && !l.resolved).forEach((l) => l.resolve());
        await settle();
      }
      expect(fake.pendingInvoke("watch_job_log")).toBeUndefined();
      fake.listens.find((l) => l.event === held)?.resolve();
      for (let round = 0; round < LOG_EVENTS.length + 1; round++) {
        fake.listens.filter((l) => !l.resolved).forEach((l) => l.resolve());
        await settle();
      }
      const open = fake.log.indexOf("invoke:watch_job_log");
      for (const event of LOG_EVENTS.slice(0, 3)) expect(open).toBeGreaterThan(fake.log.indexOf(`listen:${event}`));
      expect(fake.pendingInvoke("watch_job_log")?.args).toEqual({ id: "j1", open: true });
    },
  );

  // N3b — CONTROL: close unconditionally (drop the `if (watchOpened)`) → a close is sent, red.
  it("does not close a watch it never opened", async () => {
    const { fake, close } = await opened(job(REMOTE));
    fake.listens[0].resolve();
    await settle();
    close();
    // The second listener resolves after the close: the view stops there.
    fake.listens[1].resolve();
    await settle();
    expect(fake.listens).toHaveLength(2);
    expect(fake.invokes.filter((i) => i.cmd === "watch_job_log")).toEqual([]);
  });

  // N3c — CONTROL: never close (drop the close invoke) → red.
  it("closes the watch it opened", async () => {
    const { fake, close } = await opened(job(REMOTE));
    await attachAll(fake);
    close();
    const watches = fake.invokes.filter((i) => i.cmd === "watch_job_log").map((i) => i.args);
    expect(watches).toEqual([
      { id: "j1", open: true },
      { id: "j1", open: false },
    ]);
  });

  // N4 — CONTROL: replace `backfillsLog(current)` with `current.status !== "draft"` → red.
  it("does not backfill a remote live job, and backfills a local running job", async () => {
    const remote = await opened(job({ ...REMOTE, status: "running" }));
    await attachAll(remote.fake);
    remote.fake.pendingInvoke("watch_job_log")?.resolve(undefined);
    await settle();
    expect(remote.fake.pendingInvoke("read_job_output")).toBeUndefined();
    expect(remote.fake.pendingInvoke("read_job_convergence")).toBeUndefined();

    const local = await opened(job({ status: "running" }));
    await attachAll(local.fake);
    local.fake.pendingInvoke("watch_job_log")?.resolve(undefined);
    await settle();
    local.fake.pendingInvoke("read_job_output")?.resolve(["a", "b"]);
    await settle();
    local.fake.pendingInvoke("read_job_convergence")?.resolve([]);
    await settle();
    expect(local.seen).toContain("backfill:a,b");
  });

  // o22.14.8 — CONTROL: drop `cancelling` from `isLiveStatus` (backfillsLog's live test) → the view
  // also reads the log on disk, and each re-streamed line shows twice, red.
  it("does not backfill a cancelling remote job, so a re-streamed line shows once", async () => {
    const v = await opened(job({ ...REMOTE, status: "cancelling" }));
    await attachAll(v.fake);
    v.fake.pendingInvoke("watch_job_log")?.resolve(undefined);
    await settle();
    v.fake.emit("job:log", { job_id: "j1", lines: ["a", "b"] });
    v.fake.pendingInvoke("read_job_output")?.resolve(["a", "b"]);
    await settle();
    v.fake.pendingInvoke("read_job_convergence")?.resolve([]);
    await settle();
    expect(v.fake.invokes.some((i) => i.cmd === "read_job_output")).toBe(false);
    expect(v.seen.filter((e) => e.endsWith("a,b"))).toEqual(["lines:a,b"]);
  });

  // N5 — CONTROL: in `createListenerScope.listen`, drop the `if (disposed) unlisten()` branch →
  // the late `job:log` listener stays attached, red.
  it("unlistens a listener whose listen resolves after the view closed, and goes no further", async () => {
    const { fake, close, seen } = await opened(job(REMOTE));
    close();
    fake.listens[0].resolve();
    await settle();
    expect(fake.listens[0].unlistened).toBe(true);
    expect(fake.listens).toHaveLength(1);
    expect(fake.log).toEqual(["listen:job:log"]);
    fake.emit("job:log", { job_id: "j1", lines: ["late"] });
    expect(seen).toEqual([]);
  });

  // L2 — CONTROL: drop `|| scope.disposed` from the guard before the open → for some close timing
  // an open is sent after the close and never closed, red.
  it("never leaves a watch open when the view closes just after its last listener resolved", async () => {
    for (let ticks = 0; ticks < 8; ticks++) {
      const { fake, close } = await opened(job(REMOTE));
      for (let i = 0; i < 3; i++) {
        fake.listens[i].resolve();
        await settle();
      }
      fake.listens[3].resolve();
      for (let t = 0; t < ticks; t++) await Promise.resolve();
      close();
      await settle();
      const opens = fake.invokes.filter((i) => i.cmd === "watch_job_log" && i.args?.open === true).length;
      const closes = fake.invokes.filter((i) => i.cmd === "watch_job_log" && i.args?.open === false).length;
      // No open, or an open matched by its close — never a dangling open.
      expect({ ticks, opens }).toEqual({ ticks, opens: closes });
    }
  });

  // L3 — CONTROL: read `get_job` before attaching the listeners → the read precedes the
  // `job:status` listener, red.
  it("reads the row only after job:status is attached, so no status change falls between", async () => {
    const { fake, seen } = await opened(job({ status: "queued" }));
    expect(fake.log).not.toContain("invoke:get_job");
    await attachAll(fake);
    fake.pendingInvoke("watch_job_log")?.resolve(undefined);
    await settle();
    expect(fake.log.indexOf("invoke:get_job")).toBeGreaterThan(fake.log.indexOf("listen:job:status"));
    expect(seen[0]).toBe("job");
  });

  it("drops the held log on job:log-reset, ignores other jobs' events, then runs the submit", async () => {
    let submitted = false;
    const { fake, seen } = await opened(job({ status: "draft" }), async () => {
      submitted = true;
    });
    await attachAll(fake);
    fake.pendingInvoke("watch_job_log")?.resolve(undefined);
    await settle();
    expect(submitted).toBe(true);
    fake.emit("job:log", { job_id: "j2", lines: ["other"] });
    fake.emit("job:log", { job_id: "j1", lines: ["x"] });
    fake.emit("job:log-reset", { job_id: "j1" });
    fake.emit("job:status", { job_id: "j1", status: "queued" });
    expect(seen).toEqual(["job", "lines:x", "reset", "status:queued"]);
  });
});

// --- "Run on" on load (o20.1, o20.4a) --------------------------------------------------------------

describe("loadRunTarget (o20.1)", () => {
  const ENABLED = makeProfile({ id: "p1", run_target: { is_run_target: true, reason: null } });
  const DISABLED = makeProfile({ id: "p2", run_target: { is_run_target: false, reason: "no core mask" } });

  /** Run the load over canned replies; `undefined` reply = the command fails. */
  async function load(profiles: unknown, settings: unknown, touched = () => false) {
    const calls: string[] = [];
    const bridge: Pick<Bridge, "invoke"> = {
      invoke: async <T,>(cmd: string): Promise<T> => {
        calls.push(cmd);
        const reply = cmd === "list_server_profiles" ? profiles : cmd === "get_settings" ? settings : undefined;
        if (reply === undefined) throw new Error(`${cmd} failed`);
        return reply as T;
      },
    };
    const seen: { preselect?: string | null; error?: string | null } = {};
    loadRunTarget(bridge, { profiles: (_l, e) => (seen.error = e), preselect: (t) => (seen.preselect = t) }, touched);
    await settle();
    return { calls, seen };
  }
  const stored = (v: string) => ({ [LAST_RUN_TARGET_SETTING]: v });

  // CONTROL: write the setting in the fallback (e.g. `bridge.invoke("set_setting", …)` when the
  // resolved target differs from the stored one) → the `set_setting` call appears, red.
  it("preselects an enabled stored profile, falls back to Local otherwise, and never writes the setting", async () => {
    const cases: [unknown, unknown, string | null][] = [
      [[ENABLED, DISABLED], stored("p1"), "p1"],
      [[ENABLED, DISABLED], stored("p2"), null], // disabled today
      [[ENABLED], stored("gone"), null], // deleted
      [[ENABLED], undefined, null], // settings read failed
      [undefined, stored("p1"), null], // profile list failed
    ];
    for (const [profiles, settings, want] of cases) {
      const { calls, seen } = await load(profiles, settings);
      expect(seen.preselect).toBe(want);
      expect(calls).not.toContain("set_setting");
    }
    expect((await load(undefined, stored("p1"))).seen.error).toBe("Error: list_server_profiles failed");
  });

  // CONTROL: drop the `if (!touched())` guard → the stored choice overwrites the user's, red.
  it("leaves a choice the user made while the load was pending", async () => {
    const { seen } = await load([ENABLED], stored("p1"), () => true);
    expect(seen.preselect).toBeUndefined();
  });
});

// --- Cancel (o20.3, o22.14.2, o22.14.8, o22.14.10) -------------------------------------------------

describe("confirmThenCancel (o20.3, o22.14.2)", () => {
  /** A dialog held open until the test answers it, and a view whose actions the test can change. */
  function setup(offer: CancelSend) {
    // Optional: a version that never opens the dialog must fail on its assertions, not here.
    let answer: ((ok: boolean) => void) | undefined;
    let fail: (() => void) | undefined;
    let asked = 0;
    const view: { actions: { action: string }[] } = { actions: [{ action: offer.action }] };
    let sent = 0;
    const done = confirmThenCancel({
      offer,
      confirm: () =>
        new Promise<boolean>((resolve, reject) => {
          asked += 1;
          answer = resolve;
          fail = () => reject(new Error("dialog failed"));
        }),
      currentActions: () => view.actions,
      send: async () => {
        sent += 1;
        return "sent";
      },
    });
    return { done, view, answer: (ok: boolean) => answer?.(ok), fail: () => fail?.(), sent: () => sent, asked: () => asked };
  }

  const FIRST: CancelSend = { action: "cancel", confirm: true };

  // CONTROL: skip the confirm (`const agreed = true`) → the declined and failed cases send, red.
  it("sends nothing when declined or when the dialog fails, and exactly one cancel when confirmed", async () => {
    const declined = setup(FIRST);
    declined.answer(false);
    expect(await declined.done).toBeNull();
    expect(declined.sent()).toBe(0);

    const failed = setup(FIRST);
    failed.fail();
    expect(await failed.done).toBeNull();
    expect(failed.sent()).toBe(0);

    const agreed = setup(FIRST);
    agreed.answer(true);
    expect(await agreed.done).toBe("sent");
    expect(agreed.sent()).toBe(1);
  });

  // CONTROL: read the actions before the dialog (capture them up front) or drop the re-check → the
  // stale offer sends, red.
  it("sends nothing when the offer vanished while the dialog was open", async () => {
    const stale = setup(FIRST);
    stale.view.actions = [{ action: "undo_cancel" }]; // e.g. another view's cancel recorded meanwhile
    stale.answer(true);
    expect(await stale.done).toBeNull();
    expect(stale.sent()).toBe(0);
  });

  // CONTROL: ask whatever the offer says (drop the `if (deps.offer.confirm)`) → Retry cancel opens
  // the dialog, red.
  it("sends an offer without confirm at once, with no dialog (Retry cancel, a local Cancel)", async () => {
    const retry = setup({ action: "retry_cancel", confirm: false });
    expect(await retry.done).toBe("sent");
    expect(retry.sent()).toBe(1);
    expect(retry.asked()).toBe(0);
  });
});

describe("sendCancel (o22.14.8, o22.14.10)", () => {
  const REMOTE = { id: "j1", title: "TS search", remote_host: "uni", remote_job_dir: "/r/jobs/j1", remote_socket: "/r/tsp/slot0.sock" };
  const LOCAL = { ...REMOTE, remote_host: null, remote_job_dir: null, remote_socket: null };

  function bridge(answer: () => Promise<unknown>) {
    const calls: { cmd: string; args?: Record<string, unknown> }[] = [];
    return {
      calls,
      bridge: {
        invoke: <T,>(cmd: string, args?: Record<string, unknown>) => {
          calls.push({ cmd, args });
          return answer() as Promise<T>;
        },
      },
    };
  }

  // CONTROL: drop the `finally` reset (or reset only for a local job) → the remote state stays
  // "Sending cancel…" after `cancel_job` returned, red.
  it("ends a remote job's in-flight state when cancel_job returns, and keeps a local one's", async () => {
    for (const [job, offer, after] of [
      [REMOTE, { action: "cancel", confirm: true }, false],
      [LOCAL, { action: "cancel", confirm: false }, true],
    ] as const) {
      const { calls, bridge: b } = bridge(async () => ({ result: "pending", already_pending: false }));
      const sending: boolean[] = [];
      const result = await sendCancel({
        bridge: b,
        job,
        offer,
        confirm: async () => true,
        currentActions: () => [{ action: "cancel" }],
        sending: (on) => sending.push(on),
      });
      expect(result).toEqual({ result: "pending", already_pending: false });
      expect(calls).toEqual([{ cmd: "cancel_job", args: { id: "j1" } }]);
      expect(sending[0]).toBe(true);
      expect(sending[sending.length - 1]).toBe(after);
    }
  });

  it("ends the in-flight state when cancel_job fails, for both, and rethrows", async () => {
    for (const job of [REMOTE, LOCAL]) {
      const { bridge: b } = bridge(async () => {
        throw new Error("conflict: an operation on job j1 is already in progress");
      });
      const sending: boolean[] = [];
      await expect(
        sendCancel({ bridge: b, job, offer: { action: "cancel", confirm: false }, confirm: async () => true, currentActions: () => [], sending: (on) => sending.push(on) }),
      ).rejects.toThrow("already in progress");
      expect(sending).toEqual([true, false]);
    }
  });

  it("asks with o22.14.2's title and text for a remote job's first Cancel, and sends nothing when declined", async () => {
    const { calls, bridge: b } = bridge(async () => ({ result: "dispatched" }));
    const asked: { message: string; title: string }[] = [];
    const sending: boolean[] = [];
    const result = await sendCancel({
      bridge: b,
      job: REMOTE,
      offer: { action: "cancel", confirm: true },
      confirm: async (message, options) => {
        asked.push({ message, title: options.title });
        return false;
      },
      currentActions: () => [{ action: "cancel" }],
      sending: (on) => sending.push(on),
    });
    expect(result).toBeNull();
    expect(calls).toEqual([]);
    // Nothing is in flight before the user agreed.
    expect(sending).toEqual([]);
    expect(asked).toEqual([
      {
        title: "Cancel job",
        message:
          'Cancel "TS search" on uni? The server stops it; files already written stay on the server and are downloaded ' +
          "for viewing. You can undo until the cancel is sent.",
      },
    ]);
  });
});

describe("every Cancel goes through sendCancel (o22.14.10, source pin)", () => {
  // The dialog itself is a live-gate item; what is pinned here is that no screen can reach
  // `cancel_job` other than through the one helper behind `confirmThenCancel`. CONTROL: call
  // `invoke("cancel_job", …)` directly from either screen, or send from `sendCancel` without
  // `confirmThenCancel` → red.
  it("leaves cancel_job to the one shared helper", async () => {
    const { readFileSync } = await import("node:fs");
    const { resolve } = await import("node:path");
    // The code only: block comments, then line comments (not a `://` inside a URL).
    const read = (f: string) =>
      readFileSync(resolve(process.cwd(), f), "utf8")
        .replace(/\/\*[\s\S]*?\*\//g, "")
        .replace(/(^|[^:])\/\/.*$/gm, "$1");
    for (const screen of ["src/screens/JobDetailScreen.tsx", "src/screens/JobsScreen.tsx"]) {
      const src = read(screen);
      expect(src, screen).not.toMatch(/["'`]cancel_job["'`]/);
      expect(src, screen).toMatch(/\bsendCancel\(\{/);
    }
    const wiring = read("src/jobs/live-wiring.ts");
    expect(wiring.match(/["'`]cancel_job["'`]/g)).toHaveLength(1);
    const helper = wiring.slice(wiring.indexOf("export async function sendCancel("));
    const body = helper.slice(0, helper.indexOf("\n}\n"));
    expect(body).toMatch(/return confirmThenCancel\(\{/);
    expect(body.indexOf("confirmThenCancel(")).toBeLessThan(body.indexOf('"cancel_job"'));
  });
});

// --- The job view remounts per job (verifier M1) -----------------------------------------------------

describe("App's job view", () => {
  // A source pin (no App render harness): an in-place jobId change would keep the previous job's
  // lines, points, row and remote record. CONTROL: remove `key={screen.jobId}` → red.
  it("is keyed by the job id, so opening another job remounts it", async () => {
    const { readFileSync } = await import("node:fs");
    const { resolve } = await import("node:path");
    const src = readFileSync(resolve(process.cwd(), "src/App.tsx"), "utf8");
    expect(src).toMatch(/<JobDetailScreen\s+key=\{screen\.jobId\}/);
  });
});
