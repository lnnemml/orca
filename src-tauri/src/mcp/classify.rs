//! `classify_mcp_job` (ADR-027 decision 3): every `create_job` ends in exactly one of REFUSED,
//! PROPOSED or SUBMITTED, chosen here from plain data. **Precedence:** the REFUSED rows R1–R9 are
//! checked first, and any match gives `Refused` with every matching R reason; then the PROPOSED
//! rows P1–P5, and any match gives `Proposed` with every matching P reason; else `Submitted`.
//!
//! **It fails closed.** A grammar the parser does not recognise is R1; a key it does not know is
//! P2; a source whose terminal state is unknown is R4; a grant that cannot be shown to cover the
//! job is P5. Nothing reaches `Submitted` by default.

use serde::Serialize;

use super::allowlist::{allowlist_findings, constraint_entries, parse_index, P2Finding};
use super::charge_mult::{ChargeMult, ChargeMultError};
use super::header::{BlockItem, ParsedHeader, Violation};
use super::signature::{has_keyword, is_pal_token, method_signature, MethodSignature};
use crate::parse::elements::z_of;

/// R8's cap: open MCP proposals beyond which `create_job` is refused.
pub const MAX_OPEN_PROPOSALS: u32 = 50;

/// The Opt-family tokens (C4): exactly `RELAXED_SCAN_OPT_KEYWORDS` of `src/scene/scan.ts`, the
/// keywords a run measured to drive a relaxed scan. R7 needs one for a scan; P3 treats a job with
/// one as an optimization, not a frequency job. Anything else (NORMALOPT, COPT, OPTH …) is not Opt.
pub const OPT_FAMILY: [&str; 5] = ["opt", "optts", "tightopt", "verytightopt", "looseopt"];

/// The frequency tokens of P3. ADR-027 names `Freq` and `NumFreq`; `AnFreq` is added because it is
/// a frequency calculation too and adding it can only send a job to Anton.
pub const FREQ_TYPE: [&str; 3] = ["freq", "numfreq", "anfreq"];

/// The NEB-family tokens of R9 (C11).
pub const NEB_FAMILY: [&str; 3] = ["neb", "neb-ts", "neb-ci"];

/// Where the geometry comes from (ADR-027 decision 4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum SourceKind {
    Job,
    ScanFrame,
    NebImage,
    GoatMember,
    Molecule,
    Smiles,
}

impl SourceKind {
    /// True for a geometry taken from a job's results (the job's terminal state matters).
    pub fn is_job_derived(self) -> bool {
        matches!(self, SourceKind::Job | SourceKind::ScanFrame | SourceKind::NebImage | SourceKind::GoatMember)
    }
}

/// One source atom. Coordinates in Å (rule #11).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SourceAtom {
    pub element: String,
    pub x_ang: f64,
    pub y_ang: f64,
    pub z_ang: f64,
}

/// A geometry source the resolver (M1b) has read.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedSource {
    pub kind: SourceKind,
    /// The reference as the model wrote it, e.g. `job:<id>/scan:3`.
    pub label: String,
    pub atoms: Vec<SourceAtom>,
    /// From the source job's `* xyz` line (C1), a molecule's columns, or the sidecar.
    pub charge_mult: Result<ChargeMult, ChargeMultError>,
    /// For a job-derived source: did the job end successfully? `None` = not a job, or unknown.
    pub terminal_success: Option<bool>,
    /// The optimization verdict (`results.rs` `converged`): `None` = not an optimization.
    pub converged: Option<bool>,
    /// The method signature of the source job, when it was an optimization.
    pub opt_signature: Option<MethodSignature>,
}

/// Where the job is to run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum Target {
    Local,
    Server { profile_id: String },
}

/// The memory preflight's verdict for the target after the app's `%maxcore` (C8), computed by M1b
/// before step A.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum MemoryVerdict {
    Fits,
    /// The RAM or the disk check warns.
    Warns,
    /// The target's memory facts are unknown.
    Unknown,
}

