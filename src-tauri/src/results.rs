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

use std::path::Path;

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

use crate::error::AppError;
use crate::parse::elements::symbol_of;
use crate::parse::hess::HessFile;
use crate::parse::property::{PopulationScheme, PropertyFile, Verified};
use crate::parse::xyz::XyzFile;
use crate::parse::ReferenceGeometry;

/// Bump when the stored JSON shape or the parse semantics change.
/// - v1: property.txt only (unit 3.5).
/// - v2: + `.hess` frequencies / IR / normal modes + thermo temperature (unit 3.6).
/// - v3: + `_trj.xyz` trajectory + `orca_2json` MO energies/occupancies (unit 3.7).
pub const PARSER_VERSION: u32 = 3;

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
    /// Blocks ORCA emitted that this reader has no accessor for (rule #10).
    pub unknown_blocks: Vec<String>,
}

impl ParsedResults {
    fn from_verified(
        v: &Verified,
        hess: Option<&crate::parse::hess::Verified>,
        trajectory: Option<TrajectoryJson>,
        orbitals: Option<OrbitalsJson>,
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
                    temperature_k: h.actual_temperature(),
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
            unknown_blocks: v.unknown_block_names(),
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
    /// No `.property.txt` in the job dir — nothing to parse (not a failure).
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
    // The input's start geometry (with elements) — the reference for `.property.txt`
    // (coords only; it checks element order internally) and for `_trj.xyz` (whose
    // first frame is the start).
    let input_ref = match input_reference(input_content) {
        Some(r) if !r.z.is_empty() => r,
        _ => return ParseOutcome::ParseFailed("no * xyz * block in the job input".into()),
    };

    let verified = match PropertyFile::from_path(&path).and_then(|pf| pf.verify(&input_ref.xyz_angstrom))
    {
        Ok(v) => v,
        Err(e) => return ParseOutcome::ParseFailed(e.to_string()),
    };

    // `.hess` is optional: SP/GOAT have none (a normal state). When present, verify
    // it against the OPTIMIZED geometry (the Freq geometry) — the `.property.txt`
    // final `$Geometry`, which we already have — not `input.inp` (the start).
    let hess_path = dir.join("input.hess");
    let hess_verified = if hess_path.exists() {
        match final_geometry_reference(&verified)
            .and_then(|r| Ok(HessFile::from_path(&hess_path)?.verify(&r)?))
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
        match XyzFile::from_path(&trj_path).and_then(|x| x.verify(&input_ref)) {
            Ok(xv) => Some(trajectory_json(&xv)),
            Err(e) => return ParseOutcome::ParseFailed(format!("_trj.xyz: {e}")),
        }
    } else {
        None
    };

    // MO energies/occupancies (`orca_2json` over `.gbw`): generated lazily via the
    // user-configured ORCA path (ADR-009); absent for xTB/GOAT gbw (normal). The
    // reference is the optimized (final) geometry — orca_2json's coords are final.
    let orbitals = match read_orca_path(conn) {
        None => None,
        Some(orca_path) => match crate::orca_json::ensure_gbw_json(&orca_path, dir) {
            Err(e) => return ParseOutcome::ParseFailed(format!("orca_2json: {e}")),
            Ok(None) => None,
            Ok(Some(json)) => match final_geometry_reference(&verified)
                .and_then(|r| Ok(crate::parse::mo::MoJson::from_path(&json)?.verify(&r)?))
            {
                Ok(mv) => Some(orbitals_json(&mv)),
                Err(e) => return ParseOutcome::ParseFailed(format!("orca_2json: {e}")),
            },
        },
    };

    let results =
        match ParsedResults::from_verified(&verified, hess_verified.as_ref(), trajectory, orbitals) {
            Ok(r) => r,
            Err(e) => return ParseOutcome::ParseFailed(e.to_string()),
        };
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

/// The `.property.txt` optimized (last) geometry as the reference for the `.hess`
/// geometry post-condition — the Freq geometry, not the input geometry.
fn final_geometry_reference(v: &Verified) -> Result<ReferenceGeometry, AppError> {
    let geoms = v.geometries()?;
    let last = geoms
        .last()
        .ok_or_else(|| AppError::Internal("verified property.txt has no geometry".into()))?;
    Ok(ReferenceGeometry {
        z: last.atoms.iter().map(|a| a.z).collect(),
        xyz_angstrom: last
            .atoms
            .iter()
            .map(|a| [a.xyz[0].angstrom(), a.xyz[1].angstrom(), a.xyz[2].angstrom()])
            .collect(),
    })
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
        Some(ReferenceGeometry { z, xyz_angstrom: xyz })
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
        PropertyFile::parse(OPTFREQ).verify(&reference.xyz_angstrom).unwrap()
    }

    #[test]
    fn per_atom_charges_carry_their_element_order() {
        let r = ParsedResults::from_verified(&verified(), None, None, None).unwrap();
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
        let r = ParsedResults::from_verified(&verified(), None, None, None).unwrap();
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
        let r = ParsedResults::from_verified(&verified(), None, None, None).unwrap();
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
        let r = ParsedResults::from_verified(&verified(), None, None, None).unwrap();
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
}
