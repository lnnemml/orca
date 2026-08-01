//! Which `%block` encloses a cursor in an ORCA input — a PURE function over the input
//! text, testable without Monaco (the Monarch tokenizer is stateless: one `root`, no
//! @push/@pop, so it colours `%scf`/`end` but has no notion of "inside %scf"; the
//! qualifier the hover needs is not in the tokenizer state, so we derive it here).
//!
//! The hazard (why this is a rule #9/#10 unit, not a convenience): a scanner that
//! decides the cursor is in `%scf` when it is in `%casscf` makes the hover show the
//! WRONG section, CONFIDENTLY — a qualified lookup finds a record either way. A wrong
//! answer about `MaxIter` in CASSCF looks exactly like the right one. So the scanner is
//! built to be **conservative: it returns `null` on any ambiguity rather than guess.**
//!
//! The measured trap (`wiki/orca/input-syntax.md`): 6.1 % of `%name` openers are
//! single-line directives with **no `end`** (`%maxcore 3000`, `%moinp "x.gbw"`,
//! `%base "..."`). A naive "every `%name` opens a block" scanner leaves everything
//! after them "inside %maxcore" forever. We classify an opener as a block ONLY when a
//! matching `end` follows it (forward scan); otherwise it is a directive and pushes
//! nothing. Bare-word sub-blocks (`%geom … constraints … end end`) exist; their `end`
//! pops the enclosing `%block` early, which yields a conservative `null` (never a wrong
//! block) for tokens after the sub-block.

const OPENER = /^\s*%([A-Za-z_]\w*)/; // a %block at line start
const PCT = /%[A-Za-z_]\w*/g; // any %block token
const END = /\bend\b/gi; // a block terminator (word-bounded)

/** Strip a `#` line-comment and blank the contents of `"…"` strings (keeping length),
 *  so a `%` or `end` inside a comment or a filename can't be read as structure. */
function mask(line: string): string {
  return line.replace(/#.*$/, "").replace(/"[^"]*"/g, (m) => " ".repeat(m.length));
}

function count(s: string, re: RegExp): number {
  return s.match(re)?.length ?? 0;
}

/** Does a matching `end` for an opener on `masked[from]` (after column `after`) exist
 *  before a hard boundary (`!` line, `*` coord line, or EOF)? Depth counts nested
 *  `%block` opens; every `end` decrements. This separates a real multi-line block from
 *  a no-`end` directive — the single most important distinction here. */
function hasMatchingEnd(masked: string[], from: number, after: number): boolean {
  const rest = masked[from].slice(after);
  let depth = 1 + count(rest, PCT) - count(rest, END);
  if (depth <= 0) return true; // closed on the same line (`%pal nprocs 4 end`)
  for (let i = from + 1; i < masked.length; i++) {
    if (/^\s*[!*]/.test(masked[i])) return false; // top-level boundary hit first
    depth += count(masked[i], PCT) - count(masked[i], END);
    if (depth <= 0) return true;
  }
  return false; // ran off the end with the block still open → treat as directive
}

/**
 * The nearest enclosing `%block` name (e.g. `"%scf"`) for the token on line `line`
 * (0-based), or `null` when the cursor is at top level, inside the coordinate block, or
 * anywhere the structure is ambiguous. Only lines ABOVE `line` establish the context
 * (a `%name` on the cursor line is the block token itself, not its interior).
 */
export function enclosingBlock(text: string, line: number): string | null {
  const raw = text.split("\n");
  const masked = raw.map(mask);
  const stack: string[] = [];
  let coordOpen = false;

  for (let i = 0; i < line && i < masked.length; i++) {
    const m = masked[i];

    // Coordinate block: `* xyz 0 1` … a lone `*` closes it. Its interior is not in any
    // %block. External forms (`* xyzfile …`) are self-contained (no toggle).
    const star = m.match(/^\s*\*\s*(\S*)/);
    if (star) {
      if (coordOpen) coordOpen = false;
      else if (/^(xyz|int|gzmt)$/.test(star[1].toLowerCase())) coordOpen = true;
      continue;
    }
    if (coordOpen) continue;
    if (/^\s*!/.test(m)) continue; // simple-input line — top level

    // Push an opener only when it is a real block (a matching `end` follows); a no-`end`
    // directive pushes nothing. Then any `end`(s) on the line pop — so a self-closing
    // `%pal … end` nets to zero, and a bare-block `end` pops conservatively.
    const open = m.match(OPENER);
    if (open && hasMatchingEnd(masked, i, open[0].length)) {
      stack.push("%" + open[1].toLowerCase());
    }
    for (let ends = count(m, END); ends > 0 && stack.length; ends--) stack.pop();
  }

  return stack.length ? stack[stack.length - 1] : null;
}
