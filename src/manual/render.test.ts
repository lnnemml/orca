import { describe, it, expect } from "vitest";
import { isValidElement, type ReactNode } from "react";
import katex from "katex";

import {
  parseManualBody,
  tokenizeInline,
  xrefLabels,
  isHiddenDirective,
  isMissingInclude,
  isAnchorLabelLine,
} from "./render";
import { renderManualBody, type AnchorTarget } from "./PageView";

/**
 * The text a DOM would expose as `textContent`: the concatenation of every string/number
 * leaf of the React tree, in order. React renders string/number children as text nodes
 * and inserts nothing between siblings, so this equals the rendered element's
 * `.textContent` — computed here without a DOM (no jsdom). A node whose content comes via
 * `dangerouslySetInnerHTML` (KaTeX) has no React children, so it contributes "" — which is
 * why category-1 math is kept OUT of the category-2 preservation samples below.
 */
function reactText(node: ReactNode): string {
  if (node == null || typeof node === "boolean") return "";
  if (typeof node === "string") return node;
  if (typeof node === "number") return String(node);
  if (Array.isArray(node)) return node.map(reactText).join("");
  if (isValidElement(node)) {
    return reactText((node.props as { children?: ReactNode }).children);
  }
  return "";
}

const strip = (s: string) => s.replace(/\s+/g, "");

// ─────────────────────────────────────────────────────────────────────────────
// CATEGORY 2 — UNRECOGNIZED → VERBATIM. The preservation test, UNWEAKENED, lives
// here and ONLY here. Samples deliberately contain NO category-1 construct (no `$`,
// no inline `code`, no cross-ref) and NO hidden directive, so "every non-whitespace
// char survives" is the exact, strong assertion — a relaxed single test that also
// covered transformed text would go green while silently dropping content.
// ─────────────────────────────────────────────────────────────────────────────
const VERBATIM: Record<string, string> = {
  "plain prose": "RIJCOSX needs an AuxJ basis. It speeds up hybrid DFT.",
  "ORCA fence with indentation":
    "Run it:\n\n```orca\n%scf\n    MaxIter 200\n    Convergence Tight\nend\n```\n\nafter the block.",
  "pipe table": "| Keyword | Meaning |\n|---|---|\n| RIJCOSX | hybrid speedup |",
  "visible directive (note), plain body":
    ":::{note}\nGeometry optimisation defaults to TightOpt.\n:::",
  "visible directive wrapping a code fence":
    ":::{warning}\nMind the units:\n\n```orca\n! B3LYP def2-SVP\n```\n:::",
  "unterminated code fence": "```orca\n! B3LYP def2-SVP\n(no closing fence)",
  "prose with braces that are not a role": "Use the {geom} block heading, informally.",
};

describe("category 2 — unrecognized text is preserved char-for-char", () => {
  for (const [name, body] of Object.entries(VERBATIM)) {
    it(`preserves every non-whitespace char: ${name}`, () => {
      const rendered = reactText(renderManualBody(body));
      expect(strip(rendered)).toBe(strip(body));
    });
  }

  it("blocks tile the body: every source line lands in exactly one block", () => {
    for (const body of Object.values(VERBATIM)) {
      const joined = parseManualBody(body)
        .map((b) =>
          b.kind === "directive" ? [b.open, b.inner, b.close].filter(Boolean).join("\n") : b.text,
        )
        .join("\n");
      expect(strip(joined)).toBe(strip(body));
    }
  });
});

// ─────────────────────────────────────────────────────────────────────────────
// CATEGORY 1 — RECOGNIZED & TRANSFORMED. One test per construct, asserting the
// transform explicitly (the char-preservation check cannot: a transform CHANGES the
// text — backticks vanish, math source becomes rendered markup).
// ─────────────────────────────────────────────────────────────────────────────
describe("category 1 — inline code → the content, quotes gone", () => {
  it("tokenizes `def2/J` as code with the content only", () => {
    const toks = tokenizeInline("Set `def2/J` then note free_energy below.");
    expect(toks).toEqual([
      { kind: "text", text: "Set " },
      { kind: "code", text: "def2/J" },
      { kind: "text", text: " then note free_energy below." },
    ]);
  });
  it("renders inline code with the backticks removed but content kept", () => {
    const rendered = reactText(renderManualBody("Set `Trust` here."));
    expect(rendered).toContain("Trust");
    expect(rendered).not.toContain("`");
  });
});

