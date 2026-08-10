//! Reader for ORCA's NEB-TS band artifacts — the sixth artifact reader (ADR-012), on
//! the [`property`](super::property) template: a generic grammar layer, post-conditions-
//! as-errors (rule #9), and canonical units confirmed **at the boundary** (rule #11).
//!
//! Three files, all measured on the real Menshutkin NEB-TS probe (2026-08-10,
//! `wiki/orca/neb.md`):
//!
//! - **`input.NEB.log`** — the per-iteration band. Header (2 lines) then one block per
//!   iteration, delimited by a `>` line. Each block carries `iteration : N`,
//!   `climbing : yes|no`, `nim : <count>`, `barrier :  <Eh> (image:  <k>)`, a
//!   `distance :  <nim floats>` array (**arc length, Bohr**) and an
//!   `energy :  <nim floats>` array (**ABSOLUTE Eh**). All other rows (forces, steps,
//!   angles) are ignored. This is the E3a-2 "PES per iteration" source.
//! - **`input.final.interp`** — the converged smooth MEP. Its `Interp.:` section is
//!   rows of `<norm> <distance_Bohr> <energy_Eh>` where the energy is **RELATIVE**
//!   (image 0 = 0). (An earlier `Images:` section holds the discrete points; not read.)
//! - **`input_NEB-TS_converged.xyz`** — the converged TS geometry (standard xyz, Å).
//!   Read through the [`xyz`](super::xyz) reader — no re-implemented xyz parsing.
//!
//! # Units (rule #11)
//! Log/interp **distances are Bohr → Å at the boundary** ([`Angstrom::from_bohr`]);
//! energies are Eh (canonical). The log energy is absolute, the interp energy relative
//! — kept apart, never conflated. The TS xyz is Å already ([`Angstrom::from_angstrom`]).
//!
//! # Rule #5
//! The `.NEB.log` is small and **bounded** (≈43 KB for a 24-iteration / 10-image run —
//! NOT the unbounded `.out`); every read still size-caps and refuses a pathological file.

use std::path::Path;
use std::sync::LazyLock;

use regex::Regex;
use serde::Serialize;

use super::elements::z_of;
use super::units::Angstrom;
use super::xyz::XyzFile;
use super::{ParseError, ReferenceGeometry};

/// The `.NEB.log` grows ~1.8 KB/iteration; 16 MB is a wide margin (thousands of
/// iterations) that still refuses a pathological file (rule #5).
const MAX_BYTES: u64 = 16 * 1024 * 1024;

// --------------------------------------------------------------------------- //
// Layer 2 — typed values (canonical units)                                      //
// --------------------------------------------------------------------------- //

/// One image on the band: its arc-length distance (Å, Bohr→Å) and energy (Eh).
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct BandImage {
    pub distance_angstrom: f64,
    /// Eh. **Absolute** in an [`Iteration`] (from `.NEB.log`); **relative** (image 0 =
    /// 0) in the smooth MEP (from `.final.interp`). The container names which.
    pub energy_eh: f64,
}

/// One NEB iteration: the discrete band, its barrier, and (when climbing is on) the
/// climbing-image index.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Iteration {
    pub index: usize,
    pub images: Vec<BandImage>,
    pub barrier_eh: f64,
    /// The climbing image's 0-based index — `Some` only once `climbing : yes`; the
    /// `barrier` line's `(image: k)` names which image is the (climbing) maximum.
    pub climbing_image: Option<usize>,
}

// --------------------------------------------------------------------------- //
// Layer 1 — generic grammar (pure, text → data)                                 //
// --------------------------------------------------------------------------- //

static BARRIER_IMAGE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"image:\s*(\d+)").unwrap());

/// All whitespace-separated `f64` tokens on a line fragment (non-numeric tokens
/// dropped) — used for the `distance`/`energy` arrays.
fn parse_floats(s: &str) -> Vec<f64> {
    s.split_whitespace().filter_map(|t| t.parse::<f64>().ok()).collect()
}

