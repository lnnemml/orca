import { describe, expect, it } from "vitest";
import type { Job } from "../types";
import {
  autoRunRefusal,
  CLAUDE_BADGE,
  firstReason,
  isClaudeJob,
  isHeldByClaude,
  isProposed,
  isSubmitting,
  LEGACY_DRAFT_NOTE,
  LOCAL_MCP_TARGET,
  parseJobReasons,
  RAW_REASONS_SHOWN,
  reasonsList,
  runAction,
  submitTargetOf,
  SUBMITTING_LABEL,
} from "./claude";

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
    created_at: "2026-11-22",
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
    origin: "user",
    mcp_reasons: null,
    grant_id: null,
    mcp_state: null,
    mcp_target: null,
    ...over,
  };
}

const PROPOSED = { status: "draft", origin: "mcp", grant_id: null, mcp_state: null } as const;
const SUBMITTING = { status: "draft", origin: "mcp", grant_id: "g1", mcp_state: "submitting" } as const;
const REASONS = JSON.stringify([
  { row: "P5", message: "memory is over the limit" },
  { row: "P7", message: "server is full" },
]);
const ALL_BUT_DRAFT = ["queued", "running", "cancelling", "completed", "parsed", "failed", "cancelled", "lost"] as const;

describe("the badge", () => {
  it("is Claude's only for origin mcp, whatever else the job carries", () => {
    expect(isClaudeJob(job({ origin: "mcp" }))).toBe(true);
    expect(isClaudeJob(job({ origin: "user" }))).toBe(false);
    // A user job that carries every other MCP field is still Anton's: only `origin` decides.
    expect(isClaudeJob(job({ origin: "user", grant_id: "g1", mcp_state: "submitting", mcp_reasons: REASONS }))).toBe(false);
    expect(CLAUDE_BADGE).toBe("Claude");
  });

  it("is not claimed by a payload that lacks the field", () => {
    const old: Partial<Job> = job();
    delete old.origin;
    expect(isClaudeJob(old as Job)).toBe(false);
  });
});

describe("PROPOSED is exact", () => {
  it("is a draft, from Claude, with no grant and no mcp_state", () => {
    expect(isProposed(job(PROPOSED))).toBe(true);
  });

  it("is not a draft still in step B", () => {
    // Also a `draft` with origin mcp — but charged and submitting.
    expect(isProposed(job(SUBMITTING))).toBe(false);
    expect(isProposed(job({ ...PROPOSED, mcp_state: "submitting" }))).toBe(false);
    expect(isProposed(job({ ...PROPOSED, grant_id: "g1" }))).toBe(false);
  });

  it("drops each conjunct on its own", () => {
    expect(isProposed(job({ ...PROPOSED, status: "queued" }))).toBe(false);
    expect(isProposed(job({ ...PROPOSED, origin: "user" }))).toBe(false);
  });

  it("is false for every status but draft", () => {
    for (const status of ALL_BUT_DRAFT) expect(isProposed(job({ ...PROPOSED, status }))).toBe(false);
  });

  it("is false for a payload that lacks the grant or the state", () => {
    const p: Partial<Job> = job(PROPOSED);
    delete p.grant_id;
    expect(isProposed(p as Job)).toBe(false);
    const q: Partial<Job> = job(PROPOSED);
    delete q.mcp_state;
    expect(isProposed(q as Job)).toBe(false);
  });
});

describe("submitting", () => {
  it("is mcp_state = 'submitting' and nothing else", () => {
    expect(isSubmitting(job({ mcp_state: "submitting" }))).toBe(true);
    expect(isSubmitting(job({ mcp_state: null }))).toBe(false);
    expect(isSubmitting(job(SUBMITTING))).toBe(true);
  });
});

