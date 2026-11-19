//! The input assembly (ADR-027 decision 4; clarification C9). The input is, in order: the model's
//! header; the app's `%maxcore` (through [`crate::memory::apply_maxcore`], which inserts it after
//! the first `!` line); a `# charge/mult from <source>` comment; and the coordinate block from
//! [`orcastudio_core::emit::emit_coordinate_block`], built from the source atoms. The model never
//! writes a coordinate, a charge or a multiplicity (ADR-014 (1)).
//!
//! **The post-condition (rule #9).** The assembled text is re-parsed by a strict reader of its own —
//! the `* xyz <c> <m>` line by [`super::charge_mult::locate_xyz_header`], every row as exactly an
//! element and three finite numbers, then a closing `*` — and must give back the source's atom
//! count, element order, coordinates (within [`COORD_TOLERANCE_ANG`]; the emitter rounds to 8
//! decimals), charge and multiplicity, in exactly one coordinate block, and no line may hold two
//! or more `#` (`header::UNMEASURED_COMMENT`). The source label is made comment-safe first
//! ([`comment_safe_label`]), so `smiles:C#N` cannot put a second `#` on the comment line. A mismatch is an
//! [`AssemblyError`], which the caller turns into a REFUSED internal error. The emitter's own
//! `parse_coordinate_rows` is not used: it skips a row it cannot read.

use orcastudio_core::emit::emit_coordinate_block;
use orcastudio_core::ids::AtomId;
use orcastudio_core::scene::{Scene, SceneAtom, SceneFragment};
use serde::Serialize;
use thiserror::Error;

use super::charge_mult::{locate_xyz_header, ChargeMult};
use super::header::has_unmeasured_comment;
use super::classify::{normalise_element, RefusedReason, SourceAtom};
use crate::memory::{apply_maxcore, read_maxcore};
use crate::parse::elements::z_of;

/// The `%maxcore` the app sets for a server whose memory facts are unknown: the builder's server
/// default (TS `REMOTE_DEFAULT_MAXCORE_MIB`, `src/memory/preflight.ts`). Such a job also gets P4.
/// The local fallback stays [`crate::memory::FALLBACK_MAXCORE_MIB`].
pub const SERVER_DEFAULT_MAXCORE_MIB: u64 = 2000;

/// How far a re-parsed coordinate may lie from its source, in Å. The emitter writes 8 decimals,
/// so an honest round-trip is within 5e-9 Å.
pub const COORD_TOLERANCE_ANG: f64 = 1e-6;

/// What the app set in the input, for the response ("set by the app").
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AppSet {
    pub maxcore_mib: u64,
    pub charge: i64,
    pub multiplicity: i64,
    /// The source reference the charge and multiplicity came from.
    pub charge_mult_from: String,
    pub atom_count: usize,
}

/// The assembled input and what the app set in it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Assembled {
    pub input: String,
    pub app_set: AppSet,
}

/// Why the input could not be assembled. Every variant is a REFUSED internal error.
#[derive(Debug, Clone, PartialEq, Error, Serialize)]
pub enum AssemblyError {
    #[error("the header already sets %maxcore")]
    HeaderSetsMaxcore,
    #[error("setting %maxcore failed: {0}")]
    Maxcore(String),
    #[error("the source has no atoms")]
    NoAtoms,
    #[error("unknown element `{0}` in the source")]
    UnknownElement(String),
    #[error("source atom {0} has a non-finite coordinate")]
    NonFiniteCoordinate(usize),
    #[error("the assembled input does not re-parse to its source: {0}")]
    PostCondition(String),
}

impl AssemblyError {
    /// The REFUSED reason the caller records.
    pub fn as_refusal(&self) -> RefusedReason {
        RefusedReason::Internal { detail: self.to_string() }
    }
}

/// One source atom with its element normalised, in Å.
#[derive(Debug, Clone, PartialEq)]
struct CheckedAtom {
    element: String,
    xyz_ang: [f64; 3],
}