/// Parse `input.NEB.log` text → the per-iteration band. Pure. Each iteration's
/// `distance` array is Bohr→Å; `energy` is absolute Eh. A block whose distance/energy
/// arrays differ in length is a loud `Malformed` (rule #9), never a silent half-band.
pub fn parse_neb_log(text: &str) -> Result<Vec<Iteration>, ParseError> {
    let mut iterations = Vec::new();
    let mut cur: Option<Partial> = None;

    for line in text.lines() {
        let Some((key, val)) = line.split_once(':') else {
            continue;
        };
        match key.trim() {
            "iteration" => {
                if let Some(p) = cur.take() {
                    iterations.push(p.finish()?);
                }
                let index = val.trim().parse::<usize>().map_err(|_| ParseError::Malformed {
                    field: "NEB.log iteration".into(),
                    detail: format!("index is not an integer: {val:?}"),
                })?;
                cur = Some(Partial::new(index));
            }
            "climbing" => {
                if let Some(p) = cur.as_mut() {
                    p.climbing = val.trim().eq_ignore_ascii_case("yes");
                }
            }
            "barrier" => {
                if let Some(p) = cur.as_mut() {
                    p.barrier_eh = val.split_whitespace().next().and_then(|t| t.parse().ok());
                    p.barrier_image =
                        BARRIER_IMAGE_RE.captures(val).and_then(|c| c[1].parse::<usize>().ok());
                }
            }
            "distance" => {
                if let Some(p) = cur.as_mut() {
                    p.distances_bohr = Some(parse_floats(val));
                }
            }
            "energy" => {
                if let Some(p) = cur.as_mut() {
                    p.energies_eh = Some(parse_floats(val));
                }
            }
            _ => {}
        }
    }
    if let Some(p) = cur.take() {
        iterations.push(p.finish()?);
    }
    if iterations.is_empty() {
        return Err(ParseError::MissingField("NEB.log has no `iteration` blocks".into()));
    }
    Ok(iterations)
}

/// A partially-parsed iteration block, finalized at the next `iteration`/EOF.
struct Partial {
    index: usize,
    climbing: bool,
    barrier_eh: Option<f64>,
    barrier_image: Option<usize>,
    distances_bohr: Option<Vec<f64>>,
    energies_eh: Option<Vec<f64>>,
}

impl Partial {
    fn new(index: usize) -> Self {
        Partial {
            index,
            climbing: false,
            barrier_eh: None,
            barrier_image: None,
            distances_bohr: None,
            energies_eh: None,
        }
    }

    fn finish(self) -> Result<Iteration, ParseError> {
        let field = || format!("NEB.log iteration {}", self.index);
        let barrier_eh = self.barrier_eh.ok_or_else(|| ParseError::Malformed {
            field: field(),
            detail: "no `barrier` line".into(),
        })?;
        let dists = self.distances_bohr.ok_or_else(|| ParseError::Malformed {
            field: field(),
            detail: "no `distance` line".into(),
        })?;
        let energies = self.energies_eh.ok_or_else(|| ParseError::Malformed {
            field: field(),
            detail: "no `energy` line".into(),
        })?;
        if dists.len() != energies.len() {
            return Err(ParseError::LengthMismatch {
                field: format!("{} distance vs energy", field()),
                expected: dists.len(),
                got: energies.len(),
            });
        }
        if dists.is_empty() {
            return Err(ParseError::Malformed {
                field: field(),
                detail: "empty band (no images)".into(),
            });
        }
        let images = dists
            .iter()
            .zip(&energies)
            .map(|(&d, &e)| BandImage {
                distance_angstrom: Angstrom::from_bohr(d).angstrom(),
                energy_eh: e,
            })
            .collect();
        // climbing image index is meaningful only once climbing is active.
        let climbing_image = if self.climbing { self.barrier_image } else { None };
        Ok(Iteration { index: self.index, images, barrier_eh, climbing_image })
    }
}

/// Parse the `Interp.:` section of `input.final.interp` → the smooth MEP: rows of
/// `<norm> <distance_Bohr> <energy_Eh>` (energy RELATIVE, image 0 = 0). Distance
/// Bohr→Å. Pure. The section runs to EOF; a row without ≥ 3 floats ends it.
pub fn parse_final_interp(text: &str) -> Result<Vec<BandImage>, ParseError> {
    let mut in_section = false;
    let mut mep = Vec::new();
    for line in text.lines() {
        if line.trim_start().starts_with("Interp.") {
            in_section = true;
            continue;
        }
        if !in_section {
            continue;
        }
        let cols = parse_floats(line);
        if cols.len() < 3 {
            if mep.is_empty() {
                continue; // tolerate blank lines right after the header
            }
            break; // a non-row line ends the section
        }
        mep.push(BandImage {
            distance_angstrom: Angstrom::from_bohr(cols[1]).angstrom(),
            energy_eh: cols[2],
        });
    }
    if mep.is_empty() {
        return Err(ParseError::MissingField(
            "final.interp has no `Interp.:` section rows".into(),
        ));
    }
    Ok(mep)
}

// --------------------------------------------------------------------------- //
// The reader — files → parsed → verified                                        //
// --------------------------------------------------------------------------- //

