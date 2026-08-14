//! Reader for ORCA's relaxed-surface-scan profile — `<base>.relaxscanact.dat` /
//! `<base>.relaxscanscf.dat` — the fifth artifact reader (ADR-012), on the
//! [`property`](super::property) template: a generic grammar layer, a typestate
//! (`parse → verify(scan) → Verified`), post-conditions-as-errors (rule #9), and
//! canonical units confirmed **at the boundary** (rule #11).
//!
//! # Format (measured — unit 3.3, `wiki/orca/parse-sources.md`)
//! Each `.dat` is **N rows of `coordinate  energy`** (whitespace-separated, 2
//! columns), one row per **scan point** (N = npoints). `act` = the final composite
//! (actual) energy — r²SCAN-3c carries gCP+D4 terms; `scf` = the bare SCF energy.
//! The two energies genuinely differ and are stored **both, labelled** — never
//! conflated. Per-point is NOT per-cycle: `.property.txt` (26 `$Geometry`) and
//! `_trj.xyz` (26 frames) are per optimization cycle across all points, measured —
//! they are NOT the scan source, and this reader does not touch them.
//!
//! # Units — the load-bearing post-condition
//! A bare 2-column `.dat` carries **no unit literal**. The coordinate is *measured*
//! to be Å (energy Eh, both ratio 1.0), but a wrong unit does not crash — it draws a
//! plausible-but-wrong profile (the Bohr/Å class of trap rule #11 exists for). So the
//! unit is **confirmed at runtime, in our terms**: for a distance (`B`) scan, verify
//! recomputes the scanned distance from each point geometry (`input.NNN.xyz`, Å via
//! the [`xyz`](super::xyz) reader) and asserts it equals column 1 within 1e-3 Å. A
//! Bohr coordinate fails this ≈1.889×, loudly. Angle/dihedral (`A`/`D`) parse the
//! same but their coordinate cross-check is deferred (the coordinate is degrees).
//!
//! # Rule #5
//! The `.dat` files are tiny (≈168 B measured), but [`RelaxScan::from_path`] still
//! caps the size and refuses a pathological file rather than eating memory.

use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;

use super::xyz::XyzFile;
use super::ParseError;

/// Measured ≈ 168 B for a 6-point scan; 1 MB is a wide margin (a 30 000-point scan
/// would still fit) that still refuses a pathological file (rule #5).
const MAX_BYTES: u64 = 1024 * 1024;

/// The coordinate cross-check tolerance (rule #11): a missed Å→Bohr shows up as
/// ≈1.889×, far above this. Loose enough for the geometry-optimizer's own noise.
const COORD_CROSS_CHECK_TOL_ANGSTROM: f64 = 1e-3;

/// `act`/`scf` share the same geometries, so their coordinate columns must be equal;
/// they are printed identically, so this only guards a genuine mismatch.
const COORD_MATCH_TOL: f64 = 1e-9;

/// The scanned coordinate, from a minimal parse of the input's `%geom Scan` line.
/// `atoms` are 0-based (ORCA's scan index base — the same space `emit_scan_block`
/// writes). Kind `B` (distance, 2 atoms) is the mission case whose geometry
/// cross-check this unit implements; `A`/`D` parse but skip the cross-check.
#[derive(Clone, Debug, PartialEq)]
pub struct ScanSpec {
    pub kind: char,
    pub atoms: Vec<u32>,
}

static SCAN_LINE_RE: LazyLock<Regex> = LazyLock::new(|| {
    // A scan line inside `%geom Scan`: `B 0 1 = 1.4, 2.4, 6`. The `= ` after the
    // integer indices distinguishes it from a brace-wrapped constraint (`{B 0 1 C}`,
    // no `=`) and from a float coordinate row (no `=`). First match only (A1/A2 model
    // is a single coordinate).
    Regex::new(r"(?m)^\s*([BAD])\s+((?:\d+\s+)*\d+)\s*=").unwrap()
});

