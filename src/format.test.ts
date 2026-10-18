// The view's times (ADR-024 o22.14.6) are pinned under a **non-UTC** zone, so a parse that reads
// SQLite's UTC as local time cannot pass: Europe/Kyiv is UTC+3 in October (EEST). The zone is set in
// `beforeAll` and **restored in `afterAll`**, so it does not leak into the next file of a worker that
// is reused (`--no-isolate`, `isolate: false`; measured: a module-level `process.env.TZ = …` stayed
// "UTC" for the next file there). The first test proves the zone took effect.
import { readdirSync, readFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { formatLaptopUtc } from "./format";

let outerTz: string | undefined;
beforeAll(() => {
  outerTz = process.env.TZ;
  process.env.TZ = "Europe/Kyiv";
});
afterAll(() => {
  // `delete`, not `= undefined` (which would set the string "undefined"): Node re-reads the zone on both.
  if (outerTz === undefined) delete process.env.TZ;
  else process.env.TZ = outerTz;
  // The permanent guard (B2 verifier F3): a restore that is missing or wrong (e.g. `= undefined`,
  // which sets the string "undefined") fails the file here, instead of leaking silently into the
  // next file of a reused worker.
  expect(process.env.TZ).toBe(outerTz);
});

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

/** The code of every `.ts`/`.tsx` file under `dir`, tests excluded, comments stripped. */
function sources(dir: string, out: { file: string; code: string }[] = []) {
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const path = join(dir, entry.name);
    if (entry.isDirectory()) sources(path, out);
    else if (/\.tsx?$/.test(entry.name) && !/\.test\.tsx?$/.test(entry.name)) {
      const code = readFileSync(path, "utf8").replace(/\/\*[\s\S]*?\*\//g, "").replace(/(^|[^:])\/\/.*$/gm, "$1");
      out.push({ file: path, code });
    }
  }
  return out;
}

describe("the stored times are shown through formatLaptopUtc (o22.14.6, source pins)", () => {
  // CONTROL: bring `formatTimestamp` back anywhere in `src` (its definition, or a screen still
  // calling it) → red.
  it("retires formatTimestamp", () => {
    const files = sources(resolve(process.cwd(), "src"));
    expect(files.length).toBeGreaterThan(50);
    expect(files.filter((f) => /\bformatTimestamp\b/.test(f.code)).map((f) => f.file)).toEqual([]);
  });

  // CONTROL: render `{job.created_at}` raw (or `started ${job.started_at}`) in a screen → red.
  // Every use of a stored time in a table or the detail's header is inside `formatLaptopUtc(…)`
  // (a bare `x.started_at ?` test is a condition, not a display).
  it("shows no created_at / started_at / completed_at raw in the screens that list them", () => {
    for (const screen of ["JobsScreen", "JobDetailScreen", "MoleculesScreen", "ReactionsScreen"]) {
      const code = readFileSync(resolve(process.cwd(), `src/screens/${screen}.tsx`), "utf8")
        .replace(/\/\*[\s\S]*?\*\//g, "")
        .replace(/(^|[^:])\/\/.*$/gm, "$1");
      expect(code, screen).toMatch(/formatLaptopUtc\(/);
      const unwrapped = code.replace(/formatLaptopUtc\([^)]*\)/g, "");
      const raw = [...unwrapped.matchAll(/\.(created|started|completed)_at\b(?!\s*\?)/g)].map((m) => m[0]);
      expect(raw, screen).toEqual([]);
    }
  });
});
