//! Tests for `classify_mcp_job`: one or more per row R1–R9 and P1–P5, the precedence, and the
//! ADR-027 M1a controls.

use super::charge_mult::{ChargeMult, ChargeMultError};
use super::classify::{
    classify_mcp_job, ClassifyInput, Disposition, GrantProblem, GrantSnapshot, GrantStatus, MemoryVerdict,
    ProposedReason, RefusedReason, ResolvedSource, SourceAtom, SourceKind, SourceProblem, Target,
};
use super::header::parse_header;
use super::signature::MethodSignature;

/// The study's Opt+Freq header (ADR-027's worked example).
const STUDY: &str = "! wB97X-3c CPCM(DMF) Opt Freq\n";
/// The study's level, as Anton would paste it into a grant.
const STUDY_LEVEL: &str = "! wB97X-3c CPCM(DMF)\n";

fn atom(element: &str, x: f64, y: f64, z: f64) -> SourceAtom {
    SourceAtom { element: element.into(), x_ang: x, y_ang: y, z_ang: z }
}

/// Ethane, C2H6: 8 atoms, 18 electrons.
fn ethane() -> Vec<SourceAtom> {
    vec![
        atom("C", 0.0, 0.0, 0.7637),
        atom("C", 0.0, 0.0, -0.7637),
        atom("H", 1.0213, 0.0, 1.1567),
        atom("H", -0.5106, 0.8845, 1.1567),
        atom("H", -0.5106, -0.8845, 1.1567),
        atom("H", -1.0213, 0.0, -1.1567),
        atom("H", 0.5106, -0.8845, -1.1567),
        atom("H", 0.5106, 0.8845, -1.1567),
    ]
}

/// A converged optimization at the study level: the source every chained job starts from.
fn converged_job() -> ResolvedSource {
    ResolvedSource {
        kind: SourceKind::Job,
        label: "job:opt-1".into(),
        atoms: ethane(),
        charge_mult: Ok(ChargeMult { charge: 0, multiplicity: 1 }),
        terminal_success: Some(true),
        converged: Some(true),
        opt_signature: Some(MethodSignature::of_input(STUDY)),
    }
}

fn source_of(kind: SourceKind) -> ResolvedSource {
    let job_derived = kind.is_job_derived();
    ResolvedSource {
        kind,
        label: format!("{kind:?}"),
        terminal_success: if job_derived { Some(true) } else { None },
        converged: None,
        opt_signature: None,
        ..converged_job()
    }
}

fn grant(level: &str) -> GrantSnapshot {
    GrantSnapshot {
        status: GrantStatus::Active,
        targets: vec![Target::Local],
        levels: vec![MethodSignature::of_input(level)],
        charged_jobs: 0,
        max_jobs: 10,
        in_flight: 0,
        max_in_flight: 2,
    }
}

/// The case under test; each test changes one field from a SUBMITTED baseline.
struct Case {
    header: String,
    source: Result<ResolvedSource, String>,
    target: Target,
    memory: MemoryVerdict,
    grant: Option<GrantSnapshot>,
    open_proposals: u32,
}

impl Case {
    fn submitted() -> Case {
        Case {
            header: STUDY.into(),
            source: Ok(converged_job()),
            target: Target::Local,
            memory: MemoryVerdict::Fits,
            grant: Some(grant(STUDY_LEVEL)),
            open_proposals: 0,
        }
    }
    fn header(mut self, h: &str) -> Case {
        self.header = h.into();
        self
    }
    fn source(mut self, s: ResolvedSource) -> Case {
        self.source = Ok(s);
        self
    }
    fn run(&self) -> Disposition {
        let header = parse_header(&self.header);
        classify_mcp_job(&ClassifyInput {
            header: &header,
            source: &self.source,
            target: &self.target,
            memory: self.memory,
            grant: self.grant.as_ref(),
            open_proposals: self.open_proposals,
        })
    }
}

