//! Tests for `get_geometry` and `measure` (ADR-027 decisions 4 and 7; M2 amendment, item 8;
//! rules #9, #10 and #11).

use std::panic::{catch_unwind, AssertUnwindSafe};

use serde_json::json;

use super::geometry::{get_geometry, measure, measure_atoms, MeasureRefusal, Measured, COINCIDENT_ANG, GEOMETRY_MAX_ATOMS};
use super::test_support::{assert_fits, molecule, store_results, unitless_numeric_keys, JobRow};
use super::ToolError;
use crate::mcp::classify::SourceAtom;
use crate::mcp::test_db::TestDb;

/// The butane conformers of `sidecar/tests/test_geometry.py` (`BUTANE_ANTI`, `BUTANE_GAUCHE`), the
/// same coordinates `measure.test.ts` uses; carbons are atoms 0–3.
const BUTANE_ANTI: &str = "14\nanti\n\
C 1.93939185364579236293 0.07737406152119787051 0.24304182052609096809\n\
C 0.53797179522872373703 0.18841655754956676549 -0.34457949257460168679\n\
C -0.53797295432378267055 -0.18844136149474677300 0.67101001653575198524\n\
C -1.93939200460205274368 -0.07735434549765553280 0.08339293823298575548\n\
H 2.13568247859938598054 -0.94115392278442766560 0.57149043429382551107\n\
H 2.68872000526646859342 0.34971942494684188363 -0.49673182931641063353\n\
H 2.04710290775211722902 0.73876554547227901715 1.10022637882360330153\n\
H 0.45628686535958468129 -0.46727555515909763306 -1.21471751981858355762\n\
H 0.36748852742773696622 1.21234497266518936875 -0.68553458663930899597\n\
H -0.45626639617516862035 0.46723783351233355576 1.54115443156927311996\n\
H -0.36750752592934021745 -1.21238355959702492903 1.01192941848362494284\n\
H -2.13578267466600157931 0.94125287881583941108 -0.24474770159437347905\n\
H -2.04701298168341550010 -0.73849555042777181857 -0.77399646430233681738\n\
H -2.68870889590005024417 -0.35000697952252329825 0.82306515578045891246\n";

const BUTANE_GAUCHE: &str = "14\ngauche\n\
C 1.57493837534088720886 0.47560965087875917146 0.28746743038890165689\n\
C 0.62028514539525791971 -0.37785166769397016129 -0.53930644713614017682\n\
C -0.64413190616412951961 -0.77636815711506135251 0.22269040683532481673\n\
C -1.55681068554076151855 0.40522811341468101265 0.52970409616902125460\n\
H 1.84072813978161531345 -0.03642029037517269252 1.21005136198805751668\n\
H 2.48773877337920579222 0.67014682530236679980 -0.27139603343838297755\n\
H 1.12465750081146698136 1.43159957300775864120 0.54128168079159588721\n\
H 1.14049403580416219839 -1.28714876858371973789 -0.85058159629168583749\n\
H 0.33778352409513645593 0.16643320820288007300 -1.44353132318610333229\n\
H -0.36075279667117676752 -1.26780892734441685832 1.15642404029273149213\n\
H -1.19803127054222802172 -1.50266986554063564085 -0.37734113498986981972\n\
H -2.47232234638838477281 0.06134923645843771978 1.00605499202453874119\n\
H -1.07165667530126729190 1.10969862507090288872 1.20010746596129380670\n\
H -1.82291881399980049139 0.92820244431718368361 -0.38662193940927253033\n";

/// ASE 3.29.0's own values on those fixtures, recorded by
/// `sidecar/tests/test_geometry.py::test_ase_distance_angle_dihedral_on_butane` on 2026-11-20
/// (`get_distance(0,1)` Å, `get_angle(0,1,2)` °, `get_dihedral(0,1,2,3)` °).
const ASE_ANTI: (f64, f64, f64) = (1.5236821924758759, 111.940389836418, 179.99823131562985);
const ASE_GAUCHE: (f64, f64, f64) = (1.5242421252226686, 113.17379073940005, 67.52301077175457);
/// How close the Rust value must be to ASE's: the two differ only in floating-point order.
const ASE_TOL: f64 = 1e-9;

fn atoms_of(xyz: &str) -> Vec<SourceAtom> {
    xyz.lines()
        .skip(2)
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            let t: Vec<&str> = l.split_whitespace().collect();
            SourceAtom { element: t[0].into(), x_ang: t[1].parse().unwrap(), y_ang: t[2].parse().unwrap(), z_ang: t[3].parse().unwrap() }
        })
        .collect()
}

fn atom(element: &str, x: f64, y: f64, z: f64) -> SourceAtom {
    SourceAtom { element: element.into(), x_ang: x, y_ang: y, z_ang: z }
}