/// Extract the scanned coordinate from ORCA input text — a **minimal** parse of the
/// `%geom Scan <letter> <a1> <a2> … = …` line (not the TS `scan.ts` parser; this side
/// only needs the letter + indices for the cross-check). `None` if there is no scan
/// line. The caller (`results.rs`) passes the result into [`RelaxScan::verify`] so the
/// reader never reads `input.inp` itself (no hidden cross-module dependency).
pub fn parse_scan_spec(input_content: &str) -> Option<ScanSpec> {
    let caps = SCAN_LINE_RE.captures(input_content)?;
    let kind = caps[1].chars().next()?;
    let atoms: Vec<u32> = caps[2].split_whitespace().filter_map(|t| t.parse().ok()).collect();
    if atoms.is_empty() {
        return None;
    }
    Some(ScanSpec { kind, atoms })
}

// --------------------------------------------------------------------------- //
// Layer 1 — generic grammar                                                     //
// --------------------------------------------------------------------------- //

/// A parsed relaxed-scan profile: the two `.dat` files as `(coordinate, energy)`
/// rows, plus the job dir (so [`Self::verify`] can load the per-point `input.NNN.xyz`
/// witnesses). Unverified — no accessors until `verify` runs the post-conditions.
#[derive(Clone, Debug)]
pub struct RelaxScan {
    dir: PathBuf,
    /// `.relaxscanact.dat` rows — (coordinate, actual/composite energy Eh).
    act: Vec<(f64, f64)>,
    /// `.relaxscanscf.dat` rows — (coordinate, bare SCF energy Eh).
    scf: Vec<(f64, f64)>,
}

impl RelaxScan {
    /// Read `<dir>/input.relaxscanact.dat` + `input.relaxscanscf.dat`. **Absent is a
    /// normal state** (an Opt/SP/GOAT job has no scan) → `Ok(None)`, mirroring
    /// `orca_json::ensure_gbw_json`. When `act` exists, `scf` must too (ORCA writes
    /// both — a missing partner is a loud inconsistency, not a silent half-read).
    pub fn from_path(dir: &Path) -> Result<Option<RelaxScan>, ParseError> {
        let act_path = dir.join("input.relaxscanact.dat");
        if !act_path.exists() {
            return Ok(None);
        }
        // A 2D (multi-coordinate) scan writes **3+ columns** per row (`c1 c2 … E`). The
        // 1-coordinate reader does not own that shape, so it **stands DOWN cleanly** — `Ok(None)`,
        // NOT a `Malformed` error — so a successful 2D scan finishes without a spurious "coordinate
        // column not strictly monotone" failure (`c1` repeats across the outer loop). A 2D scan's
        // "result" is the surface, read separately by `results::read_scan_surface`. A **2-column**
        // `.dat` is a 1D scan → the full monotone + Å cross-check guard below runs UNCHANGED (the
        // 1D guard is not weakened — this only removes a false-positive on the 3-column case).
        if dat_is_multicoordinate(&act_path)? {
            return Ok(None);
        }
        let scf_path = dir.join("input.relaxscanscf.dat");
        let act = read_dat(&act_path)?;
        let scf = read_dat(&scf_path)?;
        Ok(Some(RelaxScan { dir: dir.to_path_buf(), act, scf }))
    }

