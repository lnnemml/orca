import { describe, expect, it } from "vitest";
import type { Job, RemoteState, RemoteStateEvent } from "../types";
import { makeProfile } from "../servers/test-fixtures";
import {
  NOTHING_HELD,
  applyRemoteStateEvent,
  coordinateState,
  mergeRemoteState,
  remoteView,
  runTargetOptions,
} from "./remote";

const COORDS = { remote_host: "uni", remote_job_dir: "/home/anton/.orcastudio/jobs/j1", remote_socket: "/tmp/ts-slot-0" };

function job(over: Partial<Job> = {}): Job {
  return {
    id: "j1",
    title: "remote job",
    input_content: "! HF",
    status: "queued",
    job_dir: null,
    energy: null,
    wall_time: null,
    error_message: null,
    created_at: "2026-10-06",
    started_at: null,
    completed_at: null,
    scene_json: null,
    scene_log_json: null,
    pathway_id: null,
    group_id: null,
    backend_id: "p1",
    ...COORDS,
    ...over,
  };
}

function record(over: Partial<RemoteState> = {}): RemoteState {
  return { known: null, check_error: null, fetch: { strikes: 0, stopped: false }, ...over };
}

describe("coordinateState (o19.8)", () => {
  it("is local with none, remote with all three, corrupt with some", () => {
    expect(coordinateState({ remote_host: null, remote_job_dir: null, remote_socket: null })).toBe("local");
    expect(coordinateState(COORDS)).toBe("remote");
    expect(coordinateState({ ...COORDS, remote_socket: null })).toBe("corrupt");
    expect(coordinateState({ remote_host: "uni", remote_job_dir: null, remote_socket: null })).toBe("corrupt");
  });
});

describe("the seq rule (o19.6)", () => {
  const A = record({ known: { kind: "shown", detail: { outcome: "queued" } } });
  const B = record({ known: { kind: "shown", detail: { outcome: "running" } } });
  const event = (seq: number, state: RemoteState | null, job_id = "j1"): RemoteStateEvent => ({ job_id, seq, state });

  // CONTROL: make `mergeRemoteState` always take the incoming value and this goes red (B wins).
  it("ignores an event with a lower seq delivered after the read", () => {
    let held = mergeRemoteState(NOTHING_HELD, { seq: 5, state: A });
    held = applyRemoteStateEvent(held, "j1", event(3, B));
    expect(held).toEqual({ seq: 5, state: A });
  });

  // CONTROL: same break — the older read (seq 5) overwrites the newer event, and this goes red.
  it("keeps an event emitted between the listener's resolve and the read's resolve when its seq is higher", () => {
    let held = applyRemoteStateEvent(NOTHING_HELD, "j1", event(6, B));
    held = mergeRemoteState(held, { seq: 5, state: A });
    expect(held).toEqual({ seq: 6, state: B });
  });

  it("applies a newer removal, and a first read of no record", () => {
    let held = mergeRemoteState(NOTHING_HELD, { seq: 0, state: null });
    expect(held).toEqual({ seq: 0, state: null });
    held = applyRemoteStateEvent(held, "j1", event(1, A));
    held = applyRemoteStateEvent(held, "j1", event(2, null));
    expect(held).toEqual({ seq: 2, state: null });
  });

  it("ignores another job's event", () => {
    const held = mergeRemoteState(NOTHING_HELD, { seq: 5, state: A });
    expect(applyRemoteStateEvent(held, "j1", event(9, B, "j2"))).toBe(held);
  });
});

