//! `get_results` (ADR-027 decision 7; M2 amendment, item 8): a bounded summary of a job's parsed
//! results, with no per-atom array.
//!
//! **Units (rule #11).** `read_job_results` already holds canonical units, converted once at the
//! parser boundary (Eh, Å, cm⁻¹, K). This projection keeps them and names each in its field:
//! `_eh`, `_ang`, `_deg`, `_cm1`, `_k`. A scan's coordinate is `coordinate_ang` for a distance scan
//! and `coordinate_deg` for an angle or dihedral scan, chosen by the stored `coordinate_unit`
//! literal (`"Å"` or `"°"`); any other unit gives no coordinate and a warning, never a bare number.
//!
//! **Bounds.** At most [`IMAGINARY_MAX`] imaginary frequencies (the most negative first),
//! [`SCAN_POINTS_MAX`] scan points, [`MEP_POINTS_MAX`] MEP points and [`WARNINGS_MAX`] parse warnings
//! of [`WARNING_MAX_BYTES`] each. Each list states its full count beside it.

use rusqlite::Connection;
use serde::Serialize;

use super::{clip, within_cap, ToolError, RESULT_CAP_BYTES};
use crate::commands::jobs::get_job_conn;
use crate::models::job::JobStatus;
use crate::results::{read_job_results, ParsedResults, ThermoJson};

pub const IMAGINARY_MAX: usize = 20;
pub const SCAN_POINTS_MAX: usize = 200;
pub const MEP_POINTS_MAX: usize = 100;
pub const WARNINGS_MAX: usize = 10;
pub const WARNING_MAX_BYTES: usize = 300;

/// Vibrational frequencies, summarised.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FrequencySummary {
    /// The parser's negative-frequency count (0 = a minimum, 1 = a transition state).
    pub imaginary_count: usize,
    /// The imaginary (negative) frequencies, cm⁻¹, the most negative first, at most [`IMAGINARY_MAX`].
    pub imaginary_freqs_cm1: Vec<f64>,
    /// IR intensity of each frequency above, km/mol, in the same order; `None` where the file has none.
    pub imaginary_ir_km_mol: Vec<Option<f64>>,
    pub imaginary_truncated: bool,
    /// 3N, the number of modes.
    pub mode_count: usize,
}

/// One relaxed-scan point. Exactly one of the two coordinates is set (see the module docs).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ScanPointView {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub coordinate_ang: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub coordinate_deg: Option<f64>,
    /// The composite energy (gCP + D4 included), Eh.
    pub energy_act_eh: f64,
    /// The bare SCF energy, Eh.
    pub energy_scf_eh: f64,
}

/// A relaxed-scan profile, bounded.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ScanSummary {
    /// `B` (distance), `A` (angle) or `D` (dihedral).
    pub kind: String,
    /// The scanned atoms, 0-based.
    pub atom_indices: Vec<u32>,
    pub point_count: usize,
    pub points: Vec<ScanPointView>,
    pub points_truncated: bool,
}

/// One point on the converged minimum-energy path.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MepPointView {
    /// Arc length along the path, Å.
    pub distance_ang: f64,
    /// Energy relative to the first image (image 0 = 0), Eh.
    pub relative_energy_eh: f64,
}

/// A NEB run, summarised.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct NebSummary {
    pub iteration_count: usize,
    /// The final iteration's barrier, Eh.
    pub final_barrier_eh: Option<f64>,
    /// The converged TS energy, Eh.
    pub ts_energy_eh: Option<f64>,
    /// The climbing image of the final iteration, 0-based.
    pub climbing_image_index: Option<usize>,
    pub mep_point_count: usize,
    pub mep: Vec<MepPointView>,
    pub mep_truncated: bool,
}

/// `get_results`' answer.
#[derive(Debug, Clone, Serialize)]
pub struct ResultsSummary {
    pub job_id: String,
    pub status: JobStatus,
    /// Whether the job has stored parsed results. When false, every field below is empty and
    /// `not_parsed_reason` says why.
    pub parsed: bool,
    pub not_parsed_reason: Option<String>,
    pub final_energy_eh: Option<f64>,
    /// The optimization verdict: `None` when the job is no optimization.
    pub converged: Option<bool>,
    pub atom_count: usize,
    pub frequencies: Option<FrequencySummary>,
    /// Thermochemistry at `temperature_k`; every energy Eh. `t_times_s_eh` is T·S, not S.
    pub thermochemistry: Option<ThermoJson>,
    pub scan: Option<ScanSummary>,
    pub neb: Option<NebSummary>,
    /// Auxiliary properties that failed to parse, and what this summary could not show.
    pub warnings: Vec<String>,
    pub warning_count: usize,
}

