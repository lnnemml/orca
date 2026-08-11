//! Persisting parsed `.property.txt` results into the `results` table (Phase 3,
//! ADR-012 + ADR-004).
//!
//! This is the first place per-atom data enters permanent storage, and DB rows
//! outlive the code that wrote them. So the invariant is stricter than "store the
//! numbers": **no per-atom array is stored without the element sequence it was
//! already verified against.** Charges are stored next to their own `elements` /
//! `atomic_numbers`; the gradient next to the element order of the `$Geometry`
//! with its `&GeometryIndex`. There is deliberately **no** position-keyed
//! "result atom" table — that would rebuild the positional identity ADR-010
//! exists to remove, in the one place (persistence) it is hardest to remove
//! later.
//!
//! Shape (ADR-004): a few **narrow typed columns** for the card and future
//! sorting, plus one **JSON column** holding the full structure including the
//! per-atom arrays. Large artifacts stay on disk (paths, not blobs).

use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

use crate::convergence::{ConvergenceEvent, ConvergenceParser};
use crate::error::AppError;
use crate::parse::elements::symbol_of;
use crate::parse::hess::HessFile;
use crate::parse::property::{PopulationScheme, PropertyFile, Verified};
use crate::parse::relaxscan::{parse_scan_spec, RelaxScan};
use crate::parse::xyz::XyzFile;
use crate::parse::{derived_identity_ids, identity_map_for, ReferenceGeometry};
use orcastudio_core::ids::{AtomId, IndexMap, OrcaIndex};

/// Bump when the stored JSON shape or the parse semantics change.
/// - v1: property.txt only (unit 3.5).
/// - v2: + `.hess` frequencies / IR / normal modes + thermo temperature (unit 3.6).
/// - v3: + `_trj.xyz` trajectory + `orca_2json` MO energies/occupancies (unit 3.7).
/// - v4: + relaxed-scan profile (`.relaxscanact/.relaxscanscf.dat`, Phase 4.5 B1).
pub const PARSER_VERSION: u32 = 5;

