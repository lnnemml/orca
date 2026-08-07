/**
 * Shared `%geom` block locator (Phase 4.5 Stage A1). `Constraints` and `Scan` are
 * BOTH `%geom` sub-blocks (`wiki/orca/constraints.md`, `wiki/orca/scan.md`), so the
 * one input `%geom` must be found and composed into by both `injectConstraints`
 * (constraints.ts) and `injectScan` (scan.ts) — never a second `%geom`, which ORCA
 * would silently reduce to one. This module is that single depth-tracking parser,
 * lifted out of constraints.ts so the two injectors cannot drift.
 *
 * Why the sub-block set matters: a `Scan … end` and a `Constraints … end` each open
 * and close a nested block. A locator that knew only `Constraints` (the pre-scan
 * shape) would treat a `Scan` block's `end` as closing `%geom` and mis-locate the
 * whole thing. So depth is tracked over the full recognised sub-block set.
 */

export interface Tok {
  t: string;
  start: number;
  end: number;
}

/** Whitespace-delimited tokens with their char offsets. */
export function scanTokens(s: string): Tok[] {
  const re = /\S+/g;
  const out: Tok[] = [];
  let m: RegExpExecArray | null;
  while ((m = re.exec(s))) out.push({ t: m[0], start: m.index, end: m.index + m[0].length });
  return out;
}

/**
 * The `%geom` sub-blocks OrcaStudio owns; each opens a nested `… end`. Both are
 * order-bearing, 0-based-index blocks (constraints.ts, scan.ts). A keyword NOT in
 * this set is treated as a plain `%geom` setting (e.g. `maxiter 200` — no nested
 * `end`), the same limited model the constraints-only locator had before scan.
 */
const SUBBLOCK_KEYWORDS = new Set(["constraints", "scan"]);

export interface GeomLocation {
  geomOpen: Tok;
  /** First occurrence of each recognised sub-block's `Keyword … end` char span,
   * keyed by the lowercase keyword. */
  subBlocks: Map<string, { start: number; end: number }>;
}

/**
 * Locate the `%geom` block by tracking block depth (each recognised sub-block
 * opens a level; every `end` closes one). Returns the `%geom` open token and the
 * char span of each recognised sub-block (from its keyword to its closing `end`).
 * Handles both inline (`%geom Constraints`) and separate-line forms. `null` if
 * there is no `%geom` or it never closes.
 */
export function locateGeom(text: string): GeomLocation | null {
  const toks = scanTokens(text);
  const gi = toks.findIndex((t) => t.t.toLowerCase() === "%geom");
  if (gi < 0) return null;

  let depth = 1; // %geom consumed
  const open: { name: string; start: number; depth: number }[] = [];
  const subBlocks = new Map<string, { start: number; end: number }>();

  for (let i = gi + 1; i < toks.length; i++) {
    const w = toks[i].t.toLowerCase();
    if (SUBBLOCK_KEYWORDS.has(w)) {
      open.push({ name: w, start: toks[i].start, depth });
      depth++;
    } else if (w === "end") {
      depth--;
      const top = open[open.length - 1];
      if (top && top.depth === depth) {
        open.pop();
        // Keep the first occurrence of each keyword (mirrors the old cStart guard).
        if (!subBlocks.has(top.name)) {
          subBlocks.set(top.name, { start: top.start, end: toks[i].end });
        }
      }
      if (depth === 0) return { geomOpen: toks[gi], subBlocks };
    }
  }
  return null;
}

/** The whitespace before `pos` on its line (the line's indent when `pos` is the
 * first non-space char). Shared by the sub-block injectors so a replace keeps the
 * existing leading whitespace. */
export function leadingIndent(text: string, pos: number): string {
  const lineStart = text.lastIndexOf("\n", pos - 1) + 1;
  return text.slice(lineStart, pos).match(/^\s*/)![0];
}
