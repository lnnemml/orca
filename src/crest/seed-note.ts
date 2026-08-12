//! The honest, charge-aware label for a CREST/QCG grown cluster (Stage F F1c) — pure,
//! React-free. The grown cluster is a **geometry SEED for an ORCA re-opt, never a solvated
//! result** (`wiki/modules/crest-microsolvation.md`), and its `seed_energy_eh` is an
//! xtb-ALPB energy. Two honesty facts drive the label:
//!
//!  - **Nonzero intended charge → the WRONG species' energy.** QCG grows an ion's cluster
//!    NEUTRAL (four evidences, `wiki/orca/crest.md`), so for charge ≠ 0 the seed energy is
//!    the neutral cluster's — a wrong-charge seed that must be **loudly** flagged and
//!    re-optimized in ORCA at the real charge with SMD.
//!  - **Even neutral, it is coarse.** An xtb-ALPB shell is a screening geometry; a
//!    defensible energy comes from the ORCA re-opt (+ SMD), not from this number.

export interface CrestSeedNote {
  severity: "warning" | "note";
  text: string;
}

/**
 * The seed label for a grown cluster whose solute was submitted at `intendedCharge`. Always
 * present (the panel never shows the seed without it). A nonzero charge is a **warning** (the
 * grown-neutral wrong-species trap); a neutral solute is a **note** (still refine in SMD).
 */
export function crestSeedNote(intendedCharge: number): CrestSeedNote {
  if (intendedCharge !== 0) {
    return {
      severity: "warning",
      text:
        `QCG grew this cluster NEUTRAL, so its energy is the NEUTRAL species' — NOT the ` +
        `charge ${intendedCharge >= 0 ? `+${intendedCharge}` : intendedCharge} species you ` +
        `submitted. Use this as a GEOMETRY SEED only: refine the cluster in ORCA at charge ` +
        `${intendedCharge >= 0 ? `+${intendedCharge}` : intendedCharge} with SMD for a real ` +
        `solvated energy.`,
    };
  }
  return {
    severity: "note",
    text:
      `This is a coarse xtb-ALPB seed (screening geometry + energy), not a solvated result. ` +
      `Refine the cluster in ORCA with SMD for a defensible energy.`,
  };
}