// --------------------------------------------------------------------------- //
// The stored structure (goes into results.data_json verbatim)                   //
// --------------------------------------------------------------------------- //

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AtomCharges {
    pub scheme: String, // "mulliken" | "loewdin" | "mayer"
    /// The element order these charges belong to — stored WITH the values so a
    /// future reader can re-check, not trust.
    pub elements: Vec<String>,
    pub atomic_numbers: Vec<u8>,
    pub charges: Vec<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FinalGeometry {
    pub elements: Vec<String>,
    pub xyz_angstrom: Vec<[f64; 3]>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GradientJson {
    pub geometry_index: u32,
    /// The `$Geometry(geometry_index)` element order — the bare-positional gradient's
    /// order source, made explicit in storage (not two loose fields).
    pub order_elements: Vec<String>,
    pub grad_eh_per_bohr: Vec<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DipoleJson {
    pub magnitude_au: f64,
    pub total_au: [f64; 3],
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThermoJson {
    /// Kelvin (measured literal). Needed to derive S from the T·S term.
    pub temperature_k: f64,
    pub el_energy_eh: f64,
    pub zpe_eh: f64,
    pub inner_energy_u_eh: f64,
    pub enthalpy_h_eh: f64,
    /// T·S in Eh, NOT entropy S (measured: == enthalpy_h_eh − free_energy_g_eh).
    pub t_times_s_eh: f64,
    pub free_energy_g_eh: f64,
}

/// Vibrational data from `.hess` (unit 3.6). Stored WITH the `$atoms` element order
/// (the file's order source), per the unit-3.5 rule. Modes are a matrix, not
/// per-atom rows. Frequencies cm⁻¹, IR intensity km/mol (measured); modes are
/// Cartesian (unit-3.6 gate).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FrequenciesJson {
    /// `$atoms` element order — the sequence the normal-mode rows (3N Cartesian
    /// coords, atom-major) belong to.
    pub elements: Vec<String>,
    pub frequencies_cm: Vec<f64>,
    /// Negative-frequency count: 0 = minimum, 1 = transition state, >1 = neither.
    pub imaginary_count: usize,
    pub zero_count: usize,
    pub is_linear: bool,
    pub ir_intensity_km_mol: Vec<f64>,
    /// 3N — the normal-mode matrix dimension.
    pub n_modes: usize,
    /// Row-major n×n Cartesian normal modes.
    pub normal_modes: Vec<f64>,
    pub temperature_k: Option<f64>,
    pub scale_factor: Option<f64>,
    /// `.hess` sections with no accessor (rule #10) — surfaced, not dropped.
    pub unknown_sections: Vec<String>,
}

/// Optimization/scan trajectory (unit 3.7). Frames are opt cycles, NOT scan
/// points. Element order stored once (constant across frames — unit-3.5 rule);
/// per-frame Å coords + the comment energy.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrajectoryJson {
    pub n_frames: usize,
    pub elements: Vec<String>,
    pub frames: Vec<TrajFrame>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrajFrame {
    pub energy_eh: Option<f64>,
    pub xyz_angstrom: Vec<[f64; 3]>,
}

/// Relaxed-surface-scan profile (Phase 4.5 B1) from `.relaxscanact/.relaxscanscf.dat`
/// — **one row per scan point** (NOT the per-cycle trajectory). `act` = composite
/// energy (gCP+D4), `scf` = bare SCF — both kept, never conflated. The coordinate is
/// Å for a `B` (distance) scan, confirmed at read time by the geometry cross-check
/// against each `input.NNN.xyz` (rule #11); degrees for `A`/`D` (cross-check deferred).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScanProfileJson {
    /// Scan kind: "B" (distance), "A" (angle), "D" (dihedral).
    pub kind: String,
    /// Scanned atoms, 0-based (the ORCA scan index space).
    pub atoms: Vec<u32>,
    /// "Å" for a distance scan (cross-checked), "°" for angle/dihedral.
    pub coordinate_unit: String,
    pub points: Vec<ScanPointJson>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScanPointJson {
    /// Å (B, cross-checked) or degrees (A/D).
    pub coordinate: f64,
    pub energy_act_eh: f64,
    pub energy_scf_eh: f64,
}

/// MO energies + occupancies from `orca_2json` (unit 3.7). `MOCoefficients` are
/// NEVER stored (rule #5). Occupancy is kept so HOMO/LUMO can be re-derived.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrbitalsJson {
    pub energy_unit: String,
    /// `[energy_eh, occupancy]` per MO, ascending energy.
    pub orbitals: Vec<[f64; 2]>,
    pub homo_lumo: Option<HomoLumoJson>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HomoLumoJson {
    pub homo_eh: f64,
    pub lumo_eh: f64,
    pub gap_eh: f64,
}

/// One image on a NEB band: arc-length distance (Å) + energy (Eh). In `iterations` the
/// energy is ABSOLUTE (`.NEB.log`); in `mep` it is RELATIVE, image 0 = 0 (`.final.interp`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NebImageJson {
    pub distance_angstrom: f64,
    pub energy_eh: f64,
}

/// One NEB iteration: the discrete band + its barrier + (once climbing) the CI index.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NebIterationJson {
    pub index: usize,
    pub images: Vec<NebImageJson>,
    pub barrier_eh: f64,
    pub climbing_image: Option<usize>,
}

/// NEB-TS band results (Stage E3a-1) from `.NEB.log` / `.final.interp` /
/// `_NEB-TS_converged.xyz`, or `None` when the job is not a NEB run (absent-is-normal).
/// The converged TS geometry is exposed so E3a-2 can seed OptTS from it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NebResultsJson {
    pub iterations: Vec<NebIterationJson>,
    /// The converged smooth minimum-energy path (relative energies).
    pub mep: Vec<NebImageJson>,
    /// The final iteration's barrier (Eh) — the converged NEB-TS barrier estimate.
    pub final_barrier_eh: Option<f64>,
    /// The converged transition-state geometry (elements + Å).
    pub ts_geometry: FinalGeometry,
    /// The converged TS energy (Eh) from the `_NEB-TS_converged.xyz` comment — the NEB
    /// job's authoritative single energy (`ParsedResults.final_energy_eh` for a NEB job).
    pub ts_energy_eh: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParsedResults {
    pub parser_version: u32,
    pub final_energy_eh: Option<f64>,
    pub dipole: Option<DipoleJson>,
    pub charges: Vec<AtomCharges>,
    pub thermochemistry: Option<ThermoJson>,
    /// The optimized (last) geometry — the canonical element order for the record.
    pub final_geometry: FinalGeometry,
    pub gradient: Option<GradientJson>,
    /// Vibrational data from `.hess`, or `None` when the job produced none (SP,
    /// GOAT) — a normal state, not an error.
    pub frequencies: Option<FrequenciesJson>,
    /// Optimization/scan trajectory from `_trj.xyz`, or `None` (a single-point job).
    pub trajectory: Option<TrajectoryJson>,
    /// MO energies/occupancies from `orca_2json`, or `None` (xTB/GOAT gbw — normal).
    pub orbitals: Option<OrbitalsJson>,
    /// Relaxed-scan profile from `.relaxscanact/.relaxscanscf.dat`, or `None` when the
    /// job is not a scan (SP/Opt/GOAT — normal, absent-is-not-an-error).
    pub scan: Option<ScanProfileJson>,
    /// NEB-TS band + MEP + converged TS from `.NEB.log`/`.final.interp`/`_NEB-TS_
    /// converged.xyz`, or `None` when the job is not a NEB run (absent-is-normal).
    pub neb: Option<NebResultsJson>,
    /// Mayer bond orders (`output.out`) — the **computed, authoritative** bond order
    /// of the final structure (contrast the editor's geometric estimate). `None` when
    /// the run printed no table (xTB / an SP that didn't print it — absent-is-normal).
    /// `#[serde(default)]` so results rows stored before this field read back as `None`.
    #[serde(default)]
    pub mayer_bond_orders: Option<Vec<crate::parse::mayer::MayerBond>>,
    /// Blocks ORCA emitted that this reader has no accessor for (rule #10).
    pub unknown_blocks: Vec<String>,
}

impl ParsedResults {
    fn from_verified(
        v: &Verified,
        hess: Option<&crate::parse::hess::Verified>,
        trajectory: Option<TrajectoryJson>,
        orbitals: Option<OrbitalsJson>,
        scan: Option<ScanProfileJson>,
        neb: Option<NebResultsJson>,
    ) -> Result<ParsedResults, AppError> {
        let geoms = v.geometries()?;
        let last = geoms
            .last()
            .ok_or_else(|| AppError::Internal("verified property.txt has no geometry".into()))?;
        let final_geometry = FinalGeometry {
            elements: last.atoms.iter().map(|a| a.element.clone()).collect(),
            xyz_angstrom: last
                .atoms
                .iter()
                .map(|a| [a.xyz[0].angstrom(), a.xyz[1].angstrom(), a.xyz[2].angstrom()])
                .collect(),
        };

        let ch = v.charges();
        let mut charges = Vec::new();
        for pop in [ch.mulliken, ch.loewdin, ch.mayer].into_iter().flatten() {
            let scheme = match pop.scheme {
                PopulationScheme::Mulliken => "mulliken",
                PopulationScheme::Loewdin => "loewdin",
                PopulationScheme::Mayer => "mayer",
            };
            let elements = pop
                .atomic_numbers
                .iter()
                .map(|&z| symbol_of(z).unwrap_or("?").to_string())
                .collect();
            charges.push(AtomCharges {
                scheme: scheme.to_string(),
                elements,
                atomic_numbers: pop.atomic_numbers,
                charges: pop.charges,
            });
        }

        let dipole = v.dipole().map(|d| DipoleJson {
            magnitude_au: d.magnitude_au,
            total_au: d.total_au,
        });

        let thermochemistry = v.thermochemistry().map(|t| ThermoJson {
            temperature_k: t.temperature_k,
            el_energy_eh: t.el_energy_eh,
            zpe_eh: t.zpe_eh,
            inner_energy_u_eh: t.inner_energy_u_eh,
            enthalpy_h_eh: t.enthalpy_h_eh,
            t_times_s_eh: t.t_times_s_eh,
            free_energy_g_eh: t.free_energy_g_eh,
        });

        let frequencies = match hess {
            None => None,
            Some(h) => {
                let f = h.frequencies()?;
                let atoms = h.atoms()?;
                let ir: Vec<f64> = h.ir_spectrum()?.iter().map(|r| r.intensity_km_mol).collect();
                let modes = h.normal_modes()?;
                Some(FrequenciesJson {
                    elements: atoms.iter().map(|a| a.element.clone()).collect(),
                    frequencies_cm: f.values_cm,
                    imaginary_count: f.imaginary_count,
                    zero_count: f.zero_count,
                    is_linear: f.is_linear,
                    ir_intensity_km_mol: ir,
                    n_modes: modes.n,
                    normal_modes: modes.into_row_major(),
                    // `$actual_temperature` (measured 0.0) — NOT the calculation
                    // temperature; kept only to surface the raw field. The real
                    // temperature is ThermoJson::temperature_k (from .property.txt).
                    // Never wire this into a temperature display. See hess.rs / parse-sources.md.
                    temperature_k: h.actual_temperature(),
                    // `$frequency_scale_factor` (measured 1.0 = ORCA applied none).
                    // The UI's display scale is a separate, user-owned plot choice.
                    scale_factor: h.frequency_scale_factor(),
                    unknown_sections: h.unknown_section_names(),
                })
            }
        };

        let gradient = v.last_gradient().map(|g| {
            let order_elements = v
                .geometry_for(g.geometry_index)
                .map(|geom| geom.atoms.iter().map(|a| a.element.clone()).collect())
                .unwrap_or_default();
            GradientJson {
                geometry_index: g.geometry_index,
                order_elements,
                grad_eh_per_bohr: g.grad_eh_per_bohr,
            }
        });

        Ok(ParsedResults {
            parser_version: PARSER_VERSION,
            final_energy_eh: v.final_single_point_energy(),
            dipole,
            charges,
            thermochemistry,
            final_geometry,
            gradient,
            frequencies,
            trajectory,
            orbitals,
            scan,
            neb,
            // Populated by the caller (`parse_and_store`) from `output.out`, which this
            // constructor does not have — the single place the atom count is known.
            mayer_bond_orders: None,
            unknown_blocks: v.unknown_block_names(),
        })
    }

    /// Build a scan job's result from its profile alone (Phase 4.5 B1 fix). A multi-point
    /// scan has no single structure, so the single-structure quantities
    /// (charges/dipole/thermo/gradient/frequencies/orbitals/trajectory) are absent —
    /// leaving them empty is correct, not a gap. Two fields the UI still needs:
    /// - `final_energy_eh` = the profile's LAST point (the composite `act` energy) — the
    ///   value the ResultsCard scan branch already shows; sourced from the profile, NOT
    ///   from the skipped `property.rs`.
    /// - `final_geometry` = that last point's optimized structure (`input.NNN.xyz`), so
    ///   the viewer has a molecule and the scan panel's element cross-check
    ///   (`referenceElements`) has an order. `.xyz` is Å (measured, parse-sources.md);
    ///   the profile's own cross-check already confirmed this scan's coordinate units.
    fn from_scan_profile(dir: &Path, scan: ScanProfileJson) -> Result<ParsedResults, AppError> {
        let final_energy_eh = scan.points.last().map(|p| p.energy_act_eh);
        let n = scan.points.len();
        let final_geometry = if n > 0 {
            let path = dir.join(format!("input.{n:03}.xyz"));
            let (elements, xyz_angstrom) =
                XyzFile::from_path(&path)?.first_frame().ok_or_else(|| {
                    AppError::Internal(format!(
                        "scan final-point geometry ({}) is empty",
                        path.display()
                    ))
                })?;
            FinalGeometry { elements, xyz_angstrom }
        } else {
            FinalGeometry { elements: Vec::new(), xyz_angstrom: Vec::new() }
        };
        Ok(ParsedResults {
            parser_version: PARSER_VERSION,
            final_energy_eh,
            dipole: None,
            charges: Vec::new(),
            thermochemistry: None,
            final_geometry,
            gradient: None,
            frequencies: None,
            trajectory: None,
            orbitals: None,
            scan: Some(scan),
            neb: None,
            mayer_bond_orders: None, // a scan is multi-structure — no single final table
            unknown_blocks: Vec::new(),
        })
    }

    /// Build a NEB-TS job's result from its BAND (Stage E3a-1 completion). A NEB job is
    /// multi-geometry; its authoritative result is the band + the **converged TS**, which
    /// becomes the job's `final_geometry` (element order == reactant, `neb.rs`-asserted) and
    /// `final_energy_eh` (the converged-TS comment energy). The single-structure quantities
    /// (charges/dipole/thermo/gradient/frequencies/orbitals/trajectory) are absent — a NEB-TS
    /// run has no Freq and no single reference structure; leaving them empty is correct, not
    /// a gap (same discipline as `from_scan_profile`). The band rides in `neb`.
    fn from_neb(neb: NebResultsJson) -> Result<ParsedResults, AppError> {
        Ok(ParsedResults {
            parser_version: PARSER_VERSION,
            final_energy_eh: neb.ts_energy_eh,
            dipole: None,
            charges: Vec::new(),
            thermochemistry: None,
            final_geometry: neb.ts_geometry.clone(),
            gradient: None,
            frequencies: None,
            trajectory: None,
            orbitals: None,
            scan: None,
            neb: Some(neb),
            mayer_bond_orders: None, // a NEB-TS run has no single final Mayer table
            unknown_blocks: Vec::new(),
        })
    }
}

/// Build the trajectory JSON from a verified `_trj.xyz`. Element order stored once.
fn trajectory_json(xyz: &crate::parse::xyz::Verified) -> TrajectoryJson {
    let frames = xyz.frames();
    let elements = frames
        .first()
        .map(|f| f.atoms.iter().map(|a| a.element.clone()).collect())
        .unwrap_or_default();
    let out_frames = frames
        .iter()
        .map(|f| TrajFrame {
            energy_eh: f.energy_eh,
            xyz_angstrom: f
                .atoms
                .iter()
                .map(|a| [a.xyz[0].angstrom(), a.xyz[1].angstrom(), a.xyz[2].angstrom()])
                .collect(),
        })
        .collect();
    TrajectoryJson { n_frames: frames.len(), elements, frames: out_frames }
}

/// Parse + verify the relaxed-scan profile in `dir`, or `Ok(None)` when the job is
/// not a scan (no `.relaxscanact.dat` — the absent-is-normal pattern, like
/// `orca_json::ensure_gbw_json`). The scanned coordinate spec is parsed from
/// `input_content` here (the reader never reads `input.inp`) and passed into
/// `verify`, whose geometry cross-check confirms the coordinate is Å at read time.
fn relaxscan_profile(
    dir: &Path,
    input_content: &str,
) -> Result<Option<ScanProfileJson>, crate::parse::ParseError> {
    let Some(raw) = RelaxScan::from_path(dir)? else {
        return Ok(None);
    };
    // A present `.relaxscanact.dat` means there WAS a scan; if we cannot read the
    // `%geom Scan` line from the input we cannot run the B geometry cross-check, so
    // fail loudly (rule #9) rather than store an unconfirmed coordinate.
    let spec = parse_scan_spec(input_content).ok_or_else(|| {
        crate::parse::ParseError::Malformed {
            field: "%geom Scan line".into(),
            detail: "a .relaxscanact.dat is present but the input has no parseable Scan line".into(),
        }
    })?;
    let v = raw.verify(&spec)?;
    Ok(Some(ScanProfileJson {
        kind: v.kind().to_string(),
        atoms: v.atoms().to_vec(),
        coordinate_unit: v.coordinate_unit().to_string(),
        points: v
            .points()
            .iter()
            .map(|p| ScanPointJson {
                coordinate: p.coordinate,
                energy_act_eh: p.energy_act_eh,
                energy_scf_eh: p.energy_scf_eh,
            })
            .collect(),
    }))
}

/// Build the MO JSON from a verified `orca_2json`. Coefficients are never included.
fn orbitals_json(mo: &crate::parse::mo::Verified) -> OrbitalsJson {
    let homo_lumo = mo.homo_lumo().map(|(homo, lumo, gap)| HomoLumoJson {
        homo_eh: homo,
        lumo_eh: lumo,
        gap_eh: gap,
    });
    OrbitalsJson {
        energy_unit: mo.energy_unit().unwrap_or("Eh").to_string(),
        orbitals: mo.orbitals().iter().map(|&(e, o)| [e, o]).collect(),
        homo_lumo,
    }
}

// --------------------------------------------------------------------------- //
// The completion hook                                                           //
// --------------------------------------------------------------------------- //

/// What happened when we tried to parse a completed job's results. The caller
/// (LocalBackend) maps these to job status — crucially keeping "the calculation
/// failed" (→ `failed`, decided earlier) distinct from "the calculation
/// succeeded but our parse did not" (→ stays `completed`, error recorded).
#[derive(Debug)]
pub enum ParseOutcome {
    /// Parsed, verified, stored, read back — status should become `parsed`.
    Parsed,
    /// No single-structure artifact to parse — the job stays `completed` (not a
    /// failure). Either `.property.txt` is absent, OR the job is a special type whose
    /// authoritative result is read elsewhere and must NOT be driven to `parsed` by the
    /// single-structure readers: a **GOAT** conformer search (result = the ensemble,
    /// `read_job_ensemble`). (A relaxed scan is the other special type, but it stores a
    /// profile row and returns `Parsed` — see `parse_and_store_scan`.)
    NoArtifact,
    /// The file exists but parsing/verification/storage failed — OUR problem, the
    /// calculation itself is fine. Status stays `completed`; this message is shown.
    ParseFailed(String),
}

/// Parse `input.property.txt` in `job_dir`, verify it against the job's own input
/// geometry, store it, and read it back. The reference geometry is derived here
/// from `input_content` and **passed in** — the artifact reader never reads
/// `input.inp` itself.
pub fn parse_and_store(
    conn: &Connection,
    job_id: &str,
    job_dir: &str,
    input_content: &str,
) -> ParseOutcome {
    let dir = Path::new(job_dir);
    let path = dir.join("input.property.txt");
    if !path.exists() {
        return ParseOutcome::NoArtifact;
    }

    // A relaxed scan is a MULTI-POINT job (Phase 4.5 B1 fix). Its `.property.txt` holds
    // one `$Geometry` per optimization cycle across ALL scan points, so the
    // single-structure authoritative readers do not apply: `property.rs` and the `_trj`
    // `xyz.rs` verify anchor their geometry post-condition on the INPUT (premise: "first
    // structure == input"), but a scan's first `$Geometry` is scan point 1's
    // constrained-optimized geometry, not the input; and `hess.rs`/`mo.rs` anchor on a
    // single FINAL structure, which a multi-point scan does not have. A scan's
    // authoritative result is the PROFILE (B1), whose own per-point geometry cross-check
    // is its live units guard (rule #11). Route a scan there and skip the
    // single-structure readers — detection is the same `.relaxscanact.dat` presence B1
    // already uses. See wiki/debugging/015-scan-property-post-condition.md.
    if dir.join("input.relaxscanact.dat").exists() {
        return parse_and_store_scan(conn, job_id, dir, input_content);
    }

    // A GOAT conformer search is ANOTHER special job type whose authoritative result is
    // the ENSEMBLE (`input.finalensemble.xyz`, read on demand by `read_job_ensemble`),
    // not a single-structure Results dashboard — exactly like a scan's result is the
    // profile. Its `.property.txt`/`_trj.xyz` are the internal optimization CYCLES of one
    // candidate (measured: a butanone GOAT has 17 $Geometry cycles + an 18-frame trj),
    // and its first `$Geometry` ≈ the input, so the single-structure readers would happily
    // parse it and drive the job to `parsed` — surfacing a misleading "17 optimization
    // cycles" trajectory and (via the ensemble panel's terminal-status guard) hiding the
    // conformer ensemble. So route GOAT AWAY from the single-structure parse: do not run
    // the readers, do not store a results row, and leave the job `completed` (NoArtifact →
    // Completed in the caller). The ensemble is read separately. Mirrors the scan branch
    // above and the absent-is-normal discipline. See wiki/debugging/017-goat-parsed-hid-ensemble.md.
    if input_is_goat(input_content) {
        return ParseOutcome::NoArtifact;
    }

    // A NEB-TS job is a THIRD special job type — MULTI-geometry, like a scan. Its
    // `.property.txt` holds the BAND (N `$Geometry` blocks), its `.gbw`/`.xyz` the TS, and
    // the input `* xyz` is the REACTANT — so the single-geometry reference model (input ≈
    // property-final, valid for SP/Opt) does not fit: `PropertyFile::verify(&input_ref)`
    // would fire a ~2.45 Å `GeometryMismatch` (a real different-structure, r≈1 — not a unit
    // error, not staleness) and abort before the band reader runs. Route a NEB job to its
    // own band+TS parse and skip the reactant-referenced single-structure post-conditions.
    // See wiki/debugging/020-neb-multigeometry-vs-single-geometry-reference.md.
    if input_has_neb(input_content) {
        return parse_and_store_neb(conn, job_id, dir, input_content);
    }

    // The input's start geometry (with elements) — the reference for `.property.txt`
    // (coords only; it checks element order internally) and for `_trj.xyz` (whose
    // first frame is the start).
    let mut input_ref = match input_reference(input_content) {
        Some(r) if !r.z.is_empty() => r,
        // An unreadable input coordinate block is a LOUD, named parse failure — not a
        // silent skip. The derived identity map (below) is built from this block, so
        // if it cannot be read we cannot verify atom identity at all.
        _ => return ParseOutcome::ParseFailed("no * xyz * block in the job input".into()),
    };

    // The job's IndexMap<OrcaIndex> (unit 1e): a MINTED map from `jobs.index_map_json`
    // (created at create_job from the text↔scene correspondence), or the DERIVED
    // identity map for a legacy/skipped job. `atom_ids` is the INDEPENDENT AtomId
    // anchor the map is cross-checked against inside verify() — scene-sourced when
    // minted (so a corrupted stored map cannot cancel itself out), synthetic 0..n when
    // derived. The map is verified against each artifact, never trusted.
    let (job_map, atom_ids) = match resolve_job_mapping(conn, job_id, &input_ref) {
        Ok(m) => m,
        Err(e) => return ParseOutcome::ParseFailed(e.to_string()),
    };
    input_ref.ids = atom_ids.clone();

    let verified = match PropertyFile::from_path(&path)
        .and_then(|pf| pf.verify(&input_ref, &job_map))
    {
        Ok(v) => v,
        Err(e) => return ParseOutcome::ParseFailed(e.to_string()),
    };

    // `.hess` is optional: SP/GOAT have none (a normal state). When present, verify
    // it against the OPTIMIZED geometry (the Freq geometry) — the `.property.txt`
    // final `$Geometry`, which we already have — not `input.inp` (the start).
    let hess_path = dir.join("input.hess");
    let hess_verified = if hess_path.exists() {
        match final_geometry_reference(&verified, &atom_ids)
            .and_then(|r| Ok(HessFile::from_path(&hess_path)?.verify(&r, &job_map)?))
        {
            Ok(hv) => Some(hv),
            Err(e) => return ParseOutcome::ParseFailed(format!(".hess: {e}")),
        }
    } else {
        None
    };

    // Trajectory (`_trj.xyz`): first frame == the input start geometry. Optional.
    let trj_path = dir.join("input_trj.xyz");
    let trajectory = if trj_path.exists() {
        match XyzFile::from_path(&trj_path).and_then(|x| x.verify(&input_ref, &job_map)) {
            Ok(xv) => Some(trajectory_json(&xv)),
            Err(e) => return ParseOutcome::ParseFailed(format!("_trj.xyz: {e}")),
        }
    } else {
        None
    };

    // Post-condition (rule #9): the optimization-cycle energies have TWO
    // independent sources — the streaming `.out` convergence parser (a regex over
    // a fragile text format) and the `_trj.xyz` frame-comment energies (measured,
    // unit 3.7). Cross-check them so a silent drift in either (e.g. an ORCA 6.2
    // format change that breaks the streaming regex) is a recorded diagnostic, not
    // an invisible wrong/missing energy. This is exactly the check that would have
    // caught defect 2 automatically. Skipped for GOAT: its trajectory is
    // CONFORMERS, not the cycles of one optimization, so the two are unrelated by
    // construction (GOAT has 17 inner-opt blocks vs 18 conformer frames — measured).
    if let Some(traj) = &trajectory {
        if !input_is_goat(input_content) {
            let opt_e = optpoint_energies(&dir.join("output.out"));
            let traj_e: Vec<f64> = traj.frames.iter().filter_map(|f| f.energy_eh).collect();
            if let Err(msg) = cycle_energy_cross_check(&opt_e, &traj_e) {
                return ParseOutcome::ParseFailed(msg);
            }
        }
    }

    // MO energies/occupancies (`orca_2json` over `.gbw`): generated lazily via the
    // user-configured ORCA path (ADR-009); absent for xTB/GOAT gbw (normal). The
    // reference is the optimized (final) geometry — orca_2json's coords are final.
    // An MO-pipeline failure (converter spawn OR the geometry verify) is NON-FATAL:
    // "No MO data is a normal state" (unit 3.7). Energy/geometry/thermo/charges/
    // frequencies still parse and display; only orbitals go absent. So a failure here
    // WARNS and sets `orbitals = None` — it never aborts the whole results parse.
    // (Convention: `eprintln!`, as elsewhere — the crate has no `log`/`tracing` dep.)
    let orbitals = match read_orca_path(conn) {
        None => None,
        Some(orca_path) => match crate::orca_json::ensure_gbw_json(&orca_path, dir) {
            Err(e) => {
                eprintln!("orca_2json (no orbitals): {e}");
                None
            }
            Ok(None) => None,
            Ok(Some(json)) => match final_geometry_reference(&verified, &atom_ids)
                .and_then(|r| Ok(crate::parse::mo::MoJson::from_path(&json)?.verify(&r, &job_map)?))
            {
                Ok(mv) => Some(orbitals_json(&mv)),
                Err(e) => {
                    eprintln!("orca_2json (no orbitals): {e}");
                    None
                }
            },
        },
    };

    // Relaxed-scan profile (`.relaxscanact/.relaxscanscf.dat`): present only for a
    // scan job, `None` otherwise (absent-is-normal). Its geometry cross-check confirms
    // the coordinate column is Å against each `input.NNN.xyz` (rule #11).
    let scan = match relaxscan_profile(dir, input_content) {
        Ok(s) => s,
        Err(e) => return ParseOutcome::ParseFailed(format!("relaxscan: {e}")),
    };

    // NEB-TS band: on the STANDARD path this is always `None` — a NEB job is detected up
    // front (`input_has_neb`) and routed to `parse_and_store_neb` before this point, so no
    // NEB job reaches here (like a scan branching at `.relaxscanact.dat`). Kept for the
    // absent-is-normal symmetry; the reference is the property-final only because a NEB
    // job never gets this far.
    let neb = match final_geometry_reference(&verified, &atom_ids)
        .and_then(|r| neb_results(dir, input_content, &r))
    {
        Ok(n) => n,
        Err(e) => return ParseOutcome::ParseFailed(format!("neb: {e}")),
    };

    let mut results = match ParsedResults::from_verified(
        &verified,
        hess_verified.as_ref(),
        trajectory,
        orbitals,
        scan,
        neb,
    ) {
        Ok(r) => r,
        Err(e) => return ParseOutcome::ParseFailed(e.to_string()),
    };

    // Mayer bond orders (`output.out`, streamed — rule #5): the computed authoritative
    // order of the FINAL structure, keyed by the same 0-based atom indices as
    // `final_geometry`. Absent for xTB / an SP that didn't print it (→ `None`, normal).
    // A malformed/out-of-range table is a LOUD failure (rule #9), never a silent bad pair.
    let natoms = results.final_geometry.elements.len();
    match crate::parse::mayer::read_mayer(&dir.join("output.out"), natoms) {
        Ok(m) => results.mayer_bond_orders = m,
        Err(e) => return ParseOutcome::ParseFailed(format!("mayer: {e}")),
    }

    if let Err(e) = store(conn, job_id, &results) {
        return ParseOutcome::ParseFailed(e.to_string());
    }
    // storage post-condition (rule #9): read back and check the per-atom arrays
    // survived serialization with their element order.
    if let Err(e) = verify_stored(conn, job_id, &results) {
        return ParseOutcome::ParseFailed(format!("stored results failed read-back: {e}"));
    }
    ParseOutcome::Parsed
}

/// Parse + store a relaxed-scan job from its PROFILE alone (Phase 4.5 B1 fix). The
/// caller has already confirmed `input.relaxscanact.dat` is present, so this job IS a
/// scan; the single-structure readers' premise ("first structure == input" /
/// "one final structure") is false for it and they are skipped. The profile IS the
/// scan's authoritative result and its per-point geometry cross-check is the scan's
/// units guard (rule #11) — no tolerance is loosened, the guard simply moved to where
/// its premise holds. A present `.relaxscanact.dat` that does not parse is a LOUD
/// failure (rule #9), never a silent skip.
fn parse_and_store_scan(
    conn: &Connection,
    job_id: &str,
    dir: &Path,
    input_content: &str,
) -> ParseOutcome {
    let scan = match relaxscan_profile(dir, input_content) {
        Ok(Some(s)) => s,
        Ok(None) => {
            return ParseOutcome::ParseFailed(
                "relaxscan: input.relaxscanact.dat is present but no scan profile parsed".into(),
            )
        }
        Err(e) => return ParseOutcome::ParseFailed(format!("relaxscan: {e}")),
    };
    let results = match ParsedResults::from_scan_profile(dir, scan) {
        Ok(r) => r,
        Err(e) => return ParseOutcome::ParseFailed(e.to_string()),
    };
    if let Err(e) = store(conn, job_id, &results) {
        return ParseOutcome::ParseFailed(e.to_string());
    }
    // storage post-condition (rule #9): read back and check the record survived.
    if let Err(e) = verify_stored(conn, job_id, &results) {
        return ParseOutcome::ParseFailed(format!("stored results failed read-back: {e}"));
    }
    ParseOutcome::Parsed
}

/// Parse + store a NEB-TS job from its BAND + converged TS (Stage E3a-1 completion). The
/// caller confirmed the input carries `NEB-TS`, so this job IS a NEB run: it is
/// MULTI-geometry (band + TS + product) and the single-geometry, input-reactant-referenced
/// post-conditions (`PropertyFile::verify` / `_trj` first-frame / `mo` geom) do NOT apply —
/// they would fail on the reactant≠TS difference (`debugging/020`). The authoritative result
/// is the band (`neb.rs`), and the job's `final_geometry`/energy are the **converged TS**.
/// The reactant `* xyz` supplies ONLY the element ORDER the converged TS is checked against
/// (order preserved, not a geometry match). A present-but-malformed band is LOUD (rule #9).
fn parse_and_store_neb(
    conn: &Connection,
    job_id: &str,
    dir: &Path,
    input_content: &str,
) -> ParseOutcome {
    // Reactant reference from the input `* xyz` — for the TS element-ORDER check only.
    let reactant_ref = match input_reference(input_content) {
        Some(r) if !r.z.is_empty() => r,
        _ => return ParseOutcome::ParseFailed("no * xyz * block in the NEB job input".into()),
    };
    let neb = match neb_results(dir, input_content, &reactant_ref) {
        Ok(Some(n)) => n,
        Ok(None) => {
            return ParseOutcome::ParseFailed(
                "NEB job but no `input.NEB.log` band was parsed".into(),
            )
        }
        Err(e) => return ParseOutcome::ParseFailed(format!("neb: {e}")),
    };
    let results = match ParsedResults::from_neb(neb) {
        Ok(r) => r,
        Err(e) => return ParseOutcome::ParseFailed(e.to_string()),
    };
    if let Err(e) = store(conn, job_id, &results) {
        return ParseOutcome::ParseFailed(e.to_string());
    }
    if let Err(e) = verify_stored(conn, job_id, &results) {
        return ParseOutcome::ParseFailed(format!("stored results failed read-back: {e}"));
    }
    ParseOutcome::Parsed
}

/// Whether a job's input requests a NEB run — a `NEB`/`NEB-TS`/`NEB-CI` token on a `!`
/// keyword line. Mirrors [`input_is_goat`]'s whole-token, case-insensitive tokenizer
/// (split on non-alphanumeric, so `NEB-TS` yields the `NEB` token). NOT a fragile regex.
fn input_has_neb(input_content: &str) -> bool {
    input_content
        .lines()
        .filter(|l| l.trim_start().starts_with('!'))
        .any(|l| {
            l.split(|c: char| !c.is_ascii_alphanumeric())
                .any(|w| w.eq_ignore_ascii_case("NEB"))
        })
}

/// NEB-TS band results, or `None` when the job is not a NEB run (absent-is-normal). A
/// present `.NEB.log` that does not parse/verify is a LOUD failure (rule #9). The
/// `NImages + 2` image-count post-condition uses `NImages` parsed from THIS job's input
/// (the reader never reads `input.inp`); the converged-TS element order is checked
/// against `reference` — the REACTANT order (the input `* xyz` on the NEB route, or the
/// property-final on the standard path; both equal the reactant order, ORCA preserves it).
fn neb_results(
    dir: &Path,
    input_content: &str,
    reference: &ReferenceGeometry,
) -> Result<Option<NebResultsJson>, AppError> {
    let Some(band) = crate::parse::neb::NebBand::from_path(dir)? else {
        return Ok(None);
    };
    let n_images = parse_nimages(input_content).ok_or_else(|| {
        AppError::Internal("NEB job present but its input has no `NImages`".into())
    })? + 2;
    let v = band.verify(reference, n_images)?;
    let map_images = |imgs: &[crate::parse::neb::BandImage]| -> Vec<NebImageJson> {
        imgs.iter()
            .map(|i| NebImageJson { distance_angstrom: i.distance_angstrom, energy_eh: i.energy_eh })
            .collect()
    };
    let iterations = v
        .iterations()
        .iter()
        .map(|it| NebIterationJson {
            index: it.index,
            images: map_images(&it.images),
            barrier_eh: it.barrier_eh,
            climbing_image: it.climbing_image,
        })
        .collect();
    let (els, xyz) = v.ts_geometry();
    let ts_geometry = FinalGeometry {
        elements: els.to_vec(),
        xyz_angstrom: xyz.to_vec(),
    };
    Ok(Some(NebResultsJson {
        iterations,
        mep: map_images(v.mep()),
        final_barrier_eh: v.final_barrier_eh(),
        ts_geometry,
        ts_energy_eh: v.ts_energy_eh(),
    }))
}

/// The `.property.txt` optimized (last) geometry as the reference for the `.hess`/`.mo`
/// geometry post-condition — the Freq/final geometry, not the input geometry. `atom_ids`
/// is the job's AtomId anchor (scene-sourced when minted, synthetic when derived), keyed
/// to the same emit order as the final geometry (== artifact order).

/// `NImages <n>` from a NEB input's `%neb` block (case-insensitive), or `None`.
fn parse_nimages(input_content: &str) -> Option<usize> {
    static RE: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r"(?i)NImages\s+(\d+)").unwrap());
    RE.captures(input_content)?.get(1)?.as_str().parse().ok()
}

