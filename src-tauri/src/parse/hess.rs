//! Reader for ORCA's `.hess` — the second artifact reader (ADR-012): vibrational
//! frequencies, IR intensities, and normal modes.
//!
//! It follows the [`property`](super::property) **template** — same external
//! contract — but the grammar differs, and one post-condition had to bend; both
//! are called out here.
//!
//! # Same as the template
//! - **Two layers**: a generic tokenizer that knows only the grammar, then typed
//!   accessors that convert to canonical units at the boundary (rule #11).
//! - **Typestate**: [`HessFile::parse`] returns an *unverified* handle with **no
//!   value accessors**; [`HessFile::verify`] runs the post-conditions and returns
//!   [`Verified`], the only type that exposes values (rule #9, post-condition on
//!   the path).
//! - **Unknown sections stay visible** ([`HessFile::unknown_section_names`], rule #10).
//! - **Post-conditions are errors, not warnings.**
//!
//! # Different from the template
//! - **Grammar**: `.property.txt` is uniform `$Block`/`&prop`; `.hess` is
//!   `$section` headers, each with its **own** shape (a count then rows, a
//!   dimension line then column-blocks, a bare scalar). The tokenizer collects raw
//!   lines per section; each typed accessor knows its section's shape.
//! - **Geometry post-condition is distance-based, not coordinate-based.** Measured:
//!   `$atoms` is the Freq geometry **rigidly reframed** — a **pure centre-of-mass
//!   translation, no rotation** (measured: unit-3.12 Kabsch gate, `max|R−I| ≤ 3e-13`
//!   on all three jobs incl. the asymmetric 33-atom one; the raw per-atom shift is
//!   identical for every atom — the signature of a translation). On the asymmetric
//!   saddle every atom is shifted by a uniform ~1.10 Å vs the input frame, while
//!   symmetric ethane shows 0. A coordinate compare would false-alarm on that
//!   translation. Interatomic **distances** are translation/rotation invariant, so
//!   we compare those: a missed Bohr→Å still fails loudly (every distance ×1.889 →
//!   6.6 Å off on the saddle), a reframe passes (4e-8 Å). The gate's other payoff:
//!   because the reframe carries **no rotation**, `$normal_modes` are added to the
//!   reference geometry **as-is** for animation (no mode rotation owed at any
//!   boundary). See `wiki/orca/parse-sources.md`.
//!
//! # Units (all measured — `parse-sources.md`)
//! `$atoms` coords **Bohr** → Å; `$vibrational_frequencies` **cm⁻¹** (signed);
//! `$normal_modes` **Cartesian** normalized displacement (unit-3.6 gate: pltvib÷raw
//! = 2.0 for every ethane atom, H/C = 1.0 not √12 → Cartesian, so **no ÷√m**);
//! `$ir_spectrum` col2 intensity **km/mol**. `$hessian` and `$dipole_derivatives`
//! have **UNDETERMINED** units — they are recognized but **deliberately not read**,
//! so no value here is ever shown with a unit taken on faith.

use std::path::Path;

use serde::Serialize;

use orcastudio_core::ids::{IndexMap, OrcaIndex};

use super::elements::{strip_fragment_suffix, z_of};
use super::units::Angstrom;
use super::{ParseError, ReferenceGeometry};

/// Measured max is ≈ 150 KB (saddle `99e805f5`); 16 MB is a wide safety margin.
const MAX_BYTES: u64 = 16 * 1024 * 1024;

/// Distance post-condition tolerance (rule #11): a missed Bohr→Å conversion shows
/// up as whole-Ångström distance errors, far above this.
const DISTANCE_TOL_ANGSTROM: f64 = 1e-4;

// --------------------------------------------------------------------------- //
// Layer 1 — generic tokenizer                                                   //
// --------------------------------------------------------------------------- //

/// A `$section` and the raw lines under it (until the next `$section`).
#[derive(Clone, Debug)]
pub struct RawSection {
    pub name: String,
    pub lines: Vec<String>,
}