fn frequencies_of(r: &ParsedResults) -> Option<FrequencySummary> {
    let f = r.frequencies.as_ref()?;
    let mut imaginary: Vec<(f64, Option<f64>)> = f
        .frequencies_cm
        .iter()
        .enumerate()
        .filter(|(_, v)| **v < 0.0)
        .map(|(i, v)| (*v, f.ir_intensity_km_mol.get(i).copied()))
        .collect();
    imaginary.sort_by(|a, b| a.0.total_cmp(&b.0));
    let truncated = imaginary.len() > IMAGINARY_MAX;
    imaginary.truncate(IMAGINARY_MAX);
    Some(FrequencySummary {
        imaginary_count: f.imaginary_count,
        imaginary_freqs_cm1: imaginary.iter().map(|p| p.0).collect(),
        imaginary_ir_km_mol: imaginary.iter().map(|p| p.1).collect(),
        imaginary_truncated: truncated,
        mode_count: f.n_modes,
    })
}

fn scan_of(r: &ParsedResults, warnings: &mut Vec<String>) -> Option<ScanSummary> {
    let s = r.scan.as_ref()?;
    let (ang, deg) = match s.coordinate_unit.as_str() {
        "Å" => (true, false),
        "°" => (false, true),
        other => {
            warnings.push(format!("the scan coordinate's stored unit `{other}` is not Å or °; the coordinate is not shown"));
            (false, false)
        }
    };
    let points: Vec<ScanPointView> = s
        .points
        .iter()
        .take(SCAN_POINTS_MAX)
        .map(|p| ScanPointView {
            coordinate_ang: ang.then_some(p.coordinate),
            coordinate_deg: deg.then_some(p.coordinate),
            energy_act_eh: p.energy_act_eh,
            energy_scf_eh: p.energy_scf_eh,
        })
        .collect();
    Some(ScanSummary {
        kind: clip(&s.kind, 8).0,
        atom_indices: s.atoms.clone(),
        point_count: s.points.len(),
        points_truncated: s.points.len() > points.len(),
        points,
    })
}

fn neb_of(r: &ParsedResults) -> Option<NebSummary> {
    let n = r.neb.as_ref()?;
    let mep: Vec<MepPointView> = n
        .mep
        .iter()
        .take(MEP_POINTS_MAX)
        .map(|p| MepPointView { distance_ang: p.distance_angstrom, relative_energy_eh: p.energy_eh })
        .collect();
    Some(NebSummary {
        iteration_count: n.iterations.len(),
        final_barrier_eh: n.final_barrier_eh,
        ts_energy_eh: n.ts_energy_eh,
        climbing_image_index: n.iterations.last().and_then(|it| it.climbing_image),
        mep_point_count: n.mep.len(),
        mep_truncated: n.mep.len() > mep.len(),
        mep,
    })
}

/// `get_results`: the bounded summary of job `id`'s parsed results.
pub fn get_results(conn: &Connection, id: &str) -> Result<ResultsSummary, ToolError> {
    let job = get_job_conn(conn, id).map_err(ToolError::from_app)?;
    let results = read_job_results(conn, id).map_err(ToolError::App)?;
    let Some(r) = results else {
        let summary = ResultsSummary {
            job_id: job.id,
            status: job.status,
            parsed: false,
            not_parsed_reason: Some(clip(&format!("job {id} has no parsed results (status {})", job.status.as_str()), WARNING_MAX_BYTES).0),
            final_energy_eh: None,
            converged: None,
            atom_count: 0,
            frequencies: None,
            thermochemistry: None,
            scan: None,
            neb: None,
            warnings: Vec::new(),
            warning_count: 0,
        };
        return within_cap(summary, RESULT_CAP_BYTES);
    };

    let mut warnings: Vec<String> = r.parse_warnings.clone();
    let frequencies = frequencies_of(&r);
    if let Some(f) = &frequencies {
        let negative = r.frequencies.as_ref().map_or(0, |all| all.frequencies_cm.iter().filter(|v| **v < 0.0).count());
        if negative != f.imaginary_count {
            warnings.push(format!(
                "the parser counted {} imaginary modes, but {negative} frequencies are negative",
                f.imaginary_count
            ));
        }
    }
    let scan = scan_of(&r, &mut warnings);
    let warning_count = warnings.len();
    let summary = ResultsSummary {
        job_id: job.id,
        status: job.status,
        parsed: true,
        not_parsed_reason: None,
        final_energy_eh: r.final_energy_eh,
        converged: r.converged,
        atom_count: r.final_geometry.elements.len(),
        frequencies,
        thermochemistry: r.thermochemistry.clone(),
        scan,
        neb: neb_of(&r),
        warnings: warnings.iter().take(WARNINGS_MAX).map(|w| clip(w, WARNING_MAX_BYTES).0).collect(),
        warning_count,
    };
    within_cap(summary, RESULT_CAP_BYTES)
}