fn final_geometry_reference(
    v: &Verified,
    atom_ids: &[AtomId],
) -> Result<ReferenceGeometry, AppError> {
    let geoms = v.geometries()?;
    let last = geoms
        .last()
        .ok_or_else(|| AppError::Internal("verified property.txt has no geometry".into()))?;
    let z: Vec<u8> = last.atoms.iter().map(|a| a.z).collect();
    Ok(ReferenceGeometry {
        ids: atom_ids.to_vec(),
        xyz_angstrom: last
            .atoms
            .iter()
            .map(|a| [a.xyz[0].angstrom(), a.xyz[1].angstrom(), a.xyz[2].angstrom()])
            .collect(),
        z,
    })
}

/// What `jobs.index_map_json` holds (unit 1e). Externally tagged, so the on-disk
/// shapes are `{"minted":[<AtomId u32s in text-row order>]}` and
/// `{"skipped":"<reason>"}` — the self-describing skip the parser records instead of a
/// silent NULL.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub enum StoredIndexMap {
    #[serde(rename = "minted")]
    Minted(Vec<u32>),
    #[serde(rename = "skipped")]
    Skipped(String),
}

/// Mint the stored envelope at `create_job` (ADR-016 amendment). ALWAYS returns a
/// value — a minted map or a self-describing skip — so the job is never blocked
/// (input validity is ORCA's business; the map is ours). Reads the **submitted text**
/// and verifies it against the scene; a scene-only mint is refused by construction
/// (`orcastudio_core::mint_index_map`). This is the single mint site — a clone / "new
/// iteration" runs through `create_job` and thus mints its OWN map, never inherited.
pub fn mint_stored_index_map(input_content: &str, scene_json: Option<&str>) -> StoredIndexMap {
    let Some(sj) = scene_json else {
        return StoredIndexMap::Skipped(
            "no scene snapshot on this job (created from raw text) — parse uses the derived identity map"
                .into(),
        );
    };
    match orcastudio_core::scene::deserialize_scene(sj) {
        Err(e) => StoredIndexMap::Skipped(format!(
            "scene snapshot unreadable ({e}) — derived identity map at parse"
        )),
        Ok(scene) => match orcastudio_core::mint_index_map(&scene, input_content) {
            Ok(map) => StoredIndexMap::Minted(map.order().iter().map(|a| a.get()).collect()),
            Err(reason) => StoredIndexMap::Skipped(reason),
        },
    }
}

