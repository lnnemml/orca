//! Minimal, verifiable renderer for a manual section body (MyST Markdown).
//!
//! The hazard is silent loss at the display layer: a naive Markdown renderer eats what
//! it does not understand WITHOUT error — a directive vanishes, `$\omega$B97M` math turns
//! to underscore-mush, an ORCA input loses its indentation. The reader never sees it. So
//! every source char belongs to EXACTLY ONE of three categories (measured 4.11,
//! `wiki/orca/manual-sources.md` "MyST construct census"):
//!
//!   (1) RECOGNIZED & TRANSFORMED — the transform is declared and tested per construct:
//!       inline code (` ``…`` ` → the content, quotes gone), math (`$…$`/`$$…$$` → KaTeX),
//!       cross-references (`{ref}`/`{numref}` roles + `[..](sec:/tab:)` links → a link,
//!       but ONLY when the anchor map resolves; an unresolved target stays category 2).
//!   (2) UNRECOGNIZED — shown verbatim, byte-for-byte. The preservation test lives HERE,
//!       unweakened. Code fences, pipe tables, prose, and every VISIBLE directive
//!       (`{note}`/`{warning}`/`{table}`/…) are category 2.
//!   (3) RECOGNIZED & DELIBERATELY HIDDEN — a NAMED whitelist only. The census refuted
//!       "hide anything that looks like a directive": 13.6 % of the corpus sits under a
//!       directive fence but almost all of it is VISIBLE content. FOUR positions, each
//!       measured invisible in the published Furo HTML (4.15, source-vs-HTML gate): three
//!       directives — `{index}`, `{tabularcolumns}`, `{raw}` WITH `latex` (`isHiddenDirective`)
//!       — plus the **MyST anchor label line** `(name)=` (`isAnchorLabelLine`; 1438×,
//!       invisible 184/186 in the sample — the 2 were substring noise). The label is a
//!       LINE-level construct, not a directive; it was missed by the 4.12 census because
//!       that census listed construct TYPES (directives/math/code/xrefs) and a label is none.
//!
//! We do NOT parse MyST. We recognize exactly these constructs by name/delimiter and emit
//! everything else verbatim. `render.test.ts` splits the preservation test three ways.

/** A block of the body. Directives are recognized so they can be hidden / flagged / kept. */
export type Block =
  | { kind: "fence"; text: string } // code fence, INCLUDING the ``` lines — monospace, verbatim
  | { kind: "table"; text: string } // a run of `|`-rows — monospace so columns align
  | { kind: "label"; text: string } // a MyST anchor label line `(name)=` — HIDDEN (category 3)
  | { kind: "prose"; text: string } // verbatim text carrying inline constructs (below)
  | {
      kind: "directive";
      name: string; // the `{name}` — e.g. index, note, literalinclude, raw
      arg: string; // the rest of the opener line after `}` (a path, a title, `latex`, …)
      open: string; // the opener line, verbatim
      inner: string; // the body between opener and closer (may be empty)
      close: string; // the closer line, verbatim ("" if the fence was never closed)
    };

/** One inline piece of a prose block. `raw` is the exact source, kept so an unresolved
 *  cross-reference can fall back to verbatim (category 2) instead of a dead click. */
export type Inline =
  | { kind: "text"; text: string }
  | { kind: "code"; text: string } // ``…`` → the content (backticks dropped)
  | { kind: "math"; tex: string; display: boolean } // $…$ / $$…$$ → KaTeX
  | { kind: "xref"; text: string; label: string; raw: string } // {ref}/{numref}/[..](sec:/tab:)
  | { kind: "cite"; keys: string; raw: string } // {cite}`keys` → [keys] (see below)
  | { kind: "eq"; label: string; raw: string }; // {eq}`label` → [label] (same fix as {cite})

// --- Fence handling (mirrors the Rust sectioner's fence rule, `sections.rs`) ---

/** If `line` opens a fenced block, return `[char, runLength, infoAfterRun]`. Backtick
 *  CODE fences (``` ```orca ```, info `orca`) AND colon/backtick MyST directives
 *  (`:::{table}`/``` ```{index} ```, info `{table}`), any run ≥ 3, leading space tolerated. */