/// The source label as written into the `# charge/mult from …` comment: ASCII letters and digits,
/// space and `:_-./()=[]@+` are kept; anything else — `#` (a SMILES triple bond), a line break, a
/// control or non-ASCII character — becomes `?`. So the comment line holds exactly one `#`.
pub fn comment_safe_label(label: &str) -> String {
    label
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, ' ' | ':' | '_' | '-' | '.' | '/' | '(' | ')' | '=' | '[' | ']' | '@' | '+') {
                c
            } else {
                '?'
            }
        })
        .collect()
}

/// An input's first `* xyz` block, read strictly: where it starts and ends (0-based line numbers)
/// and its charge, multiplicity and rows. The rows' elements are as written.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct CoordinateBlock {
    pub header_line: usize,
    pub closer_line: usize,
    pub charge_mult: ChargeMult,
    pub rows: Vec<(String, [f64; 3])>,
}

/// The strict reader of C9, shared by the assembly post-condition and the resolver's source
/// post-condition (`resolve.rs`): the `* xyz <c> <m>` line by [`locate_xyz_header`], then every
/// row as exactly an element and three finite numbers, up to a closing `*`. A row it cannot read
/// is an error, never skipped.
pub(crate) fn read_coordinate_block(input: &str) -> Result<CoordinateBlock, String> {
    let (at, cm) = locate_xyz_header(input).map_err(|e| e.to_string())?;
    let mut rows = Vec::new();
    for (i, line) in input.lines().enumerate().skip(at + 1) {
        let t = line.trim();
        if t == "*" {
            return Ok(CoordinateBlock { header_line: at, closer_line: i, charge_mult: cm, rows });
        }
        let toks: Vec<&str> = t.split_whitespace().collect();
        let [el, x, y, z] = toks.as_slice() else {
            return Err(format!("row `{t}` is not an element and three numbers"));
        };
        let mut xyz_ang = [0.0; 3];
        for (k, s) in [x, y, z].iter().enumerate() {
            match s.parse::<f64>() {
                Ok(v) if v.is_finite() => xyz_ang[k] = v,
                _ => return Err(format!("row `{t}` has an unreadable coordinate `{s}`")),
            }
        }
        rows.push((el.to_string(), xyz_ang));
    }
    Err("the coordinate block has no closing `*`".into())
}

/// Read back the assembled input's coordinate block (the strict reader of C9), after the two
/// whole-text checks: no line with two or more `#`, and exactly one block (two `*` lines).
fn reparse(input: &str) -> Result<(ChargeMult, Vec<CheckedAtom>), String> {
    if let Some((i, _)) = input.lines().enumerate().find(|(_, l)| has_unmeasured_comment(l)) {
        return Err(format!("line {} has two or more `#`", i + 1));
    }
    let star_lines = input
        .lines()
        .filter(|l| l.find('#').map_or(*l, |i| &l[..i]).trim_start().starts_with('*'))
        .count();
    if star_lines != 2 {
        return Err(format!("{star_lines} `*` lines, expected one block (opener and closer)"));
    }
    let block = read_coordinate_block(input)?;
    let rows = block.rows.into_iter().map(|(element, xyz_ang)| CheckedAtom { element, xyz_ang }).collect();
    Ok((block.charge_mult, rows))
}