/// The job's `IndexMap<OrcaIndex>` **and** the AtomId anchor to key references by
/// (unit 1e). Reads `jobs.index_map_json`:
/// - **Minted** + a readable scene → the stored map is used, and the anchor is the
///   scene's `atom_order()` — read from `scene_json`, **independent of the stored
///   map**, so a corrupted stored map is caught by the artifact cross-check in
///   `verify()` rather than cancelling itself out.
/// - **Skipped / NULL / minted-but-scene-gone** → the derived identity map (unit 1d
///   path), anchor `0..n`. The derived path re-verifies against the artifact anyway.
fn resolve_job_mapping(
    conn: &Connection,
    job_id: &str,
    reference: &ReferenceGeometry,
) -> Result<(IndexMap<OrcaIndex>, Vec<AtomId>), AppError> {
    let raw: Option<String> = conn.query_row(
        "SELECT index_map_json FROM jobs WHERE id = ?1",
        [job_id],
        |r| r.get(0),
    )?;
    let derived = || (identity_map_for(reference), derived_identity_ids(reference.z.len()));

    let Some(raw) = raw else { return Ok(derived()) };
    match serde_json::from_str::<StoredIndexMap>(&raw) {
        Ok(StoredIndexMap::Minted(order)) => {
            let scene_json: Option<String> = conn.query_row(
                "SELECT scene_json FROM jobs WHERE id = ?1",
                [job_id],
                |r| r.get(0),
            )?;
            match scene_json.and_then(|s| orcastudio_core::scene::deserialize_scene(&s).ok()) {
                Some(scene) => {
                    let ids: Vec<AtomId> = order.iter().map(|&u| AtomId::new(u)).collect();
                    let map = IndexMap::from_emit_order(&ids);
                    // Anchor from the SCENE, not from the stored map (independence).
                    Ok((map, scene.atom_order()))
                }
                // Minted but the scene snapshot is gone/unreadable — fall back to the
                // derived path (still artifact-verified) rather than trust the map alone.
                None => Ok(derived()),
            }
        }
        // Skipped, or an unparseable envelope — the derived path (1d), artifact-verified.
        Ok(StoredIndexMap::Skipped(_)) | Err(_) => Ok(derived()),
    }
}

