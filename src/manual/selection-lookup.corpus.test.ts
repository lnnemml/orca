//! CORPUS gate for the SELECTION-normalization rule (unit 4.13, the Task-0 gate). The
//! trigger moves from hover (one wordPattern token) to a user SELECTION (anything). A
//! selection can cut a token, span keywords, or grab xyz — so the normalization must be
//! MEASURED, not invented. This reads the real ` ```orca ` blocks and keywords.json and
//! prints the numbers `wiki/orca/manual-sources.md` records; the rule is the pure
//! candidate below (which `selection-lookup.ts` implements in the feature commit).
//!
//! The number that decides guard-vs-muffler: **how many CORRECT single tokens (whitespace-
//! delimited, resolving) would the boundary guard falsely reject.** If > 0 the class is
//! too strict. Skips with an explicit message when the (gitignored) corpus is absent.

import { readFileSync, readdirSync, existsSync, statSync } from "node:fs";
import { join, resolve } from "node:path";
import { describe, it } from "vitest";

import kwjson from "./keywords.json";

// ── The candidate rule, pure (mirrors what selection-lookup.ts implements) ──────────────
// Token-continuation class: letters/digits/_ plus the chars real keys carry — `%` (block
// prefix), `/` (def2/J, aug-cc-…/C: 78), `.` (basename.xyz, D.Print: 47), `-` (def2-SVP).
// Parens are EXCLUDED — 0 keys contain them; `(` is an argument delimiter, so a selection
// abutting `(` did NOT cut a token (that is the `CPCM` / `water` distinction).
const WORD = /[\w%/.-]/;

const keys = new Set<string>();
{
  const d = kwjson as unknown as { keywords: { keyword: string; aliases?: string[] }[] };
  for (const r of d.keywords)
    for (const k of [r.keyword, ...(r.aliases ?? [])]) keys.add(k.toLowerCase());
}

const norm = (s: string) => s.trim().toLowerCase();
/** Strip ONE trailing balanced `(...)` argument group: `CPCM(water)` → `CPCM`. */
const stripArg = (s: string) => s.replace(/\([^()]*\)\s*$/, "").trim();

/** The refined lookup order: try the WHOLE selection first (so `SV(P)`, itself a key, is
 *  kept), and only on a miss strip a trailing `(...)` and retry (so `CPCM(water)` → CPCM). */
function resolveKey(sel: string): string | null {
  const whole = norm(sel);
  if (keys.has(whole)) return whole;
  const stripped = norm(stripArg(sel));
  if (stripped && stripped !== whole && keys.has(stripped)) return stripped;
  return null;
}

// ── Corpus discovery (same as render.corpus.test.ts) ────────────────────────────────────
function findCorpus(): { files: { rel: string; text: string }[] } | null {
  const roots = [process.env.ORCA_MANUAL_ROOT, resolve(process.cwd(), "resources/manual")].filter(
    (p): p is string => !!p,
  );
  for (const root of roots) {
    if (!existsSync(root)) continue;
    let version = "6.1";
    const manifest = join(root, "manifest.json");
    if (existsSync(manifest)) {
      try {
        version = JSON.parse(readFileSync(manifest, "utf8")).orca_version ?? version;
      } catch {
        /* default */
      }
    }
    const dir = join(root, version);
    if (!existsSync(dir) || !statSync(dir).isDirectory()) continue;
    const files: { rel: string; text: string }[] = [];
    const walk = (d: string) => {
      for (const e of readdirSync(d, { withFileTypes: true })) {
        const p = join(d, e.name);
        if (e.isDirectory()) walk(p);
        else if (e.name.endsWith(".md.txt"))
          files.push({ rel: p.slice(dir.length + 1), text: readFileSync(p, "utf8") });
      }
    };
    walk(dir);
    if (files.length) return { files };
  }
  return null;
}

/** Every whitespace-delimited token on a line, with its [start,end) span in that line. */
function tokenSpans(line: string): { tok: string; start: number; end: number }[] {
  const out: { tok: string; start: number; end: number }[] = [];
  const re = /\S+/g;
  let m: RegExpExecArray | null;
  while ((m = re.exec(line))) out.push({ tok: m[0], start: m.index, end: m.index + m[0].length });
  return out;
}

const corpus = findCorpus();