    /// Run the post-conditions and, only if all pass, hand back a [`Verified`] — the
    /// only type with accessors (rule #9, the post-condition on the path). The caller
    /// supplies the [`ScanSpec`] (parsed from its own input); the reader never reads
    /// `input.inp`.
    pub fn verify(self, scan: &ScanSpec) -> Result<Verified, ParseError> {
        let n = self.act.len();
        // N ≥ 2 (a scan is at least two points).
        if n < 2 {
            return Err(ParseError::Malformed {
                field: "relaxscanact.dat".into(),
                detail: format!("a scan needs ≥ 2 points, got {n}"),
            });
        }
        // act/scf same N (same geometries).
        if self.scf.len() != n {
            return Err(ParseError::LengthMismatch {
                field: "relaxscanscf.dat rows vs relaxscanact.dat".into(),
                expected: n,
                got: self.scf.len(),
            });
        }
        // Identical coordinate column (the two files describe the same points) and
        // every energy finite.
        for k in 0..n {
            let (ca, ea) = self.act[k];
            let (cs, es) = self.scf[k];
            if (ca - cs).abs() > COORD_MATCH_TOL {
                return Err(ParseError::Malformed {
                    field: format!("scan coordinate at point {k}"),
                    detail: format!("act {ca} ≠ scf {cs} — the two files disagree on the geometry"),
                });
            }
            if !ea.is_finite() || !es.is_finite() {
                return Err(ParseError::Malformed {
                    field: format!("scan energy at point {k}"),
                    detail: format!("non-finite energy (act {ea}, scf {es})"),
                });
            }
        }
        // Coordinate strictly monotone (the scan steps start→end in one direction).
        self.check_monotone()?;
        // The load-bearing post-condition (B): confirm column 1 is Å by recomputing
        // the scanned distance from each point geometry.
        if scan.kind == 'B' {
            self.check_geometry_cross_check(scan)?;
        }

        let points = self
            .act
            .iter()
            .zip(&self.scf)
            .map(|(&(coordinate, energy_act_eh), &(_, energy_scf_eh))| ScanPoint {
                coordinate,
                energy_act_eh,
                energy_scf_eh,
            })
            .collect();
        Ok(Verified { kind: scan.kind, atoms: scan.atoms.clone(), points })
    }

    /// The coordinate column is strictly monotone — every step has the same non-zero
    /// sign as the first (matches the requested start→end direction). A flat or
    /// reversing column means the rows are not a clean scan.
    fn check_monotone(&self) -> Result<(), ParseError> {
        let ascending = self.act[1].0 > self.act[0].0;
        for k in 1..self.act.len() {
            let step = self.act[k].0 - self.act[k - 1].0;
            let ok = if ascending { step > 0.0 } else { step < 0.0 };
            if !ok {
                return Err(ParseError::Malformed {
                    field: "scan coordinate column".into(),
                    detail: format!(
                        "not strictly monotone at point {k} ({} then {})",
                        self.act[k - 1].0,
                        self.act[k].0
                    ),
                });
            }
        }
        Ok(())
    }

    /// Rule #11, the load-bearing check: for each point k, load `input.00k.xyz` (via
    /// the xyz reader — no re-implemented parsing) and assert the distance between the
    /// two scanned atoms equals `coordinate[k]` within 1e-3 Å. A Bohr coordinate fails
    /// this ≈1.889×, loudly, instead of animating a wrong profile.
    fn check_geometry_cross_check(&self, scan: &ScanSpec) -> Result<(), ParseError> {
        let (i, j) = match scan.atoms.as_slice() {
            [a, b, ..] => (*a as usize, *b as usize),
            _ => {
                return Err(ParseError::Malformed {
                    field: "scan spec".into(),
                    detail: format!("a B scan needs 2 atoms, got {}", scan.atoms.len()),
                })
            }
        };
        for k in 0..self.act.len() {
            let point_path = self.dir.join(format!("input.{:03}.xyz", k + 1));
            let xyz = XyzFile::from_path(&point_path)?;
            let dist = xyz.pair_distance_angstrom(0, i, j).ok_or_else(|| ParseError::Malformed {
                field: format!("point geometry {}", k + 1),
                detail: format!(
                    "cannot read distance between scanned atoms {i},{j} in {}",
                    point_path.display()
                ),
            })?;
            let coordinate = self.act[k].0;
            let delta = (dist - coordinate).abs();
            if delta >= COORD_CROSS_CHECK_TOL_ANGSTROM {
                return Err(ParseError::GeometryMismatch { max_delta: delta });
            }
        }
        Ok(())
    }
}

