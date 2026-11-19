//! The source resolver (ADR-027 decision 4, unit M1b): a geometry reference as the model writes it
//! → M1a's [`ResolvedSource`], read through the app's own readers. Nothing here writes anything.
//!
//! | reference | geometry | charge / multiplicity |
//! |---|---|---|
//! | `job:<id>` | `results.final_geometry` (`read_job_results`) | the job's `* xyz` line, strict ([`locate_charge_mult`]) |
//! | `job:<id>/scan:<k>` | scan point k (`read_scan_geometries`) | the same |
//! | `job:<id>/neb:<k>` | MEP image k (`read_neb_geometries`) | the same |
//! | `job:<id>/neb:ts` | the converged TS (`results.neb.ts_geometry`) | the same |
//! | `job:<id>/goat:<k>` | ensemble member k (`read_ensemble_text`) | the same |
//! | `molecule:<id>` | the molecule's xyz | its `charge` and `multiplicity` columns |
//! | `smiles:<s>` | the sidecar's `/smiles-to-3d`, through [`SmilesTo3d`] | the sidecar's |
//!
//! **Every `k` is 0-based**, like the atom indices the tool descriptions state: `scan:0` is ORCA's
//! `input.001.xyz`, `neb:0` the reactant end of the MEP, `goat:0` the ensemble's first member (the
//! `source_conformer_index` convention).
//!
//! **Per-source decisions (an M1a Open item).**
//! - `terminal_success`: for every job-derived source, the **parent job's** status — `Some(true)`
//!   for `completed`/`parsed`, `Some(false)` for `failed`/`cancelled`/`lost`, `None` while it is
//!   not terminal (R4 either way). A job that did not end in success has no geometry read at all.
//! - `converged`: `job:<id>` takes `ParsedResults.converged` (`Some(false)` is R4). A scan frame,
//!   a NEB image or TS, and a GOAT member are `None`: none of them is a converged optimization in
//!   P3's sense, so a Freq from one is PROPOSED.
//! - `opt_signature`: `job:<id>` whose input has an Opt-family token takes the signature of that
//!   input's header (the text outside its coordinate block), read **strictly**
//!   ([`MethodSignature::of_level_text`]): a line the grammar refuses gives `None`, never a signature
//!   with the line dropped. Every frame source is `None`.
//! - **A constrained optimization is not a stationary point** (Anton, 2026-11-19): a `job:<id>` whose
//!   `%geom` holds a `Constraints` or `Scan` sub-block, or any key beyond `Calc_Hess`, `Recalc_Hess`
//!   and `MaxIter`, has `opt_signature = None`, so a Freq from it is P3 PROPOSED. Any doubt about the
//!   `%geom` block makes it ineligible.
//!
//! **The post-condition (rule #9).** A job-derived geometry's element sequence must equal the source
//! job's own `* xyz` block, element for element. Every reader here keeps the input's atom order
//! (measured for GOAT, `wiki/orca/goat.md`; asserted for NEB, `parse/neb.rs`; the scan point files
//! and the stored final geometry are verified at parse time), so a mismatch means the wrong file
//! or a broken reader, and the source does not resolve. It is skipped only when the charge line
//! itself is unreadable, which R4 refuses anyway.
//!
//! **Locking.** [`resolve_source`] locks the database only for a DB-backed reference, and never for
//! a SMILES, whose HTTP call must not hold the app's one connection.

use super::assemble::read_coordinate_block;
use super::charge_mult::{locate_charge_mult, ChargeMult};
use super::classify::{normalise_element, ResolvedSource, SourceAtom, SourceKind, OPT_FAMILY};
use super::header::{parse_header, BlockItem, ParsedHeader, TokenKind};
use super::signature::{has_keyword, MethodSignature};
use crate::commands::jobs::{get_job_conn, read_ensemble_text};
use crate::commands::molecules::get_molecule_conn;
use crate::commands::settings::DbState;
use crate::memory::{maxcore_lines, read_maxcore};
use crate::models::job::Job;
use crate::parse::xyz::XyzFile;
use rusqlite::Connection;

/// The longest SMILES the resolver passes on.
pub const MAX_SMILES_CHARS: usize = 1000;

/// A NEB frame: an MEP image by index, or the converged TS.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NebFrame {
    Image(usize),
    Ts,
}

/// A parsed geometry reference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceRef {
    Job { id: String },
    Scan { id: String, k: usize },
    Neb { id: String, frame: NebFrame },
    Goat { id: String, k: usize },
    Molecule { id: String },
    Smiles { smiles: String },
}