/// The state of the newest grant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum GrantStatus {
    Active,
    Expired,
    Revoked,
}

/// The grant and its counters, read in step A (ADR-027 decision 5).
#[derive(Debug, Clone, PartialEq)]
pub struct GrantSnapshot {
    pub status: GrantStatus,
    pub targets: Vec<Target>,
    pub levels: Vec<MethodSignature>,
    pub charged_jobs: u32,
    pub max_jobs: u32,
    /// Non-terminal jobs with a non-null `grant_id`, under any grant.
    pub in_flight: u32,
    pub max_in_flight: u32,
}

/// Everything the classifier reads. Plain data; no I/O.
#[derive(Debug, Clone)]
pub struct ClassifyInput<'a> {
    pub header: &'a ParsedHeader,
    /// `Err` holds why the reference did not resolve.
    pub source: &'a Result<ResolvedSource, String>,
    pub target: &'a Target,
    pub memory: MemoryVerdict,
    /// `None` when no grant exists at all.
    pub grant: Option<&'a GrantSnapshot>,
    pub open_proposals: u32,
}

/// Why the source cannot be chained from (R4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum SourceProblem {
    Unresolved(String),
    NotTerminalSuccess,
    TerminalStateUnknown,
    NotConverged,
    NoAtoms,
    ChargeMultUnreadable(ChargeMultError),
}

/// A REFUSED reason, one variant per row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum RefusedReason {
    /// R1: lines outside the closed grammar; the first is quoted, `more` counts the rest.
    OutsideGrammar { line: usize, excerpt: String, why: String, more: usize },
    /// R2: coordinates in the text (a `*` line or a `%coords` block).
    Coordinates { line: usize, excerpt: String },
    /// R3: compute shape set by the model (`%pal`, `%maxcore`, `! PALn`).
    ComputeShape { what: String },
    /// R4: the source cannot be chained from.
    Source(SourceProblem),
    /// R5: an element the parity check cannot count.
    UnknownElement { symbol: String },
    /// R5: charge and multiplicity that no electron count allows.
    ParityImpossible { electrons: i64, multiplicity: i64 },
    /// R5: a charge or multiplicity so large that the electron count overflows (checked
    /// arithmetic; a wrapped count could pass the parity test by accident).
    ChargeMultOutOfRange { charge: i64, multiplicity: i64 },
    /// R6: a constraint atom index that is not an integer in `[0, atom_count)`; `bad` holds the
    /// tokens as written (an unreadable one included, never dropped).
    ConstraintIndex { entry: String, bad: Vec<String>, atom_count: usize },
    /// R7: a `%geom Scan` without an Opt-family token.
    ScanWithoutOpt,
    /// R8: too many open proposals.
    TooManyProposals { open: u32 },
    /// R9: a NEB-family token.
    Neb { token: String },
    /// Not a row: the assembly post-condition failed (rule #9), an internal error.
    Internal { detail: String },
}