describe("category 1 — math (dollar-delimited only; the corpus has no \\(\\) or {math})", () => {
  it("tokenizes inline $…$", () => {
    expect(tokenizeInline("energy $E = mc^2$ here")).toEqual([
      { kind: "text", text: "energy " },
      { kind: "math", tex: "E = mc^2", display: false },
      { kind: "text", text: " here" },
    ]);
  });
  it("tokenizes display $$…$$ spanning lines", () => {
    expect(tokenizeInline("$$\nE = mc^2\n$$")).toEqual([
      { kind: "math", tex: "\nE = mc^2\n", display: true },
    ]);
  });
  it("KaTeX renders valid tex", () => {
    expect(katex.renderToString("E = mc^2", { throwOnError: false })).toContain("katex");
  });
  it("throwOnError:false — an UNKNOWN macro shows its source verbatim (category 2), not a broken page", () => {
    const html = katex.renderToString("\\unknownMacro x", { throwOnError: false });
    expect(html).toContain("\\unknownMacro"); // the source survives into the output
  });
});

describe("category 1 — cross-references resolve to a link, else stay verbatim", () => {
  it("tokenizes a {ref} role and a [text](sec:…) link", () => {
    expect(tokenizeInline("see {ref}`sec:ri` and [the RI page](sec:essentialelements.ri)")).toEqual([
      { kind: "text", text: "see " },
      { kind: "xref", text: "sec:ri", label: "sec:ri", raw: "{ref}`sec:ri`" },
      { kind: "text", text: " and " },
      {
        kind: "xref",
        text: "the RI page",
        label: "sec:essentialelements.ri",
        raw: "[the RI page](sec:essentialelements.ri)",
      },
    ]);
  });
  it("{cite} is category 1 (→ a cite token); {eq} stays verbatim; neither is inline code", () => {
    expect(tokenizeInline("as in {cite}`barone1998, garcia` and {eq}`eqn:1`")).toEqual([
      { kind: "text", text: "as in " },
      { kind: "cite", keys: "barone1998, garcia", raw: "{cite}`barone1998, garcia`" },
      { kind: "text", text: " and " },
      { kind: "text", text: "{eq}`eqn:1`" }, // {eq} = an equation number we don't have → verbatim
    ]);
  });
  it("{cite}/{cite:t}`keys` renders as [keys] — the keys kept (stable + searchable), not [n]", () => {
    expect(reactText(renderManualBody("C-PCM{cite}`barone1998, garcia_neese_gcs`: the model"))).toBe(
      "C-PCM[barone1998, garcia_neese_gcs]: the model",
    );
    // the {cite} marker and backticks are gone; the keys survive
    const r = reactText(renderManualBody("see {cite:t}`Neese2020`"));
    expect(r).toBe("see [Neese2020]");
    expect(r).not.toContain("{cite");
    expect(r).not.toContain("`");
  });
  it("a RESOLVED link renders as the link text (source form dropped)", () => {
    const resolve = (l: string): AnchorTarget | null =>
      l === "sec:ri" ? { file: "contents/essentialelements/RI", section_id: 42 } : null;
    const rendered = reactText(renderManualBody("open [RI page](sec:ri).", { resolve }));
    expect(rendered).toBe("open RI page.");
  });
  it("an UNRESOLVED link stays verbatim text (no dead click)", () => {
    const rendered = reactText(renderManualBody("open [RI page](sec:ri).", { resolve: () => null }));
    expect(rendered).toBe("open [RI page](sec:ri).");
  });
  it("with no resolver at all, every cross-ref is verbatim (loss-free default)", () => {
    const body = "see {ref}`sec:x` and [y](sec:y)";
    expect(reactText(renderManualBody(body))).toBe(body);
  });
  it("xrefLabels collects labels from prose and directive bodies", () => {
    const body = "[a](sec:a)\n:::{note}\n{ref}`sec:b` and [c](tab:c)\n:::";
    expect(xrefLabels(body).sort()).toEqual(["sec:a", "sec:b", "tab:c"]);
  });
});