fn refused(d: &Disposition) -> Vec<RefusedReason> {
    match d {
        Disposition::Refused(r) => r.clone(),
        other => panic!("expected REFUSED, got {other:?}"),
    }
}

fn proposed(d: &Disposition) -> Vec<ProposedReason> {
    match d {
        Disposition::Proposed(p) => p.clone(),
        other => panic!("expected PROPOSED, got {other:?}"),
    }
}

fn rows_r(d: &Disposition) -> Vec<&'static str> {
    refused(d).iter().map(RefusedReason::row).collect()
}

fn rows_p(d: &Disposition) -> Vec<&'static str> {
    proposed(d).iter().map(ProposedReason::row).collect()
}

// --- SUBMITTED, and the precedence ---------------------------------------------------------------

#[test]
fn a_fully_allowed_dft_job_under_a_matching_active_grant_is_submitted() {
    assert_eq!(Case::submitted().run(), Disposition::Submitted);
    // The admitted blocks too.
    let h = format!("{STUDY}%geom MaxIter 300 Constraints {{C 7 C}} end end\n");
    assert_eq!(Case::submitted().header(&h).run(), Disposition::Submitted);
}

#[test]
fn the_same_job_under_an_expired_grant_is_proposed_p5() {
    let mut c = Case::submitted();
    if let Some(g) = c.grant.as_mut() {
        g.status = GrantStatus::Expired;
    }
    assert_eq!(proposed(&c.run()), vec![ProposedReason::Grant(GrantProblem::Expired)]);
}

#[test]
fn refused_rows_take_precedence_and_all_matching_reasons_are_listed() {
    // R3 + R9 together, plus a P2 block and an expired grant that must not appear.
    let mut c = Case::submitted().header("! wB97X-3c PAL16 NEB-TS\n%irc MaxIter 5 end\n");
    c.grant = None;
    assert_eq!(rows_r(&c.run()), ["R3", "R9"]);
    // Every matching P row is listed at once.
    let mut c = Case::submitted().header("! wB97X-3c def2-QZVP Freq\n%irc MaxIter 5 end\n").source(source_of(SourceKind::Smiles));
    c.memory = MemoryVerdict::Unknown;
    assert_eq!(rows_p(&c.run()), ["P1", "P2", "P3", "P4", "P5"]);
}

// --- REFUSED rows --------------------------------------------------------------------------------

#[test]
fn r1_a_line_outside_the_grammar() {
    for h in [format!("{STUDY}$new_job\n! SP\n"), format!("{STUDY}Opt\n"), format!("{STUDY}%geom MaxIter 5 end Opt\n")] {
        assert_eq!(rows_r(&Case::submitted().header(&h).run()), ["R1"], "{h:?}");
    }
}

#[test]
fn r1_reports_the_first_line_and_counts_the_rest() {
    let d = Case::submitted().header(&format!("{STUDY}foo\nbar\nbaz\n")).run();
    assert!(matches!(&refused(&d)[..], [RefusedReason::OutsideGrammar { line: 2, more: 2, .. }]));
}

#[test]
fn r2_coordinates_in_the_text() {
    let d = Case::submitted().header(&format!("{STUDY}* xyz 0 1\nH 0 0 0\nH 0 0 0.74\n*\n")).run();
    assert!(rows_r(&d).contains(&"R2"));
    let d = Case::submitted().header(&format!("{STUDY}%coords end\n")).run();
    assert_eq!(rows_r(&d), ["R2"]);
}

#[test]
fn r3_compute_shape() {
    for h in ["! wB97X-3c PAL16\n", "! wB97X-3c pal\n", "! wB97X-3c Pal0\n"] {
        assert_eq!(rows_r(&Case::submitted().header(h).run()), ["R3"], "{h:?}");
    }
    assert_eq!(rows_r(&Case::submitted().header(&format!("{STUDY}%pal nprocs 4 end\n")).run()), ["R3"]);
    // `%maxcore 3000` has no `end`, so it is R1 as well as R3.
    assert!(rows_r(&Case::submitted().header(&format!("{STUDY}%maxcore 3000\n")).run()).contains(&"R3"));
    // Nested inside an admitted block is no hiding place.
    assert!(rows_r(&Case::submitted().header(&format!("{STUDY}%geom %pal nprocs 4 end end\n")).run()).contains(&"R3"));
}