impl RefusedReason {
    /// The ADR row, e.g. `"R1"`.
    pub fn row(&self) -> &'static str {
        match self {
            RefusedReason::OutsideGrammar { .. } => "R1",
            RefusedReason::Coordinates { .. } => "R2",
            RefusedReason::ComputeShape { .. } => "R3",
            RefusedReason::Source(_) => "R4",
            RefusedReason::UnknownElement { .. }
            | RefusedReason::ParityImpossible { .. }
            | RefusedReason::ChargeMultOutOfRange { .. } => "R5",
            RefusedReason::ConstraintIndex { .. } => "R6",
            RefusedReason::ScanWithoutOpt => "R7",
            RefusedReason::TooManyProposals { .. } => "R8",
            RefusedReason::Neb { .. } => "R9",
            RefusedReason::Internal { .. } => "internal",
        }
    }

    /// The reason for a human (and the model).
    pub fn message(&self) -> String {
        match self {
            RefusedReason::OutsideGrammar { line, excerpt, why, more } => {
                let others = if *more > 0 { format!(" (and {more} more)") } else { String::new() };
                format!("line {line} is outside the header grammar: {why}: `{excerpt}`{others}")
            }
            RefusedReason::Coordinates { line, excerpt } => format!(
                "line {line} carries coordinates (`{excerpt}`); geometry comes only from the source reference"
            ),
            RefusedReason::ComputeShape { what } => {
                format!("{what} sets the compute shape, which belongs to the app")
            }
            RefusedReason::Source(p) => match p {
                SourceProblem::Unresolved(why) => format!("the source does not resolve: {why}"),
                SourceProblem::NotTerminalSuccess => "the source job did not finish successfully".into(),
                SourceProblem::TerminalStateUnknown => "the source job's final state is unknown".into(),
                SourceProblem::NotConverged => "the source optimization did not converge".into(),
                SourceProblem::NoAtoms => "the source has no atoms".into(),
                SourceProblem::ChargeMultUnreadable(e) => format!("source charge/multiplicity unreadable: {e}"),
            },
            RefusedReason::UnknownElement { symbol } => format!("unknown element `{symbol}` in the source"),
            RefusedReason::ParityImpossible { electrons, multiplicity } => format!(
                "multiplicity {multiplicity} is impossible for {electrons} electrons"
            ),
            RefusedReason::ChargeMultOutOfRange { charge, multiplicity } => format!(
                "charge {charge} and multiplicity {multiplicity} are outside any electron count"
            ),
            RefusedReason::ConstraintIndex { entry, bad, atom_count } => format!(
                "constraint `{entry}` names atom index {}, not an integer in 0..{atom_count} (0-based)",
                bad.join(", ")
            ),
            RefusedReason::ScanWithoutOpt => {
                "a %geom Scan needs an Opt-family keyword; without one ORCA runs a single point".into()
            }
            RefusedReason::TooManyProposals { open } => {
                format!("{open} proposals are already waiting for Anton (the cap is {MAX_OPEN_PROPOSALS})")
            }
            RefusedReason::Neb { token } => format!(
                "`{token}` needs two endpoints and this tool takes one geometry source; Anton runs a NEB from the app"
            ),
            RefusedReason::Internal { detail } => format!("internal error: {detail}"),
        }
    }
}

/// Why the grant does not cover the job (P5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum GrantProblem {
    NoGrant,
    /// The header names no method token (e.g. `! Opt Freq`): no level to match, whatever the grant.
    NoMethodLevel,
    Expired,
    Revoked,
    TargetNotGranted,
    LevelNotGranted { signature: String },
    MaxJobsUsed { charged: u32, max: u32 },
    MaxInFlightReached { in_flight: u32, max: u32 },
}

/// A PROPOSED reason, one variant per row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum ProposedReason {
    /// P1: a new species; Anton confirms its charge and multiplicity by running the draft.
    NewSpecies { kind: SourceKind, charge: i64, multiplicity: i64 },
    /// P2: outside the block-key allowlist.
    NotAllowlisted(P2Finding),
    /// P3: a frequency job whose source is not a converged optimization at the same signature.
    FreqWithoutConvergedOpt,
    /// P4: the memory preflight warns, or the target's memory is unknown.
    Memory(MemoryVerdict),
    /// P5: the grant does not cover the job.
    Grant(GrantProblem),
}