describe("remoteView (o19.2, o19.7)", () => {
  it("shows nothing remote for a local job and the ordinary view for a finished remote one", () => {
    expect(remoteView(job({ remote_host: null, remote_job_dir: null, remote_socket: null }), undefined)).toEqual({ kind: "local" });
    expect(remoteView(job({ status: "completed" }), undefined)).toEqual({ kind: "terminal" });
  });

  it("derives corrupt coordinates from the row and offers nothing", () => {
    const v = remoteView(job({ remote_socket: null }), undefined);
    expect(v.kind).toBe("corrupt");
    if (v.kind === "corrupt") {
      expect(v.badges.map((b) => b.kind)).toEqual(["corrupt"]);
      expect(v.actions).toEqual([]);
    }
  });

  it("with no record says checking, labels the row's text from the last check, and offers no action from it", () => {
    const v = remoteView(job({ error_message: "Fetching the results failed 3 times in a row; automatic fetches stopped" }), undefined);
    expect(v).toEqual({
      kind: "live",
      text: { body: "Fetching the results failed 3 times in a row; automatic fetches stopped", fromLastCheck: true },
      badges: [{ kind: "checking", label: "Checking the server…", tone: "muted" }],
      actions: [],
    });
  });

  it("offers retry and withdraw for not on the server", () => {
    const v = remoteView(job(), record({ known: { kind: "labelled", detail: "not_on_server" } }));
    expect(v.kind === "live" && v.actions).toEqual(["retry_submit", "withdraw"]);
  });

  it("offers withdraw only for submit interrupted", () => {
    const v = remoteView(job(), record({ known: { kind: "labelled", detail: "submit_interrupted" } }));
    expect(v.kind === "live" && v.actions).toEqual(["withdraw"]);
  });

  it("shows a decided outcome with the row's text, not from the last check, and no action", () => {
    const v = remoteView(
      job({ status: "running", error_message: "row text" }),
      record({ known: { kind: "shown", detail: { outcome: "lost", orphans: [] } } }),
    );
    expect(v).toMatchObject({ kind: "live", text: { body: "row text", fromLastCheck: false }, actions: [] });
    if (v.kind === "live") expect(v.badges).toEqual([{ kind: "outcome", outcome: "lost", stale: false, label: "Server: lost", tone: "err" }]);
  });

  it("shows a check error beside the last seen state, which stays actionable", () => {
    const v = remoteView(job(), record({ known: { kind: "labelled", detail: "not_on_server" }, check_error: "ssh: timed out" }));
    if (v.kind !== "live") throw new Error(v.kind);
    expect(v.badges.map((b) => b.label)).toEqual(["Not on the server", "Server unreachable / not decided: ssh: timed out"]);
    expect(v.actions).toEqual(["retry_submit", "withdraw"]);

    const seen = remoteView(job({ status: "running" }), record({ known: { kind: "shown", detail: { outcome: "running" } }, check_error: "x" }));
    if (seen.kind !== "live") throw new Error(seen.kind);
    expect(seen.badges[0]).toMatchObject({ kind: "outcome", stale: true, label: "Last seen: running" });
  });

  it("with only a check error says the server is unreachable, and the row's text is from the last check", () => {
    const v = remoteView(job({ error_message: "old words" }), record({ check_error: "ssh: no route to host" }));
    expect(v).toEqual({
      kind: "live",
      text: { body: "old words", fromLastCheck: true },
      badges: [{ kind: "check_error", label: "Server unreachable / not decided: ssh: no route to host", tone: "warn" }],
      actions: [],
    });
  });

  // CONTROL: label the text from the last check whenever `fetch.strikes > 0` (or drop the
  // `fetching` kind from the `known === null` test) and this goes red.
  it("does not label a failing fetch's text from the last check, and shows it fetching (o19.11)", () => {
    const v = remoteView(
      job({ status: "running", error_message: "Fetching the results failed (attempt 2 of 3)" }),
      record({ known: { kind: "fetching" }, fetch: { strikes: 2, stopped: false } }),
    );
    expect(v).toEqual({
      kind: "live",
      text: { body: "Fetching the results failed (attempt 2 of 3)", fromLastCheck: false },
      badges: [{ kind: "fetching", strikes: 2, stale: false, label: "Fetching results (2 failed attempts)", tone: "warn" }],
      actions: [],
    });
    const stopped = remoteView(job({ status: "running" }), record({ known: { kind: "fetching" }, fetch: { strikes: 3, stopped: true } }));
    expect(stopped.kind === "live" && stopped.actions).toEqual(["retry_fetch"]);
  });

  it("offers a fetch retry exactly when automatic fetches stopped", () => {
    const stopped = remoteView(job({ status: "running" }), record({ fetch: { strikes: 3, stopped: true } }));
    expect(stopped.kind === "live" && stopped.actions).toEqual(["retry_fetch"]);
    const striking = remoteView(job({ status: "running" }), record({ fetch: { strikes: 2, stopped: false } }));
    expect(striking.kind === "live" && striking.actions).toEqual([]);
  });

  // CONTROL: paste `shown_message`'s Indeterminate sentence or a "handled in unit 5.4" into remote.ts → red.
  it("carries no copy of Rust's outcome texts or its later-unit list", async () => {
    const { readFileSync } = await import("node:fs");
    const { resolve } = await import("node:path");
    // vitest roots `process.cwd()` at the repo (as `render.corpus.test.ts` relies on).
    const src = readFileSync(resolve(process.cwd(), "src/jobs/remote.ts"), "utf8");
    expect(src).not.toMatch(/5\.4/);
    expect(src).not.toMatch(/could not be decided on this check/);
  });
});

describe("runTargetOptions (ADR-024 o5, n2)", () => {
  it("offers Local and every profile, enabled iff Rust says it is a run target, with Rust's reason", () => {
    const options = runTargetOptions([
      makeProfile({ id: "p1", name: "uni", run_target: { is_run_target: true, reason: null } }),
      makeProfile({ id: "p2", name: "lab", run_target: { is_run_target: false, reason: "the profile has no core mask" } }),
      makeProfile({ id: "p3", name: "odd", verified_at: "2026-10-01 10:00:00", run_target: { is_run_target: false, reason: null } }),
    ]);
    expect(options).toEqual([
      { target: null, label: "Local", enabled: true, reason: null },
      { target: "p1", label: "uni", enabled: true, reason: null },
      { target: "p2", label: "lab", enabled: false, reason: "the profile has no core mask" },
      { target: "p3", label: "odd", enabled: false, reason: "no reason reported" },
    ]);
  });
});
