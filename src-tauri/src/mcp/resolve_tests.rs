//! Tests for the source resolver (ADR-027 decision 4; M1b controls).

use std::path::PathBuf;
use std::sync::Mutex;

use super::charge_mult::ChargeMult;
use super::classify::{
    classify_mcp_job, ClassifyInput, Disposition, GrantSnapshot, GrantStatus, MemoryVerdict, ProposedReason, RefusedReason,
    ResolvedSource, SourceKind, SourceProblem, Target,
};
use super::header::parse_header;
use super::resolve::{resolve_db_conn, resolve_source, NebFrame, SmilesStructure, SmilesTo3d, SourceRef};
use super::signature::MethodSignature;
use super::test_db::TestDb;
use crate::commands::settings::DbState;

/// Water at the study level, optimized: the source a chained Freq starts from.
const OPT_INPUT: &str = "! wB97X-3c CPCM(DMF) Opt TightSCF\n%maxcore 2000\n* xyz 0 1\nO 0.0 0.0 0.0\nH 0.0 0.76 0.59\nH 0.0 -0.76 0.59\n*\n";
const WATER: &str = r#"{"elements":["O","H","H"],"xyz_angstrom":[[0.0,0.0,0.1],[0.0,0.75,0.6],[0.0,-0.75,0.6]]}"#;

