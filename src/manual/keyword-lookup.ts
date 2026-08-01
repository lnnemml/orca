//! The consumer side of keywords.json: given a token and where it sits in an ORCA
//! input, find the record that documents THAT entity — not merely that string.
//!
//! A block-option is a qualified name (`wiki/modules/manual-keywords.md`): `MaxIter`
//! without a block is a string that occurs in 15 places. So the lookup is type- and
//! block-aware, and a match of the WRONG type is a MISS, not a hit. This is what makes
//! the hover show the same entity the author is pointing at, not the same spelling.

import kwjson from "./keywords.json";
import { enclosingBlock } from "../editor/enclosing-block";

// `undetermined`: the manual gave no positive signal for the type (no owner, no simple
// title) — a value like anchor=NULL, NOT a dumpster default. A qualified hover lookup
// only ever asks for simple / block / block-option, so an `undetermined` record is
// invisible to it (silence); it is reachable only by the panel's unqualified path
// ("documented in N places"). Contract: the hover never falls back to it.
export type KeywordType = "simple" | "block" | "block-option" | "undetermined";

export interface KeywordRecord {
  keyword: string;
  type: KeywordType;
  provenance: "seeded" | "curated";
  block?: string | null;
  owner_source?: "text" | "structural" | null;
  aliases?: string[];
  summary?: string;
  section?: number; // index into `sections`
  targets?: number[]; // …when documented in several places
}

export interface SectionDescriptor {
  file: string;
  breadcrumb: string[];
  title: string;
  nth: number;
}

interface KeywordsFile {
  schema_version: number;
  orca_version: string;
  sections: SectionDescriptor[];
  keywords: KeywordRecord[];
}

const data = kwjson as unknown as KeywordsFile;
export const orcaMapVersion = data.orca_version;
export const sectionDescriptors = data.sections;

/** Section descriptor(s) a record points at (`section` or `targets`). */
export function recordSections(r: KeywordRecord): SectionDescriptor[] {
  const idx = r.section !== undefined ? [r.section] : (r.targets ?? []);
  return idx.map((i) => sectionDescriptors[i]);
}

// keyword + every alias → records (ORCA is case-insensitive, so keys are lowercased).
const byKey = new Map<string, KeywordRecord[]>();
for (const r of data.keywords) {
  for (const k of [r.keyword.toLowerCase(), ...(r.aliases ?? []).map((a) => a.toLowerCase())]) {
    const list = byKey.get(k) ?? [];
    list.push(r);
    byKey.set(k, list);
  }
}

export interface HoverContext {
  kind: KeywordType;
  block: string | null;
}

/**
 * Which of the three lookup cases a token falls in (the qualifier the hover needs):
 * `!`-line → simple; a `%name` token → block; a token inside a `%block` → block-option
 * of THAT block. Same-line `%pal nprocs …` is handled too (the opener is on the token's
 * own line, which `enclosingBlock` — lines above — cannot see). `null` = none of the
 * three (top-level value, coordinates) → no hover.
 */
export function hoverContext(
  text: string,
  line: number,
  colBefore: number,
  word: string,
): HoverContext | null {
  const lineText = text.split("\n")[line] ?? "";
  const before = lineText.slice(0, colBefore);
  // Inside a `#` comment or a `"…"` string the token is not a keyword — no hover.
  if (before.includes("#")) return null;
  if ((before.match(/"/g)?.length ?? 0) % 2 === 1) return null;
  if (word.startsWith("%")) return { kind: "block", block: null };
  if (/^\s*!/.test(lineText)) return { kind: "simple", block: null };
  let block = enclosingBlock(text, line);
  if (!block) {
    const m = lineText.match(/^\s*%([A-Za-z_]\w*)/);
    if (m && colBefore >= m[0].length) block = "%" + m[1].toLowerCase();
  }
  if (block) return { kind: "block-option", block };
  return null;
}

/** Records matching `word` (or one of its aliases) of the RIGHT type — and, for a
 *  block-option, the right owning block. A wrong-type record is not returned. */
export function lookup(word: string, kind: KeywordType, block: string | null): KeywordRecord[] {
  const recs = byKey.get(word.toLowerCase()) ?? [];
  return recs.filter(
    (r) =>
      r.type === kind &&
      (kind !== "block-option" || (r.block ?? "").toLowerCase() === (block ?? "").toLowerCase()),
  );
}

export interface HoverMatch {
  word: string;
  kind: KeywordType;
  block: string | null;
  records: KeywordRecord[];
}

/**
 * The hover answer for a token, or `null` for a MISS. A miss means the hover shows
 * **nothing** (silence) — the consumer contract forbids falling back to an unqualified
 * or FTS search here; that is the panel's separate, deliberate path.
 */
export function resolveHover(
  text: string,
  line: number,
  colBefore: number,
  word: string,
): HoverMatch | null {
  const ctx = hoverContext(text, line, colBefore, word);
  if (!ctx) return null;
  const records = lookup(word, ctx.kind, ctx.block);
  return records.length ? { word, kind: ctx.kind, block: ctx.block, records } : null;
}
