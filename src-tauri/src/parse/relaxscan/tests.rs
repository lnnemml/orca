//! Tests for the relaxed-scan reader, against the REAL ORCA 6.1 ethane C–C scan
//! (`tests/fixtures/scan-ethane-cc/`, committed from the probe dir): 6 points,
//! `act ≠ scf`, coordinate 1.4→2.4 Å monotone, and the geometry cross-check against
//! `input.001.xyz … input.006.xyz`. The three negative controls (bohr coordinate,
//! act/scf conflated, per-cycle source) are shown red-then-green.

use std::path::PathBuf;

use super::{parse_scan_spec, RelaxScan, ScanSpec};
use crate::parse::ParseError;

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/scan-ethane-cc")
}

const ETHANE_INP: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/scan-ethane-cc/input.inp"
));

fn load() -> RelaxScan {
    RelaxScan::from_path(&fixture_dir())
        .expect("read ok")
        .expect("scan present in the fixture dir")
}

fn ethane_spec() -> ScanSpec {
    // The B 0 1 distance scan, parsed from the real input.
    parse_scan_spec(ETHANE_INP).expect("a scan line in the fixture input")
}

// ── the scan-line spec parser ─────────────────────────────────────────────────

#[test]
fn parses_the_scan_line_to_b_0_1() {
    assert_eq!(ethane_spec(), ScanSpec { kind: 'B', atoms: vec![0, 1] });
}

#[test]
fn ignores_a_constraint_line_and_absent_scan() {
    // A `{B 0 1 C}` constraint (no `=`) is NOT a scan line.
    assert_eq!(parse_scan_spec("%geom Constraints {B 0 1 C} end end"), None);
    assert_eq!(parse_scan_spec("! r2SCAN-3c Opt\n* xyz 0 1\nC 0 0 0\n*\n"), None);
}

// ── Review checkpoint 3: per-point source, 6 rows, real fixture ───────────────

#[test]
fn reads_six_points_from_the_dat_not_twentysix_from_per_cycle() {
    let v = load().verify(&ethane_spec()).expect("verify ok");
    // The `.dat` is per-point (6), NOT the 26 per-cycle rows of .property.txt/_trj.xyz.
    assert_eq!(v.points().len(), 6);
    let coords: Vec<f64> = v.points().iter().map(|p| p.coordinate).collect();
    assert_eq!(coords, vec![1.4, 1.6, 1.8, 2.0, 2.2, 2.4]);
    assert_eq!(v.kind(), 'B');
    assert_eq!(v.coordinate_unit(), "Å");
    assert_eq!(v.atoms(), &[0, 1]);
}

// ── Review checkpoint 2: act vs scf are BOTH parsed and DIFFER ────────────────

#[test]
fn act_and_scf_are_both_stored_and_genuinely_differ() {
    let v = load().verify(&ethane_spec()).unwrap();
    for p in v.points() {
        // r²SCAN-3c: act = composite (gCP+D4), scf = bare SCF — measured to differ.
        assert!(
            (p.energy_act_eh - p.energy_scf_eh).abs() > 1e-6,
            "act {} must differ from scf {}",
            p.energy_act_eh,
            p.energy_scf_eh
        );
    }
    // exact first-row values from the real fixture (act ≠ scf).
    let p0 = v.points()[0];
    assert!((p0.energy_act_eh - -79.78236865).abs() < 1e-8);
    assert!((p0.energy_scf_eh - -79.78571668).abs() < 1e-8);
}

// C-act-scf-conflated: storing `act` into both fields would make them equal — the
// assertion above (they differ) is exactly what bites that mistake. Demonstrate the
// control is live by constructing the conflated form and showing the check fails.
#[test]
fn c_act_scf_conflated_bites() {
    let v = load().verify(&ethane_spec()).unwrap();
    let p0 = v.points()[0];
    // What a conflated impl would produce: scf := act.
    let conflated_scf = p0.energy_act_eh;
    assert!(
        (p0.energy_act_eh - conflated_scf).abs() <= 1e-6,
        "the conflated pair is equal — so the >1e-6 assertion above would go RED (control lives)"
    );
    // …and the real pair is NOT equal, so the real fixture stays green.
    assert!((p0.energy_act_eh - p0.energy_scf_eh).abs() > 1e-6);
}

