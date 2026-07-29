/**
 * Viewer colour themes (2.5.2e-2). Pure / node-tested — no React, no `3dmol`.
 * `MoleculeViewer` takes a `ViewerTheme` and paints the background AND every
 * overlay colour (halo, atom-number labels, measurement line/label) from it.
 *
 * ## Why the overlay colours belong to the THEME, not the module
 *
 * 2.5.2e-1 hard-coded overlay colours in `MoleculeViewer` — including
 * `NUMBER_BG = "#0d0f13"`. On a light background that produces dark rectangles
 * behind every number; the halo `#ff2d95` and the amber measurement colour, both
 * picked for a near-black background, likewise lose contrast. So the overlay
 * palette has to move with the background. Each theme carries a matched set.
 *
 * ## Why PRESETS, not a free colour picker
 *
 * The invariant that makes this safe is a **contrast floor** (`contrastRatio`,
 * WCAG relative luminance): in every theme the halo, label text, and measurement
 * line clear 3:1 against their background — `theme.test.ts` asserts it. A free
 * background picker would let the user set, say, a magenta background that makes
 * the magenta halo vanish, and no test could catch an arbitrary user colour. A
 * fixed set of presets keeps the guarantee testable. (The four shared
 * `FRAGMENT_PALETTE` colours are ALSO contrast-tested per theme; see the test —
 * they fail on the light themes, a known limitation reported to the architect,
 * NOT silently patched, because the palette is shared with `FragmentList`.)
 */

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
}

/**
 * The four presets. **`dark` reproduces the pre-2.5.2e-2 look exactly**
 * (`#0d0f13` background, `#ff2d95` halo, `#e6e6e6` numbers, `#ffd34d` amber
 * measurement) so switching to it is a no-op and it stays the default. `black`
 * is the same overlay set on pure black; `light`/`white` swap in darker,
 * saturated overlay colours that clear 3:1 on a light background.
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

// ── WCAG contrast ─────────────────────────────────────────────────────────────

/** Parse `#rgb` / `#rrggbb` to `[r, g, b]` 0–255. */
function parseHex(hex: string): [number, number, number] {
  let h = hex.replace("#", "");
  if (h.length === 3)
    h = h[0] + h[0] + h[1] + h[1] + h[2] + h[2];
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
