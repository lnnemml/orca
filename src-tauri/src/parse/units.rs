//! Canonical length unit, held by a type so a missed conversion cannot slip out.
//!
//! Measured (domain rule #11, `wiki/orca/parse-sources.md`): `.property.txt
//! $Geometry` and `.hess $atoms` coordinates are **Bohr**, while the rest of the
//! app — Scene, merged xyz, viewer, input generator, d/θ/φ, the xtb bridge — is
//! **Ångström**. A forgotten ×0.529 does not crash; it yields a molecule 1.889×
//! too large that still *looks* like a molecule. So the canonical unit is enforced
//! at the type level, the way ADR-010 brands index spaces.
//!
//! [`Angstrom`] lives in its **own** module with a **private** field. `property`
//! is a *sibling* module, not a descendant of `units`, so it cannot write the
//! tuple constructor `Angstrom(x)` — it must pick [`Angstrom::from_bohr`] (the
//! conversion) or [`Angstrom::from_angstrom`] (already canonical: input xyz,
//! `.xyz`, `orca_2json`). Choosing wrong is the one remaining bug, and the
//! geometry post-condition (`property::PropertyFile::verify_geometry`) is the test
//! that goes red when it happens (~1.889×).

use serde::Serialize;

/// A length in the app's canonical unit, Ångström. The inner value is private on
/// purpose — see the module docs.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Angstrom(f64);

impl Angstrom {
    /// CODATA 2018 Bohr radius in Å. The measured `.property.txt`/`.hess`
    /// geometry unit is Bohr; this is the factor that makes it canonical.
    pub const BOHR_TO_ANGSTROM: f64 = 0.529_177_210_903;

    /// The **conversion** path — the only way a Bohr number becomes a canonical
    /// length. Every `.property.txt`/`.hess` coordinate goes through here.
    pub fn from_bohr(bohr: f64) -> Self {
        Angstrom(bohr * Self::BOHR_TO_ANGSTROM)
    }

    /// The **already-canonical** path — for sources measured to be Å (the input
    /// xyz used as a reference, `.xyz`, `_trj.xyz`, `orca_2json`). Using this on a
    /// Bohr number is exactly the bug the geometry post-condition catches.
    pub fn from_angstrom(angstrom: f64) -> Self {
        Angstrom(angstrom)
    }

    /// The scalar value, in Å.
    pub fn angstrom(self) -> f64 {
        self.0
    }
}
