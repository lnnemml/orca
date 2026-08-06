/**
 * Van der Waals radii (Å) for **steric-clash detection** (Phase 4.2 Stage 3).
 * Pure / node-tested, no react / 3dmol / tauri import.
 *
 * ## This is a PHYSICAL table, cited — and deliberately separate from `highlight.ts`
 *
 * `src/viewer/highlight.ts` also holds a `VDW_RADII`, but for a different purpose:
 * it is a **verbatim mirror of 3Dmol's `GLModel.vdwRadii`** so a selection halo is
 * sized to track the sphere 3Dmol actually *draws* (a visual concern), and it falls
 * back to 1.5 Å for an unlisted element (a halo always needs *some* size). This
 * table is the **physical steric radius** from the primary literature, and an
 * element it does not cover is **UNDETERMINED** — the clash check *skips* that pair
 * and says so, it does not invent a number (domain rule #11: a quantity none of our
 * sources settles is UNDETERMINED, not guessed; rule #10: cite the run/source).
 *
 * The two tables must stay separate: `highlight.ts` follows 3Dmol (and its runtime
 * drift-guard); if a future 3Dmol changed a radius, `highlight.ts` would follow it
 * to keep halos tracking, but this table must NOT — it follows Bondi/Mantina/Alvarez.
 * One physical steric table, in ONE place (here); do not scatter clash radii.
 *
 * ## Sources (cited per group — not from memory)
 *
 * - **Bondi 1964** — A. Bondi, "van der Waals Volumes and Radii", *J. Phys. Chem.*
 *   **68**, 441 (1964). The main-group non-metals + noble gases + the handful of
 *   metals Bondi tabulated.
 * - **Mantina 2009** — M. Mantina et al., "Consistent van der Waals Radii for the
 *   Whole Main Group", *J. Phys. Chem. A* **113**, 5806 (2009). Fills main-group
 *   elements Bondi omitted — notably **B (1.92 Å)**, the mission reagent's centre
 *   (BH₄⁻), and Al/Ca/Be/Ga/Ge/Sn/Sb/Bi/In/Tl/Pb.
 * - **Alvarez 2013** — S. Alvarez, "A cartography of the van der Waals territories",
 *   *Dalton Trans.* **42**, 8617 (2013). The transition metals, **including Pd
 *   (2.10 Å) and Pt (2.13 Å)** — the cross-coupling metals ADR-007 names, which
 *   Bondi lacks. (Bondi's small metal radii are volume-derived and inconsistent
 *   with the non-bonded-contact radii used for steric checks; Alvarez's are the
 *   modern contact radii, so we take the whole d-block from one consistent source.)
 *
 * Anything OUTSIDE these three sources (e.g. the f-block, W, Mo, Nb, …) is absent
 * on purpose → UNDETERMINED. Adding one means citing it here, not guessing.
 */

import { normalizeElement } from "./scene";

/** The element → source attribution, so the docstring's claim is checkable in code
 * (a test asserts every `VDW_RADII` key has a source, and vice-versa). */
export type VdwSource = "Bondi1964" | "Mantina2009" | "Alvarez2013";

/** Bondi 1964 — main-group non-metals, noble gases, and the alkali/alkaline metals
 * Bondi tabulated. */
const BONDI_1964: Record<string, number> = {
  H: 1.2, C: 1.7, N: 1.55, O: 1.52, F: 1.47,
  P: 1.8, S: 1.8, Cl: 1.75, Br: 1.85, I: 1.98,
  Si: 2.1, As: 1.85, Se: 1.9, Te: 2.06,
  He: 1.4, Ne: 1.54, Ar: 1.88, Kr: 2.02, Xe: 2.16,
  Li: 1.82, Na: 2.27, K: 2.75, Mg: 1.73,
};

/** Mantina 2009 — main-group elements Bondi omitted (boron first — the BH₄⁻ centre). */
const MANTINA_2009: Record<string, number> = {
  B: 1.92, Al: 1.84, Be: 1.53, Ca: 2.31,
  Ga: 1.87, Ge: 2.11, In: 1.93, Sn: 2.17,
  Sb: 2.06, Tl: 1.96, Pb: 2.02, Bi: 2.07,
};

/** Alvarez 2013 — the transition metals we support (incl. the cross-coupling Pd/Pt). */
const ALVAREZ_2013: Record<string, number> = {
  Ti: 2.15, Cr: 2.07, Mn: 2.05, Fe: 2.05, Co: 2.0, Ni: 1.97, Cu: 1.96, Zn: 2.01,
  Ru: 2.13, Rh: 2.1, Pd: 2.1, Ag: 2.11,
  Ir: 2.13, Pt: 2.13, Au: 2.14,
};

/** Which source each element's radius comes from (for the coverage test + provenance). */
export const VDW_SOURCE: Readonly<Record<string, VdwSource>> = {
  ...Object.fromEntries(Object.keys(BONDI_1964).map((e) => [e, "Bondi1964"])),
  ...Object.fromEntries(Object.keys(MANTINA_2009).map((e) => [e, "Mantina2009"])),
  ...Object.fromEntries(Object.keys(ALVAREZ_2013).map((e) => [e, "Alvarez2013"])),
};

/**
 * Steric van der Waals radii (Å), cited per {@link VDW_SOURCE}. An element ABSENT
 * from this map is **UNDETERMINED** — {@link vdwRadius} returns `undefined`, and the
 * clash check skips (and surfaces) any pair touching it. Never a silent fallback.
 */
export const VDW_RADII: Readonly<Record<string, number>> = {
  ...BONDI_1964,
  ...MANTINA_2009,
  ...ALVAREZ_2013,
};

/**
 * Steric vdW radius (Å) for an element, case-insensitive. Returns **`undefined`**
 * for an element none of the cited sources cover — the caller MUST treat that as
 * UNDETERMINED (skip the pair, surface it), never as radius 0 or a guess.
 */
export function vdwRadius(element: string): number | undefined {
  return VDW_RADII[normalizeElement(element)];
}
