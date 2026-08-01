import { describe, it, expect } from "vitest";

import { ORCA_TEMPLATES } from "../templates/orca-templates";
import { orcaWordPattern } from "../editor/orca-language";
import { hoverContext, lookup, resolveHover } from "./keyword-lookup";
import kwjson from "./keywords.json";

// The coverage gate REWRITTEN in consumer form. The old Rust gate matched by bare
// STRING: it counted `%maxcore` as covered because the string matched a `MAXCORE`
// block-option inside %xtb/%cis/%mdci — a match of the string, not the entity, so its
// "46/46" was partly empty. This asks the SAME question the hover asks: for a token in
// its emit context, is there a record of the RIGHT TYPE (and, in a block, the right
// block)? A wrong-type match is a MISS, named, not counted.

// Real emitted inputs: the Phase-1 template library + two block-option contexts.
const INPUTS: { name: string; text: string }[] = [
  ...ORCA_TEMPLATES.map((t) => ({ name: t.id, text: t.inputContent })),
  { name: "scf-block", text: "%scf\n  MaxIter 200\nend\n* xyz 0 1\n  H 0 0 0\n*\n" },
  { name: "geom-constraints", text: "%geom\n  Constraints\n    {B 0 1 1.5 C}\n  end\nend\n" },
];

interface Tok {
  word: string;
  line: number;
  col: number;
  text: string;
}

function tokens(text: string): Tok[] {
  const out: Tok[] = [];
  text.split("\n").forEach((lineText, line) => {
    const re = new RegExp(orcaWordPattern.source, "g");
    let m: RegExpExecArray | null;
    while ((m = re.exec(lineText))) out.push({ word: m[0], line, col: m.index, text });
  });
  return out;
}

// The OLD Rust gate's question: does ANY record match this string, `%`-STRIPPED
// (it used `norm_kw`, which drops the leading `%`)? That is exactly why `%maxcore`
// (stripped → `maxcore`) matched the `MAXCORE` block-option and was counted covered.
const strip = (s: string) => s.replace(/^%/, "").toLowerCase();
const oldGateString = new Set(
  (kwjson as { keywords: { keyword: string; aliases?: string[] }[] }).keywords.flatMap((r) => [
    strip(r.keyword),
    ...(r.aliases ?? []).map(strip),
  ]),
);

const hoverable = INPUTS.flatMap((i) => tokens(i.text)).filter(
  (t) => hoverContext(t.text, t.line, t.col, t.word) !== null,
);

describe("keywords.json coverage — the consumer (hover) question, not string match", () => {
  it("reports coverage and the difference from the old string-match 46/46", () => {
    const seen = new Map<string, { covered: boolean; falseBefore: boolean }>();
    for (const t of hoverable) {
      const ctx = hoverContext(t.text, t.line, t.col, t.word)!;
      const hit = resolveHover(t.text, t.line, t.col, t.word) !== null;
      const key = `${t.word} [${ctx.kind}${ctx.block ? " " + ctx.block : ""}]`;
      if (!seen.has(key))
        seen.set(key, { covered: hit, falseBefore: !hit && oldGateString.has(strip(t.word)) });
    }
    const entries = [...seen.entries()];
    const covered = entries.filter(([, v]) => v.covered).length;
    const misses = entries.filter(([, v]) => !v.covered).map(([k]) => k);
    const falselyBefore = entries.filter(([, v]) => v.falseBefore).map(([k]) => k);

    // eslint-disable-next-line no-console
    console.log(
      `\n[coverage] ${covered}/${entries.length} distinct (token,context) resolve to a right-TYPE record` +
        `\n  misses (hover stays SILENT — correct per contract):\n    ${misses.join("\n    ") || "(none)"}` +
        `\n  falsely 'covered' by the old string gate (string matched, wrong entity):\n    ${
          falselyBefore.join("\n    ") || "(none)"
        }`,
    );

    // The heart of the fix: the old gate counted %maxcore covered; the consumer gate
    // names it a miss (its only string match is a wrong-block block-option).
    expect(falselyBefore).toContain("%maxcore [block]");
  });

  it("every simple `!` keyword and %pal/%geom the templates emit resolves (right type)", () => {
    const unresolved: string[] = [];
    for (const t of hoverable) {
      const ctx = hoverContext(t.text, t.line, t.col, t.word)!;
      const mustResolve =
        ctx.kind === "simple" ||
        (ctx.kind === "block" && ["%pal", "%geom"].includes(t.word.toLowerCase()));
      if (mustResolve && resolveHover(t.text, t.line, t.col, t.word) === null)
        unresolved.push(`${t.word} [${ctx.kind}]`);
    }
    expect(unresolved).toEqual([]);
  });

  it("MaxIter resolves to %scf (right block), not the 15 other MaxIters", () => {
    const recs = lookup("MaxIter", "block-option", "%scf");
    expect(recs.length).toBe(1);
    expect(recs[0].block).toBe("%scf");
  });

  it("%maxcore stays a NAMED gap — no block record (its home is the {numref} layer)", () => {
    expect(lookup("%maxcore", "block", null)).toEqual([]);
  });
});