// ── Review checkpoint 1: geometry cross-check confirms Å (the Bohr control) ────

#[test]
fn green_geometry_cross_check_passes_on_the_real_angstrom_dat() {
    // The coordinate column IS Å: recomputed distance from each input.NNN.xyz matches.
    assert!(load().verify(&ethane_spec()).is_ok());
}

#[test]
fn c_bohr_coordinate_fails_the_cross_check_loudly() {
    // A `.dat` whose coordinate column is the Å values × 1.889 (i.e. Bohr) — the
    // point geometries (input.NNN.xyz) are still Å, so the recomputed distance
    // (~1.4 Å) mismatches the Bohr coordinate (~2.64) by ≈1.889×. Same-crate test
    // reaches the private rows to inject the wrong unit.
    let mut raw = load();
    const BOHR_PER_ANGSTROM: f64 = 1.0 / 0.529_177_210_903;
    for r in raw.act.iter_mut() {
        r.0 *= BOHR_PER_ANGSTROM;
    }
    for r in raw.scf.iter_mut() {
        r.0 *= BOHR_PER_ANGSTROM;
    }
    let err = raw.verify(&ethane_spec()).unwrap_err();
    match err {
        ParseError::GeometryMismatch { max_delta } => {
            assert!(max_delta > 1.0, "Bohr coord should miss by ≈1.889×, got {max_delta}");
        }
        other => panic!("expected GeometryMismatch, got {other:?}"),
    }
    // Sanity: the UNMODIFIED fixture passes — so the red above is caused by the unit,
    // not by a broken cross-check.
    assert!(load().verify(&ethane_spec()).is_ok());
}

// ── C-per-cycle-source: a per-cycle-length profile can't pass the per-point check ─

#[test]
fn c_per_cycle_source_bites() {
    // If the reader mistook the 26 per-cycle geometries (.property.txt / _trj.xyz) for
    // the profile, N would be 26 — but there are only 6 per-point `input.NNN.xyz`
    // witnesses. Extend the real 6 rows to 26 (monotone) and verify → the cross-check
    // fails loudly on the missing `input.007.xyz`.
    let mut raw = load();
    let mut c = raw.act.last().unwrap().0;
    let e = raw.act.last().unwrap().1;
    while raw.act.len() < 26 {
        c += 0.2;
        raw.act.push((c, e));
        raw.scf.push((c, e - 0.003));
    }
    let err = raw.verify(&ethane_spec()).unwrap_err();
    // Point 7's xyz does not exist → an Io error reading input.007.xyz.
    assert!(
        matches!(err, ParseError::Io { .. }),
        "a 26-row (per-cycle) profile must fail the per-point geometry cross-check, got {err:?}"
    );
}

// ── other post-conditions ─────────────────────────────────────────────────────

#[test]
fn non_monotone_coordinate_is_refused() {
    let mut raw = load();
    raw.act[3].0 = raw.act[1].0; // break the strictly-increasing column
    raw.scf[3].0 = raw.scf[1].0;
    assert!(matches!(
        raw.verify(&ethane_spec()).unwrap_err(),
        ParseError::Malformed { .. }
    ));
}

#[test]
fn act_scf_length_disagreement_is_refused() {
    let mut raw = load();
    raw.scf.pop();
    assert!(matches!(
        raw.verify(&ethane_spec()).unwrap_err(),
        ParseError::LengthMismatch { .. }
    ));
}

#[test]
fn absent_scan_is_none_not_an_error() {
    // The parse dir has no .relaxscanact.dat → Ok(None) (absent-is-normal), not an error.
    let tmp = std::env::temp_dir();
    assert!(RelaxScan::from_path(&tmp).unwrap().is_none());
}
