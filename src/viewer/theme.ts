/**
 * Viewer colour themes (2.5.2e-2; light-theme legibility fixed in 2.5.2e-3a).
 * Pure / node-tested — no React, no `3dmol`. `MoleculeViewer` takes a
 * `ViewerTheme` and paints the background AND every colour that sits on it:
 * overlay (halo, labels, measurement), the per-fragment palette, and CPK
 * element-colour overrides for atoms that would vanish against the background.
 *
 * ## Why the overlay colours belong to the THEME, not the module
 *
 * 2.5.2e-1 hard-coded overlay colours in `MoleculeViewer` — including
 * `NUMBER_BG = "#0d0f13"`. On a light background that produces dark rectangles;
 * the halo `#ff2d95` and the amber measurement colour, both picked for a
 * near-black background, likewise lose contrast. So the overlay palette moves
 * with the background. Each theme carries a matched set.
 *
 * ## Why PRESETS, not a free colour picker
 *
 * The invariant that makes this safe is a **contrast floor** (`contrastRatio`,
 * WCAG relative luminance): `theme.test.ts` asserts ≥3:1 against the background
 * for THREE families of on-canvas colour — the overlay, the fragment palette,
 * AND the CPK element colours. A free background picker could make any of them
 * vanish and no test could catch an arbitrary user colour. Fixed presets keep
 * the guarantee testable.
 *
 * ## What 2.5.2e-3a widened
 *
 * The e-2 contrast test only covered `FRAGMENT_PALETTE` (fragments 1+). But
 * fragment 0 is drawn in **CPK element colours**, and CPK hydrogen is white — on
 * a white background every hydrogen disappeared (the BH₄⁻ screenshot). The test
 * was honest but measured the wrong thing. e-3a widens the invariant to the CPK
 * table and to a per-theme fragment palette; the fix is a wider invariant, not a
 * rewritten one.
 */

import { FRAGMENT_PALETTE } from "./fragment-colors";

export interface ViewerTheme {
  id: "dark" | "black" | "light" | "white";
  label: string;
  /** 3Dmol canvas background. */
  background: string;
  /** Selection halo (wireframe sphere). */
  haloColor: string;
  /** Atom-number / measurement label text. */
  labelText: string;
  /** Backing rectangle behind a label. */
  labelBg: string;
  /** Measurement bond lines + the angle arc / dihedral axis. */
  measurementLine: string;
  /** Measurement value text (on `labelBg`). */
  measurementText: string;
  /**
   * Per-element CPK colour overrides for elements whose default colour would
   * fail 3:1 against this theme's background (empty for the dark themes, so they
   * are unchanged). `MoleculeViewer` merges these OVER 3Dmol's default element
   * colours so they land on fragment 0 (the CPK fragment).
   */
  elementColorOverrides: Readonly<Record<string, string>>;
  /**
   * Fragment palette (fragments 1+) for THIS theme. The dark themes keep the
   * global `FRAGMENT_PALETTE`; light/white use darker SAME-HUE variants. See the
   * per-theme-palette decision in the log: identity is held by hue, legibility
   * by lightness — so a swatch in the sidebar and a fragment in the viewer read
   * as the same colour at different brightness on the light themes.
   */
  fragmentPalette: readonly string[];
}

/**
 * 3Dmol's default CPK element colours — `elementColors.rasmol`, which
 * `elementColors.defaultColors` aliases (`node_modules/3dmol`, v2.5.5).
 * Transcribed verbatim for the same reason as `highlight.ts`'s vdW table: the
 * 3dmol bundle needs `window`/`document` and can't load under the node test
 * runner. `cpkColorDrift` guards this copy against a 3Dmol upgrade at runtime
 * (dev, in the real webview where 3Dmol IS loaded).
 */
export const CPK_ELEMENT_COLORS: Readonly<Record<string, string>> = {
  H: "#ffffff", He: "#ffc0cb", Li: "#b22222", B: "#00ff00", C: "#c8c8c8",
  N: "#8f8fff", O: "#f00000", F: "#daa520", Na: "#0000ff", Mg: "#228b22",
  Al: "#808090", Si: "#daa520", P: "#ffa500", S: "#ffc832", Cl: "#00ff00",
  Ca: "#808090", Ti: "#808090", Cr: "#808090", Mn: "#808090", Fe: "#ffa500",
  Ni: "#a52a2a", Cu: "#a52a2a", Zn: "#a52a2a", Br: "#a52a2a", Ag: "#808090",
  I: "#a020f0", Ba: "#ffa500", Au: "#daa520",
};

/**
 * CPK overrides for the light themes — the 13 elements whose default colour
 * fails 3:1 against `#eceff3` (the harder of the two light backgrounds). Each is
 * the SAME hue, only darker (hydrogen and carbon become greys). Tuned to clear
 * 3.2:1 against `#eceff3`, which also clears white. Shared by `light` and
 * `white` because their failing sets are identical (both 13; `theme.test.ts`
 * asserts the override keys equal the computed failing set per theme).
 */
const LIGHT_ELEMENT_OVERRIDES: Readonly<Record<string, string>> = {
  H: "#838383", He: "#ff274d", B: "#009900", C: "#848484", N: "#7373ff",
  F: "#a77e18", Si: "#a77e18", P: "#b87700", S: "#aa7c00", Cl: "#009900",
  Fe: "#b87700", Ba: "#b87700", Au: "#a77e18",
};

/** Fragment palette for the light themes — the four `FRAGMENT_PALETTE` hues at
 * lower lightness, each ≥3:1 against both light backgrounds and within ±15° of
 * its dark counterpart's hue (`theme.test.ts` locks both). */
const LIGHT_FRAGMENT_PALETTE = ["#0f766e", "#e11d48", "#a16207", "#7c3aed"] as const;