function fenceOpen(line: string): [string, number, string] | null {
  const t = line.replace(/^\s+/, "");
  const c = t[0];
  if (c !== "`" && c !== ":") return null;
  let run = 0;
  while (run < t.length && t[run] === c) run++;
  if (run < 3) return null;
  return [c, run, t.slice(run)];
}

/** A closing fence line: only the fence char, run ≥ the opening run. */
function isFenceClose(line: string, ch: string, len: number): boolean {
  const t = line.trim();
  return t.length >= len && [...t].every((x) => x === ch);
}

const PIPE_ROW = /^\s*\|/; // a Markdown pipe-table row (measured: 110 sections carry one)

/** A MyST anchor label line — a WHOLE trimmed line `(name)=` with no parens inside the
 *  name. This is EXACTLY the sectioner's `parse_label` rule (`src-tauri/src/manual/
 *  sections.rs`, rule #9 — the same construction that built the anchor map), so the render
 *  hides precisely what the index treats as a label, no wider. The whole-line requirement
 *  is the checkable boundary: a `(x)= y` mid-line, or `(x)=` INSIDE a ` ```orca ` fence
 *  (handled as a fence block before this is ever reached), is NOT a label and is kept. */
const ANCHOR_LABEL = /^\([^()]+\)=$/;
export function isAnchorLabelLine(line: string): boolean {
  return ANCHOR_LABEL.test(line.trim());
}

/**
 * Split a body into blocks. Every source line lands in exactly one block, verbatim (the
 * loss-free precondition, checked category-by-category in `render.test.ts`). A directive
 * fence (`` ```{name} `` or `:::{name}`) becomes a `directive` block spanning opener→closer
 * so the renderer can hide / flag / keep it by NAME; a bare code fence stays a `fence`
 * block; a `|`-row run a `table` block; everything else prose. An unterminated fence runs
 * to end of body and is still one block (content shown, not swallowed).
 */
export function parseManualBody(body: string): Block[] {
  const lines = body.split("\n");
  const blocks: Block[] = [];
  let prose: string[] = [];
  const flush = () => {
    if (prose.length) {
      blocks.push({ kind: "prose", text: prose.join("\n") });
      prose = [];
    }
  };
  for (let i = 0; i < lines.length; ) {
    const open = fenceOpen(lines[i]);
    if (open) {
      const [ch, run, info] = open;
      const infoTrim = info.trim();
      if (infoTrim.startsWith("{")) {
        // A MyST directive: `{name}` then the rest of the line is the arg.
        flush();
        const name = infoTrim.slice(1, infoTrim.indexOf("}") >= 0 ? infoTrim.indexOf("}") : infoTrim.length).trim();
        const arg = infoTrim.indexOf("}") >= 0 ? infoTrim.slice(infoTrim.indexOf("}") + 1).trim() : "";
        const openLine = lines[i];
        i++;
        const innerBuf: string[] = [];
        let closeLine = "";
        while (i < lines.length) {
          if (isFenceClose(lines[i], ch, run)) {
            closeLine = lines[i];
            i++;
            break;
          }
          innerBuf.push(lines[i]);
          i++;
        }
        blocks.push({
          kind: "directive",
          name,
          arg,
          open: openLine,
          inner: innerBuf.join("\n"),
          close: closeLine,
        });
        continue;
      }
      if (ch === "`") {
        // A bare-language code fence — monospace, verbatim (indentation preserved).
        flush();
        const buf = [lines[i]];
        i++;
        while (i < lines.length) {
          buf.push(lines[i]);
          const close = isFenceClose(lines[i], ch, run);
          i++;
          if (close) break;
        }
        blocks.push({ kind: "fence", text: buf.join("\n") });
        continue;
      }
      // A colon run with no `{name}` (a stray, or a closer with no opener) — prose.
      prose.push(lines[i]);
      i++;
    } else if (PIPE_ROW.test(lines[i])) {
      // A run of pipe-table rows → monospace so columns line up. No table PARSING, just
      // a font choice; content is still verbatim, so preservation is untouched.
      flush();
      const buf: string[] = [];
      while (i < lines.length && PIPE_ROW.test(lines[i])) {
        buf.push(lines[i]);
        i++;
      }
      blocks.push({ kind: "table", text: buf.join("\n") });
    } else if (isAnchorLabelLine(lines[i])) {
      // A MyST anchor label line `(name)=` — invisible in the real manual (category 3).
      // Reached only OUTSIDE a fence (fences are consumed above), so a `(x)=` inside a
      // ` ```orca ` block stays code, not a hidden label.
      flush();
      blocks.push({ kind: "label", text: lines[i] });
      i++;
    } else {
      prose.push(lines[i]);
      i++;
    }
  }
  flush();
  return blocks;
}

