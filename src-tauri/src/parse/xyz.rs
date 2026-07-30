//! Reader for ORCA's `_trj.xyz` (optimization/scan trajectory) and `.xyz` (final
//! geometry) — the third artifact reader (ADR-012), on the [`property`](super::property)
//! template: two layers, typestate (`parse → verify(reference) → Verified`),
//! post-conditions-as-errors, units by type.
//!
//! # Format
//! Multi-frame xmol xyz: `natom`, a comment line, `natom` atom rows (`SYM x y z`),
//! repeated. A `.xyz` is just a one-frame trajectory — the **same** reader.
//!
//! # Units — Ångström (measured, `parse-sources.md`)
//! Coordinates are Å, so [`Angstrom::from_angstrom`] (no conversion). But the rule
//! #11 geometry post-condition still runs: this is exactly where `from_angstrom`
//! is the *right* choice, and a test proves the *wrong* one (`from_bohr`) is
//! rejected — otherwise "units by type" would be untested for the identity case.
//!
//! # Comment line (measured, not assumed)
//! Verbatim, on every frame of every job type (Opt / GOAT / scan) and on `.xyz`:
//! `Coordinates from ORCA-job input E <energy>` — the frame energy in Eh after
//! `" E "`. Parsed to `Option<f64>` ([`Frame::energy_eh`]); a line that does not
//! match yields `None`, never a failure.
//!
//! # Not scan points
//! A relaxed scan's `_trj.xyz` has one frame **per optimization cycle** (26 for a
//! 6-point scan — measured), not per scan point. This reader exposes *frames*, never
//! "scan points". The scan-specific `.allxyz` / `.relaxscan*.dat` are Phase 4.5 and
//! out of scope; `.allxyz` (a `>`-separated, differently-commented format) is not
//! fed to this reader.

use std::path::Path;

use serde::Serialize;

use super::elements::{strip_fragment_suffix, z_of};
use super::units::Angstrom;
use super::{ParseError, ReferenceGeometry};

/// A trajectory grows with optimization cycles — the one artifact that scales with
/// run length. Measured frames are small (5–26); 64 MB is a wide cap (rule #5).
const MAX_BYTES: u64 = 64 * 1024 * 1024;

const GEOMETRY_TOL_ANGSTROM: f64 = 1e-4;

// --------------------------------------------------------------------------- //
// Layer 1 — generic tokenizer                                                   //
// --------------------------------------------------------------------------- //

#[derive(Clone, Debug)]
struct RawFrame {
    comment: String,
    atoms: Vec<(String, [f64; 3])>,
}

/// A parsed multi-frame xyz. Unverified.
#[derive(Clone, Debug)]
pub struct XyzFile {
    frames: Vec<RawFrame>,
}

impl XyzFile {
    pub fn from_path(path: &Path) -> Result<Self, ParseError> {
        let meta = std::fs::metadata(path).map_err(|e| ParseError::Io {
            path: path.display().to_string(),
            source: e,
        })?;
        if meta.len() > MAX_BYTES {
            return Err(ParseError::TooLarge {
                artifact: "xyz",
                bytes: meta.len(),
                cap: MAX_BYTES,
            });
        }
        let text = std::fs::read_to_string(path).map_err(|e| ParseError::Io {
            path: path.display().to_string(),
            source: e,
        })?;
        Self::parse(&text)
    }

    /// Tokenize the multi-frame grammar: `natom` / comment / `natom` rows, repeated.
    /// A malformed count line ends parsing gracefully (whatever frames were read
    /// stand); zero frames is caught by [`Self::verify`].
    pub fn parse(text: &str) -> Result<Self, ParseError> {
        let lines: Vec<&str> = text.lines().collect();
        let mut frames = Vec::new();
        let mut i = 0;
        while i < lines.len() {
            let Ok(natom) = lines[i].trim().parse::<usize>() else {
                // trailing blank line(s) or a non-count line → stop.
                break;
            };
            let comment_idx = i + 1;
            if comment_idx + natom >= lines.len() + 1 && comment_idx >= lines.len() {
                break;
            }
            let comment = lines.get(comment_idx).unwrap_or(&"").to_string();
            let mut atoms = Vec::with_capacity(natom);
            for k in 0..natom {
                let Some(row) = lines.get(comment_idx + 1 + k) else { break };
                let toks: Vec<&str> = row.split_whitespace().collect();
                if toks.len() < 4 {
                    break;
                }
                let sym = strip_fragment_suffix(toks[0]).to_string();
                let (Ok(x), Ok(y), Ok(z)) =
                    (toks[1].parse(), toks[2].parse(), toks[3].parse())
                else {
                    break;
                };
                atoms.push((sym, [x, y, z]));
            }
            if atoms.len() != natom {
                return Err(ParseError::Malformed {
                    field: "xyz frame".into(),
                    detail: format!("declared {natom} atoms, parsed {}", atoms.len()),
                });
            }
            frames.push(RawFrame { comment, atoms });
            i = comment_idx + 1 + natom;
        }
        Ok(XyzFile { frames })
    }

