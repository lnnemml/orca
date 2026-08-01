//! Minimal, verifiable renderer for a manual section body (MyST Markdown).
//!
//! The hazard is silent loss at the display layer: a naive Markdown renderer eats
//! what it does not understand WITHOUT error — a `:::{note}` directive vanishes,
//! `$\omega$B97M` math turns to underscore-mush, an ORCA input loses its indentation.
//! The reader never sees it. So the rule (the display analogue of the sectioner's
//! line-conservation, rule #9): **what is not recognized is shown AS IS, never dropped.**
//!
//! We recognize exactly ONE structure — fenced code blocks (` ``` `), rendered as a
//! monospace block that preserves indentation (42.7 % of corpus bytes live in fences,
//! most of it ORCA input). Everything else — prose, LaTeX, `:::{directives}`, tables,
//! inline code — is emitted VERBATIM. We do NOT try to render MyST.
//!
//! A block's `text` is the exact source; the component renders it as a text node, so
//! `render.test.ts` can assert that every non-whitespace character of `body_md` survives
//! into the rendered text (a subsequence check against the concatenated block texts,
//! which mirror the DOM `textContent`).

export type Block =
  | { kind: "fence"; text: string } // full verbatim region, INCLUDING the ``` lines
  | { kind: "prose"; text: string }; // verbatim; newlines/indentation preserved by CSS

const FENCE = /^\s*```/;

/**
 * Split a body into fence and prose blocks. Every source line lands in exactly one
 * block, verbatim — the loss-free precondition the preservation test checks. An
 * unterminated fence (no closing ` ``` `) runs to end of body and is still a single
 * fence block, so its content is shown, not swallowed.
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
    if (FENCE.test(lines[i])) {
      flush();
      const buf = [lines[i]];
      i++;
      while (i < lines.length) {
        buf.push(lines[i]);
        const close = FENCE.test(lines[i]);
        i++;
        if (close) break;
      }
      blocks.push({ kind: "fence", text: buf.join("\n") });
    } else {
      prose.push(lines[i]);
      i++;
    }
  }
  flush();
  return blocks;
}
