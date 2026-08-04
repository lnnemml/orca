import { describe, it, expect } from "vitest";

import { resolveSelection, stripTrailingArg, isTokenChar } from "./selection-lookup";

/** Build a single-line selection of the first occurrence of `needle` in `text`. */
function selectNeedle(text: string, needle: string) {
  const idx = text.indexOf(needle);
  if (idx < 0) throw new Error(`needle not found: ${needle}`);
  // locate the line + column of idx
  const pre = text.slice(0, idx);
  const line = pre.split("\n").length - 1;
  const col = idx - (pre.lastIndexOf("\n") + 1);
  return resolveSelection(text, line, col, line, col + needle.length, needle);
}

describe("resolveSelection — the measured normalization rule", () => {
  it("a full simple keyword on the ! line → hit", () => {
    const r = selectNeedle("! r2SCAN-3c def2/J", "r2SCAN-3c");
    expect(r.kind).toBe("hit");
    if (r.kind === "hit") expect(r.match.kind).toBe("simple");
  });

  it("a keyword with a slash is one token (wordPattern would split it) → hit", () => {
    const r = selectNeedle("! r2SCAN-3c def2/J", "def2/J");
    expect(r.kind).toBe("hit");
  });

  it("a %block token → hit (block)", () => {
    const r = selectNeedle("%pal\n  nprocs 4\nend", "%pal");
    expect(r.kind).toBe("hit");
    if (r.kind === "hit") expect(r.match.kind).toBe("block");
  });

  it("a block-option inside its block → hit (block-option of that block)", () => {
    const r = selectNeedle("%scf\n  MaxIter 200\nend", "MaxIter");
    expect(r.kind).toBe("hit");
    if (r.kind === "hit") {
      expect(r.match.kind).toBe("block-option");
      expect(r.match.block).toBe("%scf");
    }
  });

  // ── Task 3: the argument rule in the selection world ──────────────────────
  it("CPCM(water) selected WHOLE → hit for CPCM (the arg is stripped, whole-first)", () => {
    const r = selectNeedle("! CPCM(water)", "CPCM(water)");
    expect(r.kind).toBe("hit");
    if (r.kind === "hit") expect(r.match.word.toLowerCase()).toBe("cpcm");
  });

  it("`water` selected INSIDE the parens → silence (argument token, miss)", () => {
    const r = selectNeedle("! CPCM(water)", "water");
    expect(r.kind).toBe("miss"); // not malformed: boundaries are `(`/`)`, the arg rule silences it
  });

  // ── the boundary guard: a mid-token cut → malformed (a format hint) ────────
  it("`Tight` cut from `TightSCF` → malformed (not a confident answer about Tight)", () => {
    const r = selectNeedle("! TightSCF", "Tight");
    expect(r.kind).toBe("malformed");
  });

  it("`Opt` cut from `OptTS` (same-type substring) → malformed", () => {
    const r = selectNeedle("! OptTS", "Opt");
    expect(r.kind).toBe("malformed");
  });

  it("a selection with internal whitespace (`Opt Freq`) → malformed (0 space keys)", () => {
    const r = selectNeedle("! Opt Freq", "Opt Freq");
    expect(r.kind).toBe("malformed");
  });

  it("a selection spanning two lines → malformed", () => {
    const text = "! Opt\n! Freq";
    const r = resolveSelection(text, 0, 2, 1, 6, "Opt\n! Freq");
    expect(r.kind).toBe("malformed");
  });

  it("a trailing space grabbed with the token is trimmed, then it resolves", () => {
    // "! Opt Freq": select "Opt " (cols [2,6), with the trailing space). Trimming shifts
    // the span onto `Opt`; its source neighbours are spaces → guard passes → hit.
    const r = resolveSelection("! Opt Freq", 0, 2, 0, 6, "Opt ");
    expect(r.kind).toBe("hit");
  });

  it("a well-formed word not in the map → silence (miss), not a hint", () => {
    const r = selectNeedle("! NotARealKeyword", "NotARealKeyword");
    expect(r.kind).toBe("miss");
  });

  it("an xyz coordinate token (no block, not the ! line) → silence", () => {
    const r = selectNeedle("* xyz 0 1\n C 0.0 0.0 0.0\n*", "C");
    expect(r.kind).toBe("miss");
  });
});

describe("selection-normalization primitives (shared with the corpus gate)", () => {
  it("stripTrailingArg removes one trailing balanced (...)", () => {
    expect(stripTrailingArg("CPCM(water)")).toBe("CPCM");
    expect(stripTrailingArg("DLPNO-CCSD(T)")).toBe("DLPNO-CCSD");
    expect(stripTrailingArg("def2-SVP")).toBe("def2-SVP"); // nothing to strip
  });
  it("isTokenChar: slash/dot/dash/% continue a token; space/paren do not", () => {
    for (const c of ["a", "9", "/", ".", "-", "%", "_"]) expect(isTokenChar(c)).toBe(true);
    for (const c of [" ", "(", ")", "", "#"]) expect(isTokenChar(c)).toBe(false);
  });
});
