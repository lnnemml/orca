import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import { createElement } from "react";

import type { CancelResult, Job, Outcome, ReEnqueueWait, RemoteState, StatusStep } from "../types";
import { aggregateReopt, type ReoptChild } from "../scene/reopt-aggregate";
import { isTerminalSuccessStatus } from "../scene/ensemble";
import { RemotePanel } from "./RemotePanel";
import { UNCONFIRMED_NEVER_START, cancelNotice, isLiveStatus, isUnconfirmedNeverStart, remoteView, rowActions } from "./remote";
import { STATUS_CHIPS, queueCounts } from "./status";

/**
 * Unit 5.4b in the view (ADR-024 o25.1, o25.4, o25.6, o25.9, o25.15): the terminal `lost` status,
 * the never-start awaiting its confirmation, a waiting re-enqueue's notice and the "could not be
 * decided" notice. Each test names its negative control (a source mutation in
 * `run_54b_ts_controls.sh`).
 */

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
    created_at: "2026-11-18",
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

function record(over: Partial<RemoteState> = {}): RemoteState {
  return {
    known: null,
    check_error: null,
    fetch: { strikes: 0, stopped: false },
    last_contact: { kind: "ok" },
    cancel_sends: { failures: 0, stopped: false },
    sweeps: { count: 0, stopped: false },
    pending_submit: null,
    undecided: null,
    re_enqueue_waiting: null,
    ...over,
  };
}

const shown = (detail: Outcome): RemoteState["known"] => ({ kind: "shown", detail });
const NEVER_STARTED: Outcome = { outcome: "failed", reason: { reason: "wrapper_never_started" } };

describe("lost — a terminal status (o25.1)", () => {
  // CONTROL `ts-no-lost-chip`: drop "lost" from STATUS_CHIPS → red (and a type error in status.ts).
  it("has a chip, after the other terminal states", () => {
    expect(STATUS_CHIPS).toContain("lost");
    expect((STATUS_CHIPS as readonly string[]).indexOf("lost")).toBe(STATUS_CHIPS.length - 1);
  });

  // CONTROLS `ts-lost-live` (counted live) and `ts-lost-counted` (counted running) → red.
  it("is not live and is counted neither running nor queued", () => {
    expect(isLiveStatus("lost")).toBe(false);
    expect(queueCounts([{ status: "lost" }, { status: "lost" }, { status: "running" }])).toEqual({ running: 1, queued: 0 });
  });

  it("is a terminal remote row: the ordinary view, and open or delete", () => {
    const lost = job({ status: "lost", error_message: "Lost: the server restarted while the job ran; …" });
    expect(remoteView(lost, record({ known: shown({ outcome: "lost", orphans: [], stale_boot: true }) }))).toEqual({ kind: "terminal" });
    expect(rowActions(lost, null)).toEqual([{ action: "open" }, { action: "delete" }]);
  });

  it("is a failed child for the re-opt comparison, and no success for the ensemble", () => {
    const child: ReoptChild = {
      job_id: "c1",
      title: "re-opt #1",
      status: "lost",
      source_conformer_index: 1,
      electronic_energy_eh: null,
      gibbs_eh: null,
      imaginary_count: null,
      freq_requested: true,
      method: "r2SCAN-3c",
      element_mismatch: false,
    };
    const c = aggregateReopt(
      { source_job_id: "g", children: [child], freq_requested_count: 1, mode_inconsistent: false },
      [{ atoms: [], energy: -1, index: 1 }],
    );
    // CONTROL `ts-lost-still-running`: a lost child read as "still running" → red.
    expect(c.excluded.map((e) => e.reason)).toEqual(["job failed (lost on the server)"]);
    expect(isTerminalSuccessStatus("lost")).toBe(false);
  });

  // CONTROL `ts-no-badge-lost`: delete `.badge.lost` from the stylesheet → red.
  it("has an error-toned badge, the failed one's colours", () => {
    const css = readFileSync(resolve(process.cwd(), "src/styles/app.css"), "utf8");
    const rule = (sel: string) => css.match(new RegExp(`\\.badge\\.${sel} \\{([^}]*)\\}`))?.[1].replace(/\s+/g, " ").trim();
    expect(rule("lost")).toBeDefined();
    expect(rule("lost")).toBe(rule("failed"));
  });
});

describe("the never-start awaiting its confirmation (o25.15)", () => {
  // CONTROL `ts-unconfirmed-as-failed`: the badge reads the verdict's own name → "Server: failed",
  // red.
  it("renders as waiting, never as a terminal failure, while the row is queued", () => {
    const v = remoteView(job({ error_message: "Nothing of the job has run on the server since its one re-enqueue…" }), record({ known: shown(NEVER_STARTED) }));
    expect(v.kind).toBe("live");
    if (v.kind !== "live") return;
    expect(v.badges).toEqual([
      { kind: "outcome", outcome: "failed", stale: false, label: `Server: ${UNCONFIRMED_NEVER_START}`, tone: "warn" },
    ]);
    expect(JSON.stringify(v.badges)).not.toMatch(/Server: failed|"tone":"err"/);
    expect(v.actions.map((a) => a.action)).toEqual(["cancel"]);
    expect(isUnconfirmedNeverStart(NEVER_STARTED)).toBe(true);
    expect(isUnconfirmedNeverStart({ outcome: "failed", reason: { reason: "non_zero_exit", code: 1 } })).toBe(false);
  });
});