impl RawSection {
    /// The first non-empty line's tokens — usually the count / dimension line.
    fn header_tokens(&self) -> Vec<&str> {
        self.lines
            .iter()
            .find(|l| !l.trim().is_empty())
            .map(|l| l.split_whitespace().collect())
            .unwrap_or_default()
    }

    /// Data rows after the header line (skips blanks). Each returned as tokens.
    fn data_rows(&self) -> Vec<Vec<f64>> {
        let mut seen_header = false;
        let mut out = Vec::new();
        for l in &self.lines {
            if l.trim().is_empty() {
                continue;
            }
            if !seen_header {
                seen_header = true; // the count/dim line
                continue;
            }
            let nums: Vec<f64> = l.split_whitespace().filter_map(|t| t.parse().ok()).collect();
            if !nums.is_empty() {
                out.push(nums);
            }
        }
        out
    }

    /// A single scalar section (`$actual_temperature`, `$frequency_scale_factor`).
    fn scalar(&self) -> Option<f64> {
        self.lines
            .iter()
            .find_map(|l| l.split_whitespace().next().and_then(|t| t.parse().ok()))
    }
}

/// A parsed `.hess`: the ordered sections the tokenizer saw. Unverified.
#[derive(Clone, Debug)]
pub struct HessFile {
    pub sections: Vec<RawSection>,
}

/// Sections the typed layer interprets or deliberately ignores. `hessian` and
/// `dipole_derivatives` are recognized but **not read** (UNDETERMINED units).
/// Anything not here is surfaced by [`HessFile::unknown_section_names`] (rule #10).
const KNOWN_SECTIONS: &[&str] = &[
    // interpreted
    "atoms",
    "vibrational_frequencies",
    "normal_modes",
    "ir_spectrum",
    "actual_temperature",
    "frequency_scale_factor",
    // recognized but deliberately not read
    "orca_hessian_file",
    "act_atom",
    "act_coord",
    "act_energy",
    "multiplicity",
    "hessian",           // UNDETERMINED units
    "dipole_derivatives", // UNDETERMINED units
    "end",
];

impl HessFile {
    pub fn from_path(path: &Path) -> Result<Self, ParseError> {
        let meta = std::fs::metadata(path).map_err(|e| ParseError::Io {
            path: path.display().to_string(),
            source: e,
        })?;
        if meta.len() > MAX_BYTES {
            return Err(ParseError::TooLarge {
                artifact: "hess",
                bytes: meta.len(),
                cap: MAX_BYTES,
            });
        }
        let text = std::fs::read_to_string(path).map_err(|e| ParseError::Io {
            path: path.display().to_string(),
            source: e,
        })?;
        Ok(Self::parse(&text))
    }

    /// The generic tokenizer: split into `$section` blocks. Knows the grammar
    /// (sections + their raw lines), not any section's meaning.
    pub fn parse(text: &str) -> Self {
        let mut sections: Vec<RawSection> = Vec::new();
        let mut current: Option<RawSection> = None;
        for raw in text.lines() {
            let line = raw.trim_end();
            let trimmed = line.trim_start();
            if let Some(name) = trimmed.strip_prefix('$') {
                if let Some(s) = current.take() {
                    sections.push(s);
                }
                current = Some(RawSection {
                    name: name.trim().to_string(),
                    lines: Vec::new(),
                });
            } else if let Some(s) = current.as_mut() {
                s.lines.push(line.to_string());
            }
        }
        if let Some(s) = current.take() {
            sections.push(s);
        }
        HessFile { sections }
    }

    /// Run the post-conditions; only on success return [`Verified`], the sole type
    /// with value accessors. The caller supplies the reference geometry **and** the
    /// job's `IndexMap<OrcaIndex>` (unit 1d): the former-standalone element-order
    /// check is now the map post-condition — for the identity map the same check,
    /// so `$normal_modes` still animate the same atoms (see `check_map_order`).
    pub fn verify(
        self,
        reference: &ReferenceGeometry,
        map: &IndexMap<OrcaIndex>,
    ) -> Result<Verified, ParseError> {
        self.check_map_order(reference, map)?;
        self.check_geometry_distances(reference)?;
        self.check_lengths()?;
        self.check_zero_modes()?;
        Ok(Verified(self))
    }

