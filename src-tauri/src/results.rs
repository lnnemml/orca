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
use crate::parse::property::{PopulationScheme, PropertyFile, Verified};

/// Bump when the stored JSON shape or the parse semantics change.
pub const PARSER_VERSION: u32 = 1;

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
    pub el_energy_eh: f64,
    pub zpe_eh: f64,
    pub inner_energy_u_eh: f64,
    pub enthalpy_h_eh: f64,
    /// T·S in Eh, NOT entropy S (measured: == enthalpy_h_eh − free_energy_g_eh).
    pub t_times_s_eh: f64,
    pub free_energy_g_eh: f64,
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
    /// Blocks ORCA emitted that this reader has no accessor for (rule #10).
    pub unknown_blocks: Vec<String>,
}

impl ParsedResults {
    fn from_verified(v: &Verified) -> Result<ParsedResults, AppError> {
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
            el_energy_eh: t.el_energy_eh,
            zpe_eh: t.zpe_eh,
            inner_energy_u_eh: t.inner_energy_u_eh,
            enthalpy_h_eh: t.enthalpy_h_eh,
            t_times_s_eh: t.t_times_s_eh,
            free_energy_g_eh: t.free_energy_g_eh,
        });

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
            unknown_blocks: v.unknown_block_names(),
        })
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
    let path = Path::new(job_dir).join("input.property.txt");
    if !path.exists() {
        return ParseOutcome::NoArtifact;
    }
    let reference = match xyz_reference(input_content) {
        Some(r) if !r.is_empty() => r,
        _ => return ParseOutcome::ParseFailed("no * xyz * block in the job input".into()),
    };

    let verified = match PropertyFile::from_path(&path).and_then(|pf| pf.verify(&reference)) {
        Ok(v) => v,
        Err(e) => return ParseOutcome::ParseFailed(e.to_string()),
    };
    let results = match ParsedResults::from_verified(&verified) {
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

/// Idempotent upsert keyed by `job_id`: re-parsing the same job updates the row,
/// never duplicates it.
fn store(conn: &Connection, job_id: &str, r: &ParsedResults) -> Result<(), AppError> {
    let data_json = serde_json::to_string(r)
        .map_err(|e| AppError::Internal(format!("serialize results: {e}")))?;
    let thermo = r.thermochemistry.as_ref();
    conn.execute(
        "INSERT INTO results (
            job_id, final_energy_eh, dipole_magnitude_au,
            zpe_eh, inner_energy_u_eh, enthalpy_h_eh, t_times_s_eh, free_energy_g_eh,
            data_json, parser_version, parsed_at
         ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10, datetime('now'))
         ON CONFLICT(job_id) DO UPDATE SET
            final_energy_eh=?2, dipole_magnitude_au=?3,
            zpe_eh=?4, inner_energy_u_eh=?5, enthalpy_h_eh=?6, t_times_s_eh=?7, free_energy_g_eh=?8,
            data_json=?9, parser_version=?10, parsed_at=datetime('now')",
        params![
            job_id,
            r.final_energy_eh,
            r.dipole.as_ref().map(|d| d.magnitude_au),
            thermo.map(|t| t.zpe_eh),
            thermo.map(|t| t.inner_energy_u_eh),
            thermo.map(|t| t.enthalpy_h_eh),
            thermo.map(|t| t.t_times_s_eh),
            thermo.map(|t| t.free_energy_g_eh),
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

/// Extract the `* xyz charge mult … *` coordinate block from an ORCA input as Å
/// coordinates — the reference geometry for verification. Element symbols are not
/// needed (verification compares coordinates; element order is checked separately
/// against `&ATNO`).
fn xyz_reference(input_content: &str) -> Option<Vec<[f64; 3]>> {
    let mut out = Vec::new();
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
                match (toks[1].parse(), toks[2].parse(), toks[3].parse()) {
                    (Ok(x), Ok(y), Ok(z)) => out.push([x, y, z]),
                    _ => {}
                }
            }
        }
    }
    if inside {
        Some(out)
    } else {
        None
    }
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
        let reference = xyz_reference(OPTFREQ_INP).unwrap();
        PropertyFile::parse(OPTFREQ).verify(&reference).unwrap()
    }

    #[test]
    fn per_atom_charges_carry_their_element_order() {
        let r = ParsedResults::from_verified(&verified()).unwrap();
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
        let r = ParsedResults::from_verified(&verified()).unwrap();
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
        let r = ParsedResults::from_verified(&verified()).unwrap();
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
        let r = ParsedResults::from_verified(&verified()).unwrap();
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

    /// End-to-end on a REAL Opt+Freq job dir: the whole slice
    /// (from_path → verify → store → read-back) on real files, not a fixture
    /// string. Opt-in like the other real-data tests.
    #[test]
    #[ignore = "reads a real job dir from ~/.local/share"]
    fn real_optfreq_job_parses_stores_and_reads_back() {
        let dir = format!(
            "{}/.local/share/orcastudio/jobs/d7992449-10e3-47c9-9a16-8e22d60b955d",
            std::env::var("HOME").unwrap()
        );
        if !Path::new(&dir).join("input.property.txt").exists() {
            eprintln!("skipping: real job dir not present");
            return;
        }
        let input = std::fs::read_to_string(Path::new(&dir).join("input.inp")).unwrap();
        let conn = Connection::open_in_memory().unwrap();
        conn.execute("CREATE TABLE jobs (id TEXT PRIMARY KEY)", []).unwrap();
        conn.execute("INSERT INTO jobs (id) VALUES ('real1')", []).unwrap();
        crate::db::create_results_table(&conn).unwrap();

        let outcome = parse_and_store(&conn, "real1", &dir, &input);
        assert!(matches!(outcome, ParseOutcome::Parsed), "{outcome:?}");

        let r = read_job_results(&conn, "real1").unwrap().unwrap();
        assert!((r.final_energy_eh.unwrap() - (-79.791851376071)).abs() < 1e-6);
        let m = r.charges.iter().find(|c| c.scheme == "mulliken").unwrap();
        assert_eq!(m.elements.len(), 8);
        assert_eq!(m.charges.len(), 8);
        let t = r.thermochemistry.unwrap();
        assert!((t.t_times_s_eh - (t.enthalpy_h_eh - t.free_energy_g_eh)).abs() < 1e-9);
        eprintln!("real Opt+Freq stored: E={:?} Eh, mulliken charges={:?}", r.final_energy_eh, m.charges);
    }
}
