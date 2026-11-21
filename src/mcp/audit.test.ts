// The audit view's display helpers (ADR-027 M4 amendment item 4). The time is shown in local time,
// so the zone is Europe/Kyiv (set and restored as in `format.test.ts`).
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import type { AuditRowView } from "../types";
import { appendOlder, argsCell, auditTime, dispositionLabel, firstReasonText, reasonsText } from "./audit";

let outerTz: string | undefined;
beforeAll(() => {
  outerTz = process.env.TZ;
  process.env.TZ = "Europe/Kyiv";
});
afterAll(() => {
  if (outerTz === undefined) delete process.env.TZ;
  else process.env.TZ = outerTz;
  expect(process.env.TZ).toBe(outerTz);
});

const ROW: AuditRowView = {
  id: 7,
  at: "2026-11-21 10:15:00",
  tool: "create_job",
  args: '{"title":"x"}',
  args_clipped: false,
  args_chars: 13,
  disposition: null,
  reasons: { kind: "read", list: [{ row: "R1", message: "no grant" }, { row: "P2", message: "unknown key" }] },
  job_id: "j1",
  grant_id: null,
};

describe("an audit row's cells", () => {
  it("shows the time in local time and a missing disposition as an outcome", () => {
    expect(auditTime(ROW)).toBe("2026-11-21 12:15:00");
    expect(dispositionLabel(ROW)).toBe("outcome");
    expect(dispositionLabel({ ...ROW, disposition: "refused" })).toBe("refused");
  });

  it("lists the reasons, and shows unreadable ones as such with their raw text", () => {
    expect(reasonsText(ROW.reasons)).toBe("R1: no grant; P2: unknown key");
    expect(firstReasonText(ROW.reasons)).toBe("R1: no grant (+1)");
    expect(reasonsText({ kind: "read", list: [] })).toBe("—");
    const bad = { kind: "unreadable" as const, raw: "not json", raw_clipped: true };
    expect(reasonsText(bad)).toBe("reasons unreadable: not json…");
    expect(firstReasonText(bad)).toBe("reasons unreadable");
  });

  it("marks cut args with how much is shown", () => {
    expect(argsCell(ROW)).toEqual({ text: ROW.args, clipped: false, note: null });
    const cut = argsCell({ ...ROW, args: "a".repeat(1024), args_clipped: true, args_chars: 5000 });
    expect(cut.clipped).toBe(true);
    expect(cut.text.endsWith("…")).toBe(true);
    expect(cut.note).toBe("first 1,024 of 5,000 characters");
  });
});

describe("Load older", () => {
  it("appends the older page, never showing a row twice, and counts a repeat", () => {
    const shown = [{ ...ROW, id: 9 }, { ...ROW, id: 8 }];
    const out = appendOlder(shown, { rows: [{ ...ROW, id: 8 }, { ...ROW, id: 7 }], next_before_id: 7 });
    expect(out.rows.map((r) => r.id)).toEqual([9, 8, 7]);
    expect(out.repeated).toBe(1);
    expect(out.nextBeforeId).toBe(7);
    expect(appendOlder(shown, { rows: [], next_before_id: null })).toEqual({ rows: shown, repeated: 0, nextBeforeId: null });
  });
});
