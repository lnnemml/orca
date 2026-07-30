//! Reader for ORCA's `.property.txt` — the first of four artifact readers
//! (ADR-012). It is written as the **template** the other three (`.hess`,
//! `_trj.xyz`/`.xyz`, `orca_2json`) follow, so the shape matters as much as the
//! content.
//!
//! # Two layers
//!
//! 1. A **generic tokenizer** ([`PropertyFile::parse`]) that understands only the
//!    file *grammar* — `$Block … $End` with `&prop [&Type …, &Dim …, &Units …]`
//!    entries whose value is either inline (a scalar) or on the following lines (a
//!    coordinate table or an indexed array). It does **not** know any block by
//!    name. Every block it sees is kept, including ones it has no accessor for —
//!    so a new block in ORCA 6.2 becomes *visible* ([`PropertyFile::unknown_block_names`]),
//!    not silently dropped (domain rule #10).
//! 2. **Typed accessors** on top ([`Verified::geometries`], [`Verified::charges`], …)
//!    that interpret known blocks and — crucially — **convert to canonical units at
//!    this boundary** (rule #11). Bohr coordinates become [`Angstrom`] and never
//!    leave this layer as bare `f64`. The accessors live on [`Verified`], not
//!    [`PropertyFile`], so a value cannot be read before the post-conditions pass
//!    ([`PropertyFile::verify`]) — rule #9, the post-condition is on the path.
//!
//! # Units (all measured — `wiki/orca/parse-sources.md`)
//! `$Geometry` is **Bohr** → Å here; energies **Eh**; dipole **a.u.**; gradient
//! **Eh/Bohr**; thermochemistry **Eh**. The one field that lies to a careless
//! reader is `entropyS`: it is measured to be **T·S in Eh**, not the entropy S, so
//! it is named [`Thermochemistry::t_times_s_eh`].
//!
//! # Rule #5
//! `.property.txt` is tens–hundreds of KB (measured max ≈ 344 KB, the saddle job),
//! so reading it whole is fine — unlike `output.out`. [`PropertyFile::from_path`]
//! still caps the size and refuses a pathological file rather than eating memory.

use std::path::Path;
use std::sync::LazyLock;

use regex::Regex;
use serde::Serialize;

use super::elements::{strip_fragment_suffix, z_of};
use super::units::Angstrom;
use super::ParseError;

/// Measured max is ≈ 344 KB (saddle `99e805f5`); 16 MB is a wide safety margin.
const MAX_BYTES: u64 = 16 * 1024 * 1024;

/// The geometry post-condition tolerance (rule #11): a missed Bohr→Å conversion
/// shows up as ≈1.889×, far above this.
const GEOMETRY_TOL_ANGSTROM: f64 = 1e-4;

