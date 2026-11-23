import { describe, expect, it } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";

import type { Job } from "../types";
import { ClaudeBadge, DetailReasons, RowReason, SubmittingButton } from "./ClaudeMarks";

/**
 * The marks as the Jobs table and the detail arrange them. `claude.ts` decides (its tests pin the
 * decisions); these pin that the components draw exactly those decisions and never throw. How they
 * look is Anton's live gate.
 */

function job(over: Partial<Job> = {}): Job {
  return {
    id: "j1",
    title: "a job",
    input_content: "! HF",
    status: "draft",
    job_dir: null,
    energy: null,
    wall_time: null,
    error_message: null,
    created_at: "2026-11-23",
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
    origin: "mcp",
    mcp_reasons: null,
    grant_id: null,
    mcp_state: null,
    ...over,
  };
}

const REASONS = JSON.stringify([
  { row: "P5", message: "memory is over the limit" },
  { row: "P7", message: "server is full" },
]);

describe("ClaudeBadge", () => {
  // CONTROL: `isClaudeJob` returns true for any job → the user job gets the badge → red.
  it("is drawn for origin mcp only, in the existing badge style", () => {
    const html = renderToStaticMarkup(<ClaudeBadge job={job()} />);
    expect(html).toContain("badge");
    expect(html).toContain(">Claude<");
    expect(renderToStaticMarkup(<ClaudeBadge job={job({ origin: "user" })} />)).toBe("");
  });
});

describe("RowReason", () => {
  // CONTROL: show `firstReason`'s text for any draft (drop the PROPOSED test) → the submitting one shows it → red.
  it("shows the first reason of a PROPOSED job, with how many more", () => {
    expect(renderToStaticMarkup(<RowReason job={job({ mcp_reasons: REASONS })} />)).toContain("P5: memory is over the limit (+1)");
  });

  it("shows nothing for a submitting draft, a job past draft, or no reasons", () => {
    expect(renderToStaticMarkup(<RowReason job={job({ mcp_reasons: REASONS, grant_id: "g1", mcp_state: "submitting" })} />)).toBe("");
    expect(renderToStaticMarkup(<RowReason job={job({ mcp_reasons: REASONS, status: "queued" })} />)).toBe("");
    expect(renderToStaticMarkup(<RowReason job={job({ mcp_reasons: null })} />)).toBe("");
    expect(renderToStaticMarkup(<RowReason job={job({ mcp_reasons: REASONS, origin: "user" })} />)).toBe("");
  });

  // CONTROL: drop the try/catch around `JSON.parse` in `parseJobReasons` → the row throws → red.
  it("does not throw on malformed reasons: it says they are unreadable", () => {
    for (const raw of ["not json {", "{}", '[{"row":1}]', "", "[1,2]"]) {
      expect(renderToStaticMarkup(<RowReason job={job({ mcp_reasons: raw })} />)).toContain("reasons unreadable");
    }
  });
});

describe("DetailReasons", () => {
  it("lists each reason on its own line", () => {
    const html = renderToStaticMarkup(<DetailReasons job={job({ mcp_reasons: REASONS })} />);
    expect(html).toContain("<li>P5: memory is over the limit</li>");
    expect(html).toContain("<li>P7: server is full</li>");
  });

  // CONTROL: same as above (the parser throws) → red.
  it("shows unreadable reasons with their raw text, and nothing for a job that is not PROPOSED", () => {
    const html = renderToStaticMarkup(<DetailReasons job={job({ mcp_reasons: "garbage <b>" })} />);
    expect(html).toContain("reasons unreadable: garbage &lt;b&gt;");
    expect(renderToStaticMarkup(<DetailReasons job={job({ mcp_reasons: REASONS, mcp_state: "submitting", grant_id: "g1" })} />)).toBe("");
    expect(renderToStaticMarkup(<DetailReasons job={job({ mcp_reasons: null })} />)).toBe("");
    expect(renderToStaticMarkup(<DetailReasons job={job({ mcp_reasons: "[]" })} />)).toBe("");
  });
});

describe("SubmittingButton", () => {
  it("is a disabled button reading 'Claude is submitting'", () => {
    const html = renderToStaticMarkup(<SubmittingButton />);
    expect(html).toContain("disabled");
    expect(html).toContain(">Claude is submitting<");
  });
});