    /// Sections with no accessor and not deliberately ignored (rule #10). Stays on
    /// the unverified handle so diagnostics work even when verification fails.
    pub fn unknown_section_names(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for s in &self.sections {
            if !KNOWN_SECTIONS.contains(&s.name.as_str()) && !out.contains(&s.name) {
                out.push(s.name.clone());
            }
        }
        out
    }

    fn section(&self, name: &str) -> Option<&RawSection> {
        self.sections.iter().find(|s| s.name == name)
    }

    // ---- interpreted sections (private; exposed via Verified) -------------- //

    /// `$atoms`: `SYM mass x y z` per atom. Coords Bohr → Å; mass is skipped (an
    /// unneeded, unverified-unit column). Elements are the file's order source.
    fn atoms(&self) -> Result<Vec<HessAtom>, ParseError> {
        let s = self
            .section("atoms")
            .ok_or_else(|| ParseError::MissingField("$atoms".into()))?;
        let mut out = Vec::new();
        for row in &s.lines {
            let toks: Vec<&str> = row.split_whitespace().collect();
            if toks.len() < 5 {
                continue; // the count line / blanks
            }
            let sym = strip_fragment_suffix(toks[0]).to_string();
            let Some(z) = z_of(&sym) else { continue };
            let (Ok(x), Ok(y), Ok(zc)) =
                (toks[2].parse::<f64>(), toks[3].parse::<f64>(), toks[4].parse::<f64>())
            else {
                continue;
            };
            out.push(HessAtom {
                element: sym,
                z,
                // rule #11: Bohr → Å at the boundary.
                xyz: [Angstrom::from_bohr(x), Angstrom::from_bohr(y), Angstrom::from_bohr(zc)],
            });
        }
        if out.is_empty() {
            return Err(ParseError::MissingField("$atoms rows".into()));
        }
        Ok(out)
    }

    fn frequencies(&self) -> Result<Frequencies, ParseError> {
        let s = self
            .section("vibrational_frequencies")
            .ok_or_else(|| ParseError::MissingField("$vibrational_frequencies".into()))?;
        let values_cm: Vec<f64> = s.data_rows().iter().filter_map(|r| r.last().copied()).collect();
        let imaginary_count = values_cm.iter().filter(|&&v| v < 0.0).count();
        // Measured: the 6 (or 5, linear) translation/rotation modes are printed as
        // EXACTLY 0.0 — an exact-zero test, no threshold.
        let zero_count = values_cm.iter().filter(|&&v| v == 0.0).count();
        Ok(Frequencies {
            values_cm,
            imaginary_count,
            zero_count,
            is_linear: zero_count == 5,
        })
    }

    /// `$normal_modes`: `3N 3N` dim, then column-blocks (`col-index header`, then
    /// `rowidx v v …` rows). Assembled into a row-major 3N×3N matrix.
    fn normal_modes(&self) -> Result<NormalModes, ParseError> {
        let s = self
            .section("normal_modes")
            .ok_or_else(|| ParseError::MissingField("$normal_modes".into()))?;
        let dim_tokens = s.header_tokens();
        let n: usize = dim_tokens
            .first()
            .and_then(|t| t.parse().ok())
            .ok_or_else(|| ParseError::Malformed {
                field: "$normal_modes".into(),
                detail: "missing dimension line".into(),
            })?;
        let mut data = vec![0.0_f64; n * n];
        let mut seen_dim = false;
        let mut cols: Vec<usize> = Vec::new();
        for l in &s.lines {
            let toks: Vec<&str> = l.split_whitespace().collect();
            if toks.is_empty() {
                continue;
            }
            if !seen_dim {
                seen_dim = true; // the `3N 3N` line
                continue;
            }
            // A column-index header: every token parses as usize and there are no
            // decimals. A data row: first token is the row index, rest are floats.
            let all_ints = toks.iter().all(|t| t.parse::<usize>().is_ok());
            if all_ints && toks.len() <= n {
                cols = toks.iter().map(|t| t.parse().unwrap()).collect();
                continue;
            }
            let Ok(row) = toks[0].parse::<usize>() else { continue };
            for (c, val) in cols.iter().zip(&toks[1..]) {
                if let (true, Ok(v)) = (row < n && *c < n, val.parse::<f64>()) {
                    data[row * n + c] = v;
                }
            }
        }
        Ok(NormalModes { n, data })
    }