/// Read one `.dat` file: N rows of `coordinate  energy` (2 columns). Size-capped
/// (rule #5). A non-blank line that does not yield two finite floats is a loud
/// `Malformed` error (rule #9), not a silent skip; blank lines are ignored.
/// The column-count discriminator behind the 1D reader's stand-down: `true` iff the first
/// non-empty data row has **3+ whitespace columns** (a 2D+ scan `c1 c2 … E`). A 1D scan writes
/// exactly 2 (`coordinate energy`). Reads only up to the first data row (bounded — never loads a
/// large 2D grid the 1D reader will discard anyway).
fn dat_is_multicoordinate(path: &Path) -> Result<bool, ParseError> {
    use std::io::{BufRead, BufReader};
    let file = std::fs::File::open(path).map_err(|e| ParseError::Io {
        path: path.display().to_string(),
        source: e,
    })?;
    for line in BufReader::new(file).lines() {
        let line = line.map_err(|e| ParseError::Io {
            path: path.display().to_string(),
            source: e,
        })?;
        let n = line.split_whitespace().count();
        if n == 0 {
            continue;
        }
        return Ok(n >= 3);
    }
    Ok(false) // empty file → not multi-coordinate (read_dat handles emptiness)
}

fn read_dat(path: &Path) -> Result<Vec<(f64, f64)>, ParseError> {
    let meta = std::fs::metadata(path).map_err(|e| ParseError::Io {
        path: path.display().to_string(),
        source: e,
    })?;
    if meta.len() > MAX_BYTES {
        return Err(ParseError::TooLarge {
            artifact: "relaxscan.dat",
            bytes: meta.len(),
            cap: MAX_BYTES,
        });
    }
    let text = std::fs::read_to_string(path).map_err(|e| ParseError::Io {
        path: path.display().to_string(),
        source: e,
    })?;
    let mut rows = Vec::new();
    for line in text.lines() {
        let toks: Vec<&str> = line.split_whitespace().collect();
        if toks.is_empty() {
            continue;
        }
        match (toks.first().and_then(|t| t.parse().ok()), toks.get(1).and_then(|t| t.parse().ok())) {
            (Some(c), Some(e)) => rows.push((c, e)),
            _ => {
                return Err(ParseError::Malformed {
                    field: path.display().to_string(),
                    detail: format!("row is not `coordinate energy`: {line:?}"),
                })
            }
        }
    }
    Ok(rows)
}

// --------------------------------------------------------------------------- //
// Layer 2 — typed values (canonical units, cross-checked)                       //
// --------------------------------------------------------------------------- //

/// One scan point. Coordinate in Å for a `B` scan (confirmed by the geometry
/// cross-check), degrees for `A`/`D`. Both energies in Eh, unconverted; `act` and
/// `scf` are kept separate (they genuinely differ — gCP+D4).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScanPoint {
    pub coordinate: f64,
    pub energy_act_eh: f64,
    pub energy_scf_eh: f64,
}

/// A relaxed-scan profile whose post-conditions passed — the only type exposing the
/// points. Cannot be built except via [`RelaxScan::verify`].
#[derive(Clone, Debug)]
pub struct Verified {
    kind: char,
    atoms: Vec<u32>,
    points: Vec<ScanPoint>,
}

impl Verified {
    pub fn points(&self) -> &[ScanPoint] {
        &self.points
    }
    /// Scan kind letter — `B`/`A`/`D`.
    pub fn kind(&self) -> char {
        self.kind
    }
    /// The scanned atoms (0-based).
    pub fn atoms(&self) -> &[u32] {
        &self.atoms
    }
    /// The coordinate unit: Å for a distance scan (cross-checked), `°` for angle /
    /// dihedral (parsed, not cross-checked in B1).
    pub fn coordinate_unit(&self) -> &'static str {
        if self.kind == 'B' {
            "Å"
        } else {
            "°"
        }
    }
}

#[cfg(test)]
mod tests;
