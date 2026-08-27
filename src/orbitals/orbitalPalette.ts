//! Orbital colour palette + selection logic — pure, node-tested, React-free (F2).
//!
//! F2 shows several MOs at once (HOMO–LUMO overlap for FMO analysis). Each selected MO
//! gets its own ± phase colour PAIR, assigned by the MO's position in the ORDERED
//! selection (selection order drives the palette, so a legend always matches). Pair 0 IS
//! the viewer's single-orbital default (blue/red) — so one selected frontier orbital is
//! drawn byte-identically to before F2 (see `ORBITAL_POS_COLOR`/`ORBITAL_NEG_COLOR` in
//! `viewer/MoleculeViewer.tsx`; these hexes MUST stay in sync with pair 0).

export interface OrbitalPhasePair {
  /** +phase lobe colour. */
  posColor: string;
  /** −phase lobe colour. */
  negColor: string;
}

/**
 * The ± phase colour pairs for simultaneous orbitals. **Pair 0 = the single-orbital
 * default** (blue `#3b6fd4` / red `#d43b3b`, identical to the viewer's `ORBITAL_POS_COLOR`
 * / `ORBITAL_NEG_COLOR`) so a lone frontier orbital is unchanged. Pairs 1–3 are chosen to
 * be distinguishable both from pair 0 and from each other (starting hexes; live-tunable —
 * Anton adjusts on the render gate m5 alongside `ORBITAL_MULTI_ISO_OPACITY`). Length ===
 * `MAX_ORBITALS`.
 */
export const ORBITAL_PHASE_PAIRS: readonly OrbitalPhasePair[] = [
  { posColor: "#3b6fd4", negColor: "#d43b3b" }, // 0 — default blue / red (must match the viewer)
  { posColor: "#2e9e6b", negColor: "#e08a1e" }, // 1 — green / amber
  { posColor: "#8e5bd0", negColor: "#e0d02e" }, // 2 — violet / yellow
  { posColor: "#28b0bf", negColor: "#c43b8f" }, // 3 — teal / magenta
];

/** The most orbitals drawn at once — a legibility cap (4 pairs above). */
export const MAX_ORBITALS = ORBITAL_PHASE_PAIRS.length;

/** One selected MO with the colour pair assigned to it by selection order. */
export interface AssignedOrbital extends OrbitalPhasePair {
  /** The MO number (index into the parsed orbital array). */
  mo: number;
}

/**
 * Assign each selected MO its colour pair BY POSITION in the ordered selection: the i-th
 * selected MO → `ORBITAL_PHASE_PAIRS[i]`. Selection order (not MO number) drives the
 * palette, so the on-molecule colours and the legend stay in lockstep, and removing an
 * earlier MO re-colours the rest deterministically. Inputs beyond `MAX_ORBITALS` are
 * ignored defensively (the toggle helper already caps, so this never trims in practice).
 */
export function assignPairs(selected: readonly number[]): AssignedOrbital[] {
  return selected.slice(0, MAX_ORBITALS).map((mo, i) => ({
    mo,
    posColor: ORBITAL_PHASE_PAIRS[i].posColor,
    negColor: ORBITAL_PHASE_PAIRS[i].negColor,
  }));
}

/**
 * Toggle an MO in the ordered selection:
 *   - already present → REMOVE it (the rest keep their order → `assignPairs` re-colours);
 *   - absent and under the cap → APPEND it (selection order preserved → it takes the next
 *     free pair);
 *   - absent and AT the cap → NO-OP (returns the unchanged array — the UI shows a "max N"
 *     hint rather than silently swapping an orbital out).
 * Pure: never mutates the input.
 */
export function toggleOrbital(selected: readonly number[], mo: number): number[] {
  if (selected.includes(mo)) return selected.filter((m) => m !== mo);
  if (selected.length >= MAX_ORBITALS) return [...selected];
  return [...selected, mo];
}

/** How the isosurfaces are drawn (F2b): `solid` translucent surface, or `mesh` wireframe. */
export type SurfaceStyle = "solid" | "mesh";

/**
 * The surface style to DEFAULT to for a given number of simultaneous orbitals: **`mesh` at
 * ≥2** (a wireframe doesn't occlude, so overlapping HOMO/LUMO lobes are both readable) and
 * **`solid` for one** (a lone orbital keeps its filled 0.85 look — unchanged from F2). This
 * is only the default: once the user toggles, their choice is sticky (owned in the panel).
 */
export function defaultSurfaceStyle(count: number): SurfaceStyle {
  return count >= 2 ? "mesh" : "solid";
}