/// `measure_atoms` run so that a panic is a result, not a crash: `measure` must refuse, never panic.
fn run(atoms: &[SourceAtom], indices: &[usize]) -> Result<Result<Measured, MeasureRefusal>, String> {
    catch_unwind(AssertUnwindSafe(|| measure_atoms(atoms, indices))).map_err(|_| "panicked".to_string())
}

/// The number of a measurement, or a panic naming `tag`.
fn value(atoms: &[SourceAtom], indices: &[usize], tag: &str) -> f64 {
    match run(atoms, indices) {
        Ok(Ok(Measured::Distance { distance_ang: v } | Measured::Angle { angle_deg: v } | Measured::Dihedral { dihedral_deg: v })) => v,
        other => panic!("{tag}: no value for {indices:?}: {other:?}"),
    }
}

/// The Rust distance, angle and dihedral equal ASE's on both conformers (rule #10: ASE's values are
/// a recorded run). NEGATIVE CONTROLS: perturb `xtb::distance` (`ctl:ase_distance`), `xtb::angle`
/// (`ctl:ase_angle`) or `ase_dihedral` (`ctl:ase_dihedral`) → red.
#[test]
fn distance_angle_and_dihedral_match_ase_on_butane() {
    for (xyz, (d, a, phi)) in [(BUTANE_ANTI, ASE_ANTI), (BUTANE_GAUCHE, ASE_GAUCHE)] {
        let atoms = atoms_of(xyz);
        let got = value(&atoms, &[0, 1], "[ctl:ase_distance]");
        assert!((got - d).abs() < ASE_TOL, "[ctl:ase_distance] Rust {got} vs ASE {d}");
        let got = value(&atoms, &[0, 1, 2], "[ctl:ase_angle]");
        assert!((got - a).abs() < ASE_TOL, "[ctl:ase_angle] Rust {got} vs ASE {a}");
        let got = value(&atoms, &[0, 1, 2, 3], "[ctl:ase_dihedral]");
        assert!((got - phi).abs() < ASE_TOL, "[ctl:ase_dihedral] Rust {got} vs ASE {phi}");
        // Reversal invariance (ASE-confirmed for `measure.ts`).
        assert!((value(&atoms, &[3, 2, 1, 0], "reversed") - phi).abs() < ASE_TOL);
    }
}

/// The dihedral is folded to [0, 360): the mirror image of gauche is 360° − 67.523°, never −67.523°.
/// NEGATIVE CONTROL (`ctl:dihedral_fold`): drop the fold in `ase_dihedral` → red.
#[test]
fn the_dihedral_is_folded_to_0_360() {
    let mirrored: Vec<SourceAtom> = atoms_of(BUTANE_GAUCHE).into_iter().map(|a| SourceAtom { z_ang: -a.z_ang, ..a }).collect();
    let got = value(&mirrored, &[0, 1, 2, 3], "[ctl:dihedral_fold]");
    assert!((got - (360.0 - ASE_GAUCHE.2)).abs() < ASE_TOL, "[ctl:dihedral_fold] {got}");
    assert!((0.0..360.0).contains(&got), "[ctl:dihedral_fold] {got}");
}

/// Butane plus atom 14, a copy of atom 0 displaced by 1e-7 Å (below [`COINCIDENT_ANG`]).
fn with_ghost() -> Vec<SourceAtom> {
    let mut atoms = atoms_of(BUTANE_ANTI);
    let a0 = atoms[0].clone();
    atoms.push(SourceAtom { x_ang: a0.x_ang + 1e-7, ..a0 });
    atoms
}

/// An index past the end is refused, never a panic. NEGATIVE CONTROL (`ctl:refuse_range`): drop the
/// range check → red.
#[test]
fn an_index_out_of_range_is_refused() {
    let atoms = atoms_of(BUTANE_ANTI);
    for idx in [&[0, 14][..], &[14, 0, 1], &[0, 1, 2, 99]] {
        let bad = *idx.iter().max().unwrap();
        assert_eq!(
            run(&atoms, idx),
            Ok(Err(MeasureRefusal::IndexOutOfRange { index: bad, atom_count: 14 })),
            "[ctl:refuse_range] {idx:?}"
        );
    }
}

/// A repeated index is refused, including the dihedral 0–1–2–0, which no geometric check catches.
/// NEGATIVE CONTROL (`ctl:refuse_repeat`): drop the repeat check → red.
#[test]
fn a_repeated_index_is_refused() {
    let atoms = atoms_of(BUTANE_ANTI);
    for idx in [&[3, 3][..], &[0, 1, 0], &[0, 1, 2, 0]] {
        assert_eq!(run(&atoms, idx), Ok(Err(MeasureRefusal::RepeatedIndex { index: idx[0] })), "[ctl:refuse_repeat] {idx:?}");
    }
}