    /// `$ir_spectrum`: measured columns `freq(cm⁻¹) T²(a.u.) Int(km/mol) TX TY TZ`.
    fn ir_spectrum(&self) -> Result<Vec<IrRow>, ParseError> {
        let s = self
            .section("ir_spectrum")
            .ok_or_else(|| ParseError::MissingField("$ir_spectrum".into()))?;
        let mut out = Vec::new();
        for r in s.data_rows() {
            if r.len() >= 3 {
                out.push(IrRow {
                    frequency_cm: r[0],
                    t2_au: r[1],
                    intensity_km_mol: r[2],
                    t_au: [
                        *r.get(3).unwrap_or(&0.0),
                        *r.get(4).unwrap_or(&0.0),
                        *r.get(5).unwrap_or(&0.0),
                    ],
                });
            }
        }
        Ok(out)
    }

    // ---- post-conditions --------------------------------------------------- //

    /// The unit-1d map post-condition — the ONE per-atom function the seam touches
    /// here. `$atoms`' element sequence (the artifact order the normal modes and IR
    /// rows are indexed by) must equal the order the `IndexMap` asserts.
    fn check_map_order(
        &self,
        reference: &ReferenceGeometry,
        map: &IndexMap<OrcaIndex>,
    ) -> Result<(), ParseError> {
        let z: Vec<u8> = self.atoms()?.iter().map(|a| a.z).collect();
        super::check_map_order(&z, map, reference, "$atoms")
    }

    /// Distance-invariant geometry check (see the module note on the reframe).
    fn check_geometry_distances(&self, reference: &ReferenceGeometry) -> Result<(), ParseError> {
        let atoms = self.atoms()?;
        if atoms.len() != reference.xyz_angstrom.len() {
            return Err(ParseError::LengthMismatch {
                field: "geometry".into(),
                expected: reference.xyz_angstrom.len(),
                got: atoms.len(),
            });
        }
        let hess: Vec<[f64; 3]> = atoms
            .iter()
            .map(|a| [a.xyz[0].angstrom(), a.xyz[1].angstrom(), a.xyz[2].angstrom()])
            .collect();
        let dist = |g: &[[f64; 3]], i: usize, j: usize| {
            let (a, b) = (g[i], g[j]);
            ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
        };
        let mut max_delta = 0.0_f64;
        for i in 0..hess.len() {
            for j in (i + 1)..hess.len() {
                let d = (dist(&hess, i, j) - dist(&reference.xyz_angstrom, i, j)).abs();
                max_delta = max_delta.max(d);
            }
        }
        if max_delta >= DISTANCE_TOL_ANGSTROM {
            return Err(ParseError::GeometryMismatch { max_delta });
        }
        Ok(())
    }

    fn check_lengths(&self) -> Result<(), ParseError> {
        let n = self.atoms()?.len();
        let three_n = 3 * n;
        let freqs = self.frequencies()?;
        if freqs.values_cm.len() != three_n {
            return Err(ParseError::LengthMismatch {
                field: "$vibrational_frequencies".into(),
                expected: three_n,
                got: freqs.values_cm.len(),
            });
        }
        let modes = self.normal_modes()?;
        if modes.n != three_n {
            return Err(ParseError::LengthMismatch {
                field: "$normal_modes dimension".into(),
                expected: three_n,
                got: modes.n,
            });
        }
        let ir = self.ir_spectrum()?;
        if ir.len() != three_n {
            return Err(ParseError::LengthMismatch {
                field: "$ir_spectrum".into(),
                expected: three_n,
                got: ir.len(),
            });
        }
        Ok(())
    }