/// An index `k`: ASCII digits only (no sign, no space), within `usize`.
fn index(s: &str) -> Result<usize, String> {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
        return Err(format!("`{s}` is not a 0-based index"));
    }
    s.parse().map_err(|_| format!("`{s}` is out of range"))
}

/// An id: non-empty, printable ASCII, no `/` and no whitespace.
fn ident(s: &str) -> Result<String, String> {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_graphic() && b != b'/') {
        return Err(format!("`{s}` is not an id"));
    }
    Ok(s.to_string())
}

impl SourceRef {
    /// Parse a reference. Anything outside the six forms is an error naming the forms.
    pub fn parse(reference: &str) -> Result<SourceRef, String> {
        // A SMILES may hold `/` (a double-bond direction), so it is matched first and whole.
        if let Some(smiles) = reference.strip_prefix("smiles:") {
            if smiles.is_empty() || smiles.chars().count() > MAX_SMILES_CHARS || !smiles.bytes().all(|b| b.is_ascii_graphic()) {
                return Err(format!(
                    "a SMILES is 1 to {MAX_SMILES_CHARS} printable ASCII characters with no space"
                ));
            }
            return Ok(SourceRef::Smiles { smiles: smiles.to_string() });
        }
        if let Some(id) = reference.strip_prefix("molecule:") {
            return Ok(SourceRef::Molecule { id: ident(id)? });
        }
        let forms = "job:<id>, job:<id>/scan:<k>, job:<id>/neb:<k|ts>, job:<id>/goat:<k>, molecule:<id>, smiles:<s>";
        let Some(rest) = reference.strip_prefix("job:") else {
            return Err(format!("`{reference}` is not a source reference ({forms})"));
        };
        let (id, frame) = match rest.split_once('/') {
            None => return Ok(SourceRef::Job { id: ident(rest)? }),
            Some((id, frame)) => (ident(id)?, frame),
        };
        if let Some(k) = frame.strip_prefix("scan:") {
            Ok(SourceRef::Scan { id, k: index(k)? })
        } else if let Some(k) = frame.strip_prefix("goat:") {
            Ok(SourceRef::Goat { id, k: index(k)? })
        } else if let Some(k) = frame.strip_prefix("neb:") {
            let frame = if k == "ts" { NebFrame::Ts } else { NebFrame::Image(index(k)?) };
            Ok(SourceRef::Neb { id, frame })
        } else {
            Err(format!("`{reference}` is not a source reference ({forms})"))
        }
    }
}

/// What the sidecar's `/smiles-to-3d` returns, as far as the resolver reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SmilesStructure {
    /// Standard xyz text (count, comment, rows), Å.
    pub xyz: String,
    pub charge: i64,
    pub multiplicity: i64,
    pub num_atoms: usize,
}

/// The seam to the sidecar's `POST /smiles-to-3d`. Production (Part B / M3) calls it over HTTP with
/// `ureq`; the tests hand in a stub, so no test needs a running sidecar.
pub trait SmilesTo3d {
    fn smiles_to_3d(&self, smiles: &str) -> Result<SmilesStructure, String>;
}

/// The atoms of an xyz frame, in Å (the xyz format's unit, rule #11).
fn atoms_of(elements: Vec<String>, xyz: Vec<[f64; 3]>) -> Vec<SourceAtom> {
    elements
        .into_iter()
        .zip(xyz)
        .map(|(element, [x, y, z])| SourceAtom { element, x_ang: x, y_ang: y, z_ang: z })
        .collect()
}

/// The first frame of a standard xyz text.
fn first_frame_of(text: &str, what: &str) -> Result<Vec<SourceAtom>, String> {
    let xyz = XyzFile::parse(text).map_err(|e| format!("{what}: {e}"))?;
    let (elements, coords) = xyz.first_frame().ok_or_else(|| format!("{what} has no atoms"))?;
    Ok(atoms_of(elements, coords))
}

/// The optimization signature of a source job's input: the signature of its header — the text
/// outside its coordinate block — read strictly, when that header has an Opt-family token (C4).
/// The app's own `%maxcore <n>` directive (no `end`, o26) is set aside first, by the memory
/// preflight's own locator and only when it reads cleanly; the header grammar would otherwise read
/// it as a block that never closes. `None` when there is no Opt token, or when the block, the
/// `%maxcore` line or the header does not read cleanly (P3 then fails closed).
fn opt_signature_of(input: &str) -> Option<MethodSignature> {
    let block = read_coordinate_block(input).ok()?;
    read_maxcore(input).ok()?;
    let lines: Vec<&str> = input.lines().collect();
    let maxcore = maxcore_lines(&lines);
    let outside: Vec<&str> = lines
        .iter()
        .enumerate()
        .filter(|(i, _)| (*i < block.header_line || *i > block.closer_line) && !maxcore.contains(i))
        .map(|(_, l)| *l)
        .collect();
    let outside = outside.join("\n");
    let header = parse_header(&outside);
    if !has_keyword(&header, &OPT_FAMILY) || !geom_leaves_a_stationary_point(&header) {
        return None;
    }
    MethodSignature::of_level_text(&outside).ok()
}

