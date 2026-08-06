//! Reader for the `orca_2json` gbw→JSON output — the fourth and last artifact
//! reader (ADR-012): molecular-orbital energies + occupancies (→ HOMO/LUMO) and
//! the final geometry. On the [`property`](super::property) template: typestate
//! (`parse → verify(reference) → Verified`), post-conditions-as-errors, units by
//! type. The **spawn** that produces the JSON is separate — `crate::orca_json`
//! (ADR-009); this module only reads a JSON file that already exists.
//!
//! # Rule #5 — the reason this reader streams
//! Measured (unit-3.7 gate, `parse-sources.md`): the JSON is dominated by
//! `MolecularOrbitals.MOs[].MOCoefficients`, an n×n matrix that is ~52–62% of the
//! file and grows as (basis functions)². A 50–60-atom def2-TZVP system extrapolates
//! to **tens of MB**. We need only two small per-MO arrays (`OrbitalEnergy`,
//! `Occupancy`). So the file is **streamed** with `serde_json::from_reader` into a
//! struct that simply **omits** `MOCoefficients`: serde consumes that field as
//! `IgnoredAny` (tokenized and discarded, never allocated into a `Vec`), so peak
//! memory is the small energy/occupancy arrays — not the whole file. Reading it
//! whole into a `serde_json::Value` would be the `.out` mistake wearing a JSON hat.
//!
//! # Units (measured literals)
//! `EnergyUnit = "Eh"` for orbital energies; `CoordinateUnits = "Angs"` for
//! `Atoms.Coords` — and those coords are the **final** geometry (not the input).

use std::fs::File;
use std::io::BufReader;
use std::path::Path;

use serde::Deserialize;

use orcastudio_core::ids::{IndexMap, OrcaIndex};

use super::units::Angstrom;
use super::{ParseError, ReferenceGeometry};

const GEOMETRY_TOL_ANGSTROM: f64 = 1e-4;

// --------------------------------------------------------------------------- //
// Layer 1 — streaming deserialize (MOCoefficients deliberately absent)          //
// --------------------------------------------------------------------------- //

#[derive(Deserialize)]
struct Root {
    #[serde(rename = "Molecule")]
    molecule: Molecule,
}

#[derive(Deserialize)]
struct Molecule {
    #[serde(rename = "Atoms")]
    atoms: Vec<AtomJ>,
    #[serde(rename = "MolecularOrbitals")]
    molecular_orbitals: MolecularOrbitals,
}

#[derive(Deserialize)]
struct AtomJ {
    #[serde(rename = "ElementNumber")]
    element_number: u8,
    #[serde(rename = "Coords")]
    coords: [f64; 3],
}

#[derive(Deserialize)]
struct MolecularOrbitals {
    #[serde(rename = "EnergyUnit")]
    energy_unit: Option<String>,
    #[serde(rename = "MOs")]
    mos: Vec<Mo>,
}

/// One MO. **No `MOCoefficients` field** — serde streams past it as `IgnoredAny`,
/// so the heavy n×n block never enters memory (rule #5; see the module docs).
#[derive(Deserialize)]
struct Mo {
    #[serde(rename = "OrbitalEnergy")]
    orbital_energy: f64,
    #[serde(rename = "Occupancy")]
    occupancy: f64,
}

/// A parsed gbw-JSON, unverified.
pub struct MoJson {
    root: Root,
}

impl MoJson {
    /// Stream-parse the JSON from disk (never loaded whole — see the module docs).
    pub fn from_path(path: &Path) -> Result<Self, ParseError> {
        let file = File::open(path).map_err(|e| ParseError::Io {
            path: path.display().to_string(),
            source: e,
        })?;
        let root: Root = serde_json::from_reader(BufReader::new(file)).map_err(|e| {
            ParseError::Malformed {
                field: "orca_2json".into(),
                detail: e.to_string(),
            }
        })?;
        Ok(MoJson { root })
    }

    /// Post-conditions, then [`Verified`]. The reference is the **final** geometry
    /// (e.g. the `.property.txt` final `$Geometry`) — orca_2json's coords are final.
    /// Unit 1d adds the job's `IndexMap<OrcaIndex>`: the element-order check becomes
    /// the map post-condition (identity map ⇒ the same check).
    pub fn verify(
        self,
        reference: &ReferenceGeometry,
        map: &IndexMap<OrcaIndex>,
    ) -> Result<Verified, ParseError> {
        let atoms = &self.root.molecule.atoms;
        // The map post-condition — the ONE per-atom function the seam touches here.
        // The `Atoms` element sequence (which HOMO/LUMO occupancies index) must equal
        // the order the map asserts (map.len() vs count is the length guard).
        let z: Vec<u8> = atoms.iter().map(|a| a.element_number).collect();
        super::check_map_order(&z, map, reference, "orca_2json Atoms")?;
        // geometry post-condition (rule #11): coords are Å (from_angstrom). A missed
        // conversion (from_bohr) would scale distances by 0.529 → caught. Compared
        // by interatomic distance, so a rigid reframe (if any) is tolerated.
        let coords: Vec<[f64; 3]> = atoms
            .iter()
            .map(|a| {
                [
                    Angstrom::from_angstrom(a.coords[0]).angstrom(),
                    Angstrom::from_angstrom(a.coords[1]).angstrom(),
                    Angstrom::from_angstrom(a.coords[2]).angstrom(),
                ]
            })
            .collect();
        let dist = |g: &[[f64; 3]], i: usize, j: usize| {
            let (a, b) = (g[i], g[j]);
            ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
        };
        let mut max_delta = 0.0_f64;
        for i in 0..coords.len() {
            for j in (i + 1)..coords.len() {
                max_delta =
                    max_delta.max((dist(&coords, i, j) - dist(&reference.xyz_angstrom, i, j)).abs());
            }
        }
        if max_delta >= GEOMETRY_TOL_ANGSTROM {
            return Err(ParseError::GeometryMismatch { max_delta });
        }
        Ok(Verified(self))
    }
}

/// A gbw-JSON whose post-conditions passed.
pub struct Verified(MoJson);

impl Verified {
    /// Per-MO `(energy_eh, occupancy)` in file order (ascending energy).
    pub fn orbitals(&self) -> Vec<(f64, f64)> {
        self.0
            .root
            .molecule
            .molecular_orbitals
            .mos
            .iter()
            .map(|m| (m.orbital_energy, m.occupancy))
            .collect()
    }

    /// HOMO = highest orbital with non-zero occupancy; LUMO = the next. Returns
    /// `(homo_energy_eh, lumo_energy_eh, gap_eh)`, or `None` if there is no
    /// occupied/virtual boundary (e.g. all-occupied — not a real molecule).
    pub fn homo_lumo(&self) -> Option<(f64, f64, f64)> {
        let orbitals = self.orbitals();
        let homo = orbitals.iter().rposition(|&(_, occ)| occ > 1e-6)?;
        let (h_e, _) = orbitals[homo];
        let (l_e, _) = *orbitals.get(homo + 1)?;
        Some((h_e, l_e, l_e - h_e))
    }

    pub fn energy_unit(&self) -> Option<&str> {
        self.0.root.molecule.molecular_orbitals.energy_unit.as_deref()
    }
}

#[cfg(test)]
mod tests;
