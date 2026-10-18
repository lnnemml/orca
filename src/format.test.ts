// The view's times (ADR-024 o22.14.6) are pinned under a **non-UTC** zone, so a parse that reads
// SQLite's UTC as local time cannot pass: Europe/Kyiv is UTC+3 in October (EEST). Set before any
// `Date` is made; the first test proves the zone took effect.
process.env.TZ = "Europe/Kyiv";

import { describe, expect, it } from "vitest";
import { formatLaptopUtc } from "./format";

describe("formatLaptopUtc (o22.14.6)", () => {
  it("runs under a zone that is not UTC", () => {
    expect(new Date("2026-10-18T06:18:01Z").getHours()).toBe(9);
  });

  // CONTROLS: parse with `new Date(ts)` on the bare form, or `ts.replace(" ", "T")` without the
  // `Z` → red (both read the UTC value as Kyiv time: 06:18:01).
  it("reads SQLite's datetime('now') as UTC and shows the laptop's local time", () => {
    // The probe of 2026-10-18: SQLite said 06:18:01 while the laptop read 09:18:01 EEST.
    expect(formatLaptopUtc("2026-10-18 06:18:01")).toBe("2026-10-18 09:18:01");
    // Across midnight and a month end, and in winter time (EET, UTC+2).
    expect(formatLaptopUtc("2026-10-31 22:30:00")).toBe("2026-11-01 00:30:00");
    expect(formatLaptopUtc("2026-01-15 23:00:59")).toBe("2026-01-16 01:00:59");
  });

  it("shows anything else raw, never 'Invalid Date'", () => {
    for (const raw of ["", "2026-10-06", "2026-13-45 99:99:99", "not a time", "2026-10-18T06:18:01Z"]) {
      expect(formatLaptopUtc(raw)).toBe(raw);
    }
  });
});