/// The `%geom` keys that do not change what an optimization converges to: how the first Hessian is
/// made and how many cycles may run. Values of these keys are `true`/`false` or an integer.
const STATIONARY_GEOM_KEYS: [&str; 3] = ["calc_hess", "recalc_hess", "maxiter"];

/// Anton's decision (2026-11-19): **a constrained optimization is not a stationary point**, so its
/// geometry is not "a converged optimization" for P3. True only when every `%geom` block holds
/// nothing but [`STATIONARY_GEOM_KEYS`] and their `true`/`false`/integer values. A `Constraints` or
/// `Scan` sub-block, or any other key (`OptimizeHydrogens`, a fragment option, anything unrecognised),
/// makes the source ineligible: any doubt about the `%geom` block is not eligible (fail closed).
fn geom_leaves_a_stationary_point(header: &ParsedHeader) -> bool {
    header.blocks.iter().filter(|b| b.name == "geom").all(|b| {
        b.items.iter().all(|item| match item {
            BlockItem::SubBlock(_) => false,
            BlockItem::Token(t) => {
                let w = t.text.to_ascii_lowercase();
                t.kind == TokenKind::Word
                    && (STATIONARY_GEOM_KEYS.contains(&w.as_str())
                        || w == "true"
                        || w == "false"
                        || (!w.is_empty() && w.bytes().all(|c| c.is_ascii_digit())))
            }
        })
    })
}

/// Rule #9: the geometry's element sequence equals the source job's own `* xyz` block.
fn check_elements(job: &Job, atoms: &[SourceAtom], label: &str) -> Result<(), String> {
    let block = read_coordinate_block(&job.input_content)
        .map_err(|e| format!("{label}: the source job's coordinate block is unreadable: {e}"))?;
    if block.rows.len() != atoms.len() {
        return Err(format!(
            "{label}: {} atoms, but the source job's input has {}",
            atoms.len(),
            block.rows.len()
        ));
    }
    for (i, ((el, _), a)) in block.rows.iter().zip(atoms).enumerate() {
        if normalise_element(el) != normalise_element(&a.element) {
            return Err(format!("{label}: atom {i} is {}, but the source job's input has {el}", a.element));
        }
    }
    Ok(())
}

/// The geometry a job-derived reference names. Called only for a job that ended in success.
fn job_geometry(conn: &Connection, job: &Job, r: &SourceRef) -> Result<Vec<SourceAtom>, String> {
    let id = job.id.as_str();
    let dir = job.job_dir.as_deref();
    let err = |e: crate::error::AppError| e.to_string();
    match r {
        SourceRef::Job { .. } => {
            let results = crate::results::read_job_results(conn, id).map_err(err)?;
            let g = results.ok_or_else(|| format!("job {id} has no parsed results"))?.final_geometry;
            Ok(atoms_of(g.elements, g.xyz_angstrom))
        }
        SourceRef::Scan { k, .. } => {
            let frames = crate::results::read_scan_geometries(conn, id, dir)
                .map_err(err)?
                .ok_or_else(|| format!("job {id} is not a relaxed scan with stored points"))?;
            let n = frames.len();
            let f = frames.into_iter().nth(*k).ok_or_else(|| format!("job {id} has scan points 0..{n}, not {k}"))?;
            Ok(atoms_of(f.elements, f.xyz_angstrom))
        }
        SourceRef::Neb { frame: NebFrame::Ts, .. } => {
            let results = crate::results::read_job_results(conn, id).map_err(err)?;
            let neb = results.and_then(|r| r.neb).ok_or_else(|| format!("job {id} is not a parsed NEB run"))?;
            Ok(atoms_of(neb.ts_geometry.elements, neb.ts_geometry.xyz_angstrom))
        }
        SourceRef::Neb { frame: NebFrame::Image(k), .. } => {
            let images = crate::results::read_neb_geometries(conn, id, dir)
                .map_err(err)?
                .ok_or_else(|| format!("job {id} has no NEB band"))?;
            let n = images.len();
            let f = images.into_iter().nth(*k).ok_or_else(|| format!("job {id} has NEB images 0..{n}, not {k}"))?;
            Ok(atoms_of(f.elements, f.xyz_angstrom))
        }
        SourceRef::Goat { k, .. } => {
            let text = read_ensemble_text(id, dir).map_err(err)?;
            if text.is_empty() {
                return Err(format!("job {id} has no GOAT ensemble"));
            }
            let frames = XyzFile::parse(&text).map_err(|e| format!("job {id}'s ensemble: {e}"))?.frames_witness();
            let n = frames.len();
            let (elements, coords, _) =
                frames.into_iter().nth(*k).ok_or_else(|| format!("job {id} has ensemble members 0..{n}, not {k}"))?;
            Ok(atoms_of(elements, coords))
        }
        SourceRef::Molecule { .. } | SourceRef::Smiles { .. } => Err("not a job reference".into()),
    }
}

