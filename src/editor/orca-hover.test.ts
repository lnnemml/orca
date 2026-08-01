import { describe, it, expect } from "vitest";

import { resolveHover } from "../manual/keyword-lookup";
import { buildHoverMarkdown } from "./orca-hover";

// Uses the real keywords.json — these are consumer-level assertions.
describe("hover markdown (buildHoverMarkdown over real resolveHover results)", () => {
  it("a simple keyword documented in several places → 'Documented in N places', no pick-first", () => {
    const m = resolveHover("! RIJCOSX\n", 0, 2, "RIJCOSX"); // RIJCOSX targets [RI#Keywords, RI#RIJCOSX]
    expect(m).not.toBeNull();
    const md = buildHoverMarkdown(m!);
    expect(md).toContain("**RIJCOSX** — simple keyword");
    expect(md).toMatch(/Documented in \*\*\d+\*\* places/);
    // every listed place is a command link that opens the drawer
    expect(md).toContain("command:orca.openManualSection?");
  });

  it("a block-option resolves to its OWNING block (%scf MaxIter), single section → Open", () => {
    const text = "%scf\n  MaxIter 200\nend\n";
    const m = resolveHover(text, 1, 2, "MaxIter");
    expect(m).not.toBeNull();
    expect(m!.kind).toBe("block-option");
    expect(m!.block).toBe("%scf");
    const md = buildHoverMarkdown(m!);
    expect(md).toContain("option of `%scf`");
    expect(md).toContain("Open in manual →");
  });

  it("a MISS returns null → the provider shows nothing (silence, per contract)", () => {
    // %maxcore has no block record; a hover on it must be silent, not 'nothing found'.
    expect(resolveHover("%maxcore 3000\n", 0, 0, "%maxcore")).toBeNull();
    // a bare value token in no block, not on the ! line → no hover case
    expect(resolveHover("%scf\nend\n", 1, 0, "end")).toBeNull();
  });
});