/// Idempotent upsert keyed by `job_id`: re-parsing the same job updates the row,
/// never duplicates it.
fn store(conn: &Connection, job_id: &str, r: &ParsedResults) -> Result<(), AppError> {
    let data_json = serde_json::to_string(r)
        .map_err(|e| AppError::Internal(format!("serialize results: {e}")))?;
    let thermo = r.thermochemistry.as_ref();
    // Narrow columns for the card + job-list sorting: imaginary-mode count (the
    // minimum/TS warning) and the HOMO/LUMO gap. NULL when the source is absent.
    let imaginary_count = r.frequencies.as_ref().map(|f| f.imaginary_count as i64);
    let gap_eh = r
        .orbitals
        .as_ref()
        .and_then(|o| o.homo_lumo.as_ref())
        .map(|hl| hl.gap_eh);
    conn.execute(
        "INSERT INTO results (
            job_id, final_energy_eh, dipole_magnitude_au,
            zpe_eh, inner_energy_u_eh, enthalpy_h_eh, t_times_s_eh, free_energy_g_eh,
            imaginary_count, homo_lumo_gap_eh, data_json, parser_version, parsed_at
         ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12, datetime('now'))
         ON CONFLICT(job_id) DO UPDATE SET
            final_energy_eh=?2, dipole_magnitude_au=?3,
            zpe_eh=?4, inner_energy_u_eh=?5, enthalpy_h_eh=?6, t_times_s_eh=?7, free_energy_g_eh=?8,
            imaginary_count=?9, homo_lumo_gap_eh=?10, data_json=?11, parser_version=?12,
            parsed_at=datetime('now')",
        params![
            job_id,
            r.final_energy_eh,
            r.dipole.as_ref().map(|d| d.magnitude_au),
            thermo.map(|t| t.zpe_eh),
            thermo.map(|t| t.inner_energy_u_eh),
            thermo.map(|t| t.enthalpy_h_eh),
            thermo.map(|t| t.t_times_s_eh),
            thermo.map(|t| t.free_energy_g_eh),
            imaginary_count,
            gap_eh,
            data_json,
            r.parser_version,
        ],
    )?;
    Ok(())
}

/// The authoritative final energy for a job, from the narrow `results` column
/// (populated from `.property.txt`, ADR-012) — `None` if unparsed or absent. Read
/// via the narrow column, not by deserializing the whole `data_json` (unit 3.9
/// defect 2: the header/list energy comes from here, not the output.out tail).
pub fn stored_final_energy(conn: &Connection, job_id: &str) -> Option<f64> {
    conn.query_row(
        "SELECT final_energy_eh FROM results WHERE job_id = ?1",
        params![job_id],
        |r| r.get::<_, Option<f64>>(0),
    )
    .ok()
    .flatten()
}

/// One relaxed-scan point geometry (`input.NNN.xyz`) for the profile viewer
/// (Phase 4.5 B2). Its own element order — the UI cross-checks it against the
/// result geometry before rendering (`elementsAgree`, like the trajectory).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScanGeometry {
    pub elements: Vec<String>,
    pub xyz_angstrom: Vec<[f64; 3]>,
}

/// Load the per-point geometries of a relaxed scan — `input.001.xyz … input.00N.xyz`
/// in point order, N taken from the stored [`ScanProfileJson`] (Phase 4.5 B2). Reads
/// each file whole via the `xyz` reader (small, rule #5); **writes nothing to the job
/// dir** (rule #3). `Ok(None)` when the job is not a scan or has no dir yet
/// (absent-is-normal). Re-parses no `.dat` (ADR-012): the profile is already stored;
/// this only fetches the point geometries the chart's click-to-view needs.
pub fn read_scan_geometries(
    conn: &Connection,
    job_id: &str,
    job_dir: Option<&str>,
) -> Result<Option<Vec<ScanGeometry>>, AppError> {
    let Some(results) = read_job_results(conn, job_id)? else {
        return Ok(None);
    };
    let Some(scan) = results.scan else {
        return Ok(None); // not a scan job
    };
    let Some(job_dir) = job_dir else {
        return Ok(None); // no dir → nothing to read
    };
    let dir = Path::new(job_dir);
    let mut out = Vec::with_capacity(scan.points.len());
    for k in 1..=scan.points.len() {
        let path = dir.join(format!("input.{k:03}.xyz"));
        let xyz = XyzFile::from_path(&path)?;
        let (elements, xyz_angstrom) = xyz.first_frame().ok_or_else(|| {
            AppError::Internal(format!("scan point {k} geometry ({}) is empty", path.display()))
        })?;
        out.push(ScanGeometry { elements, xyz_angstrom });
    }
    Ok(Some(out))
}

/// Read the stored results for a job (the full JSON structure), or `None`.
pub fn read_job_results(conn: &Connection, job_id: &str) -> Result<Option<ParsedResults>, AppError> {
    let json: Option<String> = conn
        .query_row(
            "SELECT data_json FROM results WHERE job_id = ?1",
            params![job_id],
            |r| r.get(0),
        )
        .ok();
    match json {
        None => Ok(None),
        Some(j) => serde_json::from_str(&j)
            .map(Some)
            .map_err(|e| AppError::Internal(format!("deserialize results: {e}"))),
    }
}

/// Storage-boundary post-condition: what came back must have the same per-atom
/// element order and count as what we wrote. Serialization is a process boundary
/// too (rule #9).
fn verify_stored(conn: &Connection, job_id: &str, expected: &ParsedResults) -> Result<(), AppError> {
    let stored = read_job_results(conn, job_id)?
        .ok_or_else(|| AppError::Internal("results vanished immediately after write".into()))?;
    let n_atoms = stored.final_geometry.elements.len();
    if stored.charges.len() != expected.charges.len() {
        return Err(AppError::Internal("charge scheme count changed on round-trip".into()));
    }
    for (a, b) in expected.charges.iter().zip(&stored.charges) {
        if a.elements != b.elements {
            return Err(AppError::Internal(format!(
                "charge element order changed on round-trip ({} scheme)",
                a.scheme
            )));
        }
        if b.charges.len() != n_atoms {
            return Err(AppError::Internal(format!(
                "{} charges = {} but geometry has {} atoms",
                b.scheme,
                b.charges.len(),
                n_atoms
            )));
        }
    }
    Ok(())
}

/// Energy of every optimization cycle, from the streaming `.out` convergence
/// parser (`convergence.rs`). Reads line-by-line (domain rule #5) — the file is
/// never loaded whole. Empty for a single point (no cycles) or a missing file.
fn optpoint_energies(output_path: &Path) -> Vec<f64> {
    let Ok(file) = File::open(output_path) else {
        return Vec::new();
    };
    let mut parser = ConvergenceParser::new();
    let mut out = Vec::new();
    for line in BufReader::new(file).lines().map_while(Result::ok) {
        if let Some(ConvergenceEvent::Opt(p)) = parser.feed(&line) {
            if let Some(e) = p.energy {
                out.push(e);
            }
        }
    }
    out
}

/// Max |ΔE| tolerated between the two cycle-energy sources. They agree
/// bit-for-bit on every real job measured (dexketoprofen, ethane, saddle); the
/// tolerance only guards formatting/rounding, far below any real divergence.
const CYCLE_ENERGY_TOL_EH: f64 = 1e-6;

/// Compare the two independent optimization-cycle energy sequences — the `.out`
/// convergence parser vs the `_trj.xyz` frame energies. `Ok(())` when they agree.
///
/// Measured relationship (dexketoprofen / ethane / saddle): the trajectory may
/// carry ONE extra trailing frame — the converged geometry's final point, which
/// has no optimization-cycle block — so `n_traj ∈ {n_opt, n_opt+1}`, and the
/// shared prefix matches to `CYCLE_ENERGY_TOL_EH`. Either sequence empty →
/// nothing to cross-check (not a plain optimization) → `Ok`. Pure, so it is
/// tested directly.
fn cycle_energy_cross_check(opt_e: &[f64], traj_e: &[f64]) -> Result<(), String> {
    if opt_e.is_empty() || traj_e.is_empty() {
        return Ok(());
    }
    if !(traj_e.len() == opt_e.len() || traj_e.len() == opt_e.len() + 1) {
        return Err(format!(
            "cycle-energy cross-check failed: {} optimization cycles in output.out but {} \
             trajectory frames carry an energy in _trj.xyz (expected equal, or +1 for the \
             converged final frame) — one source is out of step (a parser or ORCA-format drift)",
            opt_e.len(),
            traj_e.len()
        ));
    }
    let k = opt_e.len().min(traj_e.len());
    for i in 0..k {
        let d = (opt_e[i] - traj_e[i]).abs();
        if d > CYCLE_ENERGY_TOL_EH {
            return Err(format!(
                "cycle-energy cross-check failed at cycle {}: output.out has {:.8} Eh but \
                 _trj.xyz has {:.8} Eh (Δ={:.2e} > {:.0e}) — the streaming parser and the \
                 trajectory disagree",
                i + 1,
                opt_e[i],
                traj_e[i],
                d,
                CYCLE_ENERGY_TOL_EH
            ));
        }
    }
    Ok(())
}

/// Is this a GOAT conformer search? Scans `!` keyword lines for the GOAT token
/// (word-boundary, case-insensitive) — the Rust twin of the frontend's
/// `isGoatInput`. A GOAT trajectory is conformers, not one optimization's cycles,
/// so the cycle-energy cross-check does not apply to it.
fn input_is_goat(input_content: &str) -> bool {
    input_content
        .lines()
        .filter(|l| l.trim_start().starts_with('!'))
        .any(|l| {
            l.split(|c: char| !c.is_ascii_alphanumeric())
                .any(|w| w.eq_ignore_ascii_case("GOAT"))
        })
}