    /// Run the post-conditions; only on success return [`Verified`].
    pub fn verify(self, reference: &ReferenceGeometry) -> Result<Verified, ParseError> {
        if self.frames.is_empty() {
            return Err(ParseError::MissingField("xyz frames (need ≥ 1)".into()));
        }
        // natom constant across all frames.
        let n = self.frames[0].atoms.len();
        if let Some((idx, f)) = self.frames.iter().enumerate().find(|(_, f)| f.atoms.len() != n) {
            return Err(ParseError::LengthMismatch {
                field: format!("frame {idx} atom count"),
                expected: n,
                got: f.atoms.len(),
            });
        }
        // element order of EVERY frame == reference.
        if n != reference.z.len() {
            return Err(ParseError::LengthMismatch {
                field: "atom count".into(),
                expected: reference.z.len(),
                got: n,
            });
        }
        for (fi, f) in self.frames.iter().enumerate() {
            for (ai, (sym, _)) in f.atoms.iter().enumerate() {
                if z_of(sym) != Some(reference.z[ai]) {
                    return Err(ParseError::OrderMismatch {
                        block: format!("frame {fi}"),
                        index: ai,
                    });
                }
            }
        }
        // geometry post-condition (rule #11) on the FIRST frame, after conversion.
        let mut max_delta = 0.0_f64;
        for ((_, xyz), r) in self.frames[0].atoms.iter().zip(&reference.xyz_angstrom) {
            for k in 0..3 {
                max_delta = max_delta.max((Angstrom::from_angstrom(xyz[k]).angstrom() - r[k]).abs());
            }
        }
        if max_delta >= GEOMETRY_TOL_ANGSTROM {
            return Err(ParseError::GeometryMismatch { max_delta });
        }
        Ok(Verified(self))
    }
}

/// An xyz whose post-conditions passed — the only type exposing frames.
#[derive(Clone, Debug)]
pub struct Verified(XyzFile);

impl Verified {
    /// All frames (Bohr never occurs here; coords are Å). Frames are optimization
    /// cycles for a `_trj.xyz`, a single final geometry for a `.xyz` — **never**
    /// "scan points".
    pub fn frames(&self) -> Vec<Frame> {
        self.0
            .frames
            .iter()
            .map(|f| Frame {
                energy_eh: parse_comment_energy(&f.comment),
                atoms: f
                    .atoms
                    .iter()
                    .map(|(sym, xyz)| FrameAtom {
                        element: sym.clone(),
                        z: z_of(sym).unwrap_or(0),
                        xyz: [
                            Angstrom::from_angstrom(xyz[0]),
                            Angstrom::from_angstrom(xyz[1]),
                            Angstrom::from_angstrom(xyz[2]),
                        ],
                    })
                    .collect(),
            })
            .collect()
    }
}

/// `Coordinates from ORCA-job input E <energy>` → the energy in Eh, or `None`.
fn parse_comment_energy(comment: &str) -> Option<f64> {
    // take the token after the last standalone " E ".
    let toks: Vec<&str> = comment.split_whitespace().collect();
    let pos = toks.iter().rposition(|&t| t == "E")?;
    toks.get(pos + 1)?.parse().ok()
}

// --------------------------------------------------------------------------- //
// Layer 2 — typed values                                                        //
// --------------------------------------------------------------------------- //

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct FrameAtom {
    pub element: String,
    pub z: u8,
    pub xyz: [Angstrom; 3],
}

#[derive(Clone, Debug, Serialize)]
pub struct Frame {
    /// Frame energy in Eh, from the comment line, or `None` if it did not match.
    pub energy_eh: Option<f64>,
    pub atoms: Vec<FrameAtom>,
}

#[cfg(test)]
mod tests;