fn job_kind(r: &SourceRef) -> Option<(&str, SourceKind)> {
    match r {
        SourceRef::Job { id } => Some((id, SourceKind::Job)),
        SourceRef::Scan { id, .. } => Some((id, SourceKind::ScanFrame)),
        SourceRef::Neb { id, .. } => Some((id, SourceKind::NebImage)),
        SourceRef::Goat { id, .. } => Some((id, SourceKind::GoatMember)),
        SourceRef::Molecule { .. } | SourceRef::Smiles { .. } => None,
    }
}

/// Resolve a DB-backed reference (every form but `smiles:`) on the caller's connection.
pub(crate) fn resolve_db_conn(conn: &Connection, r: &SourceRef, label: &str) -> Result<ResolvedSource, String> {
    if let SourceRef::Molecule { id } = r {
        let m = get_molecule_conn(conn, id).map_err(|e| e.to_string())?;
        return Ok(ResolvedSource {
            kind: SourceKind::Molecule,
            label: label.to_string(),
            atoms: first_frame_of(&m.xyz, &format!("molecule {id}"))?,
            charge_mult: Ok(ChargeMult { charge: i64::from(m.charge), multiplicity: i64::from(m.multiplicity) }),
            terminal_success: None,
            converged: None,
            opt_signature: None,
        });
    }
    let Some((id, kind)) = job_kind(r) else {
        return Err("a SMILES is resolved through the sidecar, not the database".into());
    };
    let job = get_job_conn(conn, id).map_err(|e| e.to_string())?;
    let charge_mult = locate_charge_mult(&job.input_content);
    let terminal_success = job.status.is_terminal().then(|| job.status.is_terminal_success());

    // A job that did not end in success has no geometry read: R4 refuses it on its status.
    let mut source = ResolvedSource {
        kind,
        label: label.to_string(),
        atoms: Vec::new(),
        charge_mult,
        terminal_success,
        converged: None,
        opt_signature: None,
    };
    if terminal_success != Some(true) {
        return Ok(source);
    }
    source.atoms = job_geometry(conn, &job, r)?;
    if source.charge_mult.is_ok() {
        check_elements(&job, &source.atoms, label)?;
    }
    if kind == SourceKind::Job {
        source.converged = crate::results::read_job_results(conn, id).map_err(|e| e.to_string())?.and_then(|r| r.converged);
        source.opt_signature = opt_signature_of(&job.input_content);
    }
    Ok(source)
}

/// Resolve a SMILES through the sidecar seam. The returned atom count must match the reply's
/// `num_atoms` (rule #9).
pub(crate) fn resolve_smiles(smiles: &str, sidecar: &dyn SmilesTo3d, label: &str) -> Result<ResolvedSource, String> {
    let s = sidecar.smiles_to_3d(smiles)?;
    let atoms = first_frame_of(&s.xyz, "the sidecar's structure")?;
    if atoms.len() != s.num_atoms {
        return Err(format!("the sidecar reported {} atoms and sent {}", s.num_atoms, atoms.len()));
    }
    Ok(ResolvedSource {
        kind: SourceKind::Smiles,
        label: label.to_string(),
        atoms,
        charge_mult: Ok(ChargeMult { charge: s.charge, multiplicity: s.multiplicity }),
        terminal_success: None,
        converged: None,
        opt_signature: None,
    })
}

/// Resolve `reference` (ADR-027 decision 4). `Err` holds why it did not resolve; the classifier
/// turns it into R4. The database is locked only for a DB-backed reference, and never across the
/// sidecar call.
pub fn resolve_source(db: &DbState, reference: &str, sidecar: &dyn SmilesTo3d) -> Result<ResolvedSource, String> {
    let r = SourceRef::parse(reference)?;
    if let SourceRef::Smiles { smiles } = &r {
        return resolve_smiles(smiles, sidecar, reference);
    }
    let conn = db.lock().map_err(|e| e.to_string())?;
    resolve_db_conn(&conn, &r, reference)
}
