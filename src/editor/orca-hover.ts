//! Monaco hover provider for ORCA `.inp` — the consumer of keywords.json in the editor.
//!
//! Three cases, kept apart (`wiki/orca/input-syntax.md`): a token on the `!` line →
//! simple; a `%name` token → block; a token inside a `%block` → block-option of THAT
//! block. The lookup is qualified (`keyword-lookup.ts`), so it shows the entity the
//! author points at, not the same spelling elsewhere.
//!
//! CONTRACT (fixed in manual-keywords.md, not re-decided here): on a qualified MISS the
//! hover shows **nothing** — no fall-back to a bare-name search, no FTS. Silence, not
//! "nothing found". Unqualified search is the panel's separate, deliberate path.

import type { Monaco } from "@monaco-editor/react";
import type { editor, Position } from "monaco-editor";

import {
  orcaMapVersion,
  recordSections,
  resolveHover,
  type HoverMatch,
  type SectionDescriptor,
} from "../manual/keyword-lookup";

/** React sets this so a hover command-link can open the section drawer. */
let openHandler: ((d: SectionDescriptor) => void) | null = null;
export function setManualOpenHandler(fn: ((d: SectionDescriptor) => void) | null) {
  openHandler = fn;
}

const OPEN_COMMAND = "orca.openManualSection";
const crumb = (d: SectionDescriptor) =>
  (d.breadcrumb.length ? d.breadcrumb.join(" › ") + " › " : "") + d.title;
const link = (d: SectionDescriptor, label: string) =>
  `[${label}](command:${OPEN_COMMAND}?${encodeURIComponent(JSON.stringify(d))})`;

/** The hover body as trusted markdown. Pure (no Monaco) so it is unit-testable. An
 *  empty `summary` is fine — seeded records have none; the identity + location is the
 *  value. Several targets → "documented in N places" with a list, NOT a picked first. */
export function buildHoverMarkdown(m: HoverMatch): string {
  const lines: string[] = [];
  const head =
    m.kind === "block-option"
      ? `**${m.word}** — option of \`${m.block}\`` +
        (m.records[0]?.owner_source ? ` _(owner: ${m.records[0].owner_source})_` : "")
      : `**${m.word}** — ${m.kind === "block" ? "input block" : "simple keyword"}`;
  lines.push(head);

  const summary = m.records.find((r) => r.summary)?.summary;
  if (summary) lines.push("", summary);

  const descriptors = dedup(m.records.flatMap(recordSections));
  if (descriptors.length === 1) {
    lines.push("", link(descriptors[0], "Open in manual →"));
    lines.push(`\n\`${crumb(descriptors[0])}\``);
  } else {
    lines.push("", `Documented in **${descriptors.length}** places:`);
    for (const d of descriptors) lines.push(`- ${link(d, crumb(d))}`);
  }
  return lines.join("\n");
}

function dedup(ds: SectionDescriptor[]): SectionDescriptor[] {
  const seen = new Set<string>();
  return ds.filter((d) => {
    const k = `${d.file}${d.breadcrumb.join("")}${d.title}${d.nth}`;
    return seen.has(k) ? false : (seen.add(k), true);
  });
}

let registered = false;
export function registerOrcaHover(monaco: Monaco, languageId: string) {
  if (registered) return;
  registered = true;

  monaco.editor.registerCommand(OPEN_COMMAND, (_accessor: unknown, json: string) => {
    if (openHandler) openHandler(JSON.parse(json) as SectionDescriptor);
  });

  monaco.languages.registerHoverProvider(languageId, {
    provideHover(model: editor.ITextModel, position: Position) {
      const word = model.getWordAtPosition(position);
      if (!word) return null;
      const match: HoverMatch | null = resolveHover(
        model.getValue(),
        position.lineNumber - 1,
        word.startColumn - 1,
        word.word,
      );
      if (!match) return null; // MISS → silence (contract)
      return {
        range: new monaco.Range(
          position.lineNumber,
          word.startColumn,
          position.lineNumber,
          word.endColumn,
        ),
        contents: [{ value: buildHoverMarkdown(match), isTrusted: true }],
      };
    },
  });
}

/** The map's ORCA version, passed to `resolve_manual_section` so a stale map is caught. */
export { orcaMapVersion };
