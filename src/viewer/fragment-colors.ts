/**
 * The colour palette for non-primary scene fragments — the single source of
 * truth shared by the multi-fragment MoleculeViewer (2.5.0c) and the fragment
 * sidebar (2.5.0d), so a fragment reads as the same colour in both places.
 *
 * Fragment 0 (the substrate — the main object of study) is deliberately NOT in
 * the palette: it keeps 3Dmol's default CPK element colours, so a single-
 * fragment scene looks identical to the pre-2.5.0c renderer and adding a reagent
 * never recolours the substrate. Fragments 1+ cycle this palette.
 */

/** Teal, coral, gold, violet — distinct on the dark (#0d0f13) background. */
export const FRAGMENT_PALETTE = ["#2dd4bf", "#fb7185", "#fbbf24", "#a78bfa"] as const;

/**
 * Colour for fragment `fragmentIndex` (0-based). Returns `undefined` for
 * fragment 0 — the caller should leave it on CPK element colours — and a palette
 * colour (cycling) for fragments 1+.
 */
export function fragmentColor(fragmentIndex: number): string | undefined {
  if (fragmentIndex <= 0) return undefined;
  return FRAGMENT_PALETTE[(fragmentIndex - 1) % FRAGMENT_PALETTE.length];
}