impl ProposedReason {
    /// The ADR row, e.g. `"P1"`.
    pub fn row(&self) -> &'static str {
        match self {
            ProposedReason::NewSpecies { .. } => "P1",
            ProposedReason::NotAllowlisted(_) => "P2",
            ProposedReason::FreqWithoutConvergedOpt => "P3",
            ProposedReason::Memory(_) => "P4",
            ProposedReason::Grant(_) => "P5",
        }
    }

    /// The reason for a human (and the model).
    pub fn message(&self) -> String {
        match self {
            ProposedReason::NewSpecies { kind, charge, multiplicity } => {
                let from = match kind {
                    SourceKind::Smiles => "the SMILES sidecar, which always gives multiplicity 1",
                    _ => "the molecule's stored columns",
                };
                format!("a new species: charge {charge}, multiplicity {multiplicity} (from {from}); Anton confirms by running it")
            }
            ProposedReason::NotAllowlisted(f) => format!("{}: {}", f.block, f.detail),
            ProposedReason::FreqWithoutConvergedOpt => {
                "a frequency job needs a converged optimization at the same method signature as its source".into()
            }
            // Read for either target (M1a Open item): a server's facts come from its connection
            // test, this machine's from /proc/meminfo and the data directory, read now.
            ProposedReason::Memory(MemoryVerdict::Unknown) => "the target's memory or free disk is unknown \
                (a server: re-run the connection test; this machine: /proc/meminfo or the data directory \
                could not be read)"
                .into(),
            ProposedReason::Memory(_) => "the memory preflight warns for this target (RAM or free disk)".into(),
            ProposedReason::Grant(g) => match g {
                GrantProblem::NoGrant => "there is no grant".into(),
                GrantProblem::NoMethodLevel => "no method level stated".into(),
                GrantProblem::Expired => "the grant has expired".into(),
                GrantProblem::Revoked => "the grant was revoked".into(),
                GrantProblem::TargetNotGranted => "the target is not in the grant".into(),
                GrantProblem::LevelNotGranted { signature } => {
                    format!("the method signature `{signature}` is not one of the grant's levels")
                }
                GrantProblem::MaxJobsUsed { charged, max } => format!("the grant's jobs are used up ({charged} of {max})"),
                GrantProblem::MaxInFlightReached { in_flight, max } => {
                    format!("{in_flight} granted jobs are in flight (the cap is {max})")
                }
            },
        }
    }
}

/// The outcome of `create_job`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum Disposition {
    Refused(Vec<RefusedReason>),
    Proposed(Vec<ProposedReason>),
    Submitted,
}

/// An element symbol with its case normalised (C6): first letter upper, the rest lower.
pub(crate) fn normalise_element(symbol: &str) -> String {
    let mut cs = symbol.chars();
    match cs.next() {
        Some(f) => f.to_ascii_uppercase().to_string() + &cs.as_str().to_ascii_lowercase(),
        None => String::new(),
    }
}

/// R1 and R2 from the parser's violations, plus R2/R3 from block names and R3/R9 from keywords.
fn header_rows(header: &ParsedHeader, out: &mut Vec<RefusedReason>) {
    let mut outside: Vec<(usize, &str, &str)> = Vec::new();
    for v in &header.violations {
        match v {
            Violation::OutsideGrammar { line, excerpt, why } => outside.push((*line, excerpt, why)),
            Violation::Coordinates { line, excerpt } => {
                out.push(RefusedReason::Coordinates { line: *line, excerpt: excerpt.clone() });
            }
        }
    }
    if let Some((line, excerpt, why)) = outside.first() {
        out.push(RefusedReason::OutsideGrammar {
            line: *line,
            excerpt: excerpt.to_string(),
            why: why.to_string(),
            more: outside.len() - 1,
        });
    }
    for (line, name) in header.all_block_names() {
        match name {
            "coords" => out.push(RefusedReason::Coordinates { line, excerpt: "%coords".into() }),
            "pal" | "maxcore" => out.push(RefusedReason::ComputeShape { what: format!("%{name}") }),
            _ => {}
        }
    }
    for k in header.keywords() {
        if is_pal_token(k) {
            out.push(RefusedReason::ComputeShape { what: format!("`! {k}`") });
        }
        if NEB_FAMILY.iter().any(|n| k.eq_ignore_ascii_case(n)) {
            out.push(RefusedReason::Neb { token: k.to_string() });
        }
    }
}

