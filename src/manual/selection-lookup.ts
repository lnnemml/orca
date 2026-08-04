//! The SELECTION consumer of keywords.json (unit 4.13) — the pure resolver behind the
//! editor's selection-triggered lookup, the successor to the hover. A hover received one
//! `wordPattern` token (grammar-bounded); a selection can be ANYTHING, so the input is
//! normalized by a MEASURED rule (`wiki/orca/manual-sources.md` "Selection normalization"),
//! not a guessed one — a plausible answer to an imprecise selection is worse than silence.
//!
//! The rule, in order (each step measured):
//!   1. multi-line or internal-whitespace selection → MALFORMED (0 keys contain a space).
//!   2. BOUNDARY GUARD — the selection must not cut a token: the source chars just OUTSIDE
//!      the (trimmed) span must not continue a token. This IS the rule, not a UX add-on:
//!      16 simple keys are substrings of a longer simple key (`opt`⊂`optts`, `ri`⊂`ri-jk`),
//!      so a mid-token cut would otherwise give a confident answer about the NEIGHBOUR, and
//!      the type qualifier is powerless there (both simple). Parens are EXCLUDED from the
//!      class (0 paren keys; `(` is an argument delimiter) — so `CPCM`-before-`(` is not a
//!      cut, and `water`-inside-`()` is handled by the argument rule, not the guard.
//!   3. WHOLE-first exact lookup (qualified by position), then strip one trailing `(…)` and
//!      retry — so `SV(P)` (were it ever a key) is kept, and `CPCM(water)` → `CPCM`.
//!   4. the position qualifier (`hoverContext`, unchanged): `!`-line → simple, `%name` →
//!      block, inside a block → block-option; an argument token → silence.
//!
//! A malformed selection is a CORRECTABLE user action (a format hint); a qualified miss is
//! the boundary of our data (silence). The two are distinct results, not one empty outcome.

import { resolveHover, type HoverMatch } from "./keyword-lookup";

/** Token-continuation class — letters/digits/_ plus the chars real keys carry: `%` (block
 *  prefix), `/` (`def2/J`), `.` (`basename.xyz`), `-` (`def2-SVP`). NOT parens, NOT space.
 *  Exported so the corpus gate measures the SAME class the resolver uses (no drift). */
const WORD = /[\w%/.-]/;
export const isTokenChar = (c: string): boolean => c !== "" && WORD.test(c);

/** Strip ONE trailing balanced `(...)` argument group: `CPCM(water)` → `CPCM`. */
export function stripTrailingArg(s: string): string {
  return s.replace(/\([^()]*\)\s*$/, "").trim();
}

export type SelectionResult =
  | { kind: "hit"; match: HoverMatch } // an exact, type/block-aware keyword record
  | { kind: "malformed" } // multi-line / internal space / mid-token cut → a format hint
  | { kind: "miss" }; // well-formed but not in the map, or an argument token → silence

/**
 * Resolve a single-line editor selection. `text` is the whole buffer; the selection is
 * `[startCol, endCol)` on `line` (all 0-based); `selected` is the exact selected substring
 * (may carry leading/trailing whitespace, which is trimmed and the span shifted with it).
 */
export function resolveSelection(
  text: string,
  line: number,
  startCol: number,
  endLine: number,
  endCol: number,
  selected: string,
): SelectionResult {
  if (line !== endLine) return { kind: "malformed" }; // a selection across lines
  const trimmed = selected.trim();
  if (!trimmed) return { kind: "miss" };
  if (/\s/.test(trimmed)) return { kind: "malformed" }; // internal whitespace (0 space keys)

  // Shift the span past any leading/trailing whitespace the user included, so the guard and
  // the lookup both act on the trimmed content.
  const lead = selected.length - selected.trimStart().length;
  const trail = selected.length - selected.trimEnd().length;
  const start = startCol + lead;
  const end = endCol - trail;

  // Boundary guard: neighbours in the SOURCE must not continue a token (else it is a cut).
  const lineText = text.split("\n")[line] ?? "";
  const before = lineText[start - 1] ?? "";
  const after = lineText[end] ?? "";
  if ((before && WORD.test(before)) || (after && WORD.test(after))) return { kind: "malformed" };

  // Whole-first exact lookup (position-qualified), then strip a trailing `(…)` and retry.
  const whole = resolveHover(text, line, start, trimmed);
  if (whole) return { kind: "hit", match: whole };
  const stripped = stripTrailingArg(trimmed);
  if (stripped && stripped !== trimmed) {
    const hit = resolveHover(text, line, start, stripped);
    if (hit) return { kind: "hit", match: hit };
  }
  return { kind: "miss" };
}