/// Extract the `* xyz charge mult … *` block from an ORCA input as the start
/// geometry (elements + Å coords). The reference for `.property.txt` (coords only —
/// it checks element order internally) and for `_trj.xyz` (elements + coords).
fn input_reference(input_content: &str) -> Option<ReferenceGeometry> {
    let (mut z, mut xyz) = (Vec::new(), Vec::new());
    let mut inside = false;
    for line in input_content.lines() {
        let t = line.trim();
        if t.to_lowercase().starts_with("* xyz") {
            inside = true;
            continue;
        }
        if inside {
            if t.starts_with('*') {
                break;
            }
            let toks: Vec<&str> = t.split_whitespace().collect();
            if toks.len() >= 4 {
                if let (Some(zz), Ok(x), Ok(y), Ok(zc)) = (
                    crate::parse::elements::z_of(toks[0]),
                    toks[1].parse(),
                    toks[2].parse(),
                    toks[3].parse(),
                ) {
                    z.push(zz);
                    xyz.push([x, y, zc]);
                }
            }
        }
    }
    if inside {
        // Legacy job (unit 1d): derived identity ids from the coordinate block's atom
        // count — the same 0..n the job map is built from, so the two agree on the
        // green path by construction (unit 1e replaces both with minted ids).
        let ids = derived_identity_ids(z.len());
        Some(ReferenceGeometry { z, xyz_angstrom: xyz, ids })
    } else {
        None
    }
}

