//! CORPUS preservation gate for the manual renderer (unit 4.12). The sample tests in
//! `render.test.ts` cover the hazards by construction; THIS runs the render over all 126
//! real leaves of `resources/manual/` and proves — per file — that nothing is lost.
//!
//! The right invariant after 4.11 is NOT "the rendered text equals the source" (a
//! transform CHANGES the text) but a **sum over the categories**: for each page,
//!
//!     multiset( source )  ==  cat1_src  ⊎  cat2_src  ⊎  cat3_src  ⊎  cat5_src        (S)
//!     multiset( rendered ) ==  cat2  ⊎  code contents  ⊎  xref texts  ⊎  include markers  (R)
//!
//! (S) says the parser+tokenizer PARTITION every source char into a category (nothing
//! unclassified → nothing silently dropped at parse). (R) says the actual React render
//! emits exactly the declared-visible chars — cat2 verbatim, the content of transformed
//! tokens (code → its content, a resolved xref → its link text, math → NOTHING because
//! KaTeX renders via `dangerouslySetInnerHTML`), and the injected `{literalinclude}`
//! markers — and NOTHING from the hidden whitelist. Together: every legitimate transform
//! is ACCOUNTED, and any undeclared loss unbalances the sum on the offending file.
//!
//! Note (`cat5`): `{literalinclude}` is a fourth bucket the three-category model does not
//! name — it drops its source and injects a marker (`render.ts` "category 5"). It is
//! accounted here so the sum stays honest.
//!
//! Every cross-reference is resolved (a stub resolver) so EVERY xref exercises the lossy
//! transform path — the strictest check, and DB-free (deterministic).
//!
//! Runs in the normal `npm test` when the (gitignored) corpus is on disk; on a machine
//! without it (CI) the suite SKIPS with an explicit message — never a silent pass.

import { readFileSync, readdirSync, existsSync, statSync } from "node:fs";
import { join, resolve } from "node:path";
import { describe, it, expect } from "vitest";
import { isValidElement, type ReactNode } from "react";

import { parseManualBody, tokenizeInline, isHiddenDirective, isMissingInclude } from "./render";
import { renderManualBody, type AnchorTarget } from "./PageView";

// --- reactText: DOM textContent without jsdom (same helper as render.test.ts). A node
//     whose content comes via dangerouslySetInnerHTML (KaTeX) has no children → "". ------
function reactText(node: ReactNode): string {
  if (node == null || typeof node === "boolean") return "";
  if (typeof node === "string") return node;
  if (typeof node === "number") return String(node);
  if (Array.isArray(node)) return node.map(reactText).join("");
  if (isValidElement(node)) return reactText((node.props as { children?: ReactNode }).children);
  return "";
}
const strip = (s: string) => s.replace(/\s+/g, "");

// --- Locate the corpus the same way `manual_root` does (repo tree), or via env var.
//     vitest roots `process.cwd()` at the repo, so `resources/manual` resolves from there. -
function findCorpus(): { versionDir: string; files: { rel: string; text: string }[] } | null {
  const roots = [
    process.env.ORCA_MANUAL_ROOT,
    resolve(process.cwd(), "resources/manual"),
  ].filter((p): p is string => !!p);
  for (const root of roots) {
    if (!existsSync(root)) continue;
    // Version dir: from manifest.json, else the first subdir that holds .md.txt files.
    let version = "6.1";
    const manifest = join(root, "manifest.json");
    if (existsSync(manifest)) {
      try {
        version = JSON.parse(readFileSync(manifest, "utf8")).orca_version ?? version;
      } catch {
        /* fall through to the default */
      }
    }
    const versionDir = join(root, version);
    if (!existsSync(versionDir) || !statSync(versionDir).isDirectory()) continue;
    const files: { rel: string; text: string }[] = [];
    const walk = (dir: string) => {
      for (const e of readdirSync(dir, { withFileTypes: true })) {
        const p = join(dir, e.name);
        if (e.isDirectory()) walk(p);
        else if (e.name.endsWith(".md.txt")) files.push({ rel: p.slice(versionDir.length + 1), text: readFileSync(p, "utf8") });
      }
    };
    walk(versionDir);
    if (files.length) return { versionDir, files };
  }
  return null;
}

// --- Classify every source char into a category bucket (recursing into visible
//     directives, whose markers are cat2 and whose body is classified in place). --------
interface Buckets {
  cat1: string; // transformed-token SOURCE (code/math/xref)
  cat2: string; // verbatim
  cat3: string; // hidden SOURCE — whitelist directives AND `(name)=` anchor labels
  cat5: string; // {literalinclude} SOURCE (dropped, marker injected)
  codeContents: string; // what code tokens render to
  xrefTexts: string; // what resolved xrefs render to
  citeVisible: string; // what {cite}`keys` renders to → `[keys]`
  includeMarkers: string; // what {literalinclude} renders to
}
const emptyBuckets = (): Buckets => ({
  cat1: "", cat2: "", cat3: "", cat5: "", codeContents: "", xrefTexts: "", citeVisible: "",
  includeMarkers: "",
});

/** The exact source of a directive block (opener + body + closer). Whitespace-only inner
 *  (a blank-line body) carries no non-ws char, so strip-based checks are unaffected. */
function dirSource(open: string, inner: string, close: string): string {
  const parts = [open];
  if (inner !== "") parts.push(inner);
  if (close !== "") parts.push(close);
  return parts.join("\n");
}

