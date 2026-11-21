//! `get_geometry` and `measure` (ADR-027 decisions 4 and 7; M2 amendment, item 8).
//!
//! **Sources.** Both read a database source through [`resolve_db_conn`]: `job:<id>`,
//! `job:<id>/scan:<k>`, `job:<id>/neb:<k|ts>`, `job:<id>/goat:<k>` and `molecule:<id>`. A `smiles:`
//! reference is refused: the read tools do not call the sidecar. A job-derived source whose job has
//! not ended in success has no geometry, and is refused with that reason. Coordinates are Å
//! (rule #11): the resolver's readers convert at their boundary.
//!
//! **`get_geometry`** returns the source as xyz text (`xyz_ang`, 8 decimals) with its charge and
//! multiplicity and where they come from. It is **refused above [`GEOMETRY_MAX_ATOMS`] atoms**, with a
//! pointer to `measure`. Post-condition (rule #9): the emitted text is read back with the app's xyz
//! reader and must give the same elements, in order, within [`XYZ_ROUND_TRIP_ANG`].
//!
//! **`measure`** returns a distance (`distance_ang`), an angle (`angle_deg`, `[0, 180]`, the middle
//! index the vertex) or a dihedral (`dihedral_deg`, folded to `[0, 360)`), chosen by the number of
//! 0-based indices (2, 3 or 4), in ASE's convention, cross-checked against ASE 3.29.0 on the butane
//! fixtures of `sidecar/tests/test_geometry.py` (rule #10). The distance and the angle come from
//! `xtb::distance` and `xtb::angle`, which match ASE. The dihedral does **not** come from
//! `xtb::dihedral`: that gives 360° − ASE's value (the cross-check caught it), so it is
//! [`ase_dihedral`], a line-for-line port of ASE's `get_dihedrals`, as `measure.ts` has. The xtb
//! functions have no range check and return a plausible number for degenerate input (0.0, NaN,
//! 0.0), so every such case is refused here first, as `measure.ts` returns null:
//! - an index out of range, or a repeated index;
//! - coincident atoms in a distance: closer than [`COINCIDENT_ANG`];
//! - a zero-length arm in an angle: an end atom within [`COINCIDENT_ANG`] of the vertex;
//! - a zero axis in a dihedral (the middle two atoms within [`COINCIDENT_ANG`]), or a collinear
//!   inner triple: either end triple with `|sin θ|` below [`COLLINEAR_SIN`] at its middle atom, or
//!   with an arm shorter than [`COINCIDENT_ANG`].
//!
//! A value outside its range after these checks is an internal error, never served.

use rusqlite::Connection;
use serde::Serialize;
use thiserror::Error;

use super::jobs::ChargeMultView;
use super::{echo, within_cap, ToolError, RESULT_CAP_BYTES};
use crate::mcp::classify::{ResolvedSource, SourceAtom, SourceKind};
use crate::mcp::resolve::{resolve_db_conn, SourceRef};
use crate::parse::xyz::XyzFile;

/// `get_geometry` refuses a source with more atoms than this (decision 7).
pub const GEOMETRY_MAX_ATOMS: usize = 600;

/// Two positions closer than this are one point, Å. The app's own coordinate round-trip tolerance
/// (`assemble::COORD_TOLERANCE_ANG`): below it two positions are not distinguishable in the app's
/// terms, so a distance, an arm or an axis that short has no direction.
pub const COINCIDENT_ANG: f64 = 1e-6;

/// An inner triple of a dihedral is collinear when `|sin θ|` at its middle atom is below this
/// (θ within about 5.7e-5° of 0° or 180°). Scale-free, like `measure.ts`'s criterion, but at the
/// same 1e-6 as [`COINCIDENT_ANG`]: with ~1 Å bonds the projection that sets the dihedral's
/// direction is then about 1e-6 Å, the size of the app's coordinate tolerance, so the angle would
/// be noise. (`measure.ts` uses 1e-9; ASE refuses only an exact zero.)
pub const COLLINEAR_SIN: f64 = 1e-6;

/// The emitted xyz text round-trips within this, Å (8 decimals round to 5e-9).
pub const XYZ_ROUND_TRIP_ANG: f64 = 1e-8;

