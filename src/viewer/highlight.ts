/**
 * Selection-halo geometry (2.5.2e-1). Pure / node-tested — no import of `3dmol`
 * (its bundle needs `window` and can't load under the node test runner), no
 * React. `MoleculeViewer` uses `highlightRadius` to size the halo sphere it draws
 * over a selected atom.
 *
 * ## Why the halo radius must be a FUNCTION of the element
 *
 * 3Dmol draws each atom as `sphere: { scale: 0.3 }`, i.e. a sphere of
 * **`vdwRadii[element] * 0.3`** — its `GLModel.getRadiusFromStyle` looks the
 * element up in `GLModel.vdwRadii` and multiplies by the style scale. So the
 * drawn radius is element-dependent:
 *
 *     H 1.20·0.3 = 0.36 Å   O 1.52·0.3 = 0.456 Å
 *     N 1.55·0.3 = 0.465 Å  C 1.70·0.3 = 0.51 Å
 *
 * The old halo used a **constant** radius 0.55 Å, so the visible shell outside
 * the drawn sphere was 0.19 Å on H but only 0.04 Å on C — which is exactly why
 * the selection was visible on hydrogen and invisible on carbon. The fix is a
 * halo proportional to the drawn radius plus a **constant shell**, so every
 * element shows the same visible thickness. A bigger constant is not a fix (it
 * would be huge on H and still thin on C).
 *
 * ## The vdW table is a documented DUPLICATE of 3Dmol's
 *
 * The values below are transcribed verbatim from `GLModel.vdwRadii`
 * (`node_modules/3dmol/build/3Dmol.js`, 3Dmol v2.5.5) so the halo tracks the
 * radius 3Dmol actually draws. We copy rather than import because the `3dmol`
 * bundle can't be loaded by the node test runner (needs `window`/`document`).
 * **Divergence risk:** if a future 3Dmol version changes its table, this copy
 * goes stale — `MoleculeViewer` guards against that at runtime with
 * `assertVdwTableMatches` (dev-only, in the real webview where 3Dmol IS loaded).
 * Coverage matches 3Dmol's own: H–Kr plus Pd(46) and Pt(78) — the
 * cross-coupling metals ADR-007 names (Sonogashira/Suzuki/Buchwald–Hartwig) —
 * and the rest of 3Dmol's list. Off-table elements fall back to 1.5 Å, the SAME
 * `defaultSphereRadius` 3Dmol uses, so their halos track the drawn sphere too.
 */

import { normalizeElement } from "../scene/scene";

/** The ball-and-stick sphere scale `MoleculeViewer` draws with (`sphere.scale`).
 * The halo is computed against the SAME scale or it would not track. */
export const SPHERE_SCALE = 0.3;

/** 3Dmol's `defaultSphereRadius` — the drawn radius (before scale) for an
 * element absent from the table. Explicit, never NaN or a silent zero. */
export const FALLBACK_VDW = 1.5;

/** Constant shell added on top of the drawn radius, in Å — the visible halo
 * thickness, the same for every element. Empirically tuned by MiniBrowser
 * screenshot (see wiki/modules/visualization.md). */
export const HALO_MARGIN = 0.25;

/** Floor on the halo radius so even the smallest atom keeps a grabbable halo.
 * Rarely binds (all real elements clear it), a safety net for tiny/fallback. */
export const MIN_HALO = 0.5;

/**
 * van-der-Waals radii, Å — a verbatim copy of 3Dmol's `GLModel.vdwRadii`
 * (v2.5.5). See the module header for why it's duplicated and the drift guard.
 */
export const VDW_RADII: Readonly<Record<string, number>> = {
  H: 1.2, He: 1.4, Li: 1.82, Be: 1.53, B: 1.92, C: 1.7, N: 1.55, O: 1.52,
  F: 1.47, Ne: 1.54, Na: 2.27, Mg: 1.73, Al: 1.84, Si: 2.1, P: 1.8, S: 1.8,
  Cl: 1.75, Ar: 1.88, K: 2.75, Ca: 2.31, Ni: 1.63, Cu: 1.4, Zn: 1.39, Ga: 1.87,
  Ge: 2.11, As: 1.85, Se: 1.9, Br: 1.85, Kr: 2.02, Rb: 3.03, Sr: 2.49, Pd: 1.63,
  Ag: 1.72, Cd: 1.58, In: 1.93, Sn: 2.17, Sb: 2.06, Te: 2.06, I: 1.98, Xe: 2.16,
  Cs: 3.43, Ba: 2.68, Pt: 1.75, Au: 1.66, Hg: 1.55, Tl: 1.96, Pb: 2.02,
  Bi: 2.07, Po: 1.97, At: 2.02, Rn: 2.2, Fr: 3.48, Ra: 2.83, U: 1.86,
};

/** vdW radius (Å) for an element, case-insensitive; {@link FALLBACK_VDW} for an
 * element the table doesn't list (matching 3Dmol's own fallback). */
export function vdwRadius(element: string): number {
  return VDW_RADII[normalizeElement(element)] ?? FALLBACK_VDW;
}

/**
 * Halo radius (Å) for a selected atom: proportional to the DRAWN sphere radius
 * (`vdw * SPHERE_SCALE`) plus a constant shell, floored at {@link MIN_HALO}. A
 * constant shell (not a constant radius) gives the same visible halo thickness
 * on H and on C — the whole point of this unit.
 */
export function highlightRadius(element: string): number {
  return Math.max(MIN_HALO, vdwRadius(element) * SPHERE_SCALE + HALO_MARGIN);
}

/**
 * The elements where our {@link VDW_RADII} copy disagrees with a reference table
 * (3Dmol's live `GLModel.vdwRadii`, injected so this stays 3dmol-free and
 * node-testable). Empty ⇒ the copy is faithful. `MoleculeViewer` calls this once
 * in dev with the real table and warns on any drift — the active guard the
 * documented duplication needs.
 */
export function vdwTableDrift(
  referenceTable: Record<string, number | undefined>,
): string[] {
  return Object.keys(VDW_RADII).filter(
    (el) => referenceTable[el] !== VDW_RADII[el],
  );
}
