import { describe, expect, it } from "vitest";
import type { CancelResult, CheckError, Job, RemoteState, RemoteStateEvent, StatusStep } from "../types";
import { makeProfile } from "../servers/test-fixtures";
import {
  NOTHING_HELD,
  RETRY_UNREACHABLE,
  applyRemoteStateEvent,
  backfillsLog,
  bannerAfterClick,
  cancelButtons,
  cancelNotice,
  cancelSendOf,
  coordinateState,
  mergeRemoteState,
  remoteView,
  rowActions,
  undoNotice,
  resolveRunTarget,
  LAST_RUN_TARGET_SETTING,
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
    cancel_requested_at: null,
    cancel_sent_at: null,
    submit_pending_since: null,
    ...over,
  };
}

/** A remote job's first Cancel, as every live `queued`/`running` row without a pending cancel offers. */
const CANCEL = { action: "cancel", confirm: true, disabled: null } as const;

/** The offered actions' names. */
function names(v: ReturnType<typeof remoteView>): string[] {
  return v.kind === "live" || v.kind === "corrupt" ? v.actions.map((a) => a.action) : [];
}

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

  it("with no record says checking, labels the row's text from the last check, and offers only Cancel", () => {
    const v = remoteView(job({ error_message: "Fetching the results failed 3 times in a row; automatic fetches stopped" }), undefined);
    expect(v).toEqual({
      kind: "live",
      text: { body: "Fetching the results failed 3 times in a row; automatic fetches stopped", fromLastCheck: true },
      badges: [{ kind: "checking", label: "Checking the server…", tone: "muted" }],
      actions: [CANCEL],
    });
  });

  it("offers retry and Cancel for not on the server (o22.7: Cancel replaces withdraw)", () => {
    const v = remoteView(job(), record({ known: { kind: "labelled", detail: "not_on_server" } }));
    expect(names(v)).toEqual(["retry_submit", "cancel"]);
  });

  it("offers Cancel only for submit interrupted", () => {
    const v = remoteView(job(), record({ known: { kind: "labelled", detail: "submit_interrupted" } }));
    expect(names(v)).toEqual(["cancel"]);
  });

  it("shows a decided outcome with the row's text, not from the last check, and only Cancel", () => {
    const v = remoteView(
      job({ status: "running", error_message: "row text" }),
      record({ known: { kind: "shown", detail: { outcome: "lost", orphans: [] } } }),
    );
    expect(v).toMatchObject({ kind: "live", text: { body: "row text", fromLastCheck: false }, actions: [CANCEL] });
    if (v.kind === "live") expect(v.badges).toEqual([{ kind: "outcome", outcome: "lost", stale: false, label: "Server: lost", tone: "err" }]);
  });

  it("shows a check error beside the last seen state, which stays actionable", () => {
    const v = remoteView(
      job(),
      record({ known: { kind: "labelled", detail: "not_on_server" }, check_error: { text: "ssh: timed out", answered: false } }),
    );
    if (v.kind !== "live") throw new Error(v.kind);
    expect(v.badges.map((b) => b.label)).toEqual(["Not on the server", "Server unreachable: ssh: timed out"]);
    expect(names(v)).toEqual(["retry_submit", "cancel"]);

    const seen = remoteView(
      job({ status: "running" }),
      record({ known: { kind: "shown", detail: { outcome: "running" } }, check_error: { text: "x", answered: true } }),
    );
    if (seen.kind !== "live") throw new Error(seen.kind);
    expect(seen.badges[0]).toMatchObject({ kind: "outcome", stale: true, label: "Last seen: running" });
  });

  it("with only a check error says the server is unreachable, and the row's text is from the last check", () => {
    const v = remoteView(job({ error_message: "old words" }), record({ check_error: { text: "ssh: no route to host", answered: false } }));
    expect(v).toEqual({
      kind: "live",
      text: { body: "old words", fromLastCheck: true },
      badges: [{ kind: "check_error", answered: false, label: "Server unreachable: ssh: no route to host", tone: "warn" }],
      actions: [CANCEL],
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
      actions: [CANCEL],
    });
    const stopped = remoteView(job({ status: "running" }), record({ known: { kind: "fetching" }, fetch: { strikes: 3, stopped: true } }));
    expect(names(stopped)).toEqual(["retry_fetch", "cancel"]);
  });

  // CONTROL (N6): print the count at 0 strikes ("(0 failed attempts)") → red.
  it("names no failed attempts while the strikes are 0, one in the singular, more in the plural", () => {
    const label = (strikes: number) => {
      const v = remoteView(job({ status: "running" }), record({ known: { kind: "fetching" }, fetch: { strikes, stopped: false } }));
      return v.kind === "live" ? v.badges.map((b) => b.label) : [];
    };
    expect(label(0)).toEqual(["Fetching results"]);
    expect(label(1)).toEqual(["Fetching results (1 failed attempt)"]);
    expect(label(2)).toEqual(["Fetching results (2 failed attempts)"]);
  });

  it("offers a fetch retry exactly when automatic fetches stopped", () => {
    const stopped = remoteView(job({ status: "running" }), record({ fetch: { strikes: 3, stopped: true } }));
    expect(names(stopped)).toEqual(["retry_fetch", "cancel"]);
    const striking = remoteView(job({ status: "running" }), record({ fetch: { strikes: 2, stopped: false } }));
    expect(names(striking)).toEqual(["cancel"]);
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

describe("remoteView's cancel offers (o22.14.1–2)", () => {
  const REQUESTED = "2026-10-18 06:18:01";
  const pending = (sent: boolean) => job({ status: "running", cancel_requested_at: REQUESTED, cancel_sent_at: sent ? "2026-10-18 06:18:05" : null });
  const stopped = (yes: boolean) => record({ cancel_sends: { failures: yes ? 3 : 1, stopped: yes } });
  const offers = (v: ReturnType<typeof remoteView>) => (v.kind === "live" ? v.actions : []);

  // CONTROLS (H1): Undo without Retry cancel for a stopped re-send with `sent_at` NULL; nothing for
  // a stopped one with `sent_at` set; Undo offered once `sent_at` is set → red.
  it("offers 14.1's table for a pending cancel", () => {
    expect(names(remoteView(pending(false), stopped(false)))).toEqual(["undo_cancel"]);
    expect(names(remoteView(pending(false), stopped(true)))).toEqual(["undo_cancel", "retry_cancel"]);
    expect(names(remoteView(pending(true), stopped(false)))).toEqual([]);
    expect(names(remoteView(pending(true), stopped(true)))).toEqual(["retry_cancel"]);
    // No record yet: nothing says the re-sends stopped.
    expect(names(remoteView(pending(false), undefined))).toEqual(["undo_cancel"]);
  });

  // CONTROLS: `retry_cancel` or `undo_cancel` with `confirm: true`, or a remote first `cancel` with
  // `confirm: false` → red.
  it("asks only on a remote job's first Cancel", () => {
    expect(offers(remoteView(job({ status: "queued" }), record()))).toEqual([{ action: "cancel", confirm: true, disabled: null }]);
    expect(offers(remoteView(job({ status: "running" }), record()))).toEqual([{ action: "cancel", confirm: true, disabled: null }]);
    for (const o of [...offers(remoteView(pending(false), stopped(true))), ...offers(remoteView(job({ status: "cancelling" }), record()))]) {
      expect(o.confirm, o.action).toBe(false);
    }
  });

  // CONTROL: treat `cancelling` as terminal (the old `queued`/`running` test) → red.
  it("keeps a cancelling job live, with Retry cancel and no confirm (o22.14.2, o22.14.8)", () => {
    const v = remoteView(job({ status: "cancelling", error_message: "processes still run" }), record({ known: { kind: "shown", detail: { outcome: "cancelling", session: 2 } } }));
    expect(v.kind).toBe("live");
    expect(offers(v)).toEqual([{ action: "retry_cancel", confirm: false, disabled: null }]);
    if (v.kind === "live") expect(v.text).toEqual({ body: "processes still run", fromLastCheck: false });
  });
});

describe("remoteView's retries (o22.8, o22.14.4)", () => {
  const notOnServer = (over: Partial<RemoteState> = {}) => record({ known: { kind: "labelled", detail: "not_on_server" }, ...over });
  const fetchStopped = (over: Partial<RemoteState> = {}) => record({ fetch: { strikes: 3, stopped: true }, ...over });

  // CONTROL: offer a retry while a cancel is pending → red.
  it("hides both retries while a cancel is pending", () => {
    const p = job({ status: "queued", cancel_requested_at: "2026-10-18 06:18:01" });
    expect(names(remoteView(p, notOnServer()))).toEqual(["undo_cancel"]);
    expect(names(remoteView({ ...p, status: "running" }, fetchStopped()))).toEqual(["undo_cancel"]);
  });

  // CONTROL: offer the retries enabled while `last_contact` is `Failed` → red.
  it("disables both retries with the reason while the last contact failed, and enables them once it answers", () => {
    const failed = { last_contact: { kind: "failed", detail: "ssh exited with 255" } } as const;
    const away = remoteView(job(), notOnServer(failed));
    expect(away.kind === "live" && away.actions[0]).toEqual({ action: "retry_submit", confirm: false, disabled: RETRY_UNREACHABLE });
    const fetchAway = remoteView(job({ status: "running" }), fetchStopped(failed));
    expect(fetchAway.kind === "live" && fetchAway.actions[0]).toEqual({ action: "retry_fetch", confirm: false, disabled: RETRY_UNREACHABLE });
    const back = remoteView(job(), notOnServer({ last_contact: { kind: "ok" } }));
    expect(back.kind === "live" && back.actions[0]).toEqual({ action: "retry_submit", confirm: false, disabled: null });
    // Cancel stays enabled while unreachable: it is the action that is useful then.
    expect(away.kind === "live" && away.actions[1]).toEqual({ action: "cancel", confirm: true, disabled: null });
  });
});

describe("remoteView's cancel badges and prefixes (o22.14.3)", () => {
  const labels = (v: ReturnType<typeof remoteView>) => (v.kind === "live" ? v.badges.map((b) => b.label) : []);

  // CONTROL: key the prefix on `last_contact` (a transport `check_error` followed by an answered
  // call → "Server answered") → red.
  it("prefixes a check error by its own flag, never by the last contact", () => {
    const transport: CheckError = { text: "collect: ssh exited with 255", answered: false };
    const broken: CheckError = { text: "collect (rc 0): broken reply", answered: true };
    expect(labels(remoteView(job(), record({ check_error: transport, last_contact: { kind: "ok" } })))).toEqual([
      "Server unreachable: collect: ssh exited with 255",
    ]);
    expect(labels(remoteView(job(), record({ check_error: broken, last_contact: { kind: "failed", detail: "x" } })))).toEqual([
      "Server answered: collect (rc 0): broken reply",
    ]);
  });

  it("says since when a cancel is pending (in the laptop's time), or that it may already be on the server", () => {
    const since = remoteView(job({ cancel_requested_at: "2026-10-18 06:18:01" }), record());
    expect(since.kind === "live" && since.badges).toEqual([
      expect.objectContaining({ kind: "cancel_pending", sent: false, tone: "warn" }),
    ]);
    expect(labels(since)[0]).toMatch(/^Cancel pending since 2026-10-18 \d\d:18:01$/);
    const sent = remoteView(job({ cancel_requested_at: "2026-10-18 06:18:01", cancel_sent_at: "2026-10-18 06:18:05" }), record());
    expect(labels(sent)).toEqual(["Cancel pending — the cancel may already be on the server"]);
  });

  // CONTROL: show a counter before the bound (a badge at `failures: 2` / `count: 2`) → red.
  it("warns of stopped re-sends while pending and of stopped sweeps on a cancelling job, and shows no counters before", () => {
    const p = { cancel_requested_at: "2026-10-18 06:18:01" };
    expect(labels(remoteView(job(p), record({ cancel_sends: { failures: 2, stopped: false } })))).toHaveLength(1);
    expect(labels(remoteView(job(p), record({ cancel_sends: { failures: 3, stopped: true } })))[1]).toBe(
      "Cancel re-sends stopped — Retry cancel to send again",
    );
    // Not pending: the stopped re-send bound is the past's, and says nothing.
    expect(labels(remoteView(job(), record({ cancel_sends: { failures: 3, stopped: true } })))).toEqual([]);
    const cancelling = job({ status: "cancelling" });
    expect(labels(remoteView(cancelling, record({ sweeps: { count: 2, stopped: false } })))).toEqual([]);
    expect(labels(remoteView(cancelling, record({ sweeps: { count: 3, stopped: true } })))).toEqual([
      "Sweeps stopped — processes still run; Retry cancel to sweep again",
    ]);
  });
});

describe("rowActions (o22.14.8)", () => {
  const LOCAL = { remote_host: null, remote_job_dir: null, remote_socket: null };
  const kinds = (j: Job, r: RemoteState | null = null) => rowActions(j, r).map((a) => a.action);

  // CONTROL: offer Delete for `cancelling` (or treat it as terminal) → red.
  it("offers Retry cancel and no Delete for a cancelling job", () => {
    expect(rowActions(job({ status: "cancelling" }), record())).toEqual([{ action: "retry_cancel", confirm: false }]);
  });

  it("offers a live job its cancel family only, and a terminal or draft row its open and Delete", () => {
    expect(rowActions(job({ status: "running" }), record())).toEqual([{ action: "cancel", confirm: true }]);
    expect(rowActions(job({ status: "running", ...LOCAL }), null)).toEqual([{ action: "cancel", confirm: false }]);
    expect(kinds(job({ status: "running", cancel_requested_at: "2026-10-18 06:18:01" }), record())).toEqual(["undo_cancel"]);
    expect(kinds(job({ status: "running", cancel_requested_at: "2026-10-18 06:18:01", cancel_sent_at: "2026-10-18 06:18:05" }), record())).toEqual([]);
    expect(kinds(job({ status: "completed" }))).toEqual(["open", "delete"]);
    expect(kinds(job({ status: "failed", ...LOCAL }))).toEqual(["open", "delete"]);
    expect(kinds(job({ status: "draft", ...LOCAL }))).toEqual(["run_locally", "delete"]);
    // Corrupt coordinates: shown, never acted on (o19.8).
    expect(kinds(job({ status: "running", remote_socket: null }))).toEqual([]);
  });

  it("names the action a Cancel button sends", () => {
    expect(cancelSendOf(rowActions(job({ status: "cancelling" }), record()))).toEqual({ action: "retry_cancel", confirm: false });
    expect(cancelSendOf([{ action: "undo_cancel", confirm: false }])).toBeNull();
    expect(cancelSendOf([{ action: "open" }, { action: "delete" }])).toBeNull();
  });
});

describe("cancelButtons (o22.14.1–2, B2)", () => {
  const REQUESTED = "2026-10-18 06:18:01";
  const SENT = "2026-10-18 06:18:05";
  const buttons = (j: Job, r: RemoteState | null, busy: Parameters<typeof cancelButtons>[1] = null) => cancelButtons(rowActions(j, r), busy);
  const labels = (b: ReturnType<typeof cancelButtons>) => b.map((x) => x.label);

  // CONTROLS: label every offer "Cancel" (the B1 state: Retry cancel kept "Cancel"); drop the Undo
  // arm → red. A pending, unsent, not-stopped row shows Undo cancel (closes 14.13.3's B1 gap).
  it("labels each button by its own offer", () => {
    expect(labels(buttons(job({ status: "running" }), record()))).toEqual(["Cancel"]);
    expect(labels(buttons(job({ status: "cancelling" }), record()))).toEqual(["Retry cancel"]);
    const pending = job({ status: "queued", cancel_requested_at: REQUESTED });
    expect(labels(buttons(pending, record()))).toEqual(["Undo cancel"]);
    expect(labels(buttons(pending, null))).toEqual(["Undo cancel"]);
    expect(buttons(pending, record()).map((b) => b.action)).toEqual(["undo_cancel"]);
    expect(labels(buttons(pending, record({ cancel_sends: { failures: 3, stopped: true } })))).toEqual(["Undo cancel", "Retry cancel"]);
    expect(labels(buttons({ ...pending, cancel_sent_at: SENT }, record({ cancel_sends: { failures: 3, stopped: true } })))).toEqual(["Retry cancel"]);
  });

  it("shows no button for a sent cancel whose re-sends run, and none for a finished job", () => {
    expect(buttons(job({ status: "running", cancel_requested_at: REQUESTED, cancel_sent_at: SENT }), record())).toEqual([]);
    expect(buttons(job({ status: "completed" }), null)).toEqual([]);
  });

  it("never draws a retry as part of the family, from a view's offers either", () => {
    const failedAway = record({ known: { kind: "labelled", detail: "not_on_server" } });
    const v = remoteView(job({ status: "queued" }), failedAway);
    expect(v.kind === "live" && names(v)).toEqual(["retry_submit", "cancel"]);
    expect(v.kind === "live" && labels(cancelButtons(v.actions, null))).toEqual(["Cancel"]);
  });

  // CONTROL: leave the buttons enabled while a click is in flight (a double click) → red.
  // CONTROL: ignore `locked` (a Retry in flight leaves the cancel buttons enabled) → red (o22.14.14.5).
  it("disables the family while another action of the view (a Retry) is in flight", () => {
    const both = job({ status: "queued", cancel_requested_at: REQUESTED });
    const stopped = record({ cancel_sends: { failures: 3, stopped: true } });
    expect(cancelButtons(rowActions(both, stopped), null, true).map((b) => [b.label, b.disabled])).toEqual([
      ["Undo cancel", true],
      ["Retry cancel", true],
    ]);
    expect(cancelButtons(rowActions(both, stopped), null, false).every((b) => !b.disabled)).toBe(true);
  });

  it("shows the click in flight and disables the whole family meanwhile", () => {
    const both = job({ status: "queued", cancel_requested_at: REQUESTED });
    const stopped = record({ cancel_sends: { failures: 3, stopped: true } });
    const sending = buttons(both, stopped, "cancel");
    expect(sending.map((b) => [b.label, b.disabled])).toEqual([
      ["Undo cancel", true],
      ["Sending cancel…", true],
    ]);
    expect(buttons(both, stopped, "undo").map((b) => [b.label, b.disabled])).toEqual([
      ["Undoing…", true],
      ["Retry cancel", true],
    ]);
    expect(buttons(both, stopped, null).every((b) => !b.disabled)).toBe(true);
    // "Cancelling…" is a status; the click's label is never it.
    expect(JSON.stringify(sending)).not.toMatch(/Cancelling/);
  });
});

describe("cancelNotice / undoNotice (o22.14.7)", () => {
  const delivered = (step: StatusStep): CancelResult => ({ result: "delivered", already_pending: false, step });
  const notSent = { cancel_sent_at: null };
  const sent = { cancel_sent_at: "2026-10-18 06:18:05" };
  const answered: CheckError = { text: "collect (rc 0): broken reply", answered: true };
  const unanswered: CheckError = { text: "ssh exited with 255", answered: false };

  // CONTROLS: `Delivered{CheckFailed}` containing "sent"; saying "is sent when it answers" when
  // `cancel_sent_at` is set (rc 0 then a failed collect); `CancelDelivered(Completed{late_cancel})`
  // → "cancel sent" → red.
  it("says sent only for a step that delivered", () => {
    expect(cancelNotice(delivered({ step: "cancel_delivered", detail: { outcome: "cancelled", started: false } }), notSent, null)).toBe("Cancel sent");
    expect(cancelNotice(delivered({ step: "cancelled" }), notSent, null)).toBe("Cancel sent");
    expect(cancelNotice(delivered({ step: "cancel_delivered", detail: { outcome: "completed", late_cancel: true } }), notSent, null)).toBe(
      "The job completed before the cancel took effect; its result is kept",
    );
    expect(cancelNotice(delivered({ step: "ended_before_cancel", detail: { outcome: "completed", late_cancel: false } }), notSent, null)).toBe(
      "The job finished before the cancel reached the server",
    );
    const failedCheck = delivered({ step: "check_failed", detail: "collect (rc 0): broken reply" });
    expect(cancelNotice(failedCheck, sent, unanswered)).toBe(
      "No answer after the cancel was sent; it may already be on the server; checked again at the next step",
    );
    expect(cancelNotice(failedCheck, notSent, unanswered)).toBe("Server unreachable — the cancel stays pending and is sent when it answers");
    expect(cancelNotice(failedCheck, notSent, answered)).toBe(
      "The server's answer could not be read: collect (rc 0): broken reply; the cancel stays pending",
    );
    expect(cancelNotice(failedCheck, notSent, null)).toBe("The cancel stays pending: collect (rc 0): broken reply");
    // No reloaded row: whether the send was marked is unknown, so it claims neither (B2).
    expect(cancelNotice(failedCheck, null, unanswered)).toBe("The cancel stays pending: collect (rc 0): broken reply");
    for (const row of [sent, notSent]) {
      for (const error of [answered, unanswered, null]) {
        expect(cancelNotice(failedCheck, row, error)).not.toMatch(/^Cancel sent/);
      }
    }
    expect(cancelNotice(failedCheck, sent, answered)).not.toMatch(/is sent when it answers/);
  });

  it("words every other answer, and nothing for a step delivery never returns", () => {
    expect(cancelNotice(delivered({ step: "cancel_not_sent", detail: "cancel.sh exited 3: not a directory" }), sent, null)).toBe(
      "The cancel failed on the server: cancel.sh exited 3: not a directory; it stays pending",
    );
    expect(cancelNotice(delivered({ step: "cancel_stopped", detail: { seen: null, unready: null } }), sent, null)).toBe("Cancel re-sends stopped");
    expect(cancelNotice(delivered({ step: "row_changed" }), sent, null)).toBe("The job changed meanwhile");
    expect(cancelNotice(delivered({ step: "not_live" }), sent, null)).toBe("The job changed meanwhile");
    for (const step of [
      { step: "busy" },
      { step: "labelled", detail: "not_on_server" },
      { step: "shown", detail: { outcome: "running" } },
      { step: "fetch_failed", detail: { strikes: 1, reason: "x" } },
      { step: "fetch_stopped" },
      { step: "swept", detail: { outcome: null, error: null } },
    ] as StatusStep[]) {
      expect(cancelNotice(delivered(step), sent, null), step.step).toBeNull();
    }
    expect(cancelNotice({ result: "pending", already_pending: false }, notSent, null)).toBe("Cancel pending — sent at the next check");
    expect(cancelNotice({ result: "already_on_server" }, notSent, null)).toBe("The cancel is already on the server; sweeping again");
    expect(cancelNotice({ result: "moved_on", status: "completed" }, notSent, null)).toBe("The job is already completed");
    expect(cancelNotice({ result: "dispatched" }, notSent, null)).toBeNull();
  });

  // CONTROL: `SendMarked` → "undone" → red.
  it("words Undo's three answers", () => {
    expect(undoNotice("undone")).toBe("Cancel undone");
    expect(undoNotice("send_marked")).toBe("Too late to undo: the cancel may already be on the server");
    expect(undoNotice("send_marked")).not.toMatch(/undone/);
    expect(undoNotice("nothing_pending")).toBe("No cancel to undo");
  });
});

describe("bannerAfterClick (o22.14.14.3)", () => {
  // CONTROLS: return `null` for a click with no answer (the old clear-at-click) → the declined
  // dialog wipes the notice, red; return `current` for an answer → the next answer does not replace.
  it("is replaced by the next answer, and kept by a click that produced none", () => {
    expect(bannerAfterClick("Cancel sent", null)).toBe("Cancel sent");
    expect(bannerAfterClick(null, null)).toBeNull();
    expect(bannerAfterClick("Cancel sent", { notice: "Cancel undone" })).toBe("Cancel undone");
    // An answer without words (a local cancel) replaces too: the old notice was about an earlier click.
    expect(bannerAfterClick("Cancel sent", { notice: null })).toBeNull();
  });

  // CONTROL: keep `current` on an error → the stale notice stands beside the new error, red.
  it("is cleared by an error: an error is an answer", () => {
    expect(bannerAfterClick("Cancel sent", "error")).toBeNull();
  });
});

describe("backfillsLog (o16.7)", () => {
  // CONTROL (N4, pure half): drop the remote-live arm → the remote `queued`/`running` rows go red.
  it("never backfills a remote live job or a draft, and backfills every other job", () => {
    const none = { remote_host: null, remote_job_dir: null, remote_socket: null };
    expect(backfillsLog(job({ status: "queued" }))).toBe(false);
    expect(backfillsLog(job({ status: "running" }))).toBe(false);
    expect(backfillsLog(job({ status: "cancelling" }))).toBe(false);
    expect(backfillsLog(job({ status: "draft" }))).toBe(false);
    expect(backfillsLog(job({ status: "draft", ...none }))).toBe(false);
    expect(backfillsLog(job({ status: "completed" }))).toBe(true);
    expect(backfillsLog(job({ status: "failed" }))).toBe(true);
    expect(backfillsLog(job({ status: "running", ...none }))).toBe(true);
    expect(backfillsLog(job({ status: "queued", ...none }))).toBe(true);
    // Corrupt coordinates are not polled, so nothing would re-stream: backfill as a local job.
    expect(backfillsLog(job({ status: "running", remote_socket: null }))).toBe(true);
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

describe("resolveRunTarget (o20.1)", () => {
  const options = runTargetOptions([
    makeProfile({ id: "p1", name: "uni", run_target: { is_run_target: true, reason: null } }),
    makeProfile({ id: "p2", name: "lab", run_target: { is_run_target: false, reason: "the profile has no core mask" } }),
  ]);
  const stored = (value: string) => ({ viewer_theme: "dark", [LAST_RUN_TARGET_SETTING]: value });

  // CONTROL: return the stored value without the `options.some(...)` check → the stale and the
  // disabled cases go red.
  it("preselects a stored profile only while it is an enabled option, else Local", () => {
    expect(resolveRunTarget(options, stored("p1"))).toBe("p1");
    expect(resolveRunTarget(options, stored("gone"))).toBeNull();
    expect(resolveRunTarget(options, stored("p2"))).toBeNull();
  });

  // CONTROL: read `settings[LAST_RUN_TARGET_SETTING]` without the `?.` → a failed read (null) throws, red.
  it("falls back to Local for Local stored, nothing stored, or a failed settings read", () => {
    expect(resolveRunTarget(options, stored(""))).toBeNull();
    expect(resolveRunTarget(options, { viewer_theme: "dark" })).toBeNull();
    expect(resolveRunTarget(options, null)).toBeNull();
  });
});
