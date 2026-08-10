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

/// Fast-path equality: below this the `.gbw`/orca_2json geometry is byte-identical
/// to the property-final geometry — the Opt+Freq / scan common case (Freq recomputes
/// the wavefunction at the exact final geometry). Unchanged; no regression there.
const GEOMETRY_TOL_ANGSTROM: f64 = 1e-4;

/// Benign same-unit `.gbw` staleness on a plain `Opt` (no Freq): the wavefunction was
/// computed one opt-step from the final reported geometry, so the two geometries lag
/// by a small amount in the SAME unit (ratio ≈ 1). Measured **0.027 Å** on the real
/// MeNH₂+EtI forward/backward jobs (OptTS+Freq masked it — Freq re-solves at the exact
/// final geometry). Below this Δ, with ratio ≈ 1, the geometry is fine for MO rendering.
const GEOMETRY_STALENESS_TOL_ANGSTROM: f64 = 0.5;

/// Distances shorter than this are dropped from the median ratio — a tiny reference
/// distance turns division into noise. 0.9 Å sits just under the shortest real bond.
const MIN_DIST_FOR_RATIO_ANGSTROM: f64 = 0.9;

/// Fractional tolerance (±8%) on the ratio signatures below. The three bands
/// (0.529, 1.0, 1.889) are far apart, so this never overlaps.
const RATIO_TOL: f64 = 0.08;

/// Å per Bohr — a coordinate left in Bohr reads ≈0.529× the true Å distance.
const ANGSTROM_PER_BOHR: f64 = 0.529_177_210_903;
/// Bohr per Å — a skipped Bohr→Å conversion reads ≈1.889× the true Å distance.
const BOHR_PER_ANGSTROM: f64 = 1.0 / ANGSTROM_PER_BOHR;

/// Verdict of the geometry post-condition, classified from paired interatomic
/// distances (json vs reference). Distance-matrix based, so still translation- and
/// rotation-invariant. Pure ⇒ unit-testable without a full [`MoJson`].
#[derive(Debug, PartialEq)]
enum GeometryVerdict {
    /// Geometries agree — either the fast path (Δ < 1e-4, Opt+Freq) or benign
    /// same-unit `.gbw` staleness (ratio ≈ 1, Δ < the staleness tolerance).
    Pass,
    /// A missed Bohr↔Å conversion — the distance ratio matches ≈1.889 or ≈0.529.
    UnitError { ratio: f64 },
    /// A genuinely different same-unit structure (ratio ≈ 1, Δ over the staleness
    /// tolerance). NOT a unit error.
    Mismatch { max_delta: f64 },
}

/// Classify the geometry post-condition from `pairs = (dist_json, dist_ref)` over
/// every atom pair. Three-way (rule #11): fast-path equality → a Bohr↔Å ratio
/// signature (loud unit error) → benign same-unit staleness → genuine mismatch. The
/// ratio test is what lets benign `.gbw` staleness (ratio ≈ 1) pass while a real
/// missed conversion (ratio ≈ 1.889 / 0.529) still fails loudly.
fn classify_geometry(pairs: &[(f64, f64)]) -> GeometryVerdict {
    let max_delta = pairs
        .iter()
        .map(|(j, r)| (j - r).abs())
        .fold(0.0_f64, f64::max);

    // 1. Fast path — byte-identical (Opt+Freq / scan). Unchanged behaviour.
    if max_delta < GEOMETRY_TOL_ANGSTROM {
        return GeometryVerdict::Pass;
    }

    // Median distance ratio over non-tiny reference pairs (robust to the odd short
    // distance; the median, not the mean, so a few outliers cannot swing it).
    let mut ratios: Vec<f64> = pairs
        .iter()
        .filter(|(_, r)| *r > MIN_DIST_FOR_RATIO_ANGSTROM)
        .map(|(j, r)| j / r)
        .collect();
    let within = |x: f64, target: f64| (x - target).abs() <= RATIO_TOL * target;

    if let Some(r) = median(&mut ratios) {
        // 2. Bohr↔Å ratio signature ⇒ a real unit error, loud.
        if within(r, BOHR_PER_ANGSTROM) || within(r, ANGSTROM_PER_BOHR) {
            return GeometryVerdict::UnitError { ratio: r };
        }
        // 3. Same unit (ratio ≈ 1), small Δ ⇒ benign `.gbw` staleness.
        if max_delta < GEOMETRY_STALENESS_TOL_ANGSTROM && within(r, 1.0) {
            return GeometryVerdict::Pass;
        }
    }

    // 4. A genuinely different structure — NOT a unit error.
    GeometryVerdict::Mismatch { max_delta }
}

/// Median of a slice (sorts in place). `None` for an empty slice.
fn median(xs: &mut [f64]) -> Option<f64> {
    if xs.is_empty() {
        return None;
    }
    xs.sort_by(|a, b| a.partial_cmp(b).expect("interatomic distances are finite"));
    let n = xs.len();
    Some(if n % 2 == 1 {
        xs[n / 2]
    } else {
        (xs[n / 2 - 1] + xs[n / 2]) / 2.0
    })
}

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
        // Paired interatomic distances (json, reference) over every atom pair — the
        // distance matrix, translation/rotation invariant. `classify_geometry`
        // three-ways it: fast-path equality, the Bohr↔Å ratio signature (a real unit
        // error, loud), benign same-unit `.gbw` staleness, or a genuine mismatch.
        let mut pairs: Vec<(f64, f64)> = Vec::with_capacity(coords.len() * coords.len() / 2);
        for i in 0..coords.len() {
            for j in (i + 1)..coords.len() {
                pairs.push((dist(&coords, i, j), dist(&reference.xyz_angstrom, i, j)));
            }
        }
        match classify_geometry(&pairs) {
            GeometryVerdict::Pass => Ok(Verified(self)),
            GeometryVerdict::UnitError { ratio } => Err(ParseError::GeometryUnitError { ratio }),
            GeometryVerdict::Mismatch { max_delta } => {
                Err(ParseError::GeometryMismatch { max_delta })
            }
        }
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