describe("parseJobReasons never throws", () => {
  it("reads null as none", () => {
    expect(parseJobReasons(job({ mcp_reasons: null }))).toEqual({ kind: "none" });
  });

  it("reads [{row, message}] as a list", () => {
    expect(parseJobReasons(job({ mcp_reasons: REASONS }))).toEqual({
      kind: "read",
      list: [
        { row: "P5", message: "memory is over the limit" },
        { row: "P7", message: "server is full" },
      ],
    });
    expect(parseJobReasons(job({ mcp_reasons: "[]" }))).toEqual({ kind: "read", list: [] });
  });

  it("keeps only row and message", () => {
    const r = parseJobReasons(job({ mcp_reasons: '[{"row":"a","message":"b","extra":1}]' }));
    expect(r).toEqual({ kind: "read", list: [{ row: "a", message: "b" }] });
  });

  it("reads anything else as unreadable, with its raw text", () => {
    const malformed = [
      "not json",
      "{",
      "",
      "null",
      "42",
      '"text"',
      "{}",
      '{"row":"a","message":"b"}',
      '[{"row":"a"}]',
      '[{"row":1,"message":"b"}]',
      '[{"row":"a","message":"b"}, 3]',
      "[null]",
      "[[]]",
    ];
    for (const raw of malformed) {
      let r: unknown;
      expect(() => {
        r = parseJobReasons(job({ mcp_reasons: raw }));
      }, raw).not.toThrow();
      expect(r, raw).toEqual({ kind: "unreadable", raw, raw_clipped: false });
    }
  });

  it("cuts a huge raw text and says so", () => {
    const raw = "x".repeat(RAW_REASONS_SHOWN + 50);
    const r = parseJobReasons(job({ mcp_reasons: raw }));
    expect(r).toEqual({ kind: "unreadable", raw: "x".repeat(RAW_REASONS_SHOWN), raw_clipped: true });
  });

  it("does not split a character when it cuts", () => {
    const raw = "𝄞".repeat(RAW_REASONS_SHOWN + 1);
    const r = parseJobReasons(job({ mcp_reasons: raw }));
    expect(r.kind === "unreadable" && [...r.raw].length).toBe(RAW_REASONS_SHOWN);
  });
});

describe("a PROPOSED job's reasons", () => {
  const proposed = (mcp_reasons: string | null) => job({ ...PROPOSED, mcp_reasons });

  it("the row shows the first, with how many more", () => {
    expect(firstReason(proposed(REASONS))).toBe("P5: memory is over the limit (+1)");
    expect(firstReason(proposed('[{"row":"P5","message":"m"}]'))).toBe("P5: m");
  });

  it("the detail shows the whole list", () => {
    expect(reasonsList(proposed(REASONS))).toEqual({
      kind: "read",
      lines: ["P5: memory is over the limit", "P7: server is full"],
    });
  });

  it("shows unreadable reasons as unreadable, not as a failure", () => {
    expect(firstReason(proposed("[oops"))).toBe("reasons unreadable");
    expect(reasonsList(proposed("[oops"))).toEqual({ kind: "unreadable", text: "reasons unreadable: [oops" });
  });

  it("shows nothing when there are none", () => {
    expect(firstReason(proposed(null))).toBeNull();
    expect(reasonsList(proposed(null))).toBeNull();
    expect(firstReason(proposed("[]"))).toBeNull();
    expect(reasonsList(proposed("[]"))).toBeNull();
  });

  it("shows nothing for a job that is not PROPOSED, even one that carries reasons", () => {
    const others = [
      job({ ...SUBMITTING, mcp_reasons: REASONS }),
      job({ origin: "user", mcp_reasons: REASONS }),
      job({ ...PROPOSED, status: "queued", mcp_reasons: REASONS }),
    ];
    for (const j of others) {
      expect(firstReason(j)).toBeNull();
      expect(reasonsList(j)).toBeNull();
    }
  });
});

describe("the Run slot", () => {
  it("offers Run on a draft that is not being submitted", () => {
    expect(runAction(job({ status: "draft" }))).toEqual({ kind: "run" });
    expect(runAction(job(PROPOSED))).toEqual({ kind: "run" });
  });

  it("replaces Run with a disabled 'Claude is submitting' while submitting", () => {
    expect(runAction(job(SUBMITTING))).toEqual({ kind: "submitting", label: "Claude is submitting", disabled: true });
    expect(SUBMITTING_LABEL).toBe("Claude is submitting");
  });

  it("offers no Run on a job that is not a draft", () => {
    for (const status of ALL_BUT_DRAFT) expect(runAction(job({ status })).kind).toBe("none");
    // Step C has queued the job but its flag still reads submitting: Cancel shows, never Run.
    expect(runAction(job({ status: "queued", mcp_state: "submitting" })).kind).toBe("none");
  });
});

