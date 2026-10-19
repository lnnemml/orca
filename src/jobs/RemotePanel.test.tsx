import { describe, expect, it } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";

import type { Job, PendingSubmit, RemoteState } from "../types";
import { RemoteJobBadges, RemotePanel } from "./RemotePanel";
import { remoteView } from "./remote";

/**
 * The pending submit as the screens draw it (ADR-024 o23.15): `remoteView` decides, these pin that
 * the panel and the Jobs row arrange exactly its values — the notice and the blockers as text in
 * the panel only, the badge in both, the interim refusal and Retry submit nowhere.
 */

const INTERIM = "not submitted (Submit step): slot busy — processes hold cores 0-3";

function waitingJob(over: Partial<Job> = {}): Job {
  return {
    id: "j1",
    title: "remote job",
    input_content: "! HF",
    status: "queued",
    job_dir: null,
    energy: null,
    wall_time: null,
    error_message: INTERIM,
    created_at: "2026-10-19",
    started_at: null,
    completed_at: null,
    scene_json: null,
    scene_log_json: null,
    pathway_id: null,
    group_id: null,
    backend_id: "p1",
    remote_host: "uni",
    remote_job_dir: "/home/anton/.orcastudio/jobs/j1",
    remote_socket: "/tmp/ts-slot-0",
    cancel_requested_at: null,
    cancel_sent_at: null,
    submit_pending_since: "2026-10-19 08:30:00",
    ...over,
  };
}

const NOTICE = 'Waiting for the slot: held by <b>my "scan"</b>, pid 99 (cores 4-7)';

function recordWith(pending: Partial<PendingSubmit> | null): RemoteState {
  return {
    known: null,
    check_error: null,
    fetch: { strikes: 0, stopped: false },
    last_contact: { kind: "ok" },
    cancel_sends: { failures: 0, stopped: false },
    sweeps: { count: 0, stopped: false },
    pending_submit: pending && {
      since: "2026-10-19 08:30:00",
      refusals: 3,
      last: {
        kind: "slot_busy",
        detail: {
          blockers: [
            { blocker: { kind: "process", pid: 1, role: "wrapper", cores: "0-3", job_dir: "/x/jobs/a" }, title: '<b>my "scan"</b>' },
            { blocker: { kind: "process", pid: 99, role: "pinned", cores: "4-7", job_dir: null }, title: null },
          ],
        },
      },
      notified: true,
      notice: NOTICE,
      ...pending,
    },
  };
}

function panel(job: Job, record: RemoteState | null): string {
  return renderToStaticMarkup(
    <RemotePanel view={remoteView(job, record)} busy={null} onAction={() => {}} cancelBusy={null} onCancelAction={() => {}} />,
  );
}

describe("RemotePanel — a pending submit (o23.15)", () => {
  // CONTROLS: drop the notice block from the panel; render a blocker with markup (e.g.
  // dangerouslySetInnerHTML) → red.
  it("shows the notice as a warn line with the blockers as text, a title never as markup", () => {
    const html = panel(waitingJob(), recordWith({}));
    expect(html).toContain('data-testid="pending-submit-notice"');
    expect(html).toContain("Waiting for the slot since 2026-10-19");
    // The notice and the title are escaped text.
    expect(html).toContain("Waiting for the slot: held by &lt;b&gt;");
    expect(html).toContain("<li>&lt;b&gt;my &quot;scan&quot;&lt;/b&gt;</li>");
    expect(html).toContain("<li>pid 99 (cores 4-7)</li>");
    expect(html).not.toContain("<b>");
  });

  // CONTROL: show the notice before `notified` → red.
  it("shows the badge alone before the notice is due, and after a restart (no record)", () => {
    for (const rec of [recordWith({ refusals: 2, notified: false, notice: null }), null]) {
      const html = panel(waitingJob(), rec);
      expect(html).toContain("Waiting for the slot since");
      expect(html).not.toContain("pending-submit-notice");
      expect(html).not.toContain("held by");
    }
  });

  // CONTROLS: render the row's `error_message` while the flag is set; offer Retry submit → red.
  it("does not show the interim refusal or Retry submit, and offers Cancel", () => {
    const html = panel(waitingJob(), recordWith({}));
    expect(html).not.toContain("slot busy");
    expect(html).not.toContain("not submitted");
    expect(html).not.toContain("Retry submit");
    expect(html).toContain(">Cancel</button>");
  });

  // CONTROL: keep suppressing the text, or hide Retry submit, once the flag is NULL → red.
  it("shows today's view once the flag is NULL: the refusal, Not on the server and Retry submit", () => {
    const ended = { ...recordWith(null), known: { kind: "labelled", detail: "not_on_server" } } as RemoteState;
    const html = panel(waitingJob({ submit_pending_since: null, error_message: "not submitted (Upload step): rsync exited with 23" }), ended);
    expect(html).toContain("Not on the server");
    expect(html).toContain("not submitted (Upload step): rsync exited with 23");
    expect(html).toContain("Retry submit");
    expect(html).not.toContain("Waiting for the slot");
  });

  // CONTROL: pass the pending cancel through the waiting view wrongly (e.g. keep Cancel) → red.
  it("shows both badges and the pending cancel's Undo, not Cancel, when a cancel is also pending", () => {
    const html = panel(waitingJob({ cancel_requested_at: "2026-10-19 08:31:00" }), recordWith({}));
    expect(html).toContain("Waiting for the slot since");
    expect(html).toContain("Cancel pending since");
    expect(html).toContain("Undo cancel");
    expect(html).not.toContain(">Cancel</button>");
    expect(html).not.toContain("Retry submit");
  });
});

describe("RemoteJobBadges — a pending submit in the Jobs row (o23.15)", () => {
  // CONTROL: put the notice or the blockers into the row → red.
  it("shows the waiting badge only, never the notice, the blockers or the interim refusal", () => {
    const html = renderToStaticMarkup(<RemoteJobBadges job={waitingJob()} record={recordWith({})} />);
    expect(html).toContain("@uni");
    expect(html).toContain("badge tone-info");
    expect(html).toContain("Waiting for the slot since");
    expect(html).not.toContain("held by");
    expect(html).not.toContain("pid 99");
    expect(html).not.toContain("slot busy");
  });

  it("shows the badge after a restart, from the row's flag alone", () => {
    const html = renderToStaticMarkup(<RemoteJobBadges job={waitingJob()} record={undefined} />);
    expect(html).toContain("Waiting for the slot since");
  });
});