// ─────────────────────────────────────────────────────────────────────────────
// CATEGORY 3 — RECOGNIZED & DELIBERATELY HIDDEN. The named whitelist, and the
// guard that NOTHING outside it is hidden.
// ─────────────────────────────────────────────────────────────────────────────
describe("category 3 — the named hide-whitelist (and only it)", () => {
  it("isHiddenDirective: exactly index / tabularcolumns / (raw, latex), case-insensitive", () => {
    expect(isHiddenDirective("index", "Installation")).toBe(true);
    expect(isHiddenDirective("INDEX", "")).toBe(true);
    expect(isHiddenDirective("tabularcolumns", "|l|c|r|")).toBe(true);
    expect(isHiddenDirective("raw", "latex")).toBe(true);
    expect(isHiddenDirective("Raw", "LaTeX")).toBe(true);
    // NOT hidden:
    expect(isHiddenDirective("raw", "html")).toBe(false); // different output target — visible
    expect(isHiddenDirective("note", "")).toBe(false);
    expect(isHiddenDirective("figure", "../_images/x.png")).toBe(false);
  });

  it("{index} renders to nothing", () => {
    expect(strip(reactText(renderManualBody("```{index} Installation\n```")))).toBe("");
  });
  it("{tabularcolumns} renders to nothing", () => {
    expect(strip(reactText(renderManualBody(":::{tabularcolumns} |l|c|r|\n:::")))).toBe("");
  });
  it("({raw}, latex) renders to nothing", () => {
    expect(strip(reactText(renderManualBody(":::{raw} latex\n\\newpage\n:::")))).toBe("");
  });

  it("KEY IS THE PAIR: {raw} html is VISIBLE, shown verbatim (a 6.2 refresh could add it)", () => {
    const body = ":::{raw} html\n<b>keep me</b>\n:::";
    const rendered = reactText(renderManualBody(body));
    expect(rendered).toContain("<b>keep me</b>");
  });
  it("a directive OUTSIDE the whitelist ({note}) is NOT hidden — its content stays", () => {
    const rendered = reactText(renderManualBody(":::{note}\nkeep this note\n:::"));
    expect(rendered).toContain("keep this note");
  });

  // ── the FOURTH position: the MyST anchor label line `(name)=` (1438×, invisible) ──
  it("a whole-line MyST anchor label `(sec:…)=` renders to nothing", () => {
    const body = "(sec:essentialelements.solvationmodels)=\n## Solvation models\nprose";
    const rendered = reactText(renderManualBody(body));
    expect(rendered).not.toContain("sec:essentialelements.solvationmodels");
    expect(rendered).not.toContain("(");
    expect(rendered).toContain("Solvation models");
  });
  it("isAnchorLabelLine: exactly a whole trimmed `(name)=` with no inner parens", () => {
    expect(isAnchorLabelLine("(sec:foo)=")).toBe(true);
    expect(isAnchorLabelLine("  (tab:bar)=  ")).toBe(true); // trimmed
    expect(isAnchorLabelLine("(compoundPrintSpecifiers)=")).toBe(true); // a real bare label
    // NOT a label (the checkable boundary):
    expect(isAnchorLabelLine("(x)= y")).toBe(false); // mid-line: content after
    expect(isAnchorLabelLine("see (x)= here")).toBe(false); // not the whole line
    expect(isAnchorLabelLine("(a(b)c)=")).toBe(false); // inner parens
    expect(isAnchorLabelLine("(sec:foo)")).toBe(false); // no `=`
  });
  it("a `(x)= y` mid-line look-alike is NOT hidden (67 such in the corpus)", () => {
    const rendered = reactText(renderManualBody("the map (r)= 0 defines the origin"));
    expect(rendered).toContain("(r)= 0");
  });
  it("a `(x)=` INSIDE a ```orca fence is code, NOT a hidden label", () => {
    const body = "```orca\n(not_a_label)=\n! B3LYP\n```";
    const rendered = reactText(renderManualBody(body));
    expect(rendered).toContain("(not_a_label)=");
  });
});

// ─────────────────────────────────────────────────────────────────────────────
// CATEGORY 5 — external content not in our corpus: a visible ABSENCE MARKER.
// ─────────────────────────────────────────────────────────────────────────────
describe("{literalinclude} → a visible absence marker (never a silent path)", () => {
  it("flags the missing input example and names the path", () => {
    expect(isMissingInclude("literalinclude")).toBe(true);
    const rendered = reactText(
      renderManualBody("```{literalinclude} ../../examples/helloWorld.inp\n```"),
    );
    expect(rendered).toContain("input example not loaded");
    expect(rendered).toContain("helloWorld.inp");
  });
});