/// Two atoms closer than the tolerance have no distance. NEGATIVE CONTROL (`ctl:refuse_coincident`):
/// drop the check → red (a plausible 1e-7 Å would be returned).
#[test]
fn coincident_atoms_have_no_distance() {
    assert_eq!(run(&with_ghost(), &[0, 14]), Ok(Err(MeasureRefusal::CoincidentAtoms { i: 0, j: 14 })), "[ctl:refuse_coincident]");
    // Just above the tolerance is a distance.
    let mut atoms = atoms_of(BUTANE_ANTI);
    let a0 = atoms[0].clone();
    atoms.push(SourceAtom { x_ang: a0.x_ang + 10.0 * COINCIDENT_ANG, ..a0 });
    assert!((value(&atoms, &[0, 14], "above the tolerance") - 1e-5).abs() < 1e-9);
}

/// An end atom on the vertex leaves an arm of zero length: no angle. NEGATIVE CONTROL
/// (`ctl:refuse_arm`): drop the check → red.
#[test]
fn a_zero_length_arm_has_no_angle() {
    let atoms = with_ghost();
    assert_eq!(run(&atoms, &[14, 0, 1]), Ok(Err(MeasureRefusal::ZeroLengthArm { vertex: 0, end: 14 })), "[ctl:refuse_arm]");
    assert_eq!(run(&atoms, &[1, 0, 14]), Ok(Err(MeasureRefusal::ZeroLengthArm { vertex: 0, end: 14 })), "[ctl:refuse_arm]");
}

/// The middle two atoms of a dihedral on one point: no axis. NEGATIVE CONTROL (`ctl:refuse_axis`):
/// drop the check → red.
#[test]
fn a_zero_axis_has_no_dihedral() {
    assert_eq!(run(&with_ghost(), &[2, 0, 14, 3]), Ok(Err(MeasureRefusal::ZeroAxis { j: 0, k: 14 })), "[ctl:refuse_axis]");
}

/// A collinear inner triple, at either end, has no dihedral; a triple just off collinear has one.
/// NEGATIVE CONTROLS: drop the first triple's check (`ctl:refuse_collinear_ijk`) or the second's
/// (`ctl:refuse_collinear_jkl`) → red.
#[test]
fn a_collinear_inner_triple_has_no_dihedral() {
    let first = [atom("C", 0.0, 0.0, 0.0), atom("C", 1.5, 0.0, 0.0), atom("C", 3.0, 0.0, 0.0), atom("C", 3.0, 1.5, 0.0)];
    assert_eq!(run(&first, &[0, 1, 2, 3]), Ok(Err(MeasureRefusal::CollinearInnerTriple { a: 0, b: 1, c: 2 })), "[ctl:refuse_collinear_ijk]");
    let second = [atom("C", 0.0, 1.5, 0.0), atom("C", 0.0, 0.0, 0.0), atom("C", 1.5, 0.0, 0.0), atom("C", 3.0, 0.0, 0.0)];
    assert_eq!(run(&second, &[0, 1, 2, 3]), Ok(Err(MeasureRefusal::CollinearInnerTriple { a: 1, b: 2, c: 3 })), "[ctl:refuse_collinear_jkl]");
    // sin θ ≈ 1e-5 at atom 1, above the tolerance: a dihedral (the projection is ~1.5e-5 Å).
    let near = [atom("C", 0.0, 1.5e-5, 0.0), atom("C", 1.5, 0.0, 0.0), atom("C", 3.0, 0.0, 0.0), atom("C", 3.0, 0.0, 1.5)];
    assert!(matches!(run(&near, &[0, 1, 2, 3]), Ok(Ok(Measured::Dihedral { .. }))));
}

#[test]
fn a_wrong_count_or_a_non_finite_coordinate_is_refused() {
    let mut atoms = atoms_of(BUTANE_ANTI);
    assert_eq!(run(&atoms, &[0]), Ok(Err(MeasureRefusal::WrongCount { count: 1 })));
    assert_eq!(run(&atoms, &[0, 1, 2, 3, 4]), Ok(Err(MeasureRefusal::WrongCount { count: 5 })));
    atoms[1].y_ang = f64::NAN;
    assert_eq!(run(&atoms, &[0, 1]), Ok(Err(MeasureRefusal::NonFiniteCoordinate { index: 1 })));
}