#[test]
fn r4_the_source_cannot_be_chained_from() {
    let mut c = Case::submitted();
    c.source = Err("no job abc".into());
    assert_eq!(refused(&c.run()), vec![RefusedReason::Source(SourceProblem::Unresolved("no job abc".into()))]);

    let cases: [(fn(&mut ResolvedSource), SourceProblem); 5] = [
        (|s| s.terminal_success = Some(false), SourceProblem::NotTerminalSuccess),
        (|s| s.terminal_success = None, SourceProblem::TerminalStateUnknown),
        (|s| s.converged = Some(false), SourceProblem::NotConverged),
        (|s| s.atoms.clear(), SourceProblem::NoAtoms),
        (|s| s.charge_mult = Err(ChargeMultError::NoXyzLine), SourceProblem::ChargeMultUnreadable(ChargeMultError::NoXyzLine)),
    ];
    for (break_it, problem) in cases {
        let mut s = converged_job();
        break_it(&mut s);
        assert_eq!(refused(&Case::submitted().source(s).run()), vec![RefusedReason::Source(problem.clone())], "{problem:?}");
    }
}

/// Control (ADR-027 M3 amendment, item 1, M3a; the M2b verifier's F8): an R4 reason quoting the
/// source's `* xyz` line. A malformed line whose text carries a coordinate row reaches the reason as
/// a kind and a line only, because `create_job` returns its reasons to the model and writes them to
/// the audit log. NEGATIVE CONTROL: format the error with its `Display` (which quotes the line) → red.
#[test]
fn r4_never_quotes_the_source_charge_line() {
    let text = "* xyz 0 1 C 0.12345 1.5 -2.25".to_string();
    let mut s = converged_job();
    s.charge_mult = Err(ChargeMultError::Malformed { line: 7, text: text.clone() });
    let reasons = refused(&Case::submitted().source(s).run());
    let [reason] = reasons.as_slice() else { panic!("{reasons:?}") };
    let message = reason.message();
    assert!(!message.contains("0.12345") && !message.contains(&text), "R4-QUOTE: the R4 reason quotes the source line: {message}");
    assert!(message.contains("line 7"), "the line is named: {message}");
}

#[test]
fn r4_does_not_ask_a_molecule_for_a_terminal_state() {
    let d = Case::submitted().source(source_of(SourceKind::Molecule)).run();
    assert_eq!(rows_p(&d), ["P1"]);
}

#[test]
fn r5_parity_impossible_or_unknown_element() {
    for (charge, multiplicity) in [(0, 2), (0, 0), (1, 1), (0, -1), (18, 3), (19, 1)] {
        let mut s = converged_job();
        s.charge_mult = Ok(ChargeMult { charge, multiplicity });
        assert_eq!(rows_r(&Case::submitted().source(s).run()), ["R5"], "charge {charge}, mult {multiplicity}");
    }
    // Allowed: the cation doublet and the neutral triplet.
    for (charge, multiplicity) in [(1, 2), (0, 3)] {
        let mut s = converged_job();
        s.charge_mult = Ok(ChargeMult { charge, multiplicity });
        assert!(!matches!(Case::submitted().source(s).run(), Disposition::Refused(_)));
    }
    // Case-normalised symbols count; an unknown symbol is refused.
    let mut s = converged_job();
    s.atoms[0].element = "c".into();
    assert_eq!(Case::submitted().source(s).run(), Disposition::Submitted);
    let mut s = converged_job();
    s.atoms[0].element = "Xx".into();
    assert_eq!(refused(&Case::submitted().source(s).run()), vec![RefusedReason::UnknownElement { symbol: "Xx".into() }]);
}

