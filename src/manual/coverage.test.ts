import { describe, it, expect } from "vitest";

import inventory from "./keyword-inventory.json";
import { lookup, resolveHover, isArgumentToken, type KeywordType } from "./keyword-lookup";

// The argument rule: a token inside `(…)` after a keyword is an ARGUMENT, not a keyword.
// This is the regression that proves the rule is a rule, not luck — `water` IS in the
// map (as a %frag block-option), yet a hover on `CPCM(water)`'s `water` must be silent.
describe("argument tokens on the ! line get no hover (even when they're in the map)", () => {
  it("`water` IS in the map (so silence must come from the RULE, not absence)", () => {
    expect(lookup("water", "block-option", "%frag").length).toBeGreaterThan(0);
  });
  it("`! CPCM(water)` — cursor on `water` → no hover; cursor on `CPCM` → resolves", () => {
    const line = "! CPCM(water)\n"; // `water` starts at column 7, `CPCM` at 2
    expect(resolveHover(line, 0, 7, "water")).toBeNull(); // argument → silence
    expect(resolveHover(line, 0, 2, "CPCM")).not.toBeNull(); // the keyword → resolves
  });
  it("isArgumentToken is a pure line check", () => {
    expect(isArgumentToken("! CPCM(water)", 7)).toBe(true);
    expect(isArgumentToken("! CPCM(water)", 2)).toBe(false);
    expect(isArgumentToken("! SV(P) def2-TZVP", 5)).toBe(true); // P inside SV(P)
    expect(isArgumentToken("! SV(P) def2-TZVP", 8)).toBe(false); // def2-TZVP after the )
  });
});

// The coverage gate over the EXPLICIT inventory (keyword-inventory.json) — the ONE
// home for the expectation set, read by this gate and the Rust generator. The earlier
// gates fixed the FORM of the question (type, not string: 46/46 → 44/46). This fixes
// the POPULATION: the set is what the app emits AND what the author TYPES (domain
// guards, the reaction workflow chain), each entry justified by a named source.
//
// A word WITHOUT a `gap` tag is HARD — it must resolve or this fails. A `gap` word is a
// KNOWN, classified hole (a/b/c/d) — reported, not a failure; if it starts resolving,
// the tag should be removed.

interface Entry {
  keyword: string;
  expect: KeywordType;
  source: string;
  block?: string;
  gap?: "a" | "b" | "c" | "d";
}
const ENTRIES = (inventory as { keywords: Entry[] }).keywords;

const resolves = (e: Entry) => lookup(e.keyword, e.expect, e.block ?? null).length > 0;

describe("coverage over the named inventory (population + form both honest)", () => {
  it("reports the honest number and every gap by closer", () => {
    const resolved = ENTRIES.filter(resolves);
    const gaps = ENTRIES.filter((e) => e.gap);
    const byCat = (c: string) => gaps.filter((e) => e.gap === c).map((e) => `${e.keyword} [${e.expect}]`);
    // eslint-disable-next-line no-console
    console.log(
      `\n[inventory coverage] ${resolved.length}/${ENTRIES.length} resolve (type-aware)` +
        `\n  gaps by closer:` +
        `\n    (a) {numref} layer:      ${byCat("a").join(", ") || "—"}` +
        `\n    (b) curated (prose):     ${byCat("b").join(", ") || "—"}` +
        `\n    (c) second simple form:  ${byCat("c").join(", ") || "—"}` +
        `\n    (d) not in corpus:       ${byCat("d").join(", ") || "—"}`,
    );
    expect(resolved.length).toBeLessThan(ENTRIES.length); // there ARE gaps, and that's the point
  });

  it("HARD post-condition: every non-gap inventory word resolves (a regression fails here)", () => {
    const brokenHard = ENTRIES.filter((e) => !e.gap && !resolves(e)).map(
      (e) => `${e.keyword} [${e.expect}${e.block ? " " + e.block : ""}] (${e.source})`,
    );
    expect(brokenHard).toEqual([]);
  });

  it("each declared gap really is a gap (a resolving gap means: remove the tag)", () => {
    const nowResolving = ENTRIES.filter((e) => e.gap && resolves(e)).map((e) => e.keyword);
    expect(nowResolving).toEqual([]);
  });

  it("a block-option resolves to its OWNING block (MaxIter → %scf, not the 15 others)", () => {
    const recs = lookup("MaxIter", "block-option", "%scf");
    expect(recs.length).toBe(1);
    expect(recs[0].block).toBe("%scf");
  });
});
