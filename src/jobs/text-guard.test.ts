import { readdirSync, readFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { describe, expect, it } from "vitest";

/**
 * The user-facing text guard, TypeScript half (ADR-024 o22.14.11, last control): no code in `src`
 * outside the tests — string literals and JSX text alike, comments stripped — says "withdraw" (the
 * command is gone, o22.7/o22.14.7), names a later unit as "unit 5.4" without its letter (o22.1), or
 * tells the user "Cancel sweeps again" (the button is Retry cancel, o22.14.2). Stricter than "string
 * literals only": JSX text is user-facing too. The Rust half is `src-tauri/src/text_guard_tests.rs`.
 */

/** Every `.ts`/`.tsx` file under `dir`, tests excluded. */
function sources(dir: string, out: string[] = []): string[] {
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const path = join(dir, entry.name);
    if (entry.isDirectory()) sources(path, out);
    else if (/\.tsx?$/.test(entry.name) && !/\.test\.tsx?$/.test(entry.name)) out.push(path);
  }
  return out;
}

/** The code without its comments: block comments, then line comments (not a `://` in a URL). */
function withoutComments(src: string): string {
  return src.replace(/\/\*[\s\S]*?\*\//g, "").replace(/(^|[^:])\/\/.*$/gm, "$1");
}

/** What a piece of code must not say, or null. */
function forbidden(code: string): string | null {
  if (/withdraw/i.test(code)) return "withdraw";
  if (/unit 5\.4(?![a-c])/.test(code)) return "unit 5.4 without its letter";
  if (/Cancel sweeps again/.test(code)) return "Cancel sweeps again";
  return null;
}

describe("the user-facing text guard (o22.14.11)", () => {
  // CONTROLS: put a "Withdraw" button label back into a screen, or "handled in unit 5.4" into a
  // string in `src/jobs` → red.
  it("finds no withdraw, unlettered unit 5.4 or 'Cancel sweeps again' in the app's code", () => {
    // vitest roots `process.cwd()` at the repo (as `render.corpus.test.ts` relies on).
    const files = sources(resolve(process.cwd(), "src"));
    expect(files.length).toBeGreaterThan(50);
    const hits: string[] = [];
    for (const file of files) {
      withoutComments(readFileSync(file, "utf8"))
        .split("\n")
        .forEach((line, i) => {
          const why = forbidden(line);
          if (why) hits.push(`${file}:${i + 1}: ${why}: ${line.trim()}`);
        });
    }
    expect(hits).toEqual([]);
  });

  it("strips comments and keeps a lettered unit", () => {
    const code = withoutComments('// withdraw\n/* withdraw */ const a = "Withdraw job"; const u = "http://x"; // unit 5.4\n');
    expect(code).toContain('"Withdraw job"');
    expect(code).toContain('"http://x"');
    expect(code).not.toContain("unit 5.4");
    expect(forbidden(code)).toBe("withdraw");
    expect(forbidden("handled in unit 5.4b")).toBeNull();
    expect(forbidden("handled in unit 5.4.")).toBe("unit 5.4 without its letter");
    expect(forbidden("automatic sweeps stopped — Retry cancel sweeps again.")).toBeNull();
    expect(forbidden("automatic sweeps stopped — Cancel sweeps again.")).toBe("Cancel sweeps again");
  });
});
