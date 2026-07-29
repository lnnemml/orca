import { describe, it, expect } from "vitest";

import {
  VIEWER_THEMES,
  viewerTheme,
  DEFAULT_THEME,
  contrastRatio,
  relativeLuminance,
  type ViewerTheme,
} from "./theme";
import { FRAGMENT_PALETTE } from "./fragment-colors";

/** WCAG's own reference values anchor the contrast maths. */
describe("contrastRatio (WCAG)", () => {
  it("black vs white is 21:1, a colour vs itself is 1:1", () => {
    expect(contrastRatio("#000000", "#ffffff")).toBeCloseTo(21, 2);
    expect(contrastRatio("#0d0f13", "#0d0f13")).toBeCloseTo(1, 6);
  });

  it("is symmetric and parses shorthand hex", () => {
    expect(contrastRatio("#fff", "#000")).toBeCloseTo(21, 2);
    expect(contrastRatio("#abc", "#123")).toBeCloseTo(
      contrastRatio("#123", "#abc"),
      12,
    );
  });

  it("luminance is monotone black → white", () => {
    expect(relativeLuminance("#000000")).toBeCloseTo(0, 6);
    expect(relativeLuminance("#ffffff")).toBeCloseTo(1, 6);
    expect(relativeLuminance("#808080")).toBeGreaterThan(0);
    expect(relativeLuminance("#808080")).toBeLessThan(1);
  });
});

describe("viewerTheme", () => {
  it("resolves each id and falls back to dark", () => {
    for (const t of VIEWER_THEMES) expect(viewerTheme(t.id).id).toBe(t.id);
    expect(viewerTheme(null).id).toBe("dark");
    expect(viewerTheme(undefined).id).toBe("dark");
    expect(viewerTheme("nonsense").id).toBe("dark");
    expect(DEFAULT_THEME.id).toBe("dark");
  });

  it("dark reproduces the pre-2.5.2e-2 look exactly (a no-op default)", () => {
    const dark = viewerTheme("dark");
    expect(dark.background).toBe("#0d0f13");
    expect(dark.haloColor).toBe("#ff2d95");
    expect(dark.measurementLine).toBe("#ffd34d");
  });
});

const AA_LARGE = 3; // WCAG 3:1 — the floor for graphical / large-text elements.

// ── The invariant: every theme's OWN overlay colours clear 3:1 ────────────────
// This is the value of the module — not the specific hexes. A preset can be
// re-tinted freely as long as it still passes here; a free colour picker could
// not (a user could set an invisible halo and no test would catch it).
describe("overlay colours clear the 3:1 contrast floor in every theme", () => {
  for (const theme of VIEWER_THEMES) {
    it(`${theme.id}: halo, label text, measurement line`, () => {
      expect(contrastRatio(theme.haloColor, theme.background)).toBeGreaterThanOrEqual(AA_LARGE);
      expect(contrastRatio(theme.labelText, theme.labelBg)).toBeGreaterThanOrEqual(AA_LARGE);
      expect(contrastRatio(theme.measurementLine, theme.background)).toBeGreaterThanOrEqual(AA_LARGE);
      // measurement text sits on the label backing, so check it there too.
      expect(contrastRatio(theme.measurementText, theme.labelBg)).toBeGreaterThanOrEqual(AA_LARGE);
    });
  }
});

// ── FRAGMENT_PALETTE contrast per theme — the reported limitation ─────────────
// The four fragment colours are SHARED with FragmentList, so this module must
// NOT re-tint them; it can only measure. They pass on the dark themes and FAIL
// on the light ones (yellow/teal/etc. on light backgrounds). The failure is
// PINNED here (exact set) so the suite stays green while the fact is on record
// for the architect's palette decision — and any change to palette or theme
// backgrounds that moves the line trips this test.
function paletteFailures(theme: ViewerTheme): string[] {
  return FRAGMENT_PALETTE.filter(
    (c) => contrastRatio(c, theme.background) < AA_LARGE,
  );
}

describe("FRAGMENT_PALETTE contrast per theme (shared palette — measured, not changed)", () => {
  it("passes on the dark themes (all four colours ≥ 3:1)", () => {
    expect(paletteFailures(viewerTheme("dark"))).toEqual([]);
    expect(paletteFailures(viewerTheme("black"))).toEqual([]);
  });

  it("FAILS on the light themes for ALL FOUR colours (known, reported)", () => {
    // Recorded ratios (bg #eceff3 / #ffffff): teal 1.61/1.86, coral 2.33/2.69,
    // gold 1.45/1.67, violet 2.36/2.72 — every one below 3:1.
    expect(paletteFailures(viewerTheme("light")).sort()).toEqual(
      [...FRAGMENT_PALETTE].sort(),
    );
    expect(paletteFailures(viewerTheme("white")).sort()).toEqual(
      [...FRAGMENT_PALETTE].sort(),
    );
  });
});