fn results_json(geometry: &str, converged: &str, extra: &str) -> String {
    format!(r#"{{"parser_version":1,"charges":[],"final_geometry":{geometry},"unknown_blocks":[],"converged":{converged}{extra}}}"#)
}

/// One job row (with a job dir) and, optionally, its stored results.
fn job(db: &TestDb, id: &str, status: &str, input: &str, results: Option<String>) -> PathBuf {
    let dir = db.dir.join("jobs").join(id);
    std::fs::create_dir_all(&dir).unwrap();
    db.conn
        .execute(
            "INSERT INTO jobs (id, title, input_content, status, job_dir) VALUES (?1, 't', ?2, ?3, ?4)",
            rusqlite::params![id, input, status, dir.to_str().unwrap()],
        )
        .unwrap();
    if let Some(r) = results {
        db.conn
            .execute("INSERT INTO results (job_id, data_json, parser_version) VALUES (?1, ?2, 1)", rusqlite::params![id, r])
            .unwrap();
    }
    dir
}

fn xyz(frames: &[[f64; 3]]) -> String {
    format!(
        "3\nE -76.4\nO {} 0.0 0.0\nH 0.0 {} 0.6\nH 0.0 {} 0.6\n",
        frames[0][0], frames[0][1], frames[0][2]
    )
}

fn resolve(db: &TestDb, reference: &str) -> Result<ResolvedSource, String> {
    resolve_db_conn(&db.conn, &SourceRef::parse(reference)?, reference)
}

/// The classifier on `source` for `header`, with a covering grant at the study level.
fn classify(header: &str, source: &Result<ResolvedSource, String>) -> Disposition {
    let h = parse_header(header);
    let grant = GrantSnapshot {
        status: GrantStatus::Active,
        targets: vec![Target::Local],
        levels: vec![MethodSignature::of_input("! wB97X-3c CPCM(DMF)\n")],
        charged_jobs: 0,
        max_jobs: 10,
        in_flight: 0,
        max_in_flight: 2,
    };
    classify_mcp_job(&ClassifyInput {
        header: &h,
        source,
        target: &Target::Local,
        memory: MemoryVerdict::Fits,
        grant: Some(&grant),
        open_proposals: 0,
    })
}

const FREQ: &str = "! wB97X-3c CPCM(DMF) Freq\n";

#[test]
fn references_parse_into_the_six_forms() {
    let ok = |s: &str| SourceRef::parse(s).unwrap();
    assert_eq!(ok("job:a1"), SourceRef::Job { id: "a1".into() });
    assert_eq!(ok("job:a1/scan:0"), SourceRef::Scan { id: "a1".into(), k: 0 });
    assert_eq!(ok("job:a1/neb:ts"), SourceRef::Neb { id: "a1".into(), frame: NebFrame::Ts });
    assert_eq!(ok("job:a1/neb:12"), SourceRef::Neb { id: "a1".into(), frame: NebFrame::Image(12) });
    assert_eq!(ok("job:a1/goat:3"), SourceRef::Goat { id: "a1".into(), k: 3 });
    assert_eq!(ok("molecule:m"), SourceRef::Molecule { id: "m".into() });
    assert_eq!(ok("smiles:F/C=C/F"), SourceRef::Smiles { smiles: "F/C=C/F".into() });
    for bad in ["job:", "job:a/scan:-1", "job:a/scan:+1", "job:a/scan:x", "job:a/irc:1", "job:a/b/scan:1", "mol:1", "smiles:", "smiles:C C", "job:a b"] {
        assert!(SourceRef::parse(bad).is_err(), "{bad}");
    }
}

/// `job:<id>` of a converged optimization: its final geometry, its `* xyz` charge, its convergence
/// and its header's signature — so a Freq at the same level passes P3.
#[test]
fn a_converged_opt_job_resolves_with_its_signature() {
    let db = TestDb::new("resolve-job");
    job(&db, "opt", "parsed", OPT_INPUT, Some(results_json(WATER, "true", "")));
    let src = resolve(&db, "job:opt").unwrap();
    assert_eq!(src.kind, SourceKind::Job);
    assert_eq!(src.atoms.len(), 3);
    assert_eq!((src.atoms[1].element.as_str(), src.atoms[1].y_ang), ("H", 0.75), "the stored final geometry, Å");
    assert_eq!(src.charge_mult, Ok(ChargeMult { charge: 0, multiplicity: 1 }));
    assert_eq!((src.terminal_success, src.converged), (Some(true), Some(true)));
    assert_eq!(src.opt_signature, Some(MethodSignature::of_input("! wB97X-3c CPCM(DMF)\n")));
    assert_eq!(classify(FREQ, &Ok(src)), Disposition::Submitted);
}

/// A job that did not end in success never resolves as terminal-success, and no geometry is read.
/// NEGATIVE CONTROL: map every status to `Some(true)` → red.
#[test]
fn a_failed_or_unfinished_job_is_not_terminal_success() {
    let db = TestDb::new("resolve-status");
    for (status, want) in [
        ("failed", Some(false)),
        ("cancelled", Some(false)),
        ("lost", Some(false)),
        ("running", None),
        ("queued", None),
        ("draft", None),
        ("completed", Some(true)),
    ] {
        let id = format!("j-{status}");
        // `lost` is remote-only by convention, not by CHECK; a local row is enough here.
        job(&db, &id, status, OPT_INPUT, Some(results_json(WATER, "true", "")));
        let src = resolve(&db, &format!("job:{id}")).unwrap();
        assert_eq!(src.terminal_success, want, "{status}");
        if want != Some(true) {
            assert!(src.atoms.is_empty(), "{status}: no geometry read");
            let r = match classify(FREQ, &Ok(src)) {
                Disposition::Refused(r) => r,
                other => panic!("{status}: expected R4, got {other:?}"),
            };
            assert!(r.iter().all(|x| x.row() == "R4"), "{status}: {r:?}");
        }
    }
}

/// A non-converged optimization is R4.
#[test]
fn a_non_converged_opt_is_refused() {
    let db = TestDb::new("resolve-nonconv");
    job(&db, "opt", "parsed", OPT_INPUT, Some(results_json(WATER, "false", "")));
    let src = resolve(&db, "job:opt").unwrap();
    assert_eq!(src.converged, Some(false));
    assert!(matches!(classify(FREQ, &Ok(src)), Disposition::Refused(r) if r == vec![RefusedReason::Source(SourceProblem::NotConverged)]));
}

/// A scan frame is a geometry, not a converged optimization: `converged` and `opt_signature` are
/// `None` even when the parent's results carry a verdict, so a Freq from it is P3. NEGATIVE
/// CONTROL: give frame sources the parent's `converged` and `opt_signature` → red.
#[test]
fn a_scan_frame_is_not_a_converged_optimization() {
    let db = TestDb::new("resolve-scan");
    let scan = r#","scan":{"kind":"B","atoms":[0,1],"coordinate_unit":"Å","points":[{"coordinate":0.9,"energy_act_eh":-76.4,"energy_scf_eh":-76.4},{"coordinate":1.0,"energy_act_eh":-76.3,"energy_scf_eh":-76.3}]}"#;
    let dir = job(&db, "scan", "parsed", OPT_INPUT, Some(results_json(WATER, "true", scan)));
    std::fs::write(dir.join("input.001.xyz"), xyz(&[[0.0, 0.7, -0.7]])).unwrap();
    std::fs::write(dir.join("input.002.xyz"), xyz(&[[0.0, 0.8, -0.8]])).unwrap();
    let src = resolve(&db, "job:scan/scan:1").unwrap();
    assert_eq!(src.kind, SourceKind::ScanFrame);
    assert_eq!(src.atoms[1].y_ang, 0.8, "scan:1 is the second point file (0-based)");
    assert_eq!((src.terminal_success, src.converged, src.opt_signature.clone()), (Some(true), None, None));
    assert_eq!(classify(FREQ, &Ok(src)), Disposition::Proposed(vec![ProposedReason::FreqWithoutConvergedOpt]));
    assert!(resolve(&db, "job:scan/scan:2").is_err(), "out of range");
}

#[test]
fn neb_images_the_ts_and_goat_members_resolve() {
    let db = TestDb::new("resolve-frames");
    let neb = format!(r#","neb":{{"iterations":[],"mep":[],"final_barrier_eh":null,"ts_geometry":{WATER},"ts_energy_eh":null}}"#);
    let dir = job(&db, "neb", "parsed", OPT_INPUT, Some(results_json(WATER, "null", &neb)));
    std::fs::write(dir.join("input_MEP_trj.xyz"), xyz(&[[0.0, 0.7, 0.0]]) + &xyz(&[[0.0, 0.9, 0.0]])).unwrap();
    let ts = resolve(&db, "job:neb/neb:ts").unwrap();
    assert_eq!((ts.kind, ts.atoms[1].y_ang, ts.converged), (SourceKind::NebImage, 0.75, None));
    assert_eq!(resolve(&db, "job:neb/neb:1").unwrap().atoms[1].y_ang, 0.9);

    let goat = "! XTB GOAT\n* xyz -1 2\nO 0 0 0\nH 0 0.76 0.59\nH 0 -0.76 0.59\n*\n";
    let dir = job(&db, "goat", "completed", goat, None);
    std::fs::write(dir.join("input.finalensemble.xyz"), xyz(&[[0.0, 0.1, 0.0]]) + &xyz(&[[0.0, 0.2, 0.0]])).unwrap();
    let m = resolve(&db, "job:goat/goat:1").unwrap();
    assert_eq!((m.kind, m.atoms[1].y_ang), (SourceKind::GoatMember, 0.2));
    assert_eq!(m.charge_mult, Ok(ChargeMult { charge: -1, multiplicity: 2 }), "the GOAT job's own * xyz line");
    assert_eq!((m.terminal_success, m.converged, m.opt_signature), (Some(true), None, None));
    assert!(resolve(&db, "job:goat/goat:2").is_err());
    assert!(resolve(&db, "job:opt-missing").is_err());
}

/// Rule #9: a frame whose element order differs from the source job's input does not resolve.
/// NEGATIVE CONTROL: skip `check_elements` → it resolves and this is red.
#[test]
fn a_frame_with_another_element_order_does_not_resolve() {
    let db = TestDb::new("resolve-order");
    let goat = "! XTB GOAT\n* xyz 0 1\nH 0 0.76 0.59\nO 0 0 0\nH 0 -0.76 0.59\n*\n";
    let dir = job(&db, "goat", "completed", goat, None);
    std::fs::write(dir.join("input.finalensemble.xyz"), xyz(&[[0.0, 0.1, 0.0]])).unwrap();
    let err = resolve(&db, "job:goat/goat:0").unwrap_err();
    assert!(err.contains("atom 0"), "{err}");
}

/// A malformed `* xyz` line is an error carried to R4, never (0, 1). NEGATIVE CONTROL: fall back
/// to `ChargeMult { 0, 1 }` on an error → red.
#[test]
fn a_malformed_charge_line_is_refused_never_zero_one() {
    let db = TestDb::new("resolve-cm");
    let bad = "! wB97X-3c CPCM(DMF) Opt\n* xyz 0\nO 0 0 0\nH 0 0.76 0.59\nH 0 -0.76 0.59\n*\n";
    job(&db, "bad", "parsed", bad, Some(results_json(WATER, "true", "")));
    let src = resolve(&db, "job:bad").unwrap();
    assert!(src.charge_mult.is_err(), "{:?}", src.charge_mult);
    let r = match classify(FREQ, &Ok(src)) {
        Disposition::Refused(r) => r,
        other => panic!("expected R4, got {other:?}"),
    };
    assert!(matches!(r.as_slice(), [RefusedReason::Source(SourceProblem::ChargeMultUnreadable(_))]), "{r:?}");
}

/// A source header the grammar refuses gives no signature (P3 fails closed), never one with the
/// line dropped.
#[test]
fn an_unreadable_source_header_gives_no_signature() {
    let db = TestDb::new("resolve-sig");
    let odd = "! wB97X-3c CPCM(DMF) Opt\n! D3BJ #x# y\n* xyz 0 1\nO 0 0 0\nH 0 0.76 0.59\nH 0 -0.76 0.59\n*\n";
    job(&db, "odd", "parsed", odd, Some(results_json(WATER, "true", "")));
    assert_eq!(resolve(&db, "job:odd").unwrap().opt_signature, None);
    let sp = "! wB97X-3c CPCM(DMF)\n* xyz 0 1\nO 0 0 0\nH 0 0.76 0.59\nH 0 -0.76 0.59\n*\n";
    job(&db, "sp", "parsed", sp, Some(results_json(WATER, "null", "")));
    assert_eq!(resolve(&db, "job:sp").unwrap().opt_signature, None, "no Opt token, no optimization signature");
}

#[test]
fn a_molecule_resolves_with_its_columns_and_is_p1() {
    let db = TestDb::new("resolve-mol");
    db.conn
        .execute(
            "INSERT INTO molecules (id, name, xyz, charge, multiplicity) VALUES ('m', 'water', ?1, -1, 2)",
            [xyz(&[[0.0, 0.7, 0.0]])],
        )
        .unwrap();
    let src = resolve(&db, "molecule:m").unwrap();
    assert_eq!((src.kind, src.atoms.len()), (SourceKind::Molecule, 3));
    assert_eq!(src.charge_mult, Ok(ChargeMult { charge: -1, multiplicity: 2 }));
    assert!(resolve(&db, "molecule:none").is_err());
}

struct Stub(Result<SmilesStructure, String>);

impl SmilesTo3d for Stub {
    fn smiles_to_3d(&self, _smiles: &str) -> Result<SmilesStructure, String> {
        self.0.clone()
    }
}

fn water_reply(num_atoms: usize) -> Stub {
    Stub(Ok(SmilesStructure { xyz: xyz(&[[0.0, 0.7, 0.0]]), charge: 0, multiplicity: 1, num_atoms }))
}

/// A SMILES goes through the seam with no database lock — shown on a poisoned mutex, which every
/// DB-backed reference reports — and is P1. A reply whose atom count disagrees does not resolve.
#[test]
fn a_smiles_resolves_through_the_seam_without_the_lock() {
    let db = TestDb::new("resolve-smiles");
    let state = DbState(Mutex::new(rusqlite::Connection::open(db.dir.join("orcastudio.db")).unwrap()));
    std::thread::scope(|s| {
        s.spawn(|| {
            let _guard = state.0.lock().unwrap();
            panic!("poison the mutex");
        })
        .join()
        .unwrap_err();
    });
    let src = resolve_source(&state, "smiles:O", &water_reply(3)).unwrap();
    assert_eq!((src.kind, src.atoms.len(), src.label.as_str()), (SourceKind::Smiles, 3, "smiles:O"));
    assert!(resolve_source(&state, "job:x", &water_reply(3)).unwrap_err().contains("poisoned"));
    match classify(FREQ, &Ok(src)) {
        Disposition::Proposed(p) => assert!(p.iter().any(|r| r.row() == "P1"), "{p:?}"),
        other => panic!("expected P1, got {other:?}"),
    }
    assert!(resolve_source(&state, "smiles:O", &water_reply(4)).is_err(), "the atom count post-condition");
    assert!(resolve_source(&state, "smiles:O", &Stub(Err("Invalid SMILES".into()))).is_err());
}

/// Anton's decision (2026-11-19): a constrained optimization is not a stationary point. A converged
/// Opt with `{B 0 1 C}` gives no optimization signature, so a Freq at the same level is P3
/// PROPOSED; the same Opt without the constraint is SUBMITTED. A `Scan`, or any `%geom` key beyond
/// `Calc_Hess`/`Recalc_Hess`/`MaxIter`, is ineligible too (fail closed); `Calc_Hess true` and
/// `MaxIter 200` are not. NEGATIVE CONTROL: ignore the `%geom` check in `opt_signature_of` → the
/// constrained source is SUBMITTED and this is red.
#[test]
fn a_constrained_optimization_is_not_a_converged_opt_for_p3() {
    let db = TestDb::new("resolve-constrained");
    let coords = "* xyz 0 1\nO 0.0 0.0 0.0\nH 0.0 0.76 0.59\nH 0.0 -0.76 0.59\n*\n";
    let head = "! wB97X-3c CPCM(DMF) Opt\n%maxcore 2000\n";
    let cases = [
        ("constrained", "%geom\n  Constraints\n    {B 0 1 C}\n  end\nend\n", false),
        ("one-line", "%geom Constraints {B 0 1 C} end end\n", false),
        ("scan", "%geom Scan B 0 1 = 0.9, 1.1, 3 end end\n", false),
        ("hydrogens", "%geom OptimizeHydrogens true end\n", false),
        ("plain", "", true),
        ("calc-hess", "%geom Calc_Hess true MaxIter 200 end\n", true),
    ];
    for (id, geom, eligible) in cases {
        job(&db, id, "parsed", &format!("{head}{geom}{coords}"), Some(results_json(WATER, "true", "")));
        let src = resolve(&db, &format!("job:{id}")).unwrap();
        assert_eq!(src.converged, Some(true), "{id}: the optimization itself converged");
        assert_eq!(src.opt_signature.is_some(), eligible, "{id}");
        let want = if eligible {
            Disposition::Submitted
        } else {
            Disposition::Proposed(vec![ProposedReason::FreqWithoutConvergedOpt])
        };
        assert_eq!(classify(FREQ, &Ok(src)), want, "{id}");
    }
}
