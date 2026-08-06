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
    it(`${theme.id}: halo, label text, measurement line/text, axis`, () => {
      expect(contrastRatio(theme.haloColor, theme.background)).toBeGreaterThanOrEqual(AA_LARGE);
      expect(contrastRatio(theme.labelText, theme.labelBg)).toBeGreaterThanOrEqual(AA_LARGE);
      expect(contrastRatio(theme.measurementLine, theme.background)).toBeGreaterThanOrEqual(AA_LARGE);
      expect(contrastRatio(theme.measurementText, theme.labelBg)).toBeGreaterThanOrEqual(AA_LARGE);
      expect(contrastRatio(theme.axisColor, theme.background)).toBeGreaterThanOrEqual(AA_LARGE);
    });
  }
});

// ── Rotation-axis colour is VISUALLY distinct (unit 3.3b-fix) ─────────────────
// The Axis⇄Distance toggle was imperceptible because the axis cylinder borrowed the
// green `haloColor` — indistinguishable from the green measurement line. The fix
// gives the axis its own azure accent; this locks it hue-far from every other
// overlay so the toggle reads as a real change (c1).
describe("rotation-axis colour is distinct from the other overlays", () => {
  // Reference overlay colours the axis must NOT be confused with. `CLASH_COLOR`
  // (#ff2d95) is `MoleculeViewer`'s magenta glow; #ff1493 is the off-table Pd/Pt
  // pink that once collided with the halo (see theme.ts / the log). Hardcoded with
  // a comment for the same reason `highlight.ts`'s vdW table is copied — the 3Dmol
  // consumer isn't importable under the node runner.
  const CLASH_MAGENTA = "#ff2d95";
  const PD_PT_PINK = "#ff1493";
  // The bug was the axis reading as the GREEN measurement line / halo, so those must
  // be a WHOLE different family (a large gap). Everything else must be clearly
  // distinct but need not be as far — the fragment palette flanks the blue with teal
  // AND violet, so the max achievable gap to the nearer of them is only ~42°; 30° is
  // a comfortable "clearly different hue" floor that both clear.
  const GREEN_FAMILY_GAP = 90; // axis vs the greens it was confused with
  const DISTINCT_GAP = 30; // axis vs every other overlay

  for (const theme of VIEWER_THEMES) {
    it(`${theme.id}: axis hue is a whole family off the greens, and clearly distinct from clash/pink/fragments`, () => {
      // The core of the fix: NOT the green measurement line / halo.
      expect(hueDistance(theme.axisColor, theme.haloColor)).toBeGreaterThan(GREEN_FAMILY_GAP);
      expect(hueDistance(theme.axisColor, theme.measurementLine)).toBeGreaterThan(GREEN_FAMILY_GAP);
      // And distinct from the other overlays that carry meaning.
      expect(hueDistance(theme.axisColor, CLASH_MAGENTA)).toBeGreaterThan(DISTINCT_GAP);
      expect(hueDistance(theme.axisColor, PD_PT_PINK)).toBeGreaterThan(DISTINCT_GAP);
      for (const c of theme.fragmentPalette) {
        expect(hueDistance(theme.axisColor, c)).toBeGreaterThan(DISTINCT_GAP);
      }
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

// ── Distinctness rule refined (2.5.2d-1): overlays split by whether they COEXIST ─
// The ≥30°-hue rule above is for overlays that mark DIFFERENT atoms/bonds (halo,
// measurement): they must not be mistaken for an atom's element colour. The
// selection halo and the edit-mask glow, by contrast, sit on the SAME atom by
// construction — the last-clicked atom is always in the mask — so they cannot be
// confused for *each other's* atom. They are distinguished by FORM (wireframe cage
// vs solid fill) and lightness, and the only colour requirement on a coexisting
// overlay is contrast against the background. So the mask deliberately reuses
// `theme.haloColor` — this is the rule, not an exception.
describe("coexisting overlays (halo + mask) are form-distinguished, not hue-distinguished", () => {
  for (const theme of VIEWER_THEMES) {
    it(`${theme.id}: the mask reuses the halo hue and both clear the background`, () => {
      // The mask IS `theme.haloColor` in MoleculeViewer (solid fill vs the halo's
      // wireframe cage). A coexisting overlay is required to clear the BACKGROUND,
      // not to differ in hue from the overlay it shares an atom with.
      expect(
        contrastRatio(theme.haloColor, theme.background),
      ).toBeGreaterThanOrEqual(AA_LARGE);
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
