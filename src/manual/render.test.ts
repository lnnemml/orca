import { describe, it, expect } from "vitest";
import { isValidElement, type ReactNode } from "react";

import { parseManualBody } from "./render";
import { renderManualBody } from "./SectionView";

/**
 * The text a DOM would expose as `textContent`: the concatenation of every string/
 * number leaf of the React tree, in order. React renders string/number children as
 * text nodes and inserts nothing between siblings, so this equals the rendered
 * element's `.textContent` — computed here without a DOM (no jsdom dependency).
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

// Samples chosen to hit the exact silent-loss failure modes named in the spec.
const SAMPLES: Record<string, string> = {
  "MyST directive": ":::{note}\nRIJCOSX needs an AuxJ basis.\n:::",
  "LaTeX math": "The $\\omega$B97M-V functional and $E = -76.4$ Eh.",
  "pipe table": "| Keyword | Meaning |\n|---|---|\n| `RIJCOSX` | hybrid speedup |",
  "ORCA fence with indentation":
    "Run it:\n\n```orca\n%scf\n    MaxIter 200\n    Convergence Tight\nend\n```\n\nafter the block.",
  "inline code + underscores": "Set `def2/J` and note free_energy_correction below.",
  "nested colon fences": ":::{admonition} Tip\n:::{note}\ninner\n:::\n:::",
  "unterminated fence": "```orca\n! B3LYP def2-SVP\n(no closing fence)",
  "mixed everything":
    "# Not-a-heading-in-body prose\n$\\alpha$ decay.\n\n```orca\n! Opt\n```\n\n:::{warning}\nmind the units\n:::",
};

describe("manual body render preserves everything it does not parse", () => {
  for (const [name, body] of Object.entries(SAMPLES)) {
    it(`preserves every non-whitespace char: ${name}`, () => {
      const rendered = reactText(renderManualBody(body));
      // Char-by-char: the rendered text, whitespace aside, is EXACTLY the body — no
      // char dropped, none invented, order kept. A stronger check than "didn't crash".
      expect(strip(rendered)).toBe(strip(body));
    });
  }

  it("blocks tile the body: every source line lands in exactly one block", () => {
    for (const body of Object.values(SAMPLES)) {
      const joined = parseManualBody(body)
        .map((b) => b.text)
        .join("\n");
      // fence/prose split drops no non-whitespace content
      expect(strip(joined)).toBe(strip(body));
    }
  });
});