/// A parsed NEB band + smooth MEP + converged TS geometry, unverified. **Absent is a
/// normal state** (a non-NEB job has no `input.NEB.log`) → `Ok(None)`, mirroring the
/// other absent-is-normal readers. When the log exists the interp and converged-TS xyz
/// must too (ORCA writes all three on a converged NEB-TS — a missing partner is a loud
/// inconsistency, not a silent half-read).
#[derive(Debug)]
pub struct NebBand {
    iterations: Vec<Iteration>,
    mep: Vec<BandImage>,
    ts_elements: Vec<String>,
    ts_xyz_angstrom: Vec<[f64; 3]>,
}

impl NebBand {
    pub fn from_path(dir: &Path) -> Result<Option<NebBand>, ParseError> {
        let log_path = dir.join("input.NEB.log");
        if !log_path.exists() {
            return Ok(None);
        }
        let iterations = parse_neb_log(&read_capped(&log_path)?)?;
        let mep = parse_final_interp(&read_capped(&dir.join("input.final.interp"))?)?;
        // Reuse the xyz reader for the converged TS — no re-implemented xyz parsing.
        let ts_path = dir.join("input_NEB-TS_converged.xyz");
        let (ts_elements, ts_xyz_angstrom) =
            XyzFile::from_path(&ts_path)?.first_frame().ok_or_else(|| {
                ParseError::MissingField("NEB-TS converged xyz has no frame".into())
            })?;
        Ok(Some(NebBand { iterations, mep, ts_elements, ts_xyz_angstrom }))
    }

    /// Post-conditions (rule #9), then [`Verified`]. `expected_images` is `NImages + 2`
    /// (the total image count the input requested), supplied by the caller from its own
    /// input — the reader never reads `input.inp`. `reference` anchors the TS
    /// element-order check to the reactant order.
    pub fn verify(
        self,
        reference: &ReferenceGeometry,
        expected_images: usize,
    ) -> Result<Verified, ParseError> {
        // (1) image count constant across iterations AND equal to NImages+2.
        for it in &self.iterations {
            if it.images.len() != expected_images {
                return Err(ParseError::LengthMismatch {
                    field: format!("NEB.log iteration {} image count", it.index),
                    expected: expected_images,
                    got: it.images.len(),
                });
            }
            // (2) arc-length distances monotonic non-decreasing within the iteration.
            for w in it.images.windows(2) {
                if w[1].distance_angstrom < w[0].distance_angstrom {
                    return Err(ParseError::Malformed {
                        field: format!("NEB.log iteration {} distance", it.index),
                        detail: format!(
                            "arc length not monotonic: {} then {}",
                            w[0].distance_angstrom, w[1].distance_angstrom
                        ),
                    });
                }
            }
        }
        // (3) the converged TS carries the reactant's element order (composition + order).
        if self.ts_elements.len() != reference.z.len() {
            return Err(ParseError::LengthMismatch {
                field: "NEB-TS converged xyz atom count".into(),
                expected: reference.z.len(),
                got: self.ts_elements.len(),
            });
        }
        for (i, sym) in self.ts_elements.iter().enumerate() {
            let z = z_of(sym).unwrap_or(0);
            if z != reference.z[i] {
                return Err(ParseError::OrderMismatch {
                    block: "NEB-TS converged xyz".into(),
                    index: i,
                });
            }
        }
        Ok(Verified(self))
    }
}

/// A NEB band whose post-conditions passed — the only type exposing accessors.
#[derive(Debug)]
pub struct Verified(NebBand);

impl Verified {
    pub fn iterations(&self) -> &[Iteration] {
        &self.0.iterations
    }
    /// The converged smooth MEP (relative energies, image 0 = 0).
    pub fn mep(&self) -> &[BandImage] {
        &self.0.mep
    }
    /// The converged TS geometry `(elements, Å coords)`.
    pub fn ts_geometry(&self) -> (&[String], &[[f64; 3]]) {
        (&self.0.ts_elements, &self.0.ts_xyz_angstrom)
    }
    /// The final iteration's barrier (Eh) — the converged NEB-TS barrier estimate.
    pub fn final_barrier_eh(&self) -> Option<f64> {
        self.0.iterations.last().map(|it| it.barrier_eh)
    }
}

/// Read a bounded artifact whole, size-capped (rule #5) — NOT the unbounded `.out`.
fn read_capped(path: &Path) -> Result<String, ParseError> {
    let meta = std::fs::metadata(path).map_err(|e| ParseError::Io {
        path: path.display().to_string(),
        source: e,
    })?;
    if meta.len() > MAX_BYTES {
        return Err(ParseError::TooLarge {
            artifact: "NEB artifact",
            bytes: meta.len(),
            cap: MAX_BYTES,
        });
    }
    std::fs::read_to_string(path).map_err(|e| ParseError::Io {
        path: path.display().to_string(),
        source: e,
    })
}

#[cfg(test)]
mod tests;