    /// Exactly 6 zero modes (non-linear) or 5 (linear) — both legal. Anything else
    /// is malformed. Measured: the trans/rot modes are printed as exact `0.0`.
    fn check_zero_modes(&self) -> Result<(), ParseError> {
        let z = self.frequencies()?.zero_count;
        if z != 5 && z != 6 {
            return Err(ParseError::Malformed {
                field: "$vibrational_frequencies".into(),
                detail: format!("{z} exact-zero (trans/rot) modes; expected 5 (linear) or 6"),
            });
        }
        Ok(())
    }
}

/// A `.hess` whose post-conditions passed — the only type exposing values.
#[derive(Clone, Debug)]
pub struct Verified(HessFile);

impl Verified {
    /// Equilibrium atoms (Bohr → Å), the element order source for the file.
    pub fn atoms(&self) -> Result<Vec<HessAtom>, ParseError> {
        self.0.atoms()
    }
    /// Signed vibrational frequencies (cm⁻¹) with imaginary / zero-mode counts.
    pub fn frequencies(&self) -> Result<Frequencies, ParseError> {
        self.0.frequencies()
    }
    /// The 3N×3N Cartesian normal-mode matrix.
    pub fn normal_modes(&self) -> Result<NormalModes, ParseError> {
        self.0.normal_modes()
    }
    /// IR spectrum rows (intensity in km/mol).
    pub fn ir_spectrum(&self) -> Result<Vec<IrRow>, ParseError> {
        self.0.ir_spectrum()
    }
    /// `$actual_temperature` as printed — **measured 0.0** on the dexketoprofen
    /// Freq run whose thermochemistry was computed at 298.15 K. So this field is
    /// **NOT the calculation temperature**; it must never be used as one. The
    /// authoritative temperature is `$THERMOCHEMISTRY_Energies temperature`
    /// (`.property.txt` → `ThermoJson::temperature_k`). See `parse-sources.md`.
    pub fn actual_temperature(&self) -> Option<f64> {
        self.0.section("actual_temperature").and_then(|s| s.scalar())
    }
    /// `$frequency_scale_factor` (dimensionless) — the factor ORCA **already
    /// applied** to the printed frequencies. **Measured 1.0** (= none applied); it
    /// is not a recommended value for the method. Not a display scale: applying a
    /// display factor is a UI choice (see `IrSpectrumPanel`), not this field.
    pub fn frequency_scale_factor(&self) -> Option<f64> {
        self.0.section("frequency_scale_factor").and_then(|s| s.scalar())
    }
    pub fn unknown_section_names(&self) -> Vec<String> {
        self.0.unknown_section_names()
    }
}

// --------------------------------------------------------------------------- //
// Layer 2 — typed values                                                        //
// --------------------------------------------------------------------------- //

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct HessAtom {
    pub element: String,
    pub z: u8,
    pub xyz: [Angstrom; 3],
}

#[derive(Clone, Debug, Serialize)]
pub struct Frequencies {
    /// Signed, cm⁻¹, 3N values (trans/rot are exact 0.0; imaginary are negative).
    pub values_cm: Vec<f64>,
    /// Negative frequencies — the saddle-point count. An explicit field, not a UI
    /// derivation: 0 = minimum, 1 = transition state, >1 = neither.
    pub imaginary_count: usize,
    /// Exact-zero (translation/rotation) modes: 6 non-linear, 5 linear.
    pub zero_count: usize,
    pub is_linear: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct NormalModes {
    /// 3N.
    pub n: usize,
    /// Row-major n×n, Cartesian normalized displacement (unit-3.6 gate: no ÷√m).
    data: Vec<f64>,
}

impl NormalModes {
    /// The raw row-major n×n matrix (for storage). Per-mode column access
    /// (`data[r*n + mode]`) is the animation unit's job, added there with a consumer.
    pub fn into_row_major(self) -> Vec<f64> {
        self.data
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct IrRow {
    pub frequency_cm: f64,
    pub t2_au: f64,
    /// Integrated absorption intensity, km/mol (measured — the col2 that matches
    /// the `.out` `IR SPECTRUM` `Int` column).
    pub intensity_km_mol: f64,
    pub t_au: [f64; 3],
}

#[cfg(test)]
mod tests;