describe("selection normalization — measured over the corpus (Task-0 gate)", () => {
  if (!corpus) {
    it.skip("SKIPPED: no corpus — set ORCA_MANUAL_ROOT or fetch resources/manual/", () => {});
    return;
  }

  it("prints the numbers (key forms, resolve rate, FALSE-REJECT, paren order)", () => {
    // [1] key forms
    const all = [...keys];
    const withSpace = all.filter((k) => /\s/.test(k)).length;
    const withParen = all.filter((k) => /[()]/.test(k)).length;
    const withSlash = all.filter((k) => k.includes("/")).length;
    const withDot = all.filter((k) => k.includes(".")).length;

    // Walk ` ```orca ` blocks → !-line tokens (with spans) and %block-option first tokens.
    interface Sel {
      tok: string;
      before: string;
      after: string;
    }
    const bang: Sel[] = [];
    const blockOpt: Sel[] = [];
    for (const { text } of corpus.files) {
      let inOrca = false;
      let curBlock: string | null = null;
      for (const raw of text.split("\n")) {
        const t = raw.trim();
        if (t.startsWith("```")) {
          const lang = t.replace(/`+/, "").trim();
          inOrca = inOrca ? false : lang.toLowerCase() === "orca";
          curBlock = null;
          continue;
        }
        if (!inOrca) continue;
        const spans = tokenSpans(raw);
        const record = (bucket: Sel[], s: { tok: string; start: number; end: number }) =>
          bucket.push({ tok: s.tok, before: raw[s.start - 1] ?? "", after: raw[s.end] ?? "" });
        if (/^\s*!/.test(raw)) {
          for (const s of spans) if (s.tok !== "!") record(bang, s);
        }
        const mb = t.match(/^%([A-Za-z_]\w*)/);
        if (mb) curBlock = "%" + mb[1].toLowerCase();
        else if (/^end\b/i.test(t)) curBlock = null;
        else if (curBlock && spans.length) record(blockOpt, spans[0]);
      }
    }

    const rate = (sels: Sel[]) => {
      const distinct = [...new Set(sels.map((s) => s.tok))];
      const resolved = distinct.filter((tok) => resolveKey(tok));
      return { distinct: distinct.length, resolved: resolved.length };
    };
    const rb = rate(bang);
    const ro = rate(blockOpt);

    // [3] FALSE-REJECT: a whitespace-delimited token that RESOLVES, whose source
    // neighbours are word-class chars (→ the guard would reject a correct selection).
    const guardWouldReject = (s: Sel) =>
      (s.before && WORD.test(s.before)) || (s.after && WORD.test(s.after));
    const correct = [...bang, ...blockOpt].filter((s) => resolveKey(s.tok));
    const falseRejects = correct.filter(guardWouldReject);
    const frExamples = [...new Set(falseRejects.map((s) => `${s.before}[${s.tok}]${s.after}`))].slice(0, 10);

    // [4] paren-arg tokens: resolve WHOLE (kept) vs STRIPPED (arg removed).
    const parenToks = [...new Set([...bang, ...blockOpt].map((s) => s.tok))].filter((t) =>
      /\w\([^()]*\)$/.test(t),
    );
    const whole = parenToks.filter((t) => keys.has(norm(t)));
    const stripped = parenToks.filter((t) => !keys.has(norm(t)) && keys.has(norm(stripArg(t))));

    console.log(`\n${"=".repeat(72)}`);
    console.log(`SELECTION NORMALIZATION — ${corpus.files.length} leaves, ${keys.size} map keys`);
    console.log("=".repeat(72));
    console.log(`\n[1] KEY FORMS: space ${withSpace} · paren ${withParen} · slash ${withSlash} · dot ${withDot}`);
    console.log(`    (0 space → internal-space selection can never be a key → silence;`);
    console.log(`     0 paren → \`(…)\` is an arg delimiter, not part of a key)`);
    console.log(`\n[2] RESOLVE (whole-then-strip, exact):`);
    console.log(`    !-line tokens:        ${rb.resolved}/${rb.distinct} distinct resolve (${((100 * rb.resolved) / rb.distinct).toFixed(0)}%)`);
    console.log(`    %block-option tokens: ${ro.resolved}/${ro.distinct} distinct resolve (${((100 * ro.resolved) / ro.distinct).toFixed(0)}%)`);
    console.log(`    (the rest correctly → silence: map covers a fraction of the ! vocabulary)`);
    console.log(`\n[3] BOUNDARY GUARD — FALSE REJECTS (the guard-vs-muffler number):`);
    console.log(`    correct resolving tokens whose neighbours are word-class chars: ${falseRejects.length} / ${correct.length}`);
    console.log(`    → the guard falsely rejects ${falseRejects.length} natural token(s)` + (frExamples.length ? `; e.g. ${frExamples.join(" ")}` : ""));
    console.log(`\n[4] PAREN-ARG ORDER (whole-first preserves \`SV(P)\`):`);
    console.log(`    KEYWORD(arg) tokens: ${parenToks.length}`);
    console.log(`    resolve WHOLE (kept, e.g. SV(P)):     ${whole.length}  ${whole.slice(0, 6).join(", ")}`);
    console.log(`    resolve STRIPPED (arg removed):       ${stripped.length}  ${stripped.slice(0, 6).join(", ")}`);
    console.log(`    SV(P) resolves as WHOLE: ${keys.has("sv(p)") ? "yes (not stripped to SV)" : "NO"}`);
    console.log(`${"=".repeat(72)}`);
  });
});
