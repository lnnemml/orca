import { describe, it, expect } from "vitest";

import {
  VIEWER_THEMES,
  viewerTheme,
  DEFAULT_THEME,
  contrastRatio,
  relativeLuminance,
  hueOf,
  hueDelta,
  cpkColorDrift,
  CPK_ELEMENT_COLORS,
  type ViewerTheme,
} from "./theme";
import { FRAGMENT_PALETTE } from "./fragment-colors";

const AA_LARGE = 3; // WCAG 3:1 — the floor for graphical / large-text elements.

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

describe("hueOf", () => {
  it("reads primary/secondary hues and treats grey as 0", () => {
    expect(hueOf("#ff0000")).toBeCloseTo(0, 0);
    expect(hueOf("#00ff00")).toBeCloseTo(120, 0);
    expect(hueOf("#0000ff")).toBeCloseTo(240, 0);
    expect(hueOf("#808080")).toBe(0); // achromatic
  });
  it("hueDelta wraps around 360°", () => {
    expect(hueDelta("#ff0000", "#ff0000")).toBeCloseTo(0, 6);
    expect(hueDelta("#ff0004", "#ff0000")).toBeLessThan(2); // 359°≈1°
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

// ── Overlay contrast — every theme's own overlay colours clear 3:1 ────────────
describe("overlay colours clear the 3:1 contrast floor in every theme", () => {
  for (const theme of VIEWER_THEMES) {
    it(`${theme.id}: halo, label text, measurement line/text`, () => {
      expect(contrastRatio(theme.haloColor, theme.background)).toBeGreaterThanOrEqual(AA_LARGE);
      expect(contrastRatio(theme.labelText, theme.labelBg)).toBeGreaterThanOrEqual(AA_LARGE);
      expect(contrastRatio(theme.measurementLine, theme.background)).toBeGreaterThanOrEqual(AA_LARGE);
      expect(contrastRatio(theme.measurementText, theme.labelBg)).toBeGreaterThanOrEqual(AA_LARGE);
    });
  }
});

// ── Fragment palette is now a theme property (2.5.2e-3a) ──────────────────────
describe("per-theme fragment palette", () => {
  it("dark and black keep the global FRAGMENT_PALETTE exactly", () => {
    expect(viewerTheme("dark").fragmentPalette).toEqual([...FRAGMENT_PALETTE]);
    expect(viewerTheme("black").fragmentPalette).toEqual([...FRAGMENT_PALETTE]);
  });

  it("every theme's 4 fragment colours clear 3:1 against its background", () => {
    for (const theme of VIEWER_THEMES) {
      expect(theme.fragmentPalette).toHaveLength(4);
      for (const c of theme.fragmentPalette) {
        expect(contrastRatio(c, theme.background)).toBeGreaterThanOrEqual(AA_LARGE);
      }
    }
  });

  it("light/white variants share the dark hue within ±15° (same colour, darker)", () => {
    // The invariant that guards "same hue, different lightness" — not a literal.
    for (const id of ["light", "white"] as const) {
      const pal = viewerTheme(id).fragmentPalette;
      pal.forEach((c, i) => {
        expect(hueDelta(c, FRAGMENT_PALETTE[i])).toBeLessThanOrEqual(15);
      });
    }
  });
});

// ── CPK element-colour overrides (2.5.2e-3a — the widened invariant) ──────────
/** Elements whose default CPK colour fails 3:1 against `bg`. */
function cpkFailures(bg: string): string[] {
  return Object.keys(CPK_ELEMENT_COLORS)
    .filter((el) => contrastRatio(CPK_ELEMENT_COLORS[el], bg) < AA_LARGE)
    .sort();
}

describe("CPK element-colour overrides", () => {
  it("dark and black ship NO overrides (the current look is untouched)", () => {
    expect(viewerTheme("dark").elementColorOverrides).toEqual({});
    expect(viewerTheme("black").elementColorOverrides).toEqual({});
  });

  it("light/white override EXACTLY the failing set — no gaps, no redundant entries", () => {
    for (const id of ["light", "white"] as const) {
      const theme = viewerTheme(id);
      expect(Object.keys(theme.elementColorOverrides).sort()).toEqual(
        cpkFailures(theme.background),
      );
    }
  });

  it("every shipped override clears 3:1 against its theme background", () => {
    for (const theme of VIEWER_THEMES) {
      for (const [el, color] of Object.entries(theme.elementColorOverrides)) {
        expect(
          contrastRatio(color, theme.background),
          `${theme.id} ${el}`,
        ).toBeGreaterThanOrEqual(AA_LARGE);
      }
    }
  });

  it("no override is redundant — the element it replaces genuinely failed", () => {
    for (const theme of VIEWER_THEMES) {
      for (const el of Object.keys(theme.elementColorOverrides)) {
        expect(
          contrastRatio(CPK_ELEMENT_COLORS[el], theme.background),
          `${theme.id} ${el} did not actually fail`,
        ).toBeLessThan(AA_LARGE);
      }
    }
  });

  it("an override keeps the element's hue recognisable (≤25° from CPK, greys aside)", () => {
    // H/C become greys (hue undefined); the coloured elements keep their hue.
    const GREYSCALE = new Set(["H", "C"]);
    for (const theme of VIEWER_THEMES) {
      for (const [el, color] of Object.entries(theme.elementColorOverrides)) {
        if (GREYSCALE.has(el)) continue;
        expect(hueDelta(color, CPK_ELEMENT_COLORS[el]), `${theme.id} ${el}`).toBeLessThanOrEqual(25);
      }
    }
  });
});

describe("cpkColorDrift (the dup guard)", () => {
  // 3Dmol stores colours as 0xRRGGBB numbers; the guard normalises before compare.
  function asNumbers(): Record<string, number> {
    const out: Record<string, number> = {};
    for (const [el, hex] of Object.entries(CPK_ELEMENT_COLORS))
      out[el] = parseInt(hex.slice(1), 16);
    return out;
  }

  it("is empty against an identical (numeric) reference", () => {
    expect(cpkColorDrift(asNumbers())).toEqual([]);
  });

  it("names elements a reference disagrees on or omits", () => {
    const ref = asNumbers();
    ref.C = 0x123456;
    delete ref.H;
    expect(cpkColorDrift(ref).sort()).toEqual(["C", "H"]);
  });
});

// A guard so a future edit that reorders/renames the presets is noticed.
describe("preset set", () => {
  it("is exactly dark/black/light/white", () => {
    expect(VIEWER_THEMES.map((t: ViewerTheme) => t.id)).toEqual([
      "dark",
      "black",
      "light",
      "white",
    ]);
  });
});
