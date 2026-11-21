// The grant editor's expiry (ADR-027 M4 amendment item 5), pinned under a **non-UTC** zone with both
// DST changes: Europe/Kyiv is UTC+2 (EET) in winter and UTC+3 (EEST) in summer; in 2026 the clocks
// go 03:00 → 04:00 on 29 March and 04:00 → 03:00 on 25 October. The zone is set in `beforeAll` and
// restored in `afterAll` (the `format.test.ts` pattern: it must not leak into the next file of a
// reused worker). The first test proves the zone took effect.
//
// Controls (`m4a_parta_controls.py`): EXPIRY-UTC, EXPIRY-PRESET-DST, DST-GAP, FALLBACK-OFFSET.
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import {
  DEFAULT_EXPIRY_PRESET,
  EXPIRY_PRESETS,
  customExpiry,
  expiryShownText,
  presetExpiry,
  presetHint,
  resolveExpiry,
  toUtcText,
  utcOffsetText,
  type ExpiryResult,
} from "./expiry";

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

function ok(r: ExpiryResult) {
  if (!r.ok) throw new Error(`expected an expiry, got: ${r.error}`);
  return r;
}

describe("the expiry under Europe/Kyiv", () => {
  it("runs under a zone that is not UTC, with both offsets", () => {
    expect(new Date("2026-11-21T10:00:00Z").getHours()).toBe(12);
    expect(new Date("2026-07-01T10:00:00Z").getHours()).toBe(13);
  });

  it("offers 1 h, 8 h (the default), 24 h and 7 days", () => {
    expect(EXPIRY_PRESETS.map((p) => p.id)).toEqual(["1h", "8h", "24h", "7d"]);
    expect(DEFAULT_EXPIRY_PRESET).toBe("8h");
  });

  // CONTROL EXPIRY-UTC: `toUtcText` reading the local getters → red.
  it("computes a preset from the injected now and sends it as UTC", () => {
    const now = new Date("2026-11-21T10:00:00Z"); // 12:00 in Kyiv (EET)
    const e = presetExpiry("8h", now);
    expect(e.utc, "EXPIRY-UTC").toBe("2026-11-21 18:00:00");
    expect(e.local).toBe("2026-11-21 20:00");
    expect(e.offset).toBe("UTC+02:00");
    expect(e.ambiguous).toBe(false);
    expect(presetExpiry("1h", now).utc).toBe("2026-11-21 11:00:00");
    expect(presetExpiry("24h", now).utc).toBe("2026-11-22 10:00:00");
    expect(presetExpiry("7d", now).utc).toBe("2026-11-28 10:00:00");
    expect(presetHint("8h", now)).toBe("8 h from Create — about 2026-11-21 20:00 (UTC+02:00)");
    expect(toUtcText(new Date("2026-07-01T23:59:58Z")), "EXPIRY-UTC").toBe("2026-07-01 23:59:58");
  });

  // CONTROL EXPIRY-PRESET-DST: a preset added as wall-clock hours (`setHours(+8)`) → red: across a
  // change, 8 h of wall clock is 7 or 9 h of real time.
  it("keeps a preset an exact span across both DST changes", () => {
    const spring = presetExpiry("8h", new Date("2026-03-28T22:00:00Z")); // 00:00 EET on 29 March
    expect(spring.utc, "EXPIRY-PRESET-DST").toBe("2026-03-29 06:00:00");
    expect([spring.local, spring.offset]).toEqual(["2026-03-29 09:00", "UTC+03:00"]);
    const fall = presetExpiry("8h", new Date("2026-10-24T21:00:00Z")); // 00:00 EEST on 25 October
    expect(fall.utc, "EXPIRY-PRESET-DST").toBe("2026-10-25 05:00:00");
    expect([fall.local, fall.offset]).toEqual(["2026-10-25 07:00", "UTC+02:00"]);
  });

  it("turns a custom local time into UTC in winter, in summer and across midnight", () => {
    expect(ok(customExpiry("2026-12-01T14:30")).utc).toBe("2026-12-01 12:30:00");
    expect(ok(customExpiry("2026-12-01T14:30")).offset).toBe("UTC+02:00");
    expect(ok(customExpiry("2026-07-01T14:30")).utc).toBe("2026-07-01 11:30:00");
    expect(ok(customExpiry("2026-07-01T14:30")).offset).toBe("UTC+03:00");
    expect(ok(customExpiry("2026-07-01T01:15")).utc).toBe("2026-06-30 22:15:00");
    expect(ok(customExpiry("2027-01-01 00:00")).utc).toBe("2026-12-31 22:00:00");
    expect(ok(customExpiry("2026-12-01T14:30:00")).utc).toBe("2026-12-01 12:30:00");
    expect(resolveExpiry({ kind: "custom", value: "2026-12-01T14:30" }, new Date(0))).toEqual(customExpiry("2026-12-01T14:30"));
    expect(ok(resolveExpiry({ kind: "preset", preset: "1h" }, new Date("2026-11-21T10:00:00Z"))).utc).toBe("2026-11-21 11:00:00");
  });

  // CONTROL DST-GAP: the round-trip check removed → red (V8 moves 03:30 to 04:30 EEST, a time Anton
  // never entered).
  it("refuses a time skipped by the spring-forward change", () => {
    for (const t of ["03:00", "03:30", "03:59"]) {
      const r = customExpiry(`2026-03-29T${t}`);
      expect(r.ok, `DST-GAP: ${t} accepted`).toBe(false);
      if (!r.ok) expect(r.error).toBe(`${t} on 2026-03-29 does not exist — DST`);
    }
    expect(ok(customExpiry("2026-03-29T02:59"))).toMatchObject({ utc: "2026-03-29 00:59:00", offset: "UTC+02:00", ambiguous: false });
    expect(ok(customExpiry("2026-03-29T04:00"))).toMatchObject({ utc: "2026-03-29 01:00:00", offset: "UTC+03:00", ambiguous: false });
  });

  // CONTROL FALLBACK-OFFSET: the shown text without the offset, or the repeated hour not flagged → red.
  it("shows a time in the repeated fall-back hour with the offset it was resolved to", () => {
    const r = ok(customExpiry("2026-10-25T03:30"));
    expect(r.ambiguous, "FALLBACK-OFFSET: the repeated hour not flagged").toBe(true);
    expect(["UTC+03:00", "UTC+02:00"]).toContain(r.offset);
    // Whatever offset the engine picked, the UTC sent is that offset's instant (V8 picks +03:00).
    const sent = r.offset === "UTC+03:00" ? "2026-10-25 00:30:00" : "2026-10-25 01:30:00";
    expect(r.utc).toBe(sent);
    expect(utcOffsetText(r.instant)).toBe(r.offset);
    const shown = expiryShownText(r);
    expect(shown, "FALLBACK-OFFSET: shown without its offset").toContain(`2026-10-25 03:30 (${r.offset})`);
    expect(shown).toContain("occurs twice");
    expect(ok(customExpiry("2026-10-25T02:59")).ambiguous).toBe(false);
    expect(ok(customExpiry("2026-10-25T04:00"))).toMatchObject({ offset: "UTC+02:00", ambiguous: false });
    expect(expiryShownText(ok(customExpiry("2026-12-01T14:30")))).toBe("2026-12-01 14:30 (UTC+02:00)");
  });

  it("refuses anything but the strict form, and dates that do not exist", () => {
    for (const bad of [
      "",
      "2026-11-21",
      "21.11.2026 14:00",
      "2026-11-21T14:00Z",
      "2026-11-21T14:00+02:00",
      "2026-11-21T14:00:30",
      "2026-11-21T14:00:00.000",
      "2026-11-21T4:00",
      "tomorrow",
    ]) {
      expect(customExpiry(bad).ok, bad).toBe(false);
    }
    for (const bad of ["2026-13-01T00:00", "2026-11-21T24:00", "2026-11-21T23:60", "2026-02-30T10:00", "2026-00-10T10:00"]) {
      expect(customExpiry(bad).ok, bad).toBe(false);
    }
    const feb = customExpiry("2026-02-30T10:00");
    expect(!feb.ok && feb.error).toBe("2026-02-30T10:00 is not a calendar date");
  });

  // The rule the module rests on, pinned in its source: no `new Date(<string>)` and no `Date.parse`.
  it("never parses a date string", () => {
    const src = readFileSync(resolve(process.cwd(), "src/mcp/expiry.ts"), "utf8")
      .replace(/\/\*[\s\S]*?\*\//g, "")
      .replace(/(^|[^:])\/\/.*$/gm, "$1");
    expect(src).not.toMatch(/Date\.parse/);
    for (const m of src.matchAll(/new Date\(([^)]*)\)/g)) {
      expect(m[1], m[0]).not.toMatch(/value|string|\btext\b|["'`]/);
    }
  });
});