// --- Inline constructs (category 1) inside a prose block --------------------

// Ordered alternation, scanned left-to-right. A role (`{name}`arg``) is matched BEFORE
// inline code so a role's backtick argument is never mistaken for code (measured: 4.11
// says role-args are NOT inline code). Only `{ref}`/`{numref}` and `[..](sec:/tab:)` are
// cross-references; every other role/link stays verbatim (its whole match → a text token).
const INLINE_RE = new RegExp(
  [
    "\\$\\$[\\s\\S]+?\\$\\$", // 1 display math
    "\\$[^$\\n]+?\\$", // 2 inline math
    "\\{[A-Za-z][A-Za-z0-9:+_-]*\\}`[^`]*`", // 3 a MyST role: {name}`arg`
    "\\[[^\\]]*\\]\\((?:sec|tab):[^)]+\\)", // 4 cross-ref link [text](sec:… | tab:…)
    "`[^`]+`", // 5 inline code
  ].join("|"),
  "g",
);

const ROLE_RE = /^\{([A-Za-z][A-Za-z0-9:+_-]*)\}`([^`]*)`$/;
const LINK_RE = /^\[([^\]]*)\]\(((?:sec|tab):[^)]+)\)$/;

/** Tokenize a prose block into inline pieces. Pure — the transform is declared here and
 *  tested in `render.test.ts`; PageView maps tokens to React (code→`<code>`, math→KaTeX,
 *  a resolved xref→`<a>`, an unresolved xref→its verbatim source). */
export function tokenizeInline(text: string): Inline[] {
  const out: Inline[] = [];
  let last = 0;
  for (const m of text.matchAll(INLINE_RE)) {
    const start = m.index ?? 0;
    if (start > last) out.push({ kind: "text", text: text.slice(last, start) });
    const s = m[0];
    if (s.startsWith("$$")) {
      out.push({ kind: "math", tex: s.slice(2, -2), display: true });
    } else if (s.startsWith("$")) {
      out.push({ kind: "math", tex: s.slice(1, -1), display: false });
    } else if (s.startsWith("{")) {
      const role = ROLE_RE.exec(s);
      if (role && (role[1] === "ref" || role[1] === "numref")) {
        out.push({ kind: "xref", text: role[2], label: role[2], raw: s });
      } else if (role && role[1].toLowerCase().startsWith("cite")) {
        // {cite}/{cite:t}/{cite:p}`keys` → `[keys]` (category 1). In Sphinx a citation is
        // VISIBLE (rendered `[n]`), so it must NOT be hidden — the census-era verbatim was a
        // DISTORTION (the loudest construct, 1002×, shown as raw syntax). We keep the KEYS,
        // not a number: `[n]` is order-dependent and re-flows on any reprint, while a bibkey
        // (`barone1998`) stably identifies the work and is directly searchable — the better
        // form, not a fallback for the missing bibliography. Text-preserving, declared, tested.
        out.push({ kind: "cite", keys: role[2], raw: s });
      } else if (role && role[1].toLowerCase() === "eq") {
        // {eq}`label` → `[label]` (category 1) — the SAME fix as {cite}, one construct later.
        // In Sphinx an equation reference is VISIBLE (rendered as a number); the census-era
        // verbatim `{eq}`eqn:gcp`` broke the sentence ("Eq. {eq}`eqn:gcp` is") with raw syntax.
        // We keep the LABEL, not a number: a number re-flows on reprint, while `eqn:gcp` stably
        // identifies the equation — the same key-over-number reasoning that settled {cite}.
        out.push({ kind: "eq", label: role[2], raw: s });
      } else {
        // {cspan}/… → verbatim (category 2); its arg is NOT inline code.
        out.push({ kind: "text", text: s });
      }
    } else if (s.startsWith("[")) {
      const link = LINK_RE.exec(s);
      if (link) out.push({ kind: "xref", text: link[1], label: link[2], raw: s });
      else out.push({ kind: "text", text: s });
    } else {
      out.push({ kind: "code", text: s.slice(1, -1) });
    }
    last = start + s.length;
  }
  if (last < text.length) out.push({ kind: "text", text: text.slice(last) });
  return out;
}

/** Every cross-reference label in a body (deduped, in document order) — the input to the
 *  Rust anchor resolver (`resolve_manual_anchors`). Collected from prose + directive
 *  bodies; never invents a resolve (the slugify rule lives in Rust, `predict_anchor`). */
export function xrefLabels(body: string): string[] {
  const seen = new Set<string>();
  const walk = (text: string) => {
    for (const b of parseManualBody(text)) {
      if (b.kind === "prose") {
        for (const t of tokenizeInline(b.text)) if (t.kind === "xref") seen.add(t.label);
      } else if (b.kind === "directive") {
        walk(b.inner); // a note/admonition body can carry cross-references too
      }
    }
  };
  walk(body);
  return [...seen];
}

// --- Category 3: the NAMED hide-whitelist -----------------------------------

/** Case-insensitive directive-name compare — ONE function, folded from day one, because
 *  admonitions already arrive as `{Note}`/`{note}`/`{NOTE}` (measured). (The eventual
 *  second normalization would be the fourth turn of the `normalize_label` pattern.) */
function sameName(a: string, b: string): boolean {
  return a.toLowerCase() === b.toLowerCase();
}

/**
 * Is this DIRECTIVE category (3) — recognized and DELIBERATELY hidden? EXACTLY three
 * directives (the fourth category-3 position, the `(name)=` anchor label, is a LINE, not a
 * directive — see `isAnchorLabelLine`). Each for a measured reason (`wiki/modules/frontend.md`):
 *
 *   - `{index}` (321×) — index markers; INVISIBLE in the real Sphinx render.
 *   - `{tabularcolumns}` (76×) — a LaTeX column spec; INVISIBLE in HTML output.
 *   - `{raw}` + arg `latex` (176×) — output aimed at a DIFFERENT builder (LaTeX), not the
 *     HTML target we reproduce. The KEY IS THE PAIR `(raw, latex)`, NOT the name `{raw}`:
 *     "the arg is always latex" is measured ON THIS CORPUS, not a property of MyST — an
 *     ORCA 6.2 refresh could introduce `{raw} html`, and hiding by name alone would
 *     swallow it. So `{raw} html` stays VISIBLE (category 2), asserted by a test.
 */
export function isHiddenDirective(name: string, arg: string): boolean {
  if (sameName(name, "index") || sameName(name, "tabularcolumns")) return true;
  if (sameName(name, "raw")) {
    const target = arg.trim().split(/\s+/)[0] ?? "";
    return sameName(target, "latex");
  }
  return false;
}

/** Is this the missing-external-content class (category 5) — a directive that references
 *  a file NOT in our corpus (the manifest fetched only `_sources/*.md.txt`)? Shown as a
 *  visible ABSENCE MARKER, never verbatim (a bare path reads as silent emptiness where the
 *  manual gave an input example). Measured: `{literalinclude}` 255×. (`{include}`, 1×,
 *  pulls in a MyST doc rather than a code sample — left as a visible directive.) */
export function isMissingInclude(name: string): boolean {
  return sameName(name, "literalinclude");
}