function classify(source: string, acc: Buckets): void {
  for (const b of parseManualBody(source)) {
    if (b.kind === "fence" || b.kind === "table") {
      acc.cat2 += b.text;
    } else if (b.kind === "prose") {
      for (const t of tokenizeInline(b.text)) {
        switch (t.kind) {
          case "text":
            acc.cat2 += t.text;
            break;
          case "code":
            acc.cat1 += "`" + t.text + "`";
            acc.codeContents += t.text;
            break;
          case "math":
            acc.cat1 += (t.display ? "$$" : "$") + t.tex + (t.display ? "$$" : "$");
            break;
          case "xref": // resolved by the stub → renders its link text
            acc.cat1 += t.raw;
            acc.xrefTexts += t.text;
            break;
          case "cite": // {cite}`keys` → [keys] (category 1)
            acc.cat1 += t.raw;
            acc.citeVisible += "[" + t.keys + "]";
            break;
        }
      }
    } else if (b.kind === "label") {
      acc.cat3 += b.text; // a MyST anchor label line — hidden, renders to nothing
    } else {
      // directive
      if (isHiddenDirective(b.name, b.arg)) {
        acc.cat3 += dirSource(b.open, b.inner, b.close);
      } else if (isMissingInclude(b.name)) {
        acc.cat5 += dirSource(b.open, b.inner, b.close);
        acc.includeMarkers += "input example not loaded (" + b.arg + ")";
      } else {
        acc.cat2 += b.open; // the marker line, kept verbatim
        classify(b.inner, acc); // body classified in place
        if (b.close) acc.cat2 += b.close;
      }
    }
  }
}

// --- Multiset (char-frequency) equality with a readable diff. ---------------------------
function counts(s: string): Map<string, number> {
  const m = new Map<string, number>();
  for (const c of s) m.set(c, (m.get(c) ?? 0) + 1);
  return m;
}
/** "" when the two strings have the same non-whitespace char multiset, else a short diff. */
function multisetDiff(aLabel: string, a: string, bLabel: string, b: string): string {
  const ca = counts(a);
  const cb = counts(b);
  const diffs: string[] = [];
  for (const ch of new Set([...ca.keys(), ...cb.keys()])) {
    const da = ca.get(ch) ?? 0;
    const db = cb.get(ch) ?? 0;
    if (da !== db) diffs.push(`${JSON.stringify(ch)}: ${aLabel}=${da} ${bLabel}=${db}`);
    if (diffs.length >= 8) break;
  }
  return diffs.length ? `Δ ${aLabel}(${a.length}) vs ${bLabel}(${b.length}) — ${diffs.join(", ")}` : "";
}

const corpus = findCorpus();
// Resolve EVERY label so every xref takes the lossy transform path (DB-free, deterministic).
const resolveAll = (): AnchorTarget => ({ file: "x", section_id: 1 });

describe("manual render — corpus preservation (three-category sum over all leaves)", () => {
  if (!corpus) {
    it.skip("SKIPPED: no corpus found — set ORCA_MANUAL_ROOT or fetch resources/manual/ (scripts/fetch-manual.py --all)", () => {});
    return;
  }

  it(`partition (S) + render (R) balance on every leaf (${corpus.files.length} files)`, () => {
    const failures: string[] = [];
    for (const { rel, text } of corpus.files) {
      const acc = emptyBuckets();
      classify(text, acc);

      // (S) the categories partition the source — nothing unclassified/dropped at parse.
      const partitionSrc = acc.cat1 + acc.cat2 + acc.cat3 + acc.cat5;
      const sDiff = multisetDiff("source", strip(text), "cat1+2+3+5", strip(partitionSrc));
      if (sDiff) failures.push(`${rel} — partition (S): ${sDiff}`);

      // (R) the render emits exactly the declared-visible chars — no render-layer loss.
      const rendered = reactText(renderManualBody(text, { resolve: resolveAll }));
      const visibleExpected =
        acc.cat2 + acc.codeContents + acc.xrefTexts + acc.citeVisible + acc.includeMarkers;
      const rDiff = multisetDiff("rendered", strip(rendered), "expected-visible", strip(visibleExpected));
      if (rDiff) failures.push(`${rel} — render (R): ${rDiff}`);
    }
    expect(failures, `preservation broke on ${failures.length} check(s):\n${failures.join("\n")}`).toEqual([]);
  });
});

// Negative control (CLAUDE.md convention: a gate whose ability to fail is undemonstrated is
// green for an unknown reason). Runs WITHOUT the corpus — proves the NEW 4.15 branches bite.
describe("the {cite} and (label)= branches unbalance the sum when a render misbehaves", () => {
  const expectedVisible = (acc: Buckets) =>
    strip(acc.cat2 + acc.codeContents + acc.xrefTexts + acc.citeVisible + acc.includeMarkers);

  it("(R) bites if a render EATS cite keys", () => {
    const body = "C-PCM{cite}`barone1998`: the model";
    const acc = emptyBuckets();
    classify(body, acc);
    const expected = expectedVisible(acc);
    const correct = strip(reactText(renderManualBody(body, { resolve: resolveAll })));
    expect(multisetDiff("rendered", correct, "expected", expected)).toBe(""); // real render balances
    const buggy = correct.replace("barone1998", ""); // a render that drops the keys
    expect(multisetDiff("buggy", buggy, "expected", expected)).not.toBe("");
  });

  it("(R) bites if a render SHOWS a hidden `(name)=` anchor label", () => {
    const body = "(sec:foo)=\nvisible prose";
    const acc = emptyBuckets();
    classify(body, acc);
    const expected = expectedVisible(acc);
    const correct = strip(reactText(renderManualBody(body)));
    expect(multisetDiff("rendered", correct, "expected", expected)).toBe(""); // label hidden → balances
    const buggy = correct + "(sec:foo)="; // a render that failed to hide the label
    expect(multisetDiff("buggy", buggy, "expected", expected)).not.toBe("");
  });
});
