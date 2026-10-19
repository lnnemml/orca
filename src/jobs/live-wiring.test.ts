import { describe, expect, it, vi } from "vitest";
import type { Job, RemoteState, RemoteStateEvent, RemoteStateRead } from "../types";

// The cancel helpers (`sendCancel`, `sendUndo`) use the real bridge and the plugin's own dialog, which a
// screen cannot replace (ADR-024 o22.14.14.1); the tests replace them at the module. Every other test
// in this file injects a fake bridge and is not affected.
const mocks = vi.hoisted(() => ({ invoke: vi.fn(), confirm: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: mocks.invoke }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ confirm: mocks.confirm }));
import { makeProfile } from "../servers/test-fixtures";
import { LAST_RUN_TARGET_SETTING, type CancelSend, type HeldRemoteState } from "./remote";
import {
  clickCancelFamily,
  confirmThenCancel,
  loadRunTarget,
  sendCancel,
  sendUndo,
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
    pending_submit: null,
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
    submit_pending_since: null,
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

/**
 * The backend, scripted: `invoke` (mocked at the module, since the helpers use `tauriBridge`) answers
 * each command from `answers` — a value, or a function giving the value or a rejected promise, called
 * per call so a test can answer the same command differently before and after the dialog. Every call
 * is logged, in order, in `log`.
 */
function backend(answers: Record<string, unknown>, log: string[] = []) {
  const calls: { cmd: string; args?: Record<string, unknown> }[] = [];
  mocks.invoke.mockReset();
  mocks.invoke.mockImplementation((cmd: string, args?: Record<string, unknown>) => {
    calls.push({ cmd, args });
    log.push(`invoke:${cmd}`);
    const answer = answers[cmd];
    if (answer === undefined) return Promise.reject(new Error(`unscripted ${cmd}`));
    return Promise.resolve(typeof answer === "function" ? (answer as () => unknown)() : answer);
  });
  return { calls, log };
}

/** The plugin's `confirm`, mocked at the module: answers `answer` (or throws) and records what it was asked. */
function dialog(answer: boolean | "fails", log: string[] = []) {
  const asked: { message: string; title: string }[] = [];
  mocks.confirm.mockReset();
  mocks.confirm.mockImplementation(async (message: string, options: { title: string }) => {
    asked.push({ message, title: options.title });
    log.push("dialog");
    if (answer === "fails") throw new Error("dialog failed");
    return answer;
  });
  return asked;
}

describe("sendCancel (o22.14.8, o22.14.10, o22.14.14.1; B1 verifier VX1, B2 verifier F1)", () => {
  const T0 = "2026-10-18 06:18:01";
  const T1 = "2026-10-18 06:18:05";
  const REMOTE_ROW = { ...REMOTE, id: "j1", title: "TS search" };
  const running = (over: Partial<Job> = {}) => job({ ...REMOTE_ROW, status: "running", ...over });
  const local = (over: Partial<Job> = {}) => job({ id: "j1", status: "running", ...over });
  const state = (r: RemoteState | null = RUNNING) => ({ seq: 1, state: r }) satisfies RemoteStateRead;
  const PENDING_ANSWER = { result: "pending", already_pending: false };
  const quiet = () => {};

  // F1 (B2 verifier) and VX1 — CONTROLS: take a forged field again (`deps.job`, `deps.record`,
  // `deps.confirm`, `deps.offer`) → the dialog is skipped or replaced, red here, and the
  // `@ts-expect-error` of that line is unused → `tsc` red. Each forged field is its own call, since
  // `tsc` reports only the first excess property of a literal. The screens cannot even express them.
  it("cannot be handed a job, a record, an offer, a dialog or a bridge: the backend's row decides, with the plugin's dialog", async () => {
    const forgedLocal = { ...REMOTE_ROW, remote_host: null, remote_job_dir: null, remote_socket: null }; // VX-new1
    const forgedCancelling = { ...REMOTE_ROW, status: "cancelling" as const }; // VX-new3: Retry cancel, no dialog
    const alwaysYes = async () => true; // VX-new2
    const fakeBridge = { invoke: async () => ({ result: "pending", already_pending: false }) };
    const attempts: (() => Promise<unknown>)[] = [
      () =>
        sendCancel({
          id: "j1",
          sending: quiet,
          // @ts-expect-error — no `job`: the helper reads the row itself (VX-new1)
          job: forgedLocal,
        }),
      () =>
        sendCancel({
          id: "j1",
          sending: quiet,
          // @ts-expect-error — no `job` (VX-new3)
          job: forgedCancelling,
        }),
      () =>
        sendCancel({
          id: "j1",
          sending: quiet,
          // @ts-expect-error — no `confirm`: the dialog is the plugin's, imported in live-wiring (VX-new2)
          confirm: alwaysYes,
        }),
      () =>
        sendCancel({
          id: "j1",
          sending: quiet,
          // @ts-expect-error — no `record`
          record: record({ cancel_sends: { failures: 3, stopped: true } }),
        }),
      () =>
        sendCancel({
          id: "j1",
          sending: quiet,
          // @ts-expect-error — no `offer` (VX1)
          offer: { action: "cancel", confirm: false },
        }),
      () =>
        sendCancel({
          id: "j1",
          sending: quiet,
          // @ts-expect-error — no `bridge`
          bridge: fakeBridge,
        }),
    ];
    for (const attempt of attempts) {
      const { calls } = backend({ get_job: running(), remote_job_state: state(), cancel_job: PENDING_ANSWER });
      const asked = dialog(false);
      expect(await attempt()).toBeNull();
      // The real dialog was asked once (the forged ones were ignored), and nothing was cancelled.
      expect(asked).toHaveLength(1);
      expect(calls.map((c) => c.cmd)).not.toContain("cancel_job");
    }
  });

  // CONTROL: send without `confirmThenCancel`, or take the offer as `{confirm: false}` → the first
  // remote Cancel sends with no dialog, red.
  it("asks with o22.14.2's title and text for a remote job's first Cancel, reading the row first, and sends nothing when declined", async () => {
    const log: string[] = [];
    const { calls } = backend({ cancel_job: PENDING_ANSWER, get_job: running(), remote_job_state: state() }, log);
    const asked = dialog(false, log);
    const sending: boolean[] = [];
    const result = await sendCancel({ id: "j1", sending: (on) => sending.push(on) });
    expect(result).toBeNull();
    expect(calls.map((c) => c.cmd).sort()).toEqual(["get_job", "remote_job_state"]);
    expect(log.indexOf("dialog")).toBe(2);
    // In flight from the click (before the first read, o22.14.14.6), and ended when declined.
    expect(sending).toEqual([true, false]);
    expect(asked).toEqual([
      {
        title: "Cancel job",
        message:
          'Cancel "TS search" on uni? The server stops it; files already written stay on the server and are downloaded ' +
          "for viewing. You can undo until the cancel is sent.",
      },
    ]);
  });

  it("sends exactly one cancel_job after the user agreed, having read the row and the record again after the dialog", async () => {
    const log: string[] = [];
    backend(
      { cancel_job: PENDING_ANSWER, get_job: running({ cancel_requested_at: T0 }), remote_job_state: state() },
      log,
    );
    // Before the dialog the backend's row offers Cancel; after it, still does (no cancel pending yet).
    let reads = 0;
    mocks.invoke.mockImplementation((cmd: string) => {
      log.push(`invoke:${cmd}`);
      if (cmd === "get_job") return Promise.resolve(running(++reads <= 2 ? {} : { cancel_requested_at: T0 }));
      if (cmd === "remote_job_state") return Promise.resolve(state());
      return Promise.resolve(PENDING_ANSWER);
    });
    dialog(true, log);
    const report = await sendCancel({ id: "j1", sending: quiet });
    expect(log.filter((l) => l === "invoke:cancel_job")).toHaveLength(1);
    // reads (get_job ‖ remote_job_state, any order), dialog, reads again, cancel_job, reload.
    const cmds = log.map((l) => l.replace("invoke:", ""));
    expect(cmds[2]).toBe("dialog");
    expect(cmds.slice(0, 2).sort()).toEqual(["get_job", "remote_job_state"]);
    expect(cmds.slice(3, 5).sort()).toEqual(["get_job", "remote_job_state"]);
    expect(cmds[5]).toBe("cancel_job");
    expect(cmds.slice(6).sort()).toEqual(["get_job", "remote_job_state"]);
    expect(report?.notice).toBe("Cancel pending — sent at the next check");
    expect(report?.job?.cancel_requested_at).toBe(T0);
  });

  // CONTROL: skip the re-read after the dialog (derive the offer once) → a cancel from a stale offer
  // is sent, red.
  it("sends nothing when the backend's row stopped offering the Cancel while the dialog was open", async () => {
    let reads = 0;
    const { calls } = backend({
      cancel_job: PENDING_ANSWER,
      // Another cancel was recorded meanwhile: only Undo is offered after the dialog.
      get_job: () => running(++reads <= 1 ? {} : { cancel_requested_at: T0 }),
      remote_job_state: state(),
    });
    dialog(true);
    expect(await sendCancel({ id: "j1", sending: quiet })).toBeNull();
    expect(calls.map((c) => c.cmd)).not.toContain("cancel_job");
  });

  // CONTROL: ask whatever the offer says (always `confirm`) → Retry cancel opens the dialog, red.
  it("sends Retry cancel at once, with no dialog (a cancelling job)", async () => {
    const { calls } = backend({
      cancel_job: { result: "already_on_server" },
      get_job: running({ status: "cancelling" }),
      remote_job_state: state(),
    });
    const asked = dialog(false);
    const report = await sendCancel({ id: "j1", sending: quiet });
    expect(asked).toHaveLength(0);
    expect(calls.find((c) => c.cmd === "cancel_job")).toEqual({ cmd: "cancel_job", args: { id: "j1" } });
    expect(report?.notice).toBe("The cancel is already on the server; sweeping again");
  });

  it("sends Retry cancel at once for a pending cancel whose re-sends stopped", async () => {
    const stopped = record({ cancel_sends: { failures: 3, stopped: true } });
    const { calls } = backend({
      cancel_job: PENDING_ANSWER,
      get_job: running({ cancel_requested_at: T0, cancel_sent_at: T1 }),
      remote_job_state: state(stopped),
    });
    const asked = dialog(false);
    await sendCancel({ id: "j1", sending: quiet });
    expect(asked).toHaveLength(0);
    expect(calls.some((c) => c.cmd === "cancel_job")).toBe(true);
  });

  it("sends nothing when the row offers no cancel send, and when it cannot be read", async () => {
    for (const j of [
      running({ cancel_requested_at: T0, cancel_sent_at: T1 }), // sent, re-sends not stopped: nothing
      running({ cancel_requested_at: T0 }), // pending, unsent: only Undo
      running({ status: "completed" }),
      running({ remote_socket: null }), // corrupt coordinates
    ]) {
      const { calls } = backend({ cancel_job: PENDING_ANSWER, get_job: j, remote_job_state: state() });
      const asked = dialog(true);
      expect(await sendCancel({ id: "j1", sending: quiet })).toBeNull();
      expect(calls.map((c) => c.cmd)).not.toContain("cancel_job");
      expect(asked).toHaveLength(0);
    }
    // No row, no offer, no cancel: the read's error is the screen's.
    const { calls } = backend({ cancel_job: PENDING_ANSWER, get_job: () => Promise.reject(new Error("no such job")), remote_job_state: state() });
    dialog(true);
    await expect(sendCancel({ id: "j1", sending: quiet })).rejects.toThrow("no such job");
    expect(calls.map((c) => c.cmd)).not.toContain("cancel_job");
  });

  it("counts a dialog that fails to open as a decline", async () => {
    const { calls } = backend({ cancel_job: PENDING_ANSWER, get_job: running(), remote_job_state: state() });
    dialog("fails");
    expect(await sendCancel({ id: "j1", sending: quiet })).toBeNull();
    expect(calls.map((c) => c.cmd)).not.toContain("cancel_job");
  });

  // CONTROL: drop the `finally` reset (or reset only for a local job) → the remote state stays
  // "Sending cancel…" after `cancel_job` returned, red. Reset before the reload → red too (the
  // order is asserted).
  it("ends a remote job's in-flight state after cancel_job returned and the reload, and keeps a local one's", async () => {
    const log: string[] = [];
    backend({ cancel_job: PENDING_ANSWER, get_job: running(), remote_job_state: state() }, log);
    dialog(true);
    const r = await sendCancel({ id: "j1", sending: (on) => log.push(`sending:${on}`) });
    expect(r?.result).toEqual(PENDING_ANSWER);
    // In flight from before the first read; ended only after cancel_job and the reload (2 reads).
    expect(log[0]).toBe("sending:true");
    expect(log.filter((l) => l.startsWith("sending"))).toEqual(["sending:true", "sending:false"]);
    const at = log.indexOf("invoke:cancel_job");
    expect(log.slice(at + 1, at + 3).sort()).toEqual(["invoke:get_job", "invoke:remote_job_state"]);
    expect(log[log.length - 1]).toBe("sending:false");
    expect(log.length).toBe(at + 4);

    const localLog: string[] = [];
    backend({ cancel_job: { result: "dispatched" }, get_job: local(), remote_job_state: state(null) }, localLog);
    dialog(false);
    const l = await sendCancel({ id: "j1", sending: (on) => localLog.push(`sending:${on}`) });
    // A local cancel: no dialog, no reload, no notice; the in-flight state stays until the terminal status.
    expect(l).toEqual({ result: { result: "dispatched" }, job: null, notice: null });
    expect(localLog.filter((x) => x.startsWith("sending"))).toEqual(["sending:true"]);
    expect(localLog[0]).toBe("sending:true");
    expect(localLog[localLog.length - 1]).toBe("invoke:cancel_job");
  });

  it("ends the in-flight state when cancel_job fails, for both, and rethrows", async () => {
    for (const j of [running(), local()]) {
      backend({
        get_job: j,
        remote_job_state: state(),
        cancel_job: () => Promise.reject(new Error("conflict: an operation on job j1 is already in progress")),
      });
      dialog(true);
      const sending: boolean[] = [];
      await expect(sendCancel({ id: "j1", sending: (on) => sending.push(on) })).rejects.toThrow("already in progress");
      expect(sending).toEqual([true, false]);
    }
  });

  // G1 — CONTROL: catch the re-read's failure in `currentActions` and fall back to the first read's
  // offers → the cancel is sent from a stale offer, red.
  it("rejects and sends nothing when the read after the dialog fails", async () => {
    const log: string[] = [];
    let afterDialog = false;
    const { calls } = backend(
      {
        cancel_job: PENDING_ANSWER,
        get_job: () => (afterDialog ? Promise.reject(new Error("db busy after the dialog")) : running()),
        remote_job_state: state(),
      },
      log,
    );
    mocks.confirm.mockReset();
    mocks.confirm.mockImplementation(async () => {
      afterDialog = true;
      return true;
    });
    const sending: boolean[] = [];
    await expect(sendCancel({ id: "j1", sending: (on) => sending.push(on) })).rejects.toThrow("db busy after the dialog");
    expect(calls.map((c) => c.cmd)).not.toContain("cancel_job");
    expect(sending).toEqual([true, false]);
  });

  // G3 — CONTROLS: move `sending(true)` back after the reads (into `send`) → the first click is not
  // in flight while its reads and its dialog are pending, a second click goes through, red.
  it("is in flight from the click: a second click during the reads or the dialog sends nothing", async () => {
    // The button as the screen draws it: disabled while a click is in flight (`cancelButtons`).
    let inFlight = false;
    const click = () => (inFlight ? Promise.resolve("disabled") : sendCancel({ id: "j1", sending: (on) => (inFlight = on) }));
    const holds: (() => void)[] = []; // each `get_job` is held until released
    const { calls } = backend({
      cancel_job: PENDING_ANSWER,
      get_job: () => new Promise((resolve) => holds.push(() => resolve(running()))),
      remote_job_state: state(),
    });
    dialog(true);
    const first = click();
    // The first click's reads are pending: it is already in flight.
    expect(inFlight).toBe(true);
    expect(await click()).toBe("disabled");
    // Release each read as it arrives: the first, the re-read after the dialog, the reload.
    for (let i = 0; i < 3; i++) {
      while (holds.length <= i) await new Promise((r) => setTimeout(r, 0));
      holds[i]();
    }
    await first;
    expect(calls.filter((c) => c.cmd === "cancel_job")).toHaveLength(1);
    expect(inFlight).toBe(false);
  });

  it("ends the in-flight state when nothing is offered, the dialog is declined or fails, and a read fails", async () => {
    const cases: [string, Record<string, unknown>, boolean | "fails"][] = [
      ["nothing offered", { get_job: running({ cancel_requested_at: T0, cancel_sent_at: T1 }), remote_job_state: state() }, true],
      ["declined", { get_job: running(), remote_job_state: state() }, false],
      ["dialog fails", { get_job: running(), remote_job_state: state() }, "fails"],
      ["first read fails", { get_job: () => Promise.reject(new Error("x")), remote_job_state: state() }, true],
    ];
    for (const [name, answers, answer] of cases) {
      backend({ cancel_job: PENDING_ANSWER, ...answers });
      dialog(answer);
      const sending: boolean[] = [];
      await sendCancel({ id: "j1", sending: (on) => sending.push(on) }).catch(() => null);
      expect(sending, name).toEqual([true, false]);
    }
  });

  // CONTROL: `cancelNotice` returning "Cancel sent" for `check_failed` → red here and in remote.test.
  it("never says the cancel was sent when the delivery's check failed", async () => {
    const failed = { result: "delivered", already_pending: false, step: { step: "check_failed", detail: "collect: ssh exited with 255" } };
    const unreachable = record({ check_error: { text: "collect: ssh exited with 255", answered: false } });
    const run = async (after: Job) => {
      let reads = 0;
      backend({
        cancel_job: failed,
        // The first two reads (before/after the dialog) see the running row; the reload sees `after`.
        get_job: () => (++reads <= 2 ? running() : after),
        remote_job_state: () => state(reads <= 2 ? RUNNING : unreachable),
      });
      dialog(true);
      return sendCancel({ id: "j1", sending: quiet });
    };
    const unsent = await run(running({ cancel_requested_at: T0 }));
    expect(unsent?.notice).toBe("Server unreachable — the cancel stays pending and is sent when it answers");
    const marked = await run(running({ cancel_requested_at: T0, cancel_sent_at: T1 }));
    expect(marked?.notice).toMatch(/^No answer after the cancel was sent; it may already be on the server/);
    for (const r of [unsent, marked]) expect(r?.notice).not.toMatch(/^Cancel sent/);
  });

  it("words the answer neutrally when the reload cannot be read, and does not throw", async () => {
    const failed = { result: "delivered", already_pending: false, step: { step: "check_failed", detail: "collect: broken" } };
    let reads = 0;
    backend({
      cancel_job: failed,
      get_job: () => (++reads <= 2 ? running() : Promise.reject(new Error("db busy"))),
      remote_job_state: () => (reads <= 2 ? state() : Promise.reject(new Error("no"))),
    });
    dialog(true);
    const r = await sendCancel({ id: "j1", sending: quiet });
    expect(r).toEqual({ result: failed, job: null, notice: "The cancel stays pending: collect: broken" });
  });
});

describe("sendUndo and clickCancelFamily (o22.14.1, o22.14.14.1)", () => {
  const T0 = "2026-10-18 06:18:01";
  const T1 = "2026-10-18 06:18:05";
  const pendingRow = (over: Partial<Job> = {}) => job({ ...REMOTE, status: "running", cancel_requested_at: T0, ...over });
  const state = (r: RemoteState | null = RUNNING) => ({ seq: 2, state: r }) satisfies RemoteStateRead;

  // CONTROL: wire Undo to `cancel_job` (the shared helper invoking the wrong command) → red.
  it("undo_cancel calls undo_cancel — never cancel_job — with no dialog, reloads and words the answer", async () => {
    const log: string[] = [];
    let undone = false;
    const { calls } = backend(
      {
        undo_cancel: () => ((undone = true), "undone"),
        get_job: () => pendingRow(undone ? { cancel_requested_at: null } : {}),
        remote_job_state: state(),
      },
      log,
    );
    const asked = dialog(true);
    const out = await clickCancelFamily("undo_cancel", { id: "j1", sending: (busy) => log.push(`sending:${busy}`) });
    expect(asked).toEqual([]);
    expect(calls.find((c) => c.cmd === "undo_cancel")).toEqual({ cmd: "undo_cancel", args: { id: "j1" } });
    expect(calls.map((c) => c.cmd)).not.toContain("cancel_job");
    expect(out?.notice).toBe("Cancel undone");
    expect(out?.job?.cancel_requested_at).toBeNull();
    expect(log[0]).toBe("sending:undo");
    expect(log.indexOf("invoke:undo_cancel")).toBeGreaterThan(2);
    expect(log[log.length - 1]).toBe("sending:null");
  });

  it("says too late when the send was marked in between, and nothing when none is pending", async () => {
    for (const [answer, notice] of [
      ["send_marked", "Too late to undo: the cancel may already be on the server"],
      ["nothing_pending", "No cancel to undo"],
    ] as const) {
      backend({ undo_cancel: answer, get_job: pendingRow(), remote_job_state: state() });
      const r = await sendUndo({ id: "j1", sending: () => {} });
      expect(r?.notice).toBe(notice);
    }
  });

  it("sends no undo when the backend's row does not offer it (the send is marked, or no cancel is pending)", async () => {
    for (const j of [pendingRow({ cancel_sent_at: T1 }), pendingRow({ cancel_requested_at: null })]) {
      const { calls } = backend({ undo_cancel: "undone", get_job: j, remote_job_state: state() });
      expect(await sendUndo({ id: "j1", sending: () => {} })).toBeNull();
      expect(calls.map((c) => c.cmd)).not.toContain("undo_cancel");
    }
  });

  it("routes Cancel and Retry cancel to cancel_job and shows them as the cancel in flight", async () => {
    const log: string[] = [];
    const stopped = record({ cancel_sends: { failures: 3, stopped: true } });
    const { calls } = backend(
      { cancel_job: { result: "pending", already_pending: true }, get_job: pendingRow({ cancel_sent_at: T1 }), remote_job_state: state(stopped) },
      log,
    );
    const asked = dialog(false);
    const out = await clickCancelFamily("retry_cancel", { id: "j1", sending: (busy) => log.push(`sending:${busy}`) });
    expect(asked).toEqual([]);
    expect(calls.some((c) => c.cmd === "cancel_job")).toBe(true);
    expect(out?.notice).toBe("Cancel pending — sent at the next check");
    expect(log.find((l) => l.startsWith("sending"))).toBe("sending:cancel");
    expect(log[log.length - 1]).toBe("sending:null");
  });

  // A Retry cancel click whose row has since become a plain running one is not a first Cancel the
  // user agreed to: nothing is sent (the backend offers "Cancel" with a dialog, not "Retry cancel").
  it("sends Retry cancel's click as the backend's offer says: a first Cancel asks, it is not sent blind", async () => {
    const { calls } = backend({ cancel_job: PENDING_ANSWER_FOR_RETRY, get_job: job({ ...REMOTE, status: "running" }), remote_job_state: state() });
    const asked = dialog(false);
    expect(await clickCancelFamily("retry_cancel", { id: "j1", sending: () => {} })).toBeNull();
    expect(asked).toHaveLength(1);
    expect(calls.map((c) => c.cmd)).not.toContain("cancel_job");
  });
});
const PENDING_ANSWER_FOR_RETRY = { result: "pending", already_pending: false };

describe("every Cancel goes through the shared helpers (o22.14.10, o22.14.14.1; source pin)", () => {
  // The dialog itself is a live-gate item; what is pinned here is that no screen can reach
  // `cancel_job` / `undo_cancel`, hand the helpers anything but the job id and the in-flight
  // callback, or import the dialog for the cancel path. CONTROLS: call `invoke("cancel_job", …)` or
  // `invoke("undo_cancel", …)` from a screen; add `confirm` / `job` / `record` to the call; import
  // `confirm` in the detail screen; a second `cancel_job` or `confirm` import in `live-wiring.ts`;
  // widen a helper's parameters → red. (The structural half is the `@ts-expect-error` lines above.)
  it("leaves cancel_job and undo_cancel to the helpers, and lets no screen hand them anything but the id and the callback", async () => {
    const { readFileSync } = await import("node:fs");
    const { resolve } = await import("node:path");
    // The code only: block comments, then line comments (not a `://` inside a URL).
    const read = (f: string) =>
      readFileSync(resolve(process.cwd(), f), "utf8")
        .replace(/\/\*[\s\S]*?\*\//g, "")
        .replace(/(^|[^:])\/\/.*$/gm, "$1");
    for (const screen of [
      "src/screens/JobDetailScreen.tsx",
      "src/screens/JobsScreen.tsx",
      "src/jobs/RemotePanel.tsx",
      "src/jobs/CancelButtons.tsx",
    ]) {
      const src = read(screen);
      expect(src, screen).not.toMatch(/["'`](cancel_job|undo_cancel|retry_cancel)["'`]/);
      expect(src, screen).not.toMatch(/\bconfirm\s*:|\boffer\s*[:=]|\bCancelSend\b|\bcancelSendOf\b|\bsendCancel\b|\bsendUndo\b/);
    }
    for (const screen of ["src/screens/JobDetailScreen.tsx", "src/screens/JobsScreen.tsx"]) {
      const src = read(screen);
      const start = src.indexOf("clickCancelFamily(action, {");
      expect(start, screen).toBeGreaterThan(-1);
      // The call's argument object, by brace matching; its top-level keys are the arguments.
      let depth = 0;
      let end = start;
      const keys: string[] = [];
      let segment = "";
      for (let i = src.indexOf("{", start); i < src.length; i++) {
        const c = src[i];
        if (c === "{" || c === "(" || c === "[") depth++;
        else if (c === "}" || c === ")" || c === "]") depth--;
        if (depth === 0) {
          end = i;
          keys.push(segment.split(":")[0].trim());
          break;
        }
        if (depth === 1 && c === ",") {
          keys.push(segment.split(":")[0].trim());
          segment = "";
        } else if (!(depth === 1 && c === "{")) segment += c;
      }
      const call = src.slice(start, end + 1);
      // Only the id and the in-flight callback.
      expect(keys.filter(Boolean), screen).toEqual(["id", "sending"]);
      expect(call, screen).not.toMatch(/\b(confirm|bridge|record|current|offer)\b/);
    }
    // The detail screen has no dialog import for the cancel path (it uses `message` for export only);
    // the Jobs screen's `confirm` is the delete's — every use is a call, none is passed along.
    const detail = read("src/screens/JobDetailScreen.tsx");
    expect(detail.match(/import \{[^}]*\} from "@tauri-apps\/plugin-dialog"/)?.[0]).not.toMatch(/\bconfirm\b/);
    expect(detail).not.toMatch(/\bconfirm\b/);
    const jobs = read("src/screens/JobsScreen.tsx").replace(/import \{[^}]*\} from "@tauri-apps\/plugin-dialog";/, "");
    expect([...jobs.matchAll(/\bconfirm\b(?!\()/g)]).toEqual([]);
    expect(jobs.match(/\bconfirm\(/g)).toHaveLength(1);

    // G2: the one-helper pin covers every non-test file of `src`, not a list: no other file names the
    // `cancel_job` command, or calls anything with `undo_cancel` as its first argument (an aliased
    // `invoke` included; the bare "undo_cancel" is also the action's name in `remote.ts`, whose
    // `offer("undo_cancel", …)` is the one allowed call).
    const { readdirSync } = await import("node:fs");
    const walk = (dir: string, out: string[] = []): string[] => {
      for (const e of readdirSync(dir, { withFileTypes: true })) {
        const path = `${dir}/${e.name}`;
        if (e.isDirectory()) walk(path, out);
        else if (/\.tsx?$/.test(e.name) && !/\.test\.tsx?$/.test(e.name)) out.push(path);
      }
      return out;
    };
    const others = walk("src").filter((f) => f !== "src/jobs/live-wiring.ts");
    expect(others.length).toBeGreaterThan(50);
    const offenders = others.filter((f) => {
      const code = read(f);
      return /["'`]cancel_job["'`]/.test(code) || /\b(?!offer\b)\w+\s*(<[^>]*>)?\(\s*["'`]undo_cancel["'`]/.test(code);
    });
    expect(offenders).toEqual([]);

    const wiring = read("src/jobs/live-wiring.ts");
    expect(wiring).toMatch(/import \{ confirm \} from "@tauri-apps\/plugin-dialog";/);
    expect(wiring.match(/["'`]cancel_job["'`]/g)).toHaveLength(1);
    expect(wiring.match(/invoke<[^>]*>\("undo_cancel"/g)).toHaveLength(1);
    // The helpers take the id and the callback, nothing else; the offer is derived from what they read.
    expect(wiring).toMatch(/export async function sendCancel\(deps: \{ id: string; sending: \(on: boolean\) => void \}\)/);
    expect(wiring).toMatch(/export async function sendUndo\(deps: \{ id: string; sending: \(on: boolean\) => void \}\)/);
    expect(wiring).toMatch(/deps: \{ id: string; sending: \(busy: CancelBusy\) => void \},\n\)/);
    const helper = wiring.slice(wiring.indexOf("export async function sendCancel("));
    const body = helper.slice(0, helper.indexOf("\n}\n"));
    expect(body).toMatch(/cancelSendOf\(rowActions\(first\.job, first\.record\)\)/);
    expect(body).toMatch(/const now = await readJobAndRecord\(id\);/);
    expect(body).toMatch(/return await confirmThenCancel\(\{/);
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