/// The user-configured ORCA binary path (rule #7 — a setting, not hard-coded).
fn read_orca_path(conn: &Connection) -> Option<String> {
    conn.query_row(
        "SELECT value FROM settings WHERE key = 'orca_path'",
        [],
        |r| r.get::<_, String>(0),
    )
    .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    const OPTFREQ: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/property_optfreq_ethane.property.txt"
    ));
    const OPTFREQ_INP: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/property_optfreq_ethane.input.inp"
    ));

    fn mem_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        // results.job_id has a FK to jobs(id); give it a parent row to point at.
        conn.execute("CREATE TABLE jobs (id TEXT PRIMARY KEY)", []).unwrap();
        conn.execute("INSERT INTO jobs (id) VALUES ('job1')", []).unwrap();
        crate::db::create_results_table(&conn).unwrap();
        conn
    }

    fn verified() -> Verified {
        let reference = input_reference(OPTFREQ_INP).unwrap();
        let map = identity_map_for(&reference);
        PropertyFile::parse(OPTFREQ).verify(&reference, &map).unwrap()
    }

    // An ethane scene (8 atoms, deliberately non-trivial AtomIds 10..17) whose element
    // order matches the OPTFREQ fixture (C C H H H H H H).
    const ETHANE_SCENE: &str = r#"{"version":2,"fragments":[{"id":"f","name":"ethane","atoms":[
        {"id":10,"element":"C","x":0.0,"y":0.0,"z":0.0},{"id":11,"element":"C","x":0.0,"y":0.0,"z":1.5},
        {"id":12,"element":"H","x":0.0,"y":0.0,"z":0.0},{"id":13,"element":"H","x":0.0,"y":0.0,"z":0.0},
        {"id":14,"element":"H","x":0.0,"y":0.0,"z":0.0},{"id":15,"element":"H","x":0.0,"y":0.0,"z":0.0},
        {"id":16,"element":"H","x":0.0,"y":0.0,"z":0.0},{"id":17,"element":"H","x":0.0,"y":0.0,"z":0.0}
    ],"charge":0,"source":"editor"}],"multiplicity":1,"nextAtomId":18}"#;

    fn jobs_db_with(scene: Option<&str>, index_map_json: Option<&str>) -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute(
            "CREATE TABLE jobs (id TEXT PRIMARY KEY, scene_json TEXT, index_map_json TEXT)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO jobs (id, scene_json, index_map_json) VALUES ('job1', ?1, ?2)",
            rusqlite::params![scene, index_map_json],
        )
        .unwrap();
        conn
    }

    #[test]
    fn minted_map_is_load_bearing_permuted_stored_map_is_refused() {
        // Negative control (b): a synthetic job whose stored minted map is PERMUTED
        // (positions 0↔2, a C and an H) makes parse's verify refuse by the artifact
        // cross-check — while the CORRECT minted map verifies. So the stored map is
        // load-bearing, not decorative persistence.
        // Correct: stored order == scene emit order.
        let mut input_ref = input_reference(OPTFREQ_INP).unwrap();
        let conn = jobs_db_with(Some(ETHANE_SCENE), Some(r#"{"minted":[10,11,12,13,14,15,16,17]}"#));
        let (map, ids) = resolve_job_mapping(&conn, "job1", &input_ref).unwrap();
        input_ref.ids = ids;
        assert!(
            PropertyFile::parse(OPTFREQ).verify(&input_ref, &map).is_ok(),
            "the correct minted map verifies"
        );

        // Permuted stored order; the reference ids still come from the SCENE
        // (independent of the stored map), so the permutation cannot cancel out.
        let mut bad_ref = input_reference(OPTFREQ_INP).unwrap();
        let conn2 = jobs_db_with(Some(ETHANE_SCENE), Some(r#"{"minted":[12,11,10,13,14,15,16,17]}"#));
        let (pmap, pids) = resolve_job_mapping(&conn2, "job1", &bad_ref).unwrap();
        bad_ref.ids = pids;
        let err = PropertyFile::parse(OPTFREQ).verify(&bad_ref, &pmap).unwrap_err();
        assert!(matches!(err, crate::parse::ParseError::OrderMismatch { .. }), "{err:?}");
    }

    #[test]
    fn skipped_and_null_fall_back_to_the_derived_identity_map() {
        let reference = input_reference(OPTFREQ_INP).unwrap();
        for stored in [Some(r#"{"skipped":"no scene snapshot"}"#), None] {
            let conn = jobs_db_with(None, stored);
            let (map, ids) = resolve_job_mapping(&conn, "job1", &reference).unwrap();
            // Derived identity: ids are 0..n and the map is the identity.
            assert_eq!(ids, derived_identity_ids(reference.z.len()));
            assert_eq!(map.order(), identity_map_for(&reference).order());
        }
    }

    #[test]
    fn per_atom_charges_carry_their_element_order() {
        let r = ParsedResults::from_verified(&verified(), None, None, None, None, None).unwrap();
        let mulliken = r.charges.iter().find(|c| c.scheme == "mulliken").unwrap();
        assert_eq!(mulliken.elements, ["C", "C", "H", "H", "H", "H", "H", "H"]);
        assert_eq!(mulliken.charges.len(), mulliken.elements.len());
        // JSON keeps them together — this is what a future reader re-checks.
        let json = serde_json::to_string(&r).unwrap();
        assert!(json.contains("\"elements\""));
        assert!(json.contains("\"charges\""));
    }

    #[test]
    fn store_is_idempotent_on_job_id() {
        let conn = mem_db();
        let r = ParsedResults::from_verified(&verified(), None, None, None, None, None).unwrap();
        store(&conn, "job1", &r).unwrap();
        store(&conn, "job1", &r).unwrap(); // re-parse → update, not duplicate
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM results WHERE job_id='job1'", [], |x| x.get(0))
            .unwrap();
        assert_eq!(n, 1);
    }

    #[test]
    fn read_back_preserves_element_order() {
        let conn = mem_db();
        let r = ParsedResults::from_verified(&verified(), None, None, None, None, None).unwrap();
        store(&conn, "job1", &r).unwrap();
        verify_stored(&conn, "job1", &r).expect("round-trip preserves per-atom order");

        let back = read_job_results(&conn, "job1").unwrap().unwrap();
        let m = back.charges.iter().find(|c| c.scheme == "mulliken").unwrap();
        assert_eq!(m.elements, ["C", "C", "H", "H", "H", "H", "H", "H"]);
        assert_eq!(m.charges.len(), 8);
    }

    #[test]
    fn narrow_columns_match_the_json() {
        let conn = mem_db();
        let r = ParsedResults::from_verified(&verified(), None, None, None, None, None).unwrap();
        store(&conn, "job1", &r).unwrap();
        let (energy, ts): (f64, f64) = conn
            .query_row(
                "SELECT final_energy_eh, t_times_s_eh FROM results WHERE job_id='job1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert!((energy - r.final_energy_eh.unwrap()).abs() < 1e-12);
        assert!((ts - r.thermochemistry.as_ref().unwrap().t_times_s_eh).abs() < 1e-12);
    }

    #[test]
    fn no_property_file_is_not_a_failure() {
        let conn = mem_db();
        let outcome = parse_and_store(&conn, "job1", "/nonexistent/dir", OPTFREQ_INP);
        assert!(matches!(outcome, ParseOutcome::NoArtifact));
    }

    // --- cycle-energy cross-check (defect 2 post-condition) -----------------

    /// The REAL dexketoprofen (33 atoms, r²SCAN-3c CPCM Opt+Freq) cycle energies:
    /// 16 optimization cycles from output.out, and 17 `_trj.xyz` frames — the 16
    /// plus one trailing converged-geometry frame. Measured (unit 3.9); the two
    /// sources agree bit-for-bit.
    const DEXKET_OPT: [f64; 16] = [
        -843.687198544630, -843.689618984769, -843.689892241908, -843.690114003287,
        -843.690169925981, -843.690267894649, -843.690305293370, -843.690314085785,
        -843.690342684298, -843.690359106485, -843.690377473507, -843.690388695645,
        -843.690386205625, -843.690394448464, -843.690395537446, -843.690395213624,
    ];
    fn dexket_traj() -> Vec<f64> {
        let mut v = DEXKET_OPT.to_vec();
        v.push(-843.690395750533); // the trailing converged frame (+1)
        v
    }

    #[test]
    fn cross_check_matches_on_real_dexketoprofen() {
        // 16 opt cycles vs 17 trajectory frames (the +1 trailing frame) → agree.
        assert!(cycle_energy_cross_check(&DEXKET_OPT, &dexket_traj()).is_ok());
        // Equal counts (no trailing frame) also agree.
        assert!(cycle_energy_cross_check(&DEXKET_OPT, &DEXKET_OPT).is_ok());
    }

    #[test]
    fn cross_check_catches_a_planted_value_divergence() {
        // Perturb one trajectory energy well past the tolerance — the exact class
        // of silent drift (a broken regex, a shifted format) this guards against.
        let mut traj = dexket_traj();
        traj[5] += 0.01;
        let err = cycle_energy_cross_check(&DEXKET_OPT, &traj).unwrap_err();
        assert!(err.contains("cycle 6"), "should name the divergent cycle: {err}");
    }

    #[test]
    fn cross_check_catches_a_count_mismatch() {
        // A trajectory with far more frames than opt cycles (not the +1 case) → err.
        let mut traj = dexket_traj();
        traj.extend([-843.7, -843.7, -843.7]);
        assert!(cycle_energy_cross_check(&DEXKET_OPT, &traj).is_err());
        // Fewer trajectory frames than opt cycles → also a mismatch.
        assert!(cycle_energy_cross_check(&DEXKET_OPT, &DEXKET_OPT[..10]).is_err());
    }

    #[test]
    fn cross_check_skips_when_either_source_is_empty() {
        // SP: no opt cycles; GOAT-skipped: nothing to compare → Ok, never a failure.
        assert!(cycle_energy_cross_check(&[], &dexket_traj()).is_ok());
        assert!(cycle_energy_cross_check(&DEXKET_OPT, &[]).is_ok());
    }

    #[test]
    fn input_is_goat_detects_the_keyword() {
        assert!(input_is_goat("! XTB GOAT\n* xyz 0 1\n*\n"));
        assert!(input_is_goat("! goat tightscf\n")); // case-insensitive
        // A plain optimization is NOT GOAT (the cross-check must run for it).
        assert!(!input_is_goat("! r2SCAN-3c CPCM(ethanol) Opt Freq TightSCF\n"));
        // "GOAT" only as a whole keyword token, not inside another word.
        assert!(!input_is_goat("! SCAPEGOATING\n"));
    }

    /// Defect 2 (unit 3.9) on the REAL 33-atom dexketoprofen job: parse the whole
    /// pipeline and assert (a) the cross-check PASSES on real data (outcome
    /// `Parsed`, so the .out cycle energies and the _trj.xyz frame energies agree),
    /// and (b) `stored_final_energy` — the AUTHORITATIVE value the header/list now
    /// shows — is the real final energy, the one the 64 KB output tail regex could
    /// not reach (164 KB back). Ignored: reads the real ~1.4 MB job dir + spawns
    /// orca_2json.
    #[test]
    #[ignore = "reads the real dexketoprofen job dir from ~/.local/share and runs orca_2json"]
    fn real_dexketoprofen_header_energy_from_results_and_cross_check_passes() {
        let src = format!(
            "{}/.local/share/orcastudio/jobs/b0d1db94-8012-47aa-9d2a-bb5924abca13",
            std::env::var("HOME").unwrap()
        );
        if !Path::new(&src).join("input.property.txt").exists() {
            eprintln!("skipping: real dexketoprofen job dir not present");
            return;
        }
        let tmp = std::env::temp_dir().join(format!("dexket-e2e-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        for f in ["input.property.txt", "input.inp", "input.hess", "input_trj.xyz", "input.gbw", "output.out"] {
            std::fs::copy(Path::new(&src).join(f), tmp.join(f)).unwrap();
        }
        let input = std::fs::read_to_string(tmp.join("input.inp")).unwrap();

        let conn = Connection::open_in_memory().unwrap();
        conn.execute("CREATE TABLE jobs (id TEXT PRIMARY KEY)", []).unwrap();
        conn.execute("INSERT INTO jobs (id) VALUES ('dexket')", []).unwrap();
        crate::db::create_results_table(&conn).unwrap();
        conn.execute_batch(
            "CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);
             INSERT INTO settings (key,value) VALUES ('orca_path','/opt/orca/orca');",
        )
        .unwrap();

        // The cross-check runs here (33-atom Opt, not GOAT) and must PASS.
        let outcome = parse_and_store(&conn, "dexket", tmp.to_str().unwrap(), &input);
        assert!(matches!(outcome, ParseOutcome::Parsed), "{outcome:?}");

        // The authoritative header energy — the value the tail regex missed.
        let e = stored_final_energy(&conn, "dexket").expect("authoritative energy");
        assert!((e - (-843.690395750533)).abs() < 1e-5, "got {e}");
        eprintln!("dexketoprofen authoritative final energy = {e} Eh (tail regex missed it by 164 KB)");
        std::fs::remove_dir_all(&tmp).ok();
    }

    /// End-to-end on a COPY of a REAL Opt+Freq job dir: all four readers
    /// (property → hess → trajectory → orca_2json) + store + read-back, on real
    /// files. Copied to a temp dir so orca_2json's `input.json` never pollutes the
    /// user's data. Opt-in like the other real-data tests (needs ORCA at /opt/orca).
    #[test]
    #[ignore = "reads a real job dir from ~/.local/share and runs orca_2json"]
    fn real_optfreq_full_pipeline_end_to_end() {
        let src = format!(
            "{}/.local/share/orcastudio/jobs/d7992449-10e3-47c9-9a16-8e22d60b955d",
            std::env::var("HOME").unwrap()
        );
        if !Path::new(&src).join("input.property.txt").exists() {
            eprintln!("skipping: real job dir not present");
            return;
        }
        // copy the artifacts we read into a scratch dir.
        let tmp = std::env::temp_dir().join(format!("results-e2e-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        for f in ["input.property.txt", "input.inp", "input.hess", "input_trj.xyz", "input.gbw"] {
            std::fs::copy(Path::new(&src).join(f), tmp.join(f)).unwrap();
        }
        let input = std::fs::read_to_string(tmp.join("input.inp")).unwrap();

        let conn = Connection::open_in_memory().unwrap();
        conn.execute("CREATE TABLE jobs (id TEXT PRIMARY KEY)", []).unwrap();
        conn.execute("INSERT INTO jobs (id) VALUES ('real1')", []).unwrap();
        crate::db::create_results_table(&conn).unwrap();
        // seed the ORCA path so the orca_2json path runs (rule #7: from settings).
        conn.execute_batch(
            "CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);
             INSERT INTO settings (key,value) VALUES ('orca_path','/opt/orca/orca');",
        )
        .unwrap();

        let outcome = parse_and_store(&conn, "real1", tmp.to_str().unwrap(), &input);
        assert!(matches!(outcome, ParseOutcome::Parsed), "{outcome:?}");

        let r = read_job_results(&conn, "real1").unwrap().unwrap();
        assert!((r.final_energy_eh.unwrap() - (-79.791851376071)).abs() < 1e-6);
        assert_eq!(r.charges.iter().find(|c| c.scheme == "mulliken").unwrap().charges.len(), 8);
        // .hess: 24 freqs, a minimum.
        let f = r.frequencies.as_ref().expect(".hess frequencies");
        assert_eq!((f.frequencies_cm.len(), f.imaginary_count), (24, 0));
        // _trj.xyz: 5 frames, first carries a comment energy.
        let trj = r.trajectory.as_ref().expect("_trj.xyz trajectory");
        assert_eq!(trj.n_frames, 5);
        assert!(trj.frames[0].energy_eh.is_some());
        // orca_2json: 68 MOs, a HOMO/LUMO gap; NO coefficients in the JSON.
        let orb = r.orbitals.as_ref().expect("orca_2json orbitals");
        assert_eq!(orb.orbitals.len(), 68);
        let gap = orb.homo_lumo.as_ref().expect("HOMO/LUMO").gap_eh;
        assert!(gap > 0.0);
        let stored_json = serde_json::to_string(&r).unwrap();
        assert!(!stored_json.contains("MOCoefficients"), "coefficients must never be stored");
        eprintln!(
            "real Opt+Freq full pipeline: E={:?} Eh, {} freqs, {} frames, gap={:.4} Eh",
            r.final_energy_eh, f.frequencies_cm.len(), trj.n_frames, gap
        );
        std::fs::remove_dir_all(&tmp).ok();
    }

    // ── read_scan_geometries (Phase 4.5 B2) — the new command's point loader ──────

    fn scan_fixture_dir() -> std::path::PathBuf {
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/scan-ethane-cc")
    }

    fn neb_fixture_dir() -> std::path::PathBuf {
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/neb")
    }

    fn ts_pair_distance(g: &FinalGeometry, i: usize, j: usize) -> f64 {
        let a = g.xyz_angstrom[i];
        let b = g.xyz_angstrom[j];
        ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
    }

    #[test]
    fn input_has_neb_detects_the_keyword() {
        assert!(input_has_neb("! r2SCAN-3c NEB-TS SMD(dmf) TightSCF\n* xyz 0 1\n*\n"));
        assert!(input_has_neb("! b3lyp neb-ci\n")); // case-insensitive, any NEB variant
        // A plain optimization / a scan is NOT NEB (the standard path must run for it).
        assert!(!input_has_neb("! r2SCAN-3c Opt Freq TightSCF\n"));
        assert!(!input_has_neb("! r2SCAN-3c LooseOpt\n%geom Scan B 0 1 = 3.0, 1.8, 12 end end\n"));
    }

    #[test]
    fn neb_job_parses_via_the_band_route_not_the_reactant_reference() {
        // The FULL pipeline on the real NEB probe fixtures: a NEB job now PARSES (the E3a-1
        // completion). Before the route it failed at PropertyFile::verify(&input_ref) with a
        // ~2.45 Å GeometryMismatch (reactant ≠ TS) — the bite below proves that pre-state.
        let dir = neb_fixture_dir();
        let input = std::fs::read_to_string(dir.join("input.inp")).unwrap();
        let conn = mem_db();

        let outcome = parse_and_store(&conn, "job1", dir.to_str().unwrap(), &input);
        assert!(matches!(outcome, ParseOutcome::Parsed), "{outcome:?}");

        let r = read_job_results(&conn, "job1").unwrap().unwrap();
        // The band is the authoritative result: 24 iterations.
        let neb = r.neb.as_ref().expect("NEB band stored");
        assert_eq!(neb.iterations.len(), 24);
        assert_eq!(neb.iterations.last().unwrap().climbing_image, Some(5));
        // final_geometry = the converged TS (N(1)···C(8) ≈ 2.35, the known saddle), NOT the
        // reactant; final_energy = the converged-TS comment energy.
        assert!((ts_pair_distance(&r.final_geometry, 1, 8) - 2.353).abs() < 0.01);
        assert!((r.final_energy_eh.unwrap() - (-472.754853216551)).abs() < 1e-6, "{:?}", r.final_energy_eh);
        // No single-structure quantities mis-attributed from a multi-geometry NEB job.
        assert!(r.charges.is_empty() && r.thermochemistry.is_none());
        assert!(r.frequencies.is_none() && r.trajectory.is_none() && r.scan.is_none());
    }

    #[test]
    fn the_reactant_reference_would_fail_on_a_neb_property_file() {
        // BITE: the OLD single-geometry path — PropertyFile::verify against the input
        // reactant — fails on a NEB job (its property.txt band ≠ the reactant), which is
        // exactly why the NEB route skips it. A large, r≈1 GeometryMismatch (not a unit error).
        let dir = neb_fixture_dir();
        let input = std::fs::read_to_string(dir.join("input.inp")).unwrap();
        let reactant_ref = input_reference(&input).unwrap();
        let map = identity_map_for(&reactant_ref);
        let err = PropertyFile::from_path(&dir.join("input.property.txt"))
            .and_then(|pf| pf.verify(&reactant_ref, &map))
            .unwrap_err();
        assert!(
            matches!(err, crate::parse::ParseError::GeometryMismatch { max_delta } if max_delta > 1.0),
            "expected a large GeometryMismatch, got {err:?}"
        );
    }

    /// Only `data_json` is read by `read_job_results`, so a minimal `results` row is
    /// enough to drive `read_scan_geometries` against the real point `.xyz` fixtures.
    fn results_db_with(r: &ParsedResults) -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute("CREATE TABLE results (job_id TEXT PRIMARY KEY, data_json TEXT)", [])
            .unwrap();
        conn.execute(
            "INSERT INTO results (job_id, data_json) VALUES ('job1', ?1)",
            params![serde_json::to_string(r).unwrap()],
        )
        .unwrap();
        conn
    }

    fn scan_results(n_points: usize) -> ParsedResults {
        let points = (0..n_points)
            .map(|k| ScanPointJson {
                coordinate: 1.4 + 0.2 * k as f64,
                energy_act_eh: -79.0 - k as f64,
                energy_scf_eh: -79.0 - k as f64 - 0.003,
            })
            .collect();
        let scan = ScanProfileJson {
            kind: "B".into(),
            atoms: vec![0, 1],
            coordinate_unit: "Å".into(),
            points,
        };
        ParsedResults::from_verified(&verified(), None, None, None, Some(scan), None).unwrap()
    }

    #[test]
    fn read_scan_geometries_loads_each_point_file_in_order() {
        let conn = results_db_with(&scan_results(6));
        let dir = scan_fixture_dir();
        let geoms = read_scan_geometries(&conn, "job1", dir.to_str())
            .unwrap()
            .expect("a scan job returns Some(geometries)");
        // one geometry per scan point, in point order.
        assert_eq!(geoms.len(), 6);
        // each carries the ethane element order (its own, UI-checked at the boundary).
        assert_eq!(geoms[0].elements, ["C", "C", "H", "H", "H", "H", "H", "H"]);
        // the scanned C–C distance rises 1.4 → 2.4 Å across the points (the reader read
        // the RIGHT file for each index).
        let cc = |g: &ScanGeometry| {
            let a = g.xyz_angstrom[0];
            let b = g.xyz_angstrom[1];
            ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
        };
        assert!((cc(&geoms[0]) - 1.4).abs() < 1e-3);
        assert!((cc(&geoms[5]) - 2.4).abs() < 1e-3);
    }

    #[test]
    fn read_scan_geometries_is_none_for_a_non_scan_job() {
        // A results row with no scan (scan: None) → Ok(None), absent-is-normal.
        let non_scan =
            ParsedResults::from_verified(&verified(), None, None, None, None, None).unwrap();
        let conn = results_db_with(&non_scan);
        assert!(read_scan_geometries(&conn, "job1", scan_fixture_dir().to_str())
            .unwrap()
            .is_none());
        // …and no dir → None even for a scan job (nothing to read).
        let conn2 = results_db_with(&scan_results(3));
        assert!(read_scan_geometries(&conn2, "job1", None).unwrap().is_none());
    }

    // ── Phase 4.5 B1 fix: scan jobs parse profile-only ───────────────────────────
    // The B2 manual gate caught a real bug: a completed relaxed scan failed the full
    // `parse_and_store`. The single-structure `property.rs` post-condition compared the
    // first `$Geometry` (scan point 1, C–C ≈ 1.400 Å — constrained) against the input
    // (C–C ≈ 1.51–1.53 Å) and failed with `GeometryMismatch` — its premise "first
    // structure == input" is structurally false for a multi-point scan. The fix routes a
    // scan to the profile (B1) and skips the single-structure readers. Root cause + the
    // measured `.property.txt` structure: wiki/debugging/015-scan-property-post-condition.md.

    /// C-scan-full-parse — the full pipeline on the REAL scan fixture dir. **RED before
    /// the fix** (the 0.056-class `GeometryMismatch`), **GREEN after** (profile parsed,
    /// `Parsed`, no error). Closes the test gap: B1 tested `relaxscan` in isolation, never
    /// the full `parse_and_store` on a scan dir. Runs on real artifacts.
    #[test]
    fn scan_job_parses_profile_only_full_pipeline() {
        let dir = scan_fixture_dir();
        let input = std::fs::read_to_string(dir.join("input.inp")).unwrap();
        let conn = mem_db();

        let outcome = parse_and_store(&conn, "job1", dir.to_str().unwrap(), &input);
        assert!(matches!(outcome, ParseOutcome::Parsed), "{outcome:?}");

        let r = read_job_results(&conn, "job1").unwrap().unwrap();
        // The profile IS the stored authoritative result: 6 points, coordinate in Å.
        let scan = r.scan.as_ref().expect("scan profile stored");
        assert_eq!(scan.points.len(), 6);
        assert_eq!(scan.coordinate_unit, "Å");
        // Header/summary energy sources from the profile's LAST point (act), not property.
        let last_act = scan.points.last().unwrap().energy_act_eh;
        assert!((r.final_energy_eh.unwrap() - last_act).abs() < 1e-12);
        assert!((r.final_energy_eh.unwrap() - (-79.69075938)).abs() < 1e-6, "{:?}", r.final_energy_eh);
        // Final geometry = the last scan point (C–C ≈ 2.4 Å); element order for the panel.
        assert_eq!(r.final_geometry.elements, ["C", "C", "H", "H", "H", "H", "H", "H"]);
        let a = r.final_geometry.xyz_angstrom[0];
        let b = r.final_geometry.xyz_angstrom[1];
        let cc = ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt();
        assert!((cc - 2.4).abs() < 1e-3, "final geometry is the last scan point: {cc}");
        // No single-structure quantities mis-attributed from a multi-point artifact.
        assert!(r.charges.is_empty(), "no per-point charges collapsed into one");
        assert!(r.thermochemistry.is_none() && r.trajectory.is_none() && r.orbitals.is_none());
    }

    /// The RED the fix routes AROUND, demonstrated as a still-biting guard: run the
    /// SINGLE-STRUCTURE `property.rs` post-condition on the scan `.property.txt` against
    /// the scan's own input and confirm it fails loudly. This is *why* a scan is routed
    /// away from it — and proof the tolerance was NOT loosened (the guard still bites
    /// where its premise is asserted). The delta is compression-scale (a real geometry
    /// difference), far below the ≈1.889× a missed Bohr→Å conversion would produce.
    #[test]
    fn single_structure_property_check_bites_on_a_scan_artifact() {
        let dir = scan_fixture_dir();
        let input = std::fs::read_to_string(dir.join("input.inp")).unwrap();
        let mut input_ref = input_reference(&input).unwrap();
        let map = identity_map_for(&input_ref);
        input_ref.ids = derived_identity_ids(input_ref.z.len());
        let err = PropertyFile::from_path(&dir.join("input.property.txt"))
            .unwrap()
            .verify(&input_ref, &map)
            .unwrap_err();
        match err {
            crate::parse::ParseError::GeometryMismatch { max_delta } => {
                // C–C compression of scan point 1, not a Bohr blowup (which would be ~1 Å).
                assert!(max_delta > 1e-4, "the guard fires: {max_delta}");
                assert!(max_delta < 0.5, "compression-scale, not a 1.889× Bohr miss: {max_delta}");
            }
            other => panic!("expected GeometryMismatch, got {other:?}"),
        }
    }

    /// C-nonscan-unaffected (routing) — a non-scan dir (no `.relaxscanact.dat`) still
    /// goes through the single-structure `property.rs` reader: the scan branch did not
    /// leak into the Opt/SP/Freq path. (The Bohr guard's *biting* on the non-scan path is
    /// covered where its premise holds — `property::tests::missed_bohr_conversion_fails_loudly`.)
    #[test]
    fn non_scan_dir_still_runs_the_single_structure_readers() {
        let tmp = std::env::temp_dir().join(format!("nonscan-route-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        std::fs::write(tmp.join("input.property.txt"), OPTFREQ).unwrap();
        std::fs::write(tmp.join("input.inp"), OPTFREQ_INP).unwrap();
        // No input.relaxscanact.dat → must NOT take the scan branch. The non-scan path
        // resolves the job's IndexMap, so the jobs table needs the full schema.
        let conn = jobs_db_with(None, None);
        crate::db::create_results_table(&conn).unwrap();
        let outcome = parse_and_store(&conn, "job1", tmp.to_str().unwrap(), OPTFREQ_INP);
        assert!(matches!(outcome, ParseOutcome::Parsed), "{outcome:?}");
        let r = read_job_results(&conn, "job1").unwrap().unwrap();
        assert!(r.scan.is_none(), "a non-scan job has no profile");
        // property.rs actually ran: it produced the per-atom charges a scan does not.
        assert!(!r.charges.is_empty(), "the single-structure property reader ran");
        std::fs::remove_dir_all(&tmp).ok();
    }

    /// C-goat-not-parsed (routing) — a GOAT job dir with a PRESENT `input.property.txt`
    /// (and no scan `.dat`) is routed AWAY from the single-structure readers:
    /// `parse_and_store` returns `NoArtifact`, so the caller leaves the job `completed`
    /// and no results row is stored (its authoritative result is the ensemble, read by
    /// `read_job_ensemble`). The bite: the GOAT input here scans the SAME ethane geometry
    /// as the fixture, so WITHOUT the GOAT branch the single-structure readers parse
    /// `OPTFREQ` and return `Parsed` — the job reaches `parsed` and the ensemble panel's
    /// terminal-status guard hid it (the regression). See debugging/017.
    #[test]
    fn goat_dir_is_routed_past_the_single_structure_readers() {
        let tmp = std::env::temp_dir().join(format!("goat-route-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        std::fs::write(tmp.join("input.property.txt"), OPTFREQ).unwrap();
        // A GOAT input over the same ethane geometry as the fixture (body after the ! line).
        let body = OPTFREQ_INP.splitn(2, '\n').nth(1).unwrap();
        let goat_inp = format!("! XTB GOAT\n{body}");
        std::fs::write(tmp.join("input.inp"), &goat_inp).unwrap();
        // Full jobs schema + results table: WITHOUT the GOAT branch this path would derive
        // an identity map, verify OPTFREQ, and return Parsed. No input.relaxscanact.dat, so
        // the scan branch is not taken — the GOAT branch must catch it first.
        let conn = jobs_db_with(None, None);
        crate::db::create_results_table(&conn).unwrap();
        let outcome = parse_and_store(&conn, "job1", tmp.to_str().unwrap(), &goat_inp);
        assert!(
            matches!(outcome, ParseOutcome::NoArtifact),
            "GOAT is routed past the single-structure parse (stays completed): {outcome:?}"
        );
        assert!(
            read_job_results(&conn, "job1").unwrap().is_none(),
            "no single-structure results row is stored for a GOAT job"
        );
        std::fs::remove_dir_all(&tmp).ok();
    }
}