/// Why a measurement is not given. Every index is the caller's 0-based one.
#[derive(Debug, Clone, PartialEq, Error, Serialize)]
#[serde(tag = "refusal", rename_all = "snake_case")]
pub enum MeasureRefusal {
    #[error("{count} indices: give 2 (a distance), 3 (an angle, the middle one the vertex) or 4 (a dihedral)")]
    WrongCount { count: usize },
    #[error("atom index {index} is out of range: the source has {atom_count} atoms, indices 0 to {atom_count} exclusive (0-based)")]
    IndexOutOfRange { index: usize, atom_count: usize },
    #[error("atom index {index} is given twice; a measurement needs distinct atoms")]
    RepeatedIndex { index: usize },
    #[error("atom {index} has a coordinate that is not a finite number")]
    NonFiniteCoordinate { index: usize },
    #[error("atoms {i} and {j} coincide (closer than {COINCIDENT_ANG} Å): the distance is not defined")]
    CoincidentAtoms { i: usize, j: usize },
    #[error("atom {end} coincides with the vertex {vertex} (closer than {COINCIDENT_ANG} Å): the angle is not defined")]
    ZeroLengthArm { vertex: usize, end: usize },
    #[error("the dihedral's axis {j}–{k} has zero length (closer than {COINCIDENT_ANG} Å): the dihedral is not defined")]
    ZeroAxis { j: usize, k: usize },
    #[error("atoms {a}–{b}–{c} are collinear (|sin θ| below {COLLINEAR_SIN} at {b}): the dihedral is not defined")]
    CollinearInnerTriple { a: usize, b: usize, c: usize },
    #[error("the {kind} came out as {value}, outside its range; not served")]
    OutOfRange { kind: &'static str, value: f64 },
}

/// A measured value, its unit in its name.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Measured {
    Distance { distance_ang: f64 },
    Angle { angle_deg: f64 },
    Dihedral { dihedral_deg: f64 },
}

type V3 = [f64; 3];

fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn norm(a: V3) -> f64 {
    (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt()
}

fn cross(a: V3, b: V3) -> V3 {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

/// `|sin θ|` between `u` and `w`; `None` when either is shorter than [`COINCIDENT_ANG`].
fn sin_between(u: V3, w: V3) -> Option<f64> {
    let (nu, nw) = (norm(u), norm(w));
    (nu >= COINCIDENT_ANG && nw >= COINCIDENT_ANG).then(|| norm(cross(u, w)) / (nu * nw))
}

/// The measurement named by `indices` on `atoms` (Å), or why there is none. Pure.
pub fn measure_atoms(atoms: &[SourceAtom], indices: &[usize]) -> Result<Measured, MeasureRefusal> {
    if !(2..=4).contains(&indices.len()) {
        return Err(MeasureRefusal::WrongCount { count: indices.len() });
    }
    for &index in indices {
        if index >= atoms.len() {
            return Err(MeasureRefusal::IndexOutOfRange { index, atom_count: atoms.len() });
        }
    }
    for (n, &index) in indices.iter().enumerate() {
        if indices[..n].contains(&index) {
            return Err(MeasureRefusal::RepeatedIndex { index });
        }
    }
    let mut p: Vec<V3> = Vec::with_capacity(indices.len());
    for &index in indices {
        let a = &atoms[index];
        let xyz = [a.x_ang, a.y_ang, a.z_ang];
        if !xyz.iter().all(|c| c.is_finite()) {
            return Err(MeasureRefusal::NonFiniteCoordinate { index });
        }
        p.push(xyz);
    }

    let value = match *indices {
        [i, j] => {
            if norm(sub(p[1], p[0])) < COINCIDENT_ANG {
                return Err(MeasureRefusal::CoincidentAtoms { i, j });
            }
            Measured::Distance { distance_ang: crate::xtb::distance(&p, 0, 1) }
        }
        [i, v, j] => {
            for (end, at) in [(i, 0), (j, 2)] {
                if norm(sub(p[at], p[1])) < COINCIDENT_ANG {
                    return Err(MeasureRefusal::ZeroLengthArm { vertex: v, end });
                }
            }
            Measured::Angle { angle_deg: crate::xtb::angle(&p, 0, 1, 2) }
        }
        [i, j, k, l] => {
            let axis = sub(p[2], p[1]);
            if norm(axis) < COINCIDENT_ANG {
                return Err(MeasureRefusal::ZeroAxis { j, k });
            }
            // The triple i–j–k at j (arms j→i and j→k), and j–k–l at k (arms k→j and k→l).
            if sin_between(sub(p[0], p[1]), axis).is_none_or(|s| s < COLLINEAR_SIN) {
                return Err(MeasureRefusal::CollinearInnerTriple { a: i, b: j, c: k });
            }
            if sin_between(sub(p[3], p[2]), axis).is_none_or(|s| s < COLLINEAR_SIN) {
                return Err(MeasureRefusal::CollinearInnerTriple { a: j, b: k, c: l });
            }
            Measured::Dihedral { dihedral_deg: ase_dihedral(p[0], p[1], p[2], p[3]) }
        }
        _ => return Err(MeasureRefusal::WrongCount { count: indices.len() }),
    };
    in_range(value)
}

fn dot(a: V3, b: V3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn scale(a: V3, s: f64) -> V3 {
    [a[0] * s, a[1] * s, a[2] * s]
}

/// The dihedral p0–p1–p2–p3 in degrees, `[0, 360)`, by ASE's `get_dihedrals`
/// (`ase/geometry/geometry.py`, ASE 3.29.0) line for line, as `measure.ts`'s `dihedralCoords` ports
/// it: project `-v0` and `v2` onto the plane normal to the axis `v1`, take `atan2((v1n × v)·w, v·w)`,
/// and fold a negative angle up by 360°. Not `xtb::dihedral`, which gives 360° − ASE's value (measured
/// on the butane fixtures: 180.0018° for ASE's 179.9982°, 292.477° for 67.523°). The caller has
/// already refused a zero axis and a collinear triple.
fn ase_dihedral(p0: V3, p1: V3, p2: V3, p3: V3) -> f64 {
    let v0 = sub(p1, p0);
    let v1 = sub(p2, p1);
    let v2 = sub(p3, p2);
    let v1n = scale(v1, 1.0 / norm(v1));
    let neg_v0 = scale(v0, -1.0);
    let v = sub(neg_v0, scale(v1n, dot(neg_v0, v1n)));
    let w = sub(v2, scale(v1n, dot(v2, v1n)));
    let mut deg = dot(cross(v1n, v), w).atan2(dot(v, w)).to_degrees();
    if deg < 0.0 {
        deg += 360.0;
    }
    // A tiny negative angle folds to exactly 360.0 in floating point; that is 0°.
    if deg >= 360.0 {
        deg -= 360.0;
    }
    deg
}

/// The post-condition: a finite value in its range (a distance above 0, an angle in `[0, 180]`, a
/// dihedral in `[0, 360)`).
fn in_range(m: Measured) -> Result<Measured, MeasureRefusal> {
    let (kind, value, ok) = match m {
        Measured::Distance { distance_ang: v } => ("distance", v, v > 0.0),
        Measured::Angle { angle_deg: v } => ("angle", v, (0.0..=180.0).contains(&v)),
        Measured::Dihedral { dihedral_deg: v } => ("dihedral", v, (0.0..360.0).contains(&v)),
    };
    if value.is_finite() && ok {
        Ok(m)
    } else {
        Err(MeasureRefusal::OutOfRange { kind, value })
    }
}

/// A source's kind, as the tools name it.
fn kind_name(kind: SourceKind) -> &'static str {
    match kind {
        SourceKind::Job => "job",
        SourceKind::ScanFrame => "scan_frame",
        SourceKind::NebImage => "neb_image",
        SourceKind::GoatMember => "goat_member",
        SourceKind::Molecule => "molecule",
        SourceKind::Smiles => "smiles",
    }
}

/// Where a resolved source's charge and multiplicity come from.
fn charge_mult_source(src: &ResolvedSource, r: &SourceRef) -> String {
    match r {
        SourceRef::Molecule { id } => format!("molecule {id}'s stored charge and multiplicity"),
        SourceRef::Job { id } | SourceRef::Scan { id, .. } | SourceRef::Neb { id, .. } | SourceRef::Goat { id, .. } => {
            format!("the `* xyz` line of job {id}'s input")
        }
        SourceRef::Smiles { .. } => format!("{}: the sidecar", src.label),
    }
}

/// A source reference longer than this is refused before it is read: the resolver's messages quote
/// the reference (or its id) whole, so this bounds every quote of it to an echo's length. A real one
/// (`job:<uuid>/neb:ts`) is about 50 bytes.
pub const SOURCE_MAX_BYTES: usize = super::ECHO_MAX_BYTES;

/// A database source with its atoms, or why the read tools cannot read it.
fn read_source(conn: &Connection, reference: &str) -> Result<(SourceRef, ResolvedSource), ToolError> {
    if reference.len() > SOURCE_MAX_BYTES {
        return Err(ToolError::refused(format!("the source reference is longer than {SOURCE_MAX_BYTES} bytes")));
    }
    let r = SourceRef::parse(reference).map_err(ToolError::refused)?;
    if matches!(r, SourceRef::Smiles { .. }) {
        return Err(ToolError::refused(
            "a smiles: reference has no stored geometry; the read tools read job: and molecule: sources only",
        ));
    }
    let src = resolve_db_conn(conn, &r, reference).map_err(ToolError::refused)?;
    if src.kind.is_job_derived() {
        match src.terminal_success {
            Some(true) => {}
            Some(false) => {
                return Err(ToolError::refused(format!(
                    "{}: the job ended without success (failed, cancelled or lost), so it has no geometry to read",
                    echo(reference)
                )))
            }
            None => {
                return Err(ToolError::refused(format!(
                    "{}: the job has not finished, so it has no final geometry yet",
                    echo(reference)
                )))
            }
        }
    }
    if src.atoms.is_empty() {
        return Err(ToolError::refused(format!("{} has no atoms", echo(reference))));
    }
    Ok((r, src))
}

/// `get_geometry`'s answer.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct GeometryView {
    pub source: String,
    pub source_kind: &'static str,
    pub atom_count: usize,
    pub charge_mult: ChargeMultView,
    /// Standard xyz text: the atom count, a comment line, then `element x y z` per atom, Å.
    pub xyz_ang: String,
}

fn xyz_text(label: &str, atoms: &[SourceAtom]) -> String {
    let mut out = format!("{}\n{label} (Å)\n", atoms.len());
    for a in atoms {
        out.push_str(&format!("{} {:.8} {:.8} {:.8}\n", a.element, a.x_ang, a.y_ang, a.z_ang));
    }
    out
}

/// The post-condition: `text` reads back to `atoms` (elements in order, coordinates within
/// [`XYZ_ROUND_TRIP_ANG`]).
fn check_round_trip(text: &str, atoms: &[SourceAtom]) -> Result<(), ToolError> {
    let fail = |why: String| ToolError::App(crate::error::AppError::Internal(format!("get_geometry's xyz text: {why}")));
    let xyz = XyzFile::parse(text).map_err(|e| fail(e.to_string()))?;
    let (elements, coords) = xyz.first_frame().ok_or_else(|| fail("no frame".into()))?;
    if elements.len() != atoms.len() {
        return Err(fail(format!("{} atoms read back, {} written", elements.len(), atoms.len())));
    }
    for (i, ((el, c), a)) in elements.iter().zip(&coords).zip(atoms).enumerate() {
        let off = sub(*c, [a.x_ang, a.y_ang, a.z_ang]).iter().fold(0.0_f64, |m, d| m.max(d.abs()));
        if *el != a.element || off > XYZ_ROUND_TRIP_ANG {
            return Err(fail(format!("atom {i} reads back as {el} {off:.1e} Å away")));
        }
    }
    Ok(())
}

/// `get_geometry`: the source's xyz with its charge and multiplicity.
pub fn get_geometry(conn: &Connection, reference: &str) -> Result<GeometryView, ToolError> {
    let (r, src) = read_source(conn, reference)?;
    if src.atoms.len() > GEOMETRY_MAX_ATOMS {
        return Err(ToolError::refused(format!(
            "{} has {} atoms, above get_geometry's cap of {GEOMETRY_MAX_ATOMS}; use measure for distances, \
             angles and dihedrals by 0-based atom index",
            echo(reference),
            src.atoms.len()
        )));
    }
    let xyz_ang = xyz_text(reference, &src.atoms);
    check_round_trip(&xyz_ang, &src.atoms)?;
    let view = GeometryView {
        source: reference.to_string(),
        source_kind: kind_name(src.kind),
        atom_count: src.atoms.len(),
        charge_mult: ChargeMultView::of(&src.charge_mult, charge_mult_source(&src, &r)),
        xyz_ang,
    };
    within_cap(view, RESULT_CAP_BYTES)
}

/// `measure`'s answer.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MeasureView {
    pub source: String,
    /// The atoms measured, 0-based, as given.
    pub atom_indices: Vec<usize>,
    pub elements: Vec<String>,
    #[serde(flatten)]
    pub value: Measured,
}

/// `measure`: a distance, angle or dihedral on the source, by 0-based indices.
pub fn measure(conn: &Connection, reference: &str, indices: &[usize]) -> Result<MeasureView, ToolError> {
    let (_, src) = read_source(conn, reference)?;
    let value = measure_atoms(&src.atoms, indices)?;
    let view = MeasureView {
        source: reference.to_string(),
        atom_indices: indices.to_vec(),
        elements: indices.iter().map(|&i| src.atoms[i].element.clone()).collect(),
        value,
    };
    within_cap(view, RESULT_CAP_BYTES)
}