/// `measure` on a database source (a molecule and a job's final geometry): the value with its unit
/// in its name, the elements, the indices.
#[test]
fn measure_reads_a_molecule_or_a_job_through_the_resolver() {
    let db = TestDb::new("tools-measure");
    molecule(&db.conn, "butane", BUTANE_GAUCHE, 0, 1);
    let m = measure(&db.conn, "molecule:butane", &[0, 1, 2, 3]).unwrap();
    assert_eq!((m.atom_indices.as_slice(), m.elements.join("")), (&[0, 1, 2, 3][..], "CCCC".to_string()));
    let v = serde_json::to_value(&m).unwrap();
    assert_eq!(v["kind"], "dihedral");
    assert!((v["dihedral_deg"].as_f64().unwrap() - ASE_GAUCHE.2).abs() < ASE_TOL);
    assert_eq!(unitless_numeric_keys(&v, &[]), Vec::<String>::new());

    let input = "! SP\n* xyz 0 1\nO 0 0 0\nH 0 0.75 0.6\nH 0 -0.75 0.6\n*\n";
    JobRow { input, ..JobRow::new("w") }.insert(&db.conn);
    store_results(
        &db.conn,
        "w",
        &json!({"parser_version": 6, "charges": [], "unknown_blocks": [],
                "final_geometry": {"elements": ["O", "H", "H"], "xyz_angstrom": [[0.0, 0.0, 0.0], [0.0, 0.75, 0.6], [0.0, -0.75, 0.6]]}}),
    );
    let d = measure(&db.conn, "job:w", &[0, 1]).unwrap();
    assert!(matches!(d.value, Measured::Distance { distance_ang } if (distance_ang - (0.75f64 * 0.75 + 0.36).sqrt()).abs() < 1e-12));
    assert!(matches!(measure(&db.conn, "job:w", &[0, 3]), Err(ToolError::Measure(MeasureRefusal::IndexOutOfRange { .. }))));
}

/// A source with no readable geometry is refused with its reason: a SMILES (no stored geometry),
/// an unfinished or failed job, an unknown molecule.
#[test]
fn sources_without_a_geometry_are_refused() {
    let db = TestDb::new("tools-geometry-refused");
    JobRow { status: "running", ..JobRow::new("run") }.insert(&db.conn);
    JobRow { status: "failed", ..JobRow::new("bad") }.insert(&db.conn);
    for (r, needle) in [("smiles:CCO", "smiles"), ("job:run", "not finished"), ("job:bad", "without success"), ("molecule:none", "none"), ("xyz:1", "not a source")] {
        match get_geometry(&db.conn, r) {
            Err(ToolError::Refused(why)) => assert!(why.contains(needle), "{r}: {why}"),
            other => panic!("{r} must be refused: {other:?}"),
        }
    }
}

/// `get_geometry` gives xyz text with the charge and multiplicity and their source, and the text
/// reads back to the source (the post-condition).
#[test]
fn get_geometry_gives_xyz_with_charge_and_multiplicity() {
    let db = TestDb::new("tools-geometry");
    molecule(&db.conn, "butane", BUTANE_ANTI, -1, 2);
    let g = get_geometry(&db.conn, "molecule:butane").unwrap();
    assert_eq!((g.atom_count, g.source_kind), (14, "molecule"));
    assert_eq!((g.charge_mult.charge, g.charge_mult.multiplicity), (Some(-1), Some(2)));
    assert!(g.charge_mult.source.contains("molecule butane"), "{}", g.charge_mult.source);
    let lines: Vec<&str> = g.xyz_ang.lines().collect();
    assert_eq!(lines[0], "14");
    assert_eq!(lines[2], "C 1.93939185 0.07737406 0.24304182");
}

/// Molecule `id` with `n` atoms on a wide grid (coordinates up to −999 Å, the widest text).
fn grid_molecule(db: &TestDb, id: &str, n: usize) {
    let mut xyz = format!("{n}\ngrid\n");
    for i in 0..n {
        let f = -999.0 + i as f64 * 1.000_000_01;
        xyz.push_str(&format!("C {f:.8} {:.8} {:.8}\n", f + 0.5, f - 0.5));
    }
    molecule(&db.conn, id, &xyz, 0, 1);
}

/// 600 atoms are served and fit under the cap; 601 are refused with a pointer to `measure`.
/// NEGATIVE CONTROL (`ctl:geometry_600`): accept above 600 → red.
#[test]
fn get_geometry_is_refused_above_600_atoms() {
    let db = TestDb::new("tools-geometry-600");
    grid_molecule(&db, "at", GEOMETRY_MAX_ATOMS);
    grid_molecule(&db, "over", GEOMETRY_MAX_ATOMS + 1);
    let g = get_geometry(&db.conn, "molecule:at").expect("600 atoms are served");
    assert_fits(&g, "[ctl:geometry_600] 600 atoms");
    match get_geometry(&db.conn, "molecule:over") {
        Err(ToolError::Refused(why)) => assert!(why.contains("601 atoms") && why.contains("measure"), "{why}"),
        other => panic!("[ctl:geometry_600] 601 atoms must be refused: {other:?}"),
    }
    // `measure` still reads the large source.
    assert!(measure(&db.conn, "molecule:over", &[0, 600]).is_ok());
}