/// R4, R5 and R6 from the source.
fn source_rows(header: &ParsedHeader, source: &Result<ResolvedSource, String>, out: &mut Vec<RefusedReason>) {
    let src = match source {
        Ok(s) => s,
        Err(why) => {
            out.push(RefusedReason::Source(SourceProblem::Unresolved(why.clone())));
            return;
        }
    };
    if src.terminal_success == Some(false) {
        out.push(RefusedReason::Source(SourceProblem::NotTerminalSuccess));
    } else if src.kind.is_job_derived() && src.terminal_success.is_none() {
        out.push(RefusedReason::Source(SourceProblem::TerminalStateUnknown));
    }
    if src.converged == Some(false) {
        out.push(RefusedReason::Source(SourceProblem::NotConverged));
    }
    if src.atoms.is_empty() {
        out.push(RefusedReason::Source(SourceProblem::NoAtoms));
    }

    // R5 (C6): electrons = Σ Z − charge; impossible if the parity is wrong, the multiplicity is
    // below 1, or there are more unpaired electrons than electrons.
    // Checked arithmetic throughout: a wrapped count could pass the parity test by accident, and an
    // overflow is itself R5 (the M1a verifier's LOW finding).
    let mut z_sum: Option<i64> = Some(0);
    let mut elements_known = true;
    for a in &src.atoms {
        match z_of(&normalise_element(&a.element)) {
            Some(z) => z_sum = z_sum.and_then(|s| s.checked_add(i64::from(z))),
            None => {
                elements_known = false;
                out.push(RefusedReason::UnknownElement { symbol: a.element.clone() });
            }
        }
    }
    match &src.charge_mult {
        Err(e) => out.push(RefusedReason::Source(SourceProblem::ChargeMultUnreadable(e.clone()))),
        Ok(cm) if elements_known && !src.atoms.is_empty() => {
            let m = cm.multiplicity;
            let electrons = z_sum.and_then(|s| s.checked_sub(cm.charge));
            // m ≥ 1 is checked first, so `m - 1` cannot overflow.
            let unpaired = if m < 1 { None } else { Some(m - 1) };
            match (electrons, unpaired) {
                (None, _) => out.push(RefusedReason::ChargeMultOutOfRange { charge: cm.charge, multiplicity: m }),
                (Some(electrons), None) => out.push(RefusedReason::ParityImpossible { electrons, multiplicity: m }),
                (Some(electrons), Some(unpaired)) => match electrons.checked_add(unpaired) {
                    None => out.push(RefusedReason::ChargeMultOutOfRange { charge: cm.charge, multiplicity: m }),
                    Some(sum) if electrons < 0 || sum % 2 != 0 || unpaired > electrons => {
                        out.push(RefusedReason::ParityImpossible { electrons, multiplicity: m });
                    }
                    Some(_) => {}
                },
            }
        }
        Ok(_) => {}
    }

    // R6 (C10): every constraint entry's indices, whatever its type, against the source's count.
    let n = src.atoms.len();
    for entry in constraint_entries(header) {
        let bad: Vec<String> = entry
            .index_tokens
            .into_iter()
            .filter(|t| parse_index(t).is_none_or(|i| i as usize >= n))
            .collect();
        if !bad.is_empty() {
            out.push(RefusedReason::ConstraintIndex { entry: entry.text, bad, atom_count: n });
        }
    }
}

/// True when the header has a `%geom` block with a `Scan` sub-block.
fn has_scan(header: &ParsedHeader) -> bool {
    header.blocks.iter().filter(|b| b.name == "geom").any(|b| {
        b.items.iter().any(|i| matches!(i, BlockItem::SubBlock(s) if s.name == "scan"))
    })
}