/// The post-condition: `input` re-parses to exactly `atoms`, `cm` and `maxcore_mib`.
fn verify(input: &str, atoms: &[CheckedAtom], cm: ChargeMult, maxcore_mib: u64) -> Result<(), AssemblyError> {
    let fail = |m: String| Err(AssemblyError::PostCondition(m));
    let (got_cm, rows) = reparse(input).map_err(AssemblyError::PostCondition)?;
    if got_cm != cm {
        return fail(format!("charge/multiplicity {got_cm:?}, source {cm:?}"));
    }
    if rows.len() != atoms.len() {
        return fail(format!("{} atoms, source {}", rows.len(), atoms.len()));
    }
    for (i, (r, a)) in rows.iter().zip(atoms).enumerate() {
        if r.element != a.element {
            return fail(format!("atom {i} is {}, source {}", r.element, a.element));
        }
        let off = r.xyz_ang.iter().zip(&a.xyz_ang).map(|(p, q)| (p - q).abs()).fold(0.0, f64::max);
        if off > COORD_TOLERANCE_ANG {
            return fail(format!("atom {i} lies {off:.3e} Å from its source"));
        }
    }
    match read_maxcore(input) {
        Ok(Some(v)) if v == maxcore_mib => Ok(()),
        other => fail(format!("%maxcore reads {other:?}, expected {maxcore_mib}")),
    }
}

/// The emitter's block text for a scene (the production emitter).
fn emit_block(scene: &Scene) -> String {
    emit_coordinate_block(scene).0
}

/// Assemble the input for a job (C9).
pub fn assemble(
    header: &str,
    maxcore_mib: u64,
    atoms: &[SourceAtom],
    cm: ChargeMult,
    source_label: &str,
) -> Result<Assembled, AssemblyError> {
    assemble_with(header, maxcore_mib, atoms, cm, source_label, emit_block)
}

/// [`assemble`] with the coordinate emitter as a seam, so a test can hand it a faulty one and see
/// the post-condition refuse it.
pub(crate) fn assemble_with(
    header: &str,
    maxcore_mib: u64,
    atoms: &[SourceAtom],
    cm: ChargeMult,
    source_label: &str,
    emit: impl Fn(&Scene) -> String,
) -> Result<Assembled, AssemblyError> {
    if atoms.is_empty() {
        return Err(AssemblyError::NoAtoms);
    }
    let header = header.replace("\r\n", "\n");
    if !matches!(read_maxcore(&header), Ok(None)) {
        return Err(AssemblyError::HeaderSetsMaxcore);
    }

    let mut checked = Vec::with_capacity(atoms.len());
    for (i, a) in atoms.iter().enumerate() {
        let element = normalise_element(&a.element);
        if z_of(&element).is_none() {
            return Err(AssemblyError::UnknownElement(a.element.clone()));
        }
        let xyz_ang = [a.x_ang, a.y_ang, a.z_ang];
        if !xyz_ang.iter().all(|v| v.is_finite()) {
            return Err(AssemblyError::NonFiniteCoordinate(i));
        }
        checked.push(CheckedAtom { element, xyz_ang });
    }

    // One fragment, ids 0..n, the charge on the fragment and the multiplicity on the scene (C9).
    let scene = Scene {
        fragments: vec![SceneFragment {
            id: "source".into(),
            name: source_label.to_string(),
            atoms: checked
                .iter()
                .enumerate()
                .map(|(i, a)| SceneAtom {
                    id: AtomId::new(i as u32),
                    element: a.element.clone(),
                    x: a.xyz_ang[0],
                    y: a.xyz_ang[1],
                    z: a.xyz_ang[2],
                })
                .collect(),
            charge: cm.charge,
            source: "import".into(),
            source_label: None,
        }],
        multiplicity: cm.multiplicity,
        next_atom_id: checked.len() as u32,
    };

    let head = format!("{}\n", header.trim_end());
    let head = apply_maxcore(&head, maxcore_mib).map_err(AssemblyError::Maxcore)?;
    let label = comment_safe_label(source_label);
    let input = format!("{head}\n# charge/mult from {label}\n{}\n", emit(&scene));

    verify(&input, &checked, cm, maxcore_mib)?;
    Ok(Assembled {
        input,
        app_set: AppSet {
            maxcore_mib,
            charge: cm.charge,
            multiplicity: cm.multiplicity,
            charge_mult_from: source_label.to_string(),
            atom_count: checked.len(),
        },
    })
}