static UNITS_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"&Units\s+"([^"]+)""#).unwrap());

// --------------------------------------------------------------------------- //
// Layer 1 — generic tokenizer                                                   //
// --------------------------------------------------------------------------- //

/// A `&prop` entry as seen by the tokenizer, uninterpreted. Only `units` is kept
/// from the `[…]` bracket — the typed layer interprets values by field name + data
/// shape, and checks array lengths against N rather than trusting `&Dim` (rule
/// #11). A future reader that needs `&Type`/`&Dim` adds them back with a consumer.
#[derive(Clone, Debug)]
pub struct RawProp {
    pub name: String,
    pub units: Option<String>,
    /// The first token after `]` (or after the name when there is no bracket) — a
    /// scalar value like `8`, `-79.79…`, `true`, `"6.1.0"`.
    pub inline: Option<String>,
    /// Lines following the entry until the next `&`/`$` — a coordinate table or an
    /// indexed array. Empty for a scalar.
    pub data_lines: Vec<String>,
}

impl RawProp {
    /// Parse `inline` as an `f64` (a scalar Double). Trailing `"comment"` on the
    /// same line is already excluded — `inline` is only the first token.
    fn scalar_f64(&self) -> Option<f64> {
        self.inline.as_ref()?.parse().ok()
    }

    /// Values of an indexed single-column array (`&AtomicCharges`, `&grad`,
    /// `&ATNO`, `&FREQ`, `&totalEnergy`). A data row is `rowindex value…`; the lone
    /// `0` column-header line and the blank line are skipped by the ≥2-token rule.
    fn numbers(&self) -> Vec<f64> {
        let mut out = Vec::new();
        for line in &self.data_lines {
            let toks: Vec<&str> = line.split_whitespace().collect();
            if toks.len() >= 2 && toks[0].parse::<usize>().is_ok() {
                if let Ok(v) = toks[1].parse::<f64>() {
                    out.push(v);
                }
            }
        }
        out
    }

    /// Element rows of a `Coordinates` value: `SYM x y z` (element may carry a
    /// `(1)` fragment suffix, measured in GOAT/xTB).
    fn coordinates(&self) -> Vec<(String, [f64; 3])> {
        let mut out = Vec::new();
        for line in &self.data_lines {
            let toks: Vec<&str> = line.split_whitespace().collect();
            if toks.len() < 4 {
                continue;
            }
            let sym = strip_fragment_suffix(toks[0]).to_string();
            if z_of(&sym).is_none() {
                continue;
            }
            match (toks[1].parse(), toks[2].parse(), toks[3].parse()) {
                (Ok(x), Ok(y), Ok(z)) => out.push((sym, [x, y, z])),
                _ => {}
            }
        }
        out
    }
}

/// A `$Block … $End`, uninterpreted.
#[derive(Clone, Debug)]
pub struct RawBlock {
    pub name: String,
    pub geometry_index: Option<u32>,
    pub props: Vec<RawProp>,
}

impl RawBlock {
    fn prop(&self, name: &str) -> Option<&RawProp> {
        self.props.iter().find(|p| p.name == name)
    }
}

/// A parsed `.property.txt`: the ordered list of blocks the tokenizer saw.
#[derive(Clone, Debug)]
pub struct PropertyFile {
    pub blocks: Vec<RawBlock>,
}

/// Blocks the typed layer either interprets or deliberately ignores (metadata /
/// timings / the Hessian, which is the `.hess` reader's job). Anything **not** in
/// this list is surfaced by [`PropertyFile::unknown_block_names`] so a new ORCA
/// block does not vanish (rule #10).
const KNOWN_BLOCKS: &[&str] = &[
    // interpreted here
    "Geometry",
    "SCF_Energy",
    "DFT_Energy",
    "Single_Point_Data",
    "SCF_Mulliken_Population_Analysis",
    "SCF_Loewdin_Population_Analysis",
    "SCF_Mayer_Population_Analysis",
    "SCF_Dipole_Moment",
    "SCF_Nuc_Gradient",
    "THERMOCHEMISTRY_Energies",
    // recognized but deliberately not read here
    "Calculation_Status",
    "Calculation_Info",
    "Calculation_Timings",
    "SCF_Timings",
    "VdW_Correction",
    "gCP_Energy",
    "Hessian", // the .hess reader owns the Hessian
    "Solvation_Details",
];

impl PropertyFile {
    /// Read + tokenize a `.property.txt` from disk, refusing a pathological size
    /// (rule #5). The reader itself never re-reads or streams — the file is small.
    pub fn from_path(path: &Path) -> Result<Self, ParseError> {
        let meta = std::fs::metadata(path).map_err(|e| ParseError::Io {
            path: path.display().to_string(),
            source: e,
        })?;
        if meta.len() > MAX_BYTES {
            return Err(ParseError::TooLarge {
                artifact: "property.txt",
                bytes: meta.len(),
                cap: MAX_BYTES,
            });
        }
        let text = std::fs::read_to_string(path).map_err(|e| ParseError::Io {
            path: path.display().to_string(),
            source: e,
        })?;
        Ok(Self::parse(&text))
    }

    /// The generic tokenizer. Knows the grammar, not the blocks.
    pub fn parse(text: &str) -> Self {
        let mut blocks: Vec<RawBlock> = Vec::new();
        let mut block: Option<RawBlock> = None;
        let mut prop: Option<RawProp> = None;

        // flush the in-progress prop into the current block
        fn flush_prop(block: &mut Option<RawBlock>, prop: &mut Option<RawProp>) {
            if let (Some(b), Some(p)) = (block.as_mut(), prop.take()) {
                b.props.push(p);
            }
        }

        for raw in text.lines() {
            let line = raw.trim_end();
            let trimmed = line.trim_start();

            if let Some(name) = trimmed.strip_prefix('$') {
                flush_prop(&mut block, &mut prop);
                if name == "End" {
                    if let Some(b) = block.take() {
                        blocks.push(b);
                    }
                } else {
                    if let Some(b) = block.take() {
                        blocks.push(b);
                    }
                    block = Some(RawBlock {
                        name: name.trim().to_string(),
                        geometry_index: None,
                        props: Vec::new(),
                    });
                }
            } else if let Some(rest) = trimmed.strip_prefix('&') {
                flush_prop(&mut block, &mut prop);
                prop = Some(parse_prop_line(rest));
                // capture &GeometryIndex onto the block
                if let (Some(b), Some(p)) = (block.as_mut(), prop.as_ref()) {
                    if p.name == "GeometryIndex" {
                        b.geometry_index = p.inline.as_ref().and_then(|s| s.parse().ok());
                    }
                }
            } else if let Some(p) = prop.as_mut() {
                // a data line (coordinate / array row / column header / blank)
                p.data_lines.push(line.to_string());
            }
        }
        flush_prop(&mut block, &mut prop);
        if let Some(b) = block.take() {
            blocks.push(b);
        }
        PropertyFile { blocks }
    }

    /// Run the three post-conditions and, only if all pass, hand back a
    /// [`Verified`] — the **only** type with value accessors. Rule #9 says the
    /// post-condition lives *on the path*, not beside it: an unverified
    /// [`PropertyFile`] has no `charges()`/`energy()`/`geometries()` at all (not
    /// "present but warns"), so a caller physically cannot read a number without
    /// having verified it first — the same shape as ADR-010's `parse_output`,
    /// which cannot be called without the `IndexMap` from a paired `emit_input`.
    ///
    /// The **caller supplies the reference** (each job has its own `input.inp`);
    /// the reader never reads `input.inp` itself — that would be a hidden
    /// cross-module dependency.
    pub fn verify(self, reference_angstrom: &[[f64; 3]]) -> Result<Verified, ParseError> {
        self.check_geometry(reference_angstrom)?;
        self.check_charge_order()?;
        self.check_lengths()?;
        Ok(Verified(self))
    }

    /// Block names the typed layer neither interprets nor deliberately ignores —
    /// deduped, in first-seen order. Empty is the healthy case; a non-empty result
    /// means ORCA emitted something new that deserves a look (rule #10). Stays on
    /// the **unverified** handle so rule-#10 diagnostics work even when
    /// verification fails.
    pub fn unknown_block_names(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for b in &self.blocks {
            if !KNOWN_BLOCKS.contains(&b.name.as_str()) && !out.contains(&b.name) {
                out.push(b.name.clone());
            }
        }
        out
    }

    fn last_block(&self, name: &str) -> Option<&RawBlock> {
        self.blocks.iter().rev().find(|b| b.name == name)
    }

    // ---- geometries ------------------------------------------------------- //

    /// Every `$Geometry` block, Bohr → Å. **These are one per `&GeometryIndex`,
    /// i.e. one per optimization cycle — NOT scan points.** (Measured: a 6-point
    /// scan has 26 of them.) Per-scan-point data lives in `.relaxscanact.dat`, a
    /// different reader.
    fn geometries(&self) -> Result<Vec<Geometry>, ParseError> {
        self.blocks
            .iter()
            .filter(|b| b.name == "Geometry")
            .map(Self::geometry_from_block)
            .collect()
    }

    /// The first `$Geometry` (the starting geometry — what an input xyz matches).
    fn first_geometry(&self) -> Result<Geometry, ParseError> {
        self.blocks
            .iter()
            .find(|b| b.name == "Geometry")
            .ok_or_else(|| ParseError::MissingField("Geometry".into()))
            .and_then(Self::geometry_from_block)
    }

    fn geometry_from_block(b: &RawBlock) -> Result<Geometry, ParseError> {
        let prop = b
            .prop("CartesianCoordinates")
            .ok_or_else(|| ParseError::MissingField("CartesianCoordinates".into()))?;
        let rows = prop.coordinates();
        if rows.is_empty() {
            return Err(ParseError::MissingField("CartesianCoordinates rows".into()));
        }
        let bohr = matches!(prop.units.as_deref(), Some("Bohr"));
        let atoms = rows
            .into_iter()
            .map(|(element, xyz)| {
                let z = z_of(&element).unwrap_or(0);
                // rule #11: convert AT THIS boundary, by the measured unit literal.
                let to = |v: f64| {
                    if bohr {
                        Angstrom::from_bohr(v)
                    } else {
                        Angstrom::from_angstrom(v)
                    }
                };
                GeomAtom {
                    element,
                    z,
                    xyz: [to(xyz[0]), to(xyz[1]), to(xyz[2])],
                }
            })
            .collect();
        Ok(Geometry {
            geometry_index: b.geometry_index.unwrap_or(0),
            atoms,
        })
    }

    // ---- energies (Eh, no conversion) ------------------------------------- //

    /// Converged final single-point energy in Eh — the **last** `$Single_Point_Data`
    /// `&FinalEnergy` (an optimization prints one per cycle).
    fn final_single_point_energy(&self) -> Option<f64> {
        self.last_block("Single_Point_Data")?
            .prop("FinalEnergy")?
            .scalar_f64()
    }

    // ---- population analyses ---------------------------------------------- //

    /// Atomic charges from all three schemes (each `None` if absent — e.g. GOAT).
    /// Measured field names differ: Mulliken/Loewdin charge = `&AtomicCharges`,
    /// **Mayer charge = `&QA`** (its `&AtomicCharges` does not exist).
    fn charges(&self) -> Charges {
        Charges {
            mulliken: self.population(PopulationScheme::Mulliken),
            loewdin: self.population(PopulationScheme::Loewdin),
            mayer: self.population(PopulationScheme::Mayer),
        }
    }

    fn population(&self, scheme: PopulationScheme) -> Option<PopulationAnalysis> {
        let b = self.last_block(scheme.block_name())?;
        let atno = b.prop("ATNO")?.numbers().iter().map(|&z| z as u8).collect();
        let charges = b.prop(scheme.charge_field())?.numbers();
        Some(PopulationAnalysis {
            scheme,
            geometry_index: b.geometry_index.unwrap_or(0),
            atomic_numbers: atno,
            charges,
        })
    }

    // ---- dipole (a.u.) ---------------------------------------------------- //

    fn dipole(&self) -> Option<Dipole> {
        let b = self.last_block("SCF_Dipole_Moment")?;
        let magnitude_au = b.prop("dipoleMagnitude")?.scalar_f64()?;
        let total = b.prop("dipoleTotal").map(|p| p.numbers()).unwrap_or_default();
        let total_au = [
            *total.first().unwrap_or(&0.0),
            *total.get(1).unwrap_or(&0.0),
            *total.get(2).unwrap_or(&0.0),
        ];
        Some(Dipole {
            geometry_index: b.geometry_index.unwrap_or(0),
            magnitude_au,
            total_au,
        })
    }

    // ---- gradient (Eh/Bohr, bare positional, bound to its $Geometry) ------- //

    /// The **last** `$SCF_Nuc_Gradient`, or `None`. The array is bare positional;
    /// its atom order is the `$Geometry` with the same `geometry_index` — fetch it
    /// via [`PropertyFile::geometry_for`].
    fn last_gradient(&self) -> Option<Gradient> {
        let b = self.last_block("SCF_Nuc_Gradient")?;
        Some(Gradient {
            geometry_index: b.geometry_index.unwrap_or(0),
            grad_eh_per_bohr: b.prop("grad")?.numbers(),
        })
    }

    /// The `$Geometry` whose `&GeometryIndex` matches — the order source for a
    /// bare positional array (gradient). Bohr → Å.
    fn geometry_for(&self, geometry_index: u32) -> Option<Geometry> {
        self.blocks
            .iter()
            .find(|b| b.name == "Geometry" && b.geometry_index == Some(geometry_index))
            .and_then(|b| Self::geometry_from_block(b).ok())
    }

    // ---- thermochemistry (Eh) --------------------------------------------- //

    fn thermochemistry(&self) -> Option<Thermochemistry> {
        let b = self.last_block("THERMOCHEMISTRY_Energies")?;
        let f = |name: &str| b.prop(name).and_then(|p| p.scalar_f64());
        Some(Thermochemistry {
            geometry_index: b.geometry_index.unwrap_or(0),
            // `&temperature`; K by the `.out` literal "Temperature … 298.15 K".
            temperature_k: f("temperature")?,
            el_energy_eh: f("elEnergy")?,
            zpe_eh: f("zpe")?,
            inner_energy_u_eh: f("innerEnergyU")?,
            enthalpy_h_eh: f("enthalpyH")?,
            // MEASURED: entropyS == enthalpyH − freeEnergyG, i.e. T·S in Eh, NOT S.
            t_times_s_eh: f("entropyS")?,
            free_energy_g_eh: f("freeEnergyG")?,
        })
    }

    // ---- post-conditions (rule #9 + #11) ---------------------------------- //

    /// Rule #11: recompute the first geometry after conversion and assert it
    /// matches a known Å reference (the input xyz) to < 1e-4 Å. A missed Bohr→Å
    /// conversion fails here at ≈1.889×, loudly, instead of animating a plausible
    /// wrong molecule. This is an **error**, not a warning.
    fn check_geometry(&self, reference_angstrom: &[[f64; 3]]) -> Result<(), ParseError> {
        let g = self.first_geometry()?;
        verify_geometry_atoms(&g.atoms, reference_angstrom)
    }

    /// The element order of each present population block's `&ATNO` equals the
    /// first geometry's element order — else the charges would sit on the wrong
    /// atoms. Error on mismatch.
    fn check_charge_order(&self) -> Result<(), ParseError> {
        let geom = self.first_geometry()?;
        let z_geom: Vec<u8> = geom.atoms.iter().map(|a| a.z).collect();
        let ch = self.charges();
        for pop in [ch.mulliken.as_ref(), ch.loewdin.as_ref(), ch.mayer.as_ref()]
            .into_iter()
            .flatten()
        {
            if pop.atomic_numbers != z_geom {
                let index = pop
                    .atomic_numbers
                    .iter()
                    .zip(&z_geom)
                    .position(|(a, b)| a != b)
                    .unwrap_or(pop.atomic_numbers.len().min(z_geom.len()));
                return Err(ParseError::OrderMismatch {
                    block: pop.scheme.block_name().to_string(),
                    index,
                });
            }
        }
        Ok(())
    }

    /// Array lengths against N (from the first geometry), **measured not trusted**:
    /// charges = N, `&ATNO` = N, gradient = 3N, thermochemistry `&FREQ` = 3N.
    fn check_lengths(&self) -> Result<(), ParseError> {
        let n = self.first_geometry()?.atoms.len();
        let check = |field: &str, got: usize, expected: usize| {
            if got == expected {
                Ok(())
            } else {
                Err(ParseError::LengthMismatch {
                    field: field.to_string(),
                    expected,
                    got,
                })
            }
        };
        let ch = self.charges();
        for pop in [ch.mulliken.as_ref(), ch.loewdin.as_ref(), ch.mayer.as_ref()]
            .into_iter()
            .flatten()
        {
            let tag = pop.scheme.block_name();
            check(&format!("{tag}.charges"), pop.charges.len(), n)?;
            check(&format!("{tag}.ATNO"), pop.atomic_numbers.len(), n)?;
        }
        if let Some(g) = self.last_gradient() {
            check("SCF_Nuc_Gradient.grad", g.grad_eh_per_bohr.len(), 3 * n)?;
        }
        if let Some(b) = self.last_block("THERMOCHEMISTRY_Energies") {
            if let Some(freq) = b.prop("FREQ") {
                check("THERMOCHEMISTRY.FREQ", freq.numbers().len(), 3 * n)?;
            }
        }
        Ok(())
    }
}

/// A `.property.txt` whose three post-conditions passed — the **only** type that
/// exposes values. You cannot construct one except via [`PropertyFile::verify`],
/// so every number read below has been checked (geometry ↔ reference, `&ATNO`
/// order ↔ geometry, array lengths ↔ N).
#[derive(Clone, Debug)]
pub struct Verified(PropertyFile);

impl Verified {
    /// Every `$Geometry` (Bohr → Å) — one per **optimization cycle**, not scan points.
    pub fn geometries(&self) -> Result<Vec<Geometry>, ParseError> {
        self.0.geometries()
    }
    /// Converged final single-point energy, Eh.
    pub fn final_single_point_energy(&self) -> Option<f64> {
        self.0.final_single_point_energy()
    }
    /// Atomic charges from all three schemes (each `None` if the block is absent).
    pub fn charges(&self) -> Charges {
        self.0.charges()
    }
    /// Dipole (a.u.), or `None`.
    pub fn dipole(&self) -> Option<Dipole> {
        self.0.dipole()
    }
    /// The last SCF gradient (Eh/Bohr, bare positional), or `None`.
    pub fn last_gradient(&self) -> Option<Gradient> {
        self.0.last_gradient()
    }
    /// The `$Geometry` (Bohr → Å) whose `&GeometryIndex` matches — the order source
    /// for a bare positional gradient.
    pub fn geometry_for(&self, geometry_index: u32) -> Option<Geometry> {
        self.0.geometry_for(geometry_index)
    }
    /// Thermochemistry (Eh), or `None`.
    pub fn thermochemistry(&self) -> Option<Thermochemistry> {
        self.0.thermochemistry()
    }
    /// Diagnostic: blocks with no accessor (rule #10). Also on the unverified handle.
    pub fn unknown_block_names(&self) -> Vec<String> {
        self.0.unknown_block_names()
    }
}

fn verify_geometry_atoms(
    atoms: &[GeomAtom],
    reference_angstrom: &[[f64; 3]],
) -> Result<(), ParseError> {
    if atoms.len() != reference_angstrom.len() {
        return Err(ParseError::LengthMismatch {
            field: "geometry".into(),
            expected: reference_angstrom.len(),
            got: atoms.len(),
        });
    }
    let mut max_delta = 0.0_f64;
    for (a, r) in atoms.iter().zip(reference_angstrom) {
        for k in 0..3 {
            max_delta = max_delta.max((a.xyz[k].angstrom() - r[k]).abs());
        }
    }
    if max_delta >= GEOMETRY_TOL_ANGSTROM {
        return Err(ParseError::GeometryMismatch { max_delta });
    }
    Ok(())
}

/// Parse the part of a `&prop …` line after the leading `&`.
fn parse_prop_line(rest: &str) -> RawProp {
    let name: String = rest.chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
    let after_name = &rest[name.len()..];

    let (bracket, tail) = match (after_name.find('['), after_name.find(']')) {
        (Some(i), Some(j)) if j > i => (Some(&after_name[i + 1..j]), &after_name[j + 1..]),
        _ => (None, after_name),
    };

    let units = bracket.and_then(|b| UNITS_RE.captures(b)).map(|c| c[1].to_string());

    // inline scalar: the first token of the tail that is not the start of a quoted
    // trailing comment (e.g. `"No Van der Waals correction"`).
    let inline = tail
        .split_whitespace()
        .next()
        .filter(|t| !t.starts_with('"'))
        .map(|t| t.to_string());

    RawProp {
        name,
        units,
        inline,
        data_lines: Vec::new(),
    }
}

// --------------------------------------------------------------------------- //
// Layer 2 — typed values (canonical units)                                      //
// --------------------------------------------------------------------------- //

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct GeomAtom {
    pub element: String,
    pub z: u8,
    /// Canonical — built via [`Angstrom::from_bohr`] at the reader boundary.
    pub xyz: [Angstrom; 3],
}

#[derive(Clone, Debug, Serialize)]
pub struct Geometry {
    /// ORCA's `&GeometryIndex` — an **optimization cycle**, not a scan point.
    pub geometry_index: u32,
    pub atoms: Vec<GeomAtom>,
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
pub enum PopulationScheme {
    Mulliken,
    Loewdin,
    Mayer,
}

impl PopulationScheme {
    fn block_name(self) -> &'static str {
        match self {
            PopulationScheme::Mulliken => "SCF_Mulliken_Population_Analysis",
            PopulationScheme::Loewdin => "SCF_Loewdin_Population_Analysis",
            PopulationScheme::Mayer => "SCF_Mayer_Population_Analysis",
        }
    }
    /// Measured: Mulliken/Loewdin use `&AtomicCharges`; Mayer uses `&QA`.
    fn charge_field(self) -> &'static str {
        match self {
            PopulationScheme::Mayer => "QA",
            _ => "AtomicCharges",
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct PopulationAnalysis {
    pub scheme: PopulationScheme,
    pub geometry_index: u32,
    /// `&ATNO`, the block's own element labels — verified == geometry order.
    pub atomic_numbers: Vec<u8>,
    /// `&AtomicCharges` (Mulliken/Loewdin) or `&QA` (Mayer).
    pub charges: Vec<f64>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Charges {
    pub mulliken: Option<PopulationAnalysis>,
    pub loewdin: Option<PopulationAnalysis>,
    pub mayer: Option<PopulationAnalysis>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Dipole {
    pub geometry_index: u32,
    /// `&dipoleMagnitude`, measured unit literal `a.u.`.
    pub magnitude_au: f64,
    pub total_au: [f64; 3],
}

#[derive(Clone, Debug, Serialize)]
pub struct Gradient {
    pub geometry_index: u32,
    /// Eh/Bohr, length 3N, **bare positional** — order is the `$Geometry` with the
    /// same `geometry_index` (measured). Fetch it via `PropertyFile::geometry_for`.
    pub grad_eh_per_bohr: Vec<f64>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Thermochemistry {
    pub geometry_index: u32,
    /// `&temperature`, in Kelvin (`.out` literal "Temperature … K").
    pub temperature_k: f64,
    pub el_energy_eh: f64,
    pub zpe_eh: f64,
    pub inner_energy_u_eh: f64,
    pub enthalpy_h_eh: f64,
    /// `&entropyS` — **T·S in Eh, NOT the entropy S** (measured: equals
    /// `enthalpy_h_eh − free_energy_g_eh`). Named so it cannot be read as S in
    /// J/(mol·K).
    pub t_times_s_eh: f64,
    pub free_energy_g_eh: f64,
}

#[cfg(test)]
mod tests;