describe("held by Claude (what holds Delete back)", () => {
  // CONTROL: `isHeldByClaude` true only for `submitting`, or never -> red on the odd value / the status line.
  it("is any set mcp_state, the predicate of Rust's delete refusal", () => {
    expect(isHeldByClaude(job(SUBMITTING))).toBe(true);
    expect(isHeldByClaude(job({ status: "completed", mcp_state: "submitting" }))).toBe(true);
    expect(isHeldByClaude(job({ mcp_state: null }))).toBe(false);
    // An odd value (Rust fails closed on any non-NULL) and a payload that lacks the field.
    expect(isHeldByClaude(job({ mcp_state: "busy" as unknown as "submitting" }))).toBe(true);
    expect(isHeldByClaude({} as Job)).toBe(false);
  });
});

describe("the auto-run submit", () => {
  // CONTROL: return null always -> the submitting draft is auto-submitted from the UI -> red.
  it("is refused for a draft Claude is still submitting, in the same words as the Run slot", () => {
    expect(autoRunRefusal(job(SUBMITTING), null)).toContain(SUBMITTING_LABEL);
  });

  it("goes ahead for every other draft, for a job that is not a draft, and for an unread row", () => {
    expect(autoRunRefusal(job({ status: "draft" }), null)).toBeNull();
    // A Claude draft on its own stored target (M4c): `local` runs on this machine.
    expect(autoRunRefusal(job({ ...PROPOSED, mcp_target: "local" }), null)).toBeNull();
    expect(autoRunRefusal(job({ status: "queued", mcp_state: "submitting" }), null)).toBeNull();
    expect(autoRunRefusal(null, null)).toBeNull();
  });
});

// --- The stored target (ADR-027 M4c amendment) ---------------------------------------------------

describe("the submit target of a stored mcp_target", () => {
  // CONTROL (ctl:ui_local_null): return `mcpTarget` as is -> `local` reaches `submit_job` as a profile
  // named "local" -> red.
  it("maps local to null (this machine) and a profile id to itself", () => {
    expect(submitTargetOf("local"), "[ctl:ui_local_null]").toBeNull();
    expect(LOCAL_MCP_TARGET).toBe("local");
    expect(submitTargetOf("p-uni")).toBe("p-uni");
  });
});

describe("the auto-run submit of a Claude draft (M4c)", () => {
  const UNI = { ...PROPOSED, mcp_target: "p-uni" } as const;

  // CONTROL (ctl:ui_autorun_mismatch): drop the target comparison -> the uni draft auto-runs locally -> red.
  it("declines a target other than the stored one, and goes ahead on the stored one", () => {
    expect(autoRunRefusal(job(UNI), null), "[ctl:ui_autorun_mismatch]").toBe(
      "Not run: Claude assembled this job for the server profile p-uni; run it there, or delete it.",
    );
    expect(autoRunRefusal(job(UNI), "p-dark")).toContain("for the server profile p-uni");
    expect(autoRunRefusal(job(UNI), "p-uni")).toBeNull();
    expect(autoRunRefusal(job({ ...PROPOSED, mcp_target: "local" }), "p-uni")).toBe(
      "Not run: Claude assembled this job for this machine; run it there, or delete it.",
    );
    // `Backend::for_submit(Some("local"))` is a profile named "local", not this machine.
    expect(autoRunRefusal(job({ ...PROPOSED, mcp_target: "local" }), "local")).not.toBeNull();
  });

  // CONTROL (ctl:ui_autorun_legacy): let a null mcp_target through -> the legacy draft auto-runs -> red.
  it("declines a legacy Claude draft on every target, failing closed on a payload without the field", () => {
    for (const target of [null, "p-uni", "local"]) {
      expect(autoRunRefusal(job(PROPOSED), target), "[ctl:ui_autorun_legacy]").toBe(`Not run: ${LEGACY_DRAFT_NOTE}.`);
    }
    const old = job(PROPOSED) as Partial<Job>;
    delete old.mcp_target;
    expect(autoRunRefusal(old as Job, null)).toBe(`Not run: ${LEGACY_DRAFT_NOTE}.`);
    expect(LEGACY_DRAFT_NOTE).toBe("assembled before targets were stored — delete it and ask Claude again");
  });

  it("leaves a user draft as it was, on any target", () => {
    for (const target of [null, "p-uni"]) expect(autoRunRefusal(job({ status: "draft" }), target)).toBeNull();
  });
});
