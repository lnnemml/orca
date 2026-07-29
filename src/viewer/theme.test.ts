import { describe, it, expect } from "vitest";

import {
  VIEWER_THEMES,
  viewerTheme,
  DEFAULT_THEME,
  contrastRatio,
  relativeLuminance,
  hueOf,
  hueDistance,
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
  it("hueDistance wraps around 360°", () => {
    expect(hueDistance("#ff0000", "#ff0000")).toBeCloseTo(0, 6);
    expect(hueDistance("#ff0004", "#ff0000")).toBeLessThan(2); // 359°≈1°
    expect(hueDistance("#ff0000", "#00ff00")).toBeCloseTo(120, 0);
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

  it("dark keeps its background, CPK colours and palette (only halo/measurement moved)", () => {
    const dark = viewerTheme("dark");
    expect(dark.background).toBe("#0d0f13");
    expect(dark.elementColorOverrides).toEqual({}); // CPK untouched
    expect(dark.fragmentPalette).toEqual([...FRAGMENT_PALETTE]);
    // The ONLY e-3b change to dark: halo/measurement moved off pink (which
    // collided with 3Dmol's defaultColor #ff1493) onto the chartreuse band.
    expect(dark.haloColor).toBe("#adee2b");
    expect(dark.measurementLine).toBe("#b1eb70");
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
        expect(hueDistance(c, FRAGMENT_PALETTE[i])).toBeLessThanOrEqual(15);
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
        expect(hueDistance(color, CPK_ELEMENT_COLORS[el]), `${theme.id} ${el}`).toBeLessThanOrEqual(25);
      }
    }
  });

  it("includes the ADR-007 metals (Pd Pt Rh Ru Ir Os) — else 3Dmol paints them defaultColor", () => {
    for (const m of ["Pd", "Pt", "Rh", "Ru", "Ir", "Os"]) {
      expect(CPK_ELEMENT_COLORS[m], m).toBeDefined();
    }
  });
});

describe("cpkColorDrift (two-directional dup guard)", () => {
  // 3Dmol stores colours as 0xRRGGBB numbers; the guard normalises before compare.
  function asNumbers(): Record<string, number> {
    const out: Record<string, number> = {};
    for (const [el, hex] of Object.entries(CPK_ELEMENT_COLORS))
      out[el] = parseInt(hex.slice(1), 16);
    return out;
  }

  it("is empty in both directions against an identical reference", () => {
    expect(cpkColorDrift(asNumbers())).toEqual({ changed: [], missing: [] });
  });

  it("changed: names our keys the reference has with a different value", () => {
    const ref = asNumbers();
    ref.C = 0x123456;
    const drift = cpkColorDrift(ref);
    expect(drift.changed).toEqual(["C"]);
    expect(drift.missing).toEqual([]);
  });

  it("missing: names a reference element absent from our copy (the closed blind spot)", () => {
    const ref = asNumbers();
    ref.Rf = 0x123456; // 3Dmol gains an element we don't mirror
    const drift = cpkColorDrift(ref);
    expect(drift.missing).toEqual(["Rf"]);
    expect(drift.changed).toEqual([]);
  });

  it("ignores PDB uppercase aliases (HE/LI) in the missing direction", () => {
    const ref = asNumbers();
    ref.HE = ref.He; // 3Dmol keeps such aliases; we don't mirror them
    ref.LI = ref.Li;
    expect(cpkColorDrift(ref).missing).toEqual([]);
  });

  it("does NOT flag the Jmol-sourced metals as changed against defaultColors", () => {
    // The real app passes elementColors.defaultColors (= rasmol, NO metals). Our
    // metals came from Jmol; the reference lacking them must not be 'changed'.
    const ref = asNumbers();
    for (const m of ["Pd", "Pt", "Rh", "Ru", "Ir", "Os"]) delete ref[m];
    expect(cpkColorDrift(ref).changed).toEqual([]);
  });
});

// ── Distinctness invariant (2.5.2e-3b) — halo/measurement vs every element ────
describe("halo & measurement are ≥30° in hue from every element/palette colour", () => {
  const DEFAULT_COLOR = "#ff1493"; // 3Dmol's elementColors.defaultColor (off-table)
  const MIN_HUE = 30;

  for (const theme of VIEWER_THEMES) {
    it(`${theme.id}: halo and measurementLine clear the annotation band`, () => {
      // Element colours AS DRAWN in this theme (CPK with the theme's overrides).
      const drawn: Record<string, string> = {
        ...CPK_ELEMENT_COLORS,
        ...theme.elementColorOverrides,
      };
      const avoid = [
        ...Object.values(drawn),
        DEFAULT_COLOR,
        ...theme.fragmentPalette,
      ];
      for (const annotation of [theme.haloColor, theme.measurementLine]) {
        for (const other of avoid) {
          expect(
            hueDistance(annotation, other),
            `${theme.id} ${annotation} vs ${other}`,
          ).toBeGreaterThanOrEqual(MIN_HUE);
        }
      }
    });
  }
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