#[test]
fn r6_an_out_of_range_constraint_index() {
    // Ethane has 8 atoms: index 8 is out of range (0-based), 7 is the last one.
    let d = Case::submitted().header(&format!("{STUDY}%geom Constraints {{C 8 C}} end end\n")).run();
    assert_eq!(
        refused(&d),
        vec![RefusedReason::ConstraintIndex { entry: "{C 8 C}".into(), bad: vec!["8".into()], atom_count: 8 }]
    );
    // Every entry is checked, including one P2 would only propose.
    let d = Case::submitted().header(&format!("{STUDY}%geom Constraints {{B 3 17 C}} end end\n")).run();
    assert_eq!(rows_r(&d), ["R6"]);
    let d = Case::submitted().header(&format!("{STUDY}%geom Constraints {{C -1 C}} end end\n")).run();
    assert_eq!(rows_r(&d), ["R6"]);
}

#[test]
fn r7_a_scan_without_opt() {
    let scan = "%geom Scan B 0 1 = 1.4, 2.4, 6 end end\n";
    let d = Case::submitted().header(&format!("! wB97X-3c\n{scan}")).run();
    assert_eq!(rows_r(&d), ["R7"]);
    // Not an Opt-family token (C4): NORMALOPT does not count.
    let d = Case::submitted().header(&format!("! wB97X-3c NormalOpt\n{scan}")).run();
    assert_eq!(rows_r(&d), ["R7"]);
    // With Opt it is not refused; the Scan is still P2 (Anton runs it).
    let d = Case::submitted().header(&format!("! wB97X-3c TightOpt\n{scan}")).run();
    assert_eq!(rows_p(&d), ["P2", "P5"]);
}

#[test]
fn r8_too_many_open_proposals() {
    let mut c = Case::submitted();
    c.open_proposals = 50;
    assert_eq!(refused(&c.run()), vec![RefusedReason::TooManyProposals { open: 50 }]);
    c.open_proposals = 49;
    assert_eq!(c.run(), Disposition::Submitted);
}

#[test]
fn r9_a_neb_family_token() {
    for t in ["NEB-TS", "neb", "NEB-CI"] {
        let d = Case::submitted().header(&format!("! wB97X-3c {t}\n")).run();
        assert_eq!(refused(&d), vec![RefusedReason::Neb { token: t.into() }]);
    }
}

// --- PROPOSED rows -------------------------------------------------------------------------------

#[test]
fn p1_a_smiles_or_molecule_source() {
    let d = Case::submitted().source(source_of(SourceKind::Smiles)).run();
    let p = proposed(&d);
    assert_eq!(p, vec![ProposedReason::NewSpecies { kind: SourceKind::Smiles, charge: 0, multiplicity: 1 }]);
    assert!(p[0].message().contains("always gives multiplicity 1"));
    assert_eq!(rows_p(&Case::submitted().source(source_of(SourceKind::Molecule)).run()), ["P1"]);
}

#[test]
fn p2_a_block_or_key_outside_the_allowlist() {
    let d = Case::submitted().header(&format!("{STUDY}%irc MaxIter 5 end\n")).run();
    assert_eq!(rows_p(&d), ["P2"]);
    let d = Case::submitted().header(&format!("{STUDY}%geom Constraints {{B 3 7 C}} end end\n")).run();
    assert_eq!(rows_p(&d), ["P2"]);
}

#[test]
fn p3_freq_from_a_scan_frame() {
    let d = Case::submitted().header("! wB97X-3c CPCM(DMF) Freq\n").source(source_of(SourceKind::ScanFrame)).run();
    assert_eq!(rows_p(&d), ["P3"]);
}