describe("a waiting re-enqueue's notice (o25.4, o25.14.3, o25.14.6)", () => {
  const slotBusy: ReEnqueueWait = {
    kind: "slot",
    detail: {
      kind: "slot_busy",
      detail: { blockers: [{ blocker: { kind: "process", pid: 99, role: "pinned", cores: "4-7", job_dir: null }, title: null }] },
    },
  };
  const waiting = (refusals: number, notified: boolean, last: ReEnqueueWait = slotBusy) =>
    record({
      known: shown({ outcome: "re_enqueue" }),
      // The notice is set even before `notified`, so the view's own gate on `notified` is what is tested.
      re_enqueue_waiting: { refusals, notified, notice: "The re-enqueue … waits. Waiting for the slot: held by pid 99 (cores 4-7)", last },
    });

  // CONTROL `ts-reenqueue-notice-at-first`: shown before `notified` → red.
  it("comes at the 3rd transient refusal, with the busy slot's blockers", () => {
    for (const n of [1, 2]) {
      const v = remoteView(job(), waiting(n, false));
      expect(v.kind === "live" && v.reEnqueueWaiting).toBe(null);
    }
    const v = remoteView(job(), waiting(3, true));
    expect(v.kind === "live" && v.reEnqueueWaiting).toEqual({
      text: "The re-enqueue … waits. Waiting for the slot: held by pid 99 (cores 4-7)",
      blockers: ["pid 99 (cores 4-7)"],
    });
  });

  it("says to re-test the profile, at once, for an unverified one", () => {
    const notice = "The re-enqueue of a job that never started waits: re-test the server profile to continue (not verified).";
    const v = remoteView(
      job(),
      record({
        known: shown({ outcome: "re_enqueue" }),
        re_enqueue_waiting: { refusals: 0, notified: true, notice, last: { kind: "unverified", detail: "not verified" } },
      }),
    );
    expect(v.kind === "live" && v.reEnqueueWaiting).toEqual({ text: notice, blockers: [] });
  });

  // CONTROL `ts-reenqueue-ungated`: the shown-verdict gate dropped → red (a lingering field after
  // another verdict shows the notice).
  it("is keyed on the shown re_enqueue verdict, never on pending_submit", () => {
    const lingering = { ...waiting(3, true), known: shown({ outcome: "queued" }) };
    const v = remoteView(job(), lingering);
    expect(v.kind === "live" && v.reEnqueueWaiting).toBe(null);
    expect(v.kind === "live" && v.waiting).toBe(null);
  });

  // CONTROL `ts-panel-no-reenqueue-notice`: the panel does not draw it → red.
  it("is drawn by the panel, the blockers as text", () => {
    const html = renderToStaticMarkup(
      createElement(RemotePanel, {
        view: remoteView(job(), waiting(3, true)),
        busy: null,
        onAction: () => {},
        cancelBusy: null,
        onCancelAction: () => {},
      }),
    );
    expect(html).toContain('data-testid="re-enqueue-notice"');
    expect(html).toContain("<li>pid 99 (cores 4-7)</li>");
  });
});

describe("the undecided notice (o25.6)", () => {
  // CONTROL `ts-undecided-missing`: the view drops the record's notice → red.
  it("shows the record's notice from the 3rd pass, and nothing before it", () => {
    const notice = "The server's state of this job could not be decided 3 times in a row: the .started read disagreed twice";
    const v = remoteView(job({ status: "running" }), record({ known: shown({ outcome: "running" }), undecided: { passes: 3, notice } }));
    expect(v.kind === "live" && v.undecided).toBe(notice);
    const html = renderToStaticMarkup(
      createElement(RemotePanel, { view: v, busy: null, onAction: () => {}, cancelBusy: null, onCancelAction: () => {} }),
    );
    expect(html).toContain('data-testid="undecided-notice"');
    const before = remoteView(job({ status: "running" }), record({ known: shown({ outcome: "running" }) }));
    expect(before.kind === "live" && before.undecided).toBe(null);
  });
});

describe("the cancel notice over the 5.4b steps (o25.9)", () => {
  // The switch has no default fall-through: a step left out is a type error (CONTROL
  // `ts-cancelnotice-missing-case`, red by `tsc`). A delivery never returns these, so no "sent".
  it("says nothing for an ordinary status step's 5.4b result", () => {
    const steps: StatusStep[] = [
      { step: "lost_swept", detail: { outcome: null, error: null } },
      { step: "re_enqueued", detail: { tsp_id: 4 } },
      { step: "re_enqueue_waiting", detail: { kind: "unverified", detail: "x" } },
      { step: "re_enqueue_refused", detail: { refusals: 1, why: "marker: x" } },
    ];
    for (const step of steps) {
      const result: CancelResult = { result: "delivered", already_pending: false, step };
      expect(cancelNotice(result, null, null)).toBe(null);
    }
  });
});