/// P5: every way the grant fails to cover the job.
fn grant_rows(grant: Option<&GrantSnapshot>, target: &Target, sig: &MethodSignature, out: &mut Vec<ProposedReason>) {
    // An empty signature never matches a level, even an empty one a grant might hold.
    if sig.keywords.is_empty() {
        out.push(ProposedReason::Grant(GrantProblem::NoMethodLevel));
    }
    let Some(g) = grant else {
        out.push(ProposedReason::Grant(GrantProblem::NoGrant));
        return;
    };
    match g.status {
        GrantStatus::Active => {}
        GrantStatus::Expired => out.push(ProposedReason::Grant(GrantProblem::Expired)),
        GrantStatus::Revoked => out.push(ProposedReason::Grant(GrantProblem::Revoked)),
    }
    if !g.targets.contains(target) {
        out.push(ProposedReason::Grant(GrantProblem::TargetNotGranted));
    }
    if !g.levels.contains(sig) {
        out.push(ProposedReason::Grant(GrantProblem::LevelNotGranted { signature: sig.display() }));
    }
    if g.charged_jobs >= g.max_jobs {
        out.push(ProposedReason::Grant(GrantProblem::MaxJobsUsed { charged: g.charged_jobs, max: g.max_jobs }));
    }
    if g.in_flight >= g.max_in_flight {
        out.push(ProposedReason::Grant(GrantProblem::MaxInFlightReached { in_flight: g.in_flight, max: g.max_in_flight }));
    }
}

/// P5 alone: every way `grant` fails to cover a job for `target` at `sig`. Step A
/// (`steps::charge_and_insert_conn`) re-runs exactly these rows on the grant it re-reads inside its
/// `BEGIN IMMEDIATE` transaction, so the classifier and step A can never disagree on what covers.
pub(crate) fn grant_problems(grant: Option<&GrantSnapshot>, target: &Target, sig: &MethodSignature) -> Vec<ProposedReason> {
    let mut out = Vec::new();
    grant_rows(grant, target, sig, &mut out);
    out
}

/// Classify one `create_job` call (ADR-027 decision 3).
pub fn classify_mcp_job(input: &ClassifyInput) -> Disposition {
    let header = input.header;
    let has_opt = has_keyword(header, &OPT_FAMILY);

    // --- REFUSED: R1–R9 ---
    let mut refused = Vec::new();
    header_rows(header, &mut refused); // R1, R2, R3, R9
    source_rows(header, input.source, &mut refused); // R4, R5, R6
    if has_scan(header) && !has_opt {
        refused.push(RefusedReason::ScanWithoutOpt); // R7
    }
    if input.open_proposals >= MAX_OPEN_PROPOSALS {
        refused.push(RefusedReason::TooManyProposals { open: input.open_proposals }); // R8
    }
    if !refused.is_empty() {
        return Disposition::Refused(refused);
    }
    // R4 passed, so the source resolved; this `Err` arm is unreachable but fails closed.
    let src = match input.source {
        Ok(s) => s,
        Err(why) => return Disposition::Refused(vec![RefusedReason::Source(SourceProblem::Unresolved(why.clone()))]),
    };

    // --- PROPOSED: P1–P5 ---
    let mut proposed = Vec::new();
    if matches!(src.kind, SourceKind::Molecule | SourceKind::Smiles) {
        // R4 passed, so charge_mult is Ok.
        if let Ok(cm) = &src.charge_mult {
            proposed.push(ProposedReason::NewSpecies { kind: src.kind, charge: cm.charge, multiplicity: cm.multiplicity });
        }
    }
    proposed.extend(allowlist_findings(header).into_iter().map(ProposedReason::NotAllowlisted));
    let sig = method_signature(header);
    let freq_type = has_keyword(header, &FREQ_TYPE) && !has_opt;
    let converged_opt_at_same_level =
        src.kind == SourceKind::Job && src.converged == Some(true) && src.opt_signature.as_ref() == Some(&sig);
    if freq_type && !converged_opt_at_same_level {
        proposed.push(ProposedReason::FreqWithoutConvergedOpt);
    }
    if input.memory != MemoryVerdict::Fits {
        proposed.push(ProposedReason::Memory(input.memory));
    }
    grant_rows(input.grant, input.target, &sig, &mut proposed);
    if !proposed.is_empty() {
        return Disposition::Proposed(proposed);
    }
    Disposition::Submitted
}
