//! The CREST/QCG solvent-monomer library (Stage F F1c) — pure data, React-free.
//!
//! Each entry pairs the **ALPB solvent name CREST accepts** (`-alpb <alpbName>`) with the
//! **monomer geometry** written to `solvent.xyz`. These two are the ONLY solvents the QCG
//! probe actually ran (`wiki/orca/crest.md`): water (rung 0, benzoic acid) and methanol
//! (rung 1, BH₄⁻). The geometries are the **exact probed monomers** (rung0 `water.xyz`,
//! rung1 `methanol.xyz`) — not re-invented.
//!
//! The library is extensible, BUT (rule #10) every new `alpbName` must be **run once**
//! against a real CREST grow before it ships here — an ALPB name CREST rejects, or a
//! monomer that docks badly, is a "terminated normally, wrong chemistry" trap. Do not add a
//! solvent from a name list alone.

export interface CrestSolvent {
  /** What the user picks in the dropdown. */
  display: string;
  /** The `-alpb <name>` string CREST accepts (measured). */
  alpbName: string;
  /** The solvent monomer as a standard `.xyz` — written to `solvent.xyz` for QCG. */
  xyz: string;
}

/** Probed water monomer (rung 0). */
const WATER_XYZ = `3
water
O    0.000000    0.000000    0.117300
H    0.000000    0.757200   -0.469200
H    0.000000   -0.757200   -0.469200
`;

/** Probed methanol monomer (rung 1). */
const METHANOL_XYZ = `6
methanol
C   -0.046800    0.664900    0.000000
O   -0.046800   -0.758900    0.000000
H   -1.086800    0.977300    0.000000
H    0.436600    1.081100    0.888100
H    0.436600    1.081100   -0.888100
H    0.837900   -1.072900    0.000000
`;

/** Exactly the TWO probed solvents — water and methanol. Adding a third needs a real run first. */
export const SOLVENT_LIBRARY: CrestSolvent[] = [
  { display: "Water", alpbName: "water", xyz: WATER_XYZ },
  { display: "Methanol", alpbName: "methanol", xyz: METHANOL_XYZ },
];