/**
 * The four presets. **`dark` reproduces the pre-2.5.2e-2 look exactly** — empty
 * `elementColorOverrides` and the global `FRAGMENT_PALETTE` — so switching to it
 * is a no-op and it stays the default. `black` is the same overlay set on pure
 * black; `light`/`white` swap in darker overlay colours, CPK overrides, and the
 * darker fragment palette that all clear 3:1 on a light background.
 */
export const VIEWER_THEMES: readonly ViewerTheme[] = [
  {
    id: "dark",
    label: "Dark",
    background: "#0d0f13",
    haloColor: "#ff2d95",
    labelText: "#e6e6e6",
    labelBg: "#0d0f13",
    measurementLine: "#ffd34d",
    measurementText: "#ffd34d",
    elementColorOverrides: {},
    fragmentPalette: FRAGMENT_PALETTE,
  },
  {
    id: "black",
    label: "Black",
    background: "#000000",
    haloColor: "#ff2d95",
    labelText: "#e6e6e6",
    labelBg: "#000000",
    measurementLine: "#ffd34d",
    measurementText: "#ffd34d",
    elementColorOverrides: {},
    fragmentPalette: FRAGMENT_PALETTE,
  },
  {
    id: "light",
    label: "Light",
    background: "#eceff3",
    haloColor: "#c2185b",
    labelText: "#12151a",
    labelBg: "#f4f5f7",
    measurementLine: "#b45309",
    measurementText: "#b45309",
    elementColorOverrides: LIGHT_ELEMENT_OVERRIDES,
    fragmentPalette: LIGHT_FRAGMENT_PALETTE,
  },
  {
    id: "white",
    label: "White",
    background: "#ffffff",
    haloColor: "#c2185b",
    labelText: "#12151a",
    labelBg: "#eef0f2",
    measurementLine: "#b45309",
    measurementText: "#b45309",
    elementColorOverrides: LIGHT_ELEMENT_OVERRIDES,
    fragmentPalette: LIGHT_FRAGMENT_PALETTE,
  },
] as const;

/** The default theme — `dark`, the pre-2.5.2e-2 look. */
export const DEFAULT_THEME: ViewerTheme = VIEWER_THEMES[0];

/**
 * Resolve a persisted theme id to a theme; unknown / null / undefined → `dark`.
 * Never throws — the settings value is user/DB data.
 */
export function viewerTheme(id: string | null | undefined): ViewerTheme {
  return VIEWER_THEMES.find((t) => t.id === id) ?? DEFAULT_THEME;
}

// ── Colour maths ──────────────────────────────────────────────────────────────

/** Parse `#rgb` / `#rrggbb` to `[r, g, b]` 0–255. */
function parseHex(hex: string): [number, number, number] {
  let h = hex.replace("#", "");
  if (h.length === 3) h = h[0] + h[0] + h[1] + h[1] + h[2] + h[2];
  return [
    parseInt(h.slice(0, 2), 16),
    parseInt(h.slice(2, 4), 16),
    parseInt(h.slice(4, 6), 16),
  ];
}

/** sRGB channel (0–255) → linear, per WCAG. */
function toLinear(channel: number): number {
  const c = channel / 255;
  return c <= 0.03928 ? c / 12.92 : Math.pow((c + 0.055) / 1.055, 2.4);
}

/** WCAG relative luminance of a hex colour. */
export function relativeLuminance(hex: string): number {
  const [r, g, b] = parseHex(hex);
  return 0.2126 * toLinear(r) + 0.7152 * toLinear(g) + 0.0722 * toLinear(b);
}

/**
 * WCAG contrast ratio between two colours: `(L_light + 0.05) / (L_dark + 0.05)`,
 * in `[1, 21]`. Symmetric in its arguments.
 */
export function contrastRatio(hexA: string, hexB: string): number {
  const la = relativeLuminance(hexA);
  const lb = relativeLuminance(hexB);
  const hi = Math.max(la, lb);
  const lo = Math.min(la, lb);
  return (hi + 0.05) / (lo + 0.05);
}

/**
 * Hue angle (0–360°) of a hex colour — for the per-theme-palette invariant (a
 * light variant must share its dark counterpart's hue, only be darker).
 * Achromatic (grey) → 0.
 */
export function hueOf(hex: string): number {
  const [r, g, b] = parseHex(hex).map((v) => v / 255);
  const max = Math.max(r, g, b);
  const min = Math.min(r, g, b);
  const d = max - min;
  if (d === 0) return 0;
  let h: number;
  if (max === r) h = ((g - b) / d) % 6;
  else if (max === g) h = (b - r) / d + 2;
  else h = (r - g) / d + 4;
  h *= 60;
  return (h + 360) % 360;
}

/** Smallest absolute difference between two hue angles, 0–180°. */
export function hueDelta(a: string, b: string): number {
  const d = Math.abs(hueOf(a) - hueOf(b));
  return Math.min(d, 360 - d);
}

/**
 * Elements where our {@link CPK_ELEMENT_COLORS} copy disagrees with a reference
 * table (3Dmol's live `elementColors.defaultColors`, whose values are
 * `0xRRGGBB` numbers), injected so this stays 3dmol-free and node-testable.
 * Empty ⇒ the copy is faithful. `MoleculeViewer` calls it once in dev.
 */
export function cpkColorDrift(
  reference: Record<string, string | number | undefined>,
): string[] {
  return Object.keys(CPK_ELEMENT_COLORS).filter((el) => {
    const ref = reference[el];
    const refHex =
      typeof ref === "number"
        ? "#" + (ref & 0xffffff).toString(16).padStart(6, "0")
        : ref;
    return (refHex ?? "").toLowerCase() !== CPK_ELEMENT_COLORS[el].toLowerCase();
  });
}