#[test]
fn p3_needs_a_converged_optimization_at_the_same_signature() {
    let freq = "! wB97X-3c CPCM(DMF) Freq\n";
    // The converged Opt at the study level: no P3.
    assert_eq!(Case::submitted().header(freq).run(), Disposition::Submitted);
    // Every other source of a frequency job is P3.
    for kind in [SourceKind::NebImage, SourceKind::GoatMember] {
        assert_eq!(rows_p(&Case::submitted().header(freq).source(source_of(kind)).run()), ["P3"], "{kind:?}");
    }
    let mut other_level = converged_job();
    other_level.opt_signature = Some(MethodSignature::of_input("! r2SCAN-3c Opt\n"));
    assert_eq!(rows_p(&Case::submitted().header(freq).source(other_level).run()), ["P3"]);
    let mut single_point = converged_job();
    (single_point.converged, single_point.opt_signature) = (None, None);
    assert_eq!(rows_p(&Case::submitted().header("! wB97X-3c CPCM(DMF) NumFreq\n").source(single_point).run()), ["P3"]);
    // Opt+Freq from a scan frame is an optimization, not a frequency job: no P3.
    assert_eq!(Case::submitted().source(source_of(SourceKind::ScanFrame)).run(), Disposition::Submitted);
}

#[test]
fn p4_memory_warns_or_is_unknown() {
    let mut c = Case::submitted();
    c.memory = MemoryVerdict::Warns;
    assert_eq!(proposed(&c.run()), vec![ProposedReason::Memory(MemoryVerdict::Warns)]);
    c.memory = MemoryVerdict::Unknown;
    let p = proposed(&c.run());
    assert_eq!(p, vec![ProposedReason::Memory(MemoryVerdict::Unknown)]);
    // The target here is Local: the message must read for this machine too (M1a Open item; the
    // old text said "server memory unknown" for a Local job).
    let m = p[0].message();
    assert!(m.contains("this machine") && m.contains("a server"), "{m}");
}

/// R5 with checked arithmetic (the M1a verifier's LOW finding): a charge or multiplicity that makes
/// the electron count overflow is R5, never a wrapped count. NEGATIVE CONTROL: restore `z_sum -
/// cm.charge` (or `electrons + m - 1`) and this panics on overflow in the test build → red.
#[test]
fn r5_an_extreme_charge_or_multiplicity_is_refused_not_wrapped() {
    // Each pair overflows a different step: Σ Z − charge (the first two and the last) or
    // electrons + (multiplicity − 1) (the third).
    for (charge, multiplicity) in [(i64::MIN, 1), (i64::MIN + 1, 1), (0, i64::MAX), (-(i64::MAX - 10), 2)] {
        let src = ResolvedSource { charge_mult: Ok(ChargeMult { charge, multiplicity }), ..converged_job() };
        let r = refused(&Case::submitted().source(src).run());
        assert_eq!(r, vec![RefusedReason::ChargeMultOutOfRange { charge, multiplicity }], "({charge}, {multiplicity})");
        assert_eq!(r[0].row(), "R5");
    }
}

#[test]
fn p5_every_way_the_grant_fails_to_cover_the_job() {
    let mut c = Case::submitted();
    c.grant = None;
    assert_eq!(proposed(&c.run()), vec![ProposedReason::Grant(GrantProblem::NoGrant)]);

    let mut c = Case::submitted();
    c.grant = Some(GrantSnapshot {
        status: GrantStatus::Revoked,
        targets: vec![Target::Server { profile_id: "uni".into() }],
        levels: vec![MethodSignature::of_input("! r2SCAN-3c\n")],
        charged_jobs: 10,
        max_jobs: 10,
        in_flight: 2,
        max_in_flight: 2,
    });
    assert_eq!(
        proposed(&c.run()),
        vec![
            ProposedReason::Grant(GrantProblem::Revoked),
            ProposedReason::Grant(GrantProblem::TargetNotGranted),
            ProposedReason::Grant(GrantProblem::LevelNotGranted { signature: "CPCM(DMF) WB97X-3C".into() }),
            ProposedReason::Grant(GrantProblem::MaxJobsUsed { charged: 10, max: 10 }),
            ProposedReason::Grant(GrantProblem::MaxInFlightReached { in_flight: 2, max: 2 }),
        ]
    );
}

