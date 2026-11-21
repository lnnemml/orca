//! Tests for `get_results` (ADR-027 decision 7; M2 amendment, items 7 and 8; rule #11).

use serde_json::Value;

use super::results::{get_results, IMAGINARY_MAX, MEP_POINTS_MAX, SCAN_POINTS_MAX, WARNINGS_MAX};
use super::test_support::{assert_fits, results_json, store_results, unitless_numeric_keys, JobRow};
use super::ToolError;
use crate::mcp::test_db::TestDb;


fn job_with(db: &TestDb, id: &str, data: Option<Value>) {
    JobRow::new(id).insert(&db.conn);
    if let Some(d) = data {
        store_results(&db.conn, id, &d);
    }
}

#[test]
fn get_results_summarises_energy_modes_thermo_scan_and_neb() {
    let db = TestDb::new("tools-results");
    job_with(&db, "r", Some(results_json(11, 9, 2, 1, "Å")));
    let s = get_results(&db.conn, "r").unwrap();
    assert!(s.parsed && s.not_parsed_reason.is_none());
    assert_eq!((s.final_energy_eh, s.converged, s.atom_count), (Some(-76.408_912_345), Some(true), 3));
    let f = s.frequencies.as_ref().unwrap();
    assert_eq!(f.imaginary_count, 2);
    assert_eq!(f.imaginary_freqs_cm1, [-101.0, -100.0], "the most negative first");
    assert_eq!(f.imaginary_ir_km_mol, [Some(10.1), Some(10.0)]);
    assert_eq!(s.thermochemistry.as_ref().unwrap().free_energy_g_eh, -76.4053);
    let scan = s.scan.as_ref().unwrap();
    assert_eq!((scan.kind.as_str(), scan.atom_indices.as_slice(), scan.point_count), ("B", &[0u32, 1][..], 11));
    assert_eq!((scan.points[0].coordinate_ang, scan.points[0].coordinate_deg), (Some(1.0), None));
    let neb = s.neb.as_ref().unwrap();
    assert_eq!((neb.iteration_count, neb.climbing_image_index, neb.mep_point_count), (2, Some(4), 9));
    assert_eq!(neb.final_barrier_eh, Some(0.04));
    assert_eq!(s.warning_count, 1);
}

/// The scan coordinate's unit comes from the stored literal: `°` gives `coordinate_deg`, anything
/// but `Å`/`°` gives no coordinate and a warning — never a bare number.
#[test]
fn a_scan_coordinate_is_named_by_its_stored_unit() {
    let db = TestDb::new("tools-results-scan-unit");
    job_with(&db, "deg", Some(results_json(3, 2, 0, 0, "°")));
    job_with(&db, "bohr", Some(results_json(3, 2, 0, 0, "bohr")));
    let deg = get_results(&db.conn, "deg").unwrap();
    let p = &deg.scan.as_ref().unwrap().points[0];
    assert_eq!((p.coordinate_ang, p.coordinate_deg), (None, Some(1.0)));
    let bohr = get_results(&db.conn, "bohr").unwrap();
    let p = &bohr.scan.as_ref().unwrap().points[0];
    assert_eq!((p.coordinate_ang, p.coordinate_deg), (None, None));
    assert!(bohr.warnings.iter().any(|w| w.contains("`bohr`")), "{:?}", bohr.warnings);
}

/// Rule #11: every numeric field of the serialised summary names its unit (or is a count or an
/// index). NEGATIVE CONTROL (`ctl:units_suffix`): serialise `final_energy_eh` as `final_energy` → red.
#[test]
fn every_number_in_get_results_names_its_unit() {
    let db = TestDb::new("tools-results-units");
    job_with(&db, "r", Some(results_json(5, 5, 2, 1, "Å")));
    job_with(&db, "d", Some(results_json(5, 5, 2, 1, "°")));
    for id in ["r", "d"] {
        let v = serde_json::to_value(get_results(&db.conn, id).unwrap()).unwrap();
        assert!(v.to_string().contains("-76.408912345"), "the energy is in the walk");
        assert_eq!(unitless_numeric_keys(&v, &[]), Vec::<String>::new(), "[ctl:units_suffix] {id}");
    }
}

/// The largest summary fits: 5,000 scan points, 5,000 MEP points, 3,000 imaginary modes and 100
/// warnings of 5,000 characters. NEGATIVE CONTROLS: lift the scan cap (`ctl:results_cap_scan`),
/// the MEP cap (`ctl:results_cap_mep`), the imaginary-mode cap (`ctl:results_cap_imag`) or the
/// warning cap (`ctl:results_cap_warn`) → red.
#[test]
fn the_largest_get_results_fits_under_the_cap() {
    let db = TestDb::new("tools-results-big");
    job_with(&db, "big", Some(results_json(5_000, 5_000, 3_000, 100, "Å")));
    let s = get_results(&db.conn, "big").expect("[ctl:results_cap] the largest summary is served");
    assert_fits(&s, "[ctl:results_cap]");
    let scan = s.scan.as_ref().unwrap();
    assert_eq!((scan.points.len(), scan.point_count, scan.points_truncated), (SCAN_POINTS_MAX, 5_000, true), "[ctl:results_cap_scan]");
    let neb = s.neb.as_ref().unwrap();
    assert_eq!((neb.mep.len(), neb.mep_point_count, neb.mep_truncated), (MEP_POINTS_MAX, 5_000, true), "[ctl:results_cap_mep]");
    let f = s.frequencies.as_ref().unwrap();
    assert_eq!((f.imaginary_freqs_cm1.len(), f.imaginary_count, f.imaginary_truncated), (IMAGINARY_MAX, 3_000, true), "[ctl:results_cap_imag]");
    assert_eq!((s.warnings.len(), s.warning_count), (WARNINGS_MAX, 100), "[ctl:results_cap_warn]");
}

#[test]
fn a_job_without_results_says_so_and_a_missing_job_is_refused() {
    let db = TestDb::new("tools-results-none");
    job_with(&db, "q", None);
    db.conn.execute("UPDATE jobs SET status = 'running' WHERE id = 'q'", []).unwrap();
    let s = get_results(&db.conn, "q").unwrap();
    assert!(!s.parsed);
    assert!(s.not_parsed_reason.as_deref().is_some_and(|r| r.contains("running")), "{:?}", s.not_parsed_reason);
    assert!(matches!(get_results(&db.conn, "nope"), Err(ToolError::Refused(_))));
}