#[test]
fn a_foreign_basis_on_the_granted_composite_is_proposed_never_submitted() {
    // ORCA's keyword is `wB97X-3c`; the ω spelling is refused outright (a non-ASCII `!` line, R1).
    let mut c = Case::submitted().header("! wB97X-3c def2-QZVP Opt Freq\n");
    c.grant = Some(grant("! wB97X-3c\n"));
    c.source = Ok(ResolvedSource { opt_signature: Some(MethodSignature::of_input("! wB97X-3c Opt\n")), ..converged_job() });
    let p = proposed(&c.run());
    assert!(matches!(&p[..], [ProposedReason::Grant(GrantProblem::LevelNotGranted { .. })]), "{p:?}");
    // The control: the same composite without the foreign basis is SUBMITTED.
    c.header = "! wB97X-3c Opt Freq\n".into();
    assert_eq!(c.run(), Disposition::Submitted);
}

#[test]
fn every_reason_has_a_row_and_a_message() {
    let d = Case::submitted().header("! wB97X-3c PAL16 NEB-TS\n$new_job\n%coords end\n").run();
    for r in refused(&d) {
        assert!(r.row().starts_with('R') && !r.message().is_empty(), "{r:?}");
    }
}

// --- Verifier round 1 (FAIL: HIGH-1, HIGH-2; MED; LOW-1, LOW-2) ----------------------------------

#[test]
fn r6_an_index_that_does_not_parse_to_u32_is_refused_not_dropped() {
    for entry in ["{C 99999999999999999999 C}", "{C 9223372036854775808 C}", "{C 4294967296 C}", "{B 0 +1 C}"] {
        let d = Case::submitted().header(&format!("{STUDY}%geom Constraints {entry} end end\n")).run();
        assert_eq!(rows_r(&d), ["R6"], "{entry}");
    }
}

#[test]
fn r1_a_second_hash_is_never_read_as_closing_or_continuing_a_comment() {
    for h in [
        "! wB97X-3c CPCM(DMF) Opt Freq #x# PAL16\n".to_string(),
        "! wB97X-3c CPCM(DMF) Opt Freq #x# def2-QZVP\n".to_string(),
        format!("{STUDY}%geom MaxIter 5 #x# Recalc_Hess 1\nend\n"),
        format!("{STUDY}# x # %maxcore 99999\n"),
    ] {
        let d = Case::submitted().header(&h).run();
        assert!(rows_r(&d).contains(&"R1"), "{h:?}: {d:?}");
    }
}

#[test]
fn r1_a_non_ascii_bang_line() {
    for h in ["! wB97X-3c CPCM(DMF) Opt Freq ſp\n", "! ωB97X-3c CPCM(DMF) Opt Freq\n"] {
        assert_eq!(rows_r(&Case::submitted().header(h).run()), ["R1"], "{h:?}");
    }
}

#[test]
fn p5_an_empty_method_signature_is_never_submitted() {
    // Even under a grant whose only level is itself empty.
    let mut c = Case::submitted().header("! Opt Freq\n");
    c.grant = Some(grant("! Opt\n"));
    let p = proposed(&c.run());
    assert_eq!(p, vec![ProposedReason::Grant(GrantProblem::NoMethodLevel)]);
    assert_eq!(p[0].message(), "no method level stated");
}

#[test]
fn p2_unrecorded_layouts_are_never_submitted() {
    // R6 still range-checks a leading-zero index by value: 007 is in range for ethane's 8 atoms.
    let d = Case::submitted().header(&format!("{STUDY}%geom Constraints {{C 007 C}} end end\n")).run();
    assert_eq!(rows_p(&d), ["P2"]);
    let d = Case::submitted().header(&format!("{STUDY}%geom Constraints {{C 008 C}} end end\n")).run();
    assert_eq!(rows_r(&d), ["R6"]);
    // The verifier's repro: Calc_Hess with its value on the next line.
    let d = Case::submitted().header(&format!("{STUDY}%geom\n  Calc_Hess\n  true\nend\n")).run();
    assert_eq!(rows_p(&d), ["P2"]);
}
