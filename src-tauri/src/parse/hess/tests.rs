//! Tests for the `.hess` reader, against REAL ORCA 6.1.0 artifacts (the author's
//! own runs — rule #7 is about not redistributing ORCA itself). Both an ethane
//! minimum and a saddle point are used: the saddle carries the imaginary mode.

use super::HessFile;
use crate::parse::elements::z_of;
use crate::parse::{derived_identity_ids, identity_map_for, ParseError, ReferenceGeometry};
use orcastudio_core::ids::{IndexMap, OrcaIndex};

macro_rules! fixture {
    ($name:literal) => {
        include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/", $name))
    };
}

const ETHANE_HESS: &str = fixture!("hess_optfreq_ethane.hess");
const ETHANE_XYZ: &str = fixture!("hess_optfreq_ethane.xyz");
const SADDLE_HESS: &str = fixture!("hess_saddle.hess");
const SADDLE_XYZ: &str = fixture!("hess_saddle.xyz");

/// Build a `ReferenceGeometry` from a standard `.xyz` (the optimized/final geometry
/// the caller supplies — never read by the reader itself).
fn reference(xyz: &str) -> ReferenceGeometry {
    let mut z = Vec::new();
    let mut coords = Vec::new();
    for line in xyz.lines().skip(2) {
        let t: Vec<&str> = line.split_whitespace().collect();
        if t.len() >= 4 {
            if let Some(zz) = z_of(t[0]) {
                z.push(zz);
                coords.push([
                    t[1].parse().unwrap(),
                    t[2].parse().unwrap(),
                    t[3].parse().unwrap(),
                ]);
            }
        }
    }
    let ids = derived_identity_ids(z.len());
    ReferenceGeometry { z, xyz_angstrom: coords, ids }
}

/// The identity map for a reference — the derived (unit 1d) case every green test uses.
fn map_of(r: &ReferenceGeometry) -> IndexMap<OrcaIndex> {
    identity_map_for(r)
}

#[test]
fn ethane_dimensions_and_is_a_minimum() {
    let r = reference(ETHANE_XYZ);
    let v = HessFile::parse(ETHANE_HESS)
        .verify(&r, &map_of(&r))
        .expect("ethane .hess verifies against its final geometry");
    let f = v.frequencies().unwrap();
    assert_eq!(f.values_cm.len(), 24); // 3N, N=8
    assert_eq!(v.normal_modes().unwrap().n, 24); // 3N × 3N
    assert_eq!(v.ir_spectrum().unwrap().len(), 24);
    // a minimum: no imaginary modes, exactly 6 trans/rot zeros, non-linear.
    assert_eq!(f.imaginary_count, 0);
    assert_eq!(f.zero_count, 6);
    assert!(!f.is_linear);
}

#[test]
fn saddle_has_one_imaginary_mode() {
    let r = reference(SADDLE_XYZ);
    let v = HessFile::parse(SADDLE_HESS)
        .verify(&r, &map_of(&r))
        .expect("saddle .hess verifies (distance-invariant, tolerates the reframe)");
    let f = v.frequencies().unwrap();
    assert_eq!(f.values_cm.len(), 57); // 3N, N=19
    assert_eq!(v.normal_modes().unwrap().n, 57);
    // the transition state: exactly one negative frequency, ≈ -33.66 cm⁻¹.
    assert_eq!(f.imaginary_count, 1);
    let most_negative = f.values_cm.iter().cloned().fold(f64::INFINITY, f64::min);
    assert!((most_negative - (-33.66)).abs() < 0.1, "{most_negative}");
    assert_eq!(f.zero_count, 6);
}

#[test]
fn saddle_geometry_passes_despite_rigid_reframe() {
    // Measured: $atoms on the saddle is translated 1.041 Å vs the input frame, yet
    // interatomic distances match to 4e-8 Å. The distance-based post-condition must
    // pass (a coordinate compare would false-alarm).
    let r = reference(SADDLE_XYZ);
    HessFile::parse(SADDLE_HESS)
        .verify(&r, &map_of(&r))
        .expect("distance-invariant geometry check tolerates the reframe");
}

#[test]
fn missed_bohr_conversion_fails_loudly() {
    // The reader always converts Bohr→Å; to exercise the post-condition we feed a
    // reference in BOHR magnitudes (as if the units disagreed). Interatomic
    // distances then differ by ~1.889×, and the check must reject it.
    let bohr = 0.529_177_210_903_f64;
    let mut r = reference(ETHANE_XYZ);
    for c in &mut r.xyz_angstrom {
        for x in c {
            *x /= bohr; // Bohr-magnitude reference
        }
    }
    match HessFile::parse(ETHANE_HESS).verify(&r, &map_of(&r)) {
        Err(ParseError::GeometryMismatch { max_delta }) => {
            assert!(max_delta > 0.5, "max_delta = {max_delta}");
        }
        other => panic!("expected GeometryMismatch, got {other:?}"),
    }
}

#[test]
fn ir_columns_are_not_transposed() {
    // Measured columns: freq | T² (a.u.) | Int (km/mol) | TX TY TZ. Mode 7 (831.20
    // cm⁻¹) is IR-active — its intensity is ~5.87 km/mol and its T² is tiny; a
    // column swap would put them the other way round.
    let r = reference(ETHANE_XYZ);
    let v = HessFile::parse(ETHANE_HESS).verify(&r, &map_of(&r)).unwrap();
    let ir = v.ir_spectrum().unwrap();
    let row = ir
        .iter()
        .find(|r| (r.frequency_cm - 831.20).abs() < 0.5)
        .expect("mode at 831 cm⁻¹");
    assert!((row.intensity_km_mol - 5.8666).abs() < 0.01, "int={}", row.intensity_km_mol);
    assert!(row.t2_au < 0.01, "t2={}", row.t2_au);
}

#[test]
fn unknown_section_is_surfaced() {
    let text = "\
$atoms
1
 H     1.008      0.0   0.0   0.0
$totally_new_hess_section
  42
$end
";
    let h = HessFile::parse(text);
    assert_eq!(h.unknown_section_names(), vec!["totally_new_hess_section".to_string()]);
}

// --- a synthetic linear diatomic: 5 trans/rot zeros must be accepted ---------

const LINEAR_HESS: &str = "\
$atoms
2
 C    12.011     0.0   0.0   0.0
 O    15.999     0.0   0.0   3.96844
$vibrational_frequencies
6
   0     0.0
   1     0.0
   2     0.0
   3     0.0
   4     0.0
   5   2100.0
$normal_modes
6 6
                 0        1        2        3        4        5
0   0.0 0.0 0.0 0.0 0.0 0.0
1   0.0 0.0 0.0 0.0 0.0 0.0
2   0.0 0.0 0.0 0.0 0.0 1.0
3   0.0 0.0 0.0 0.0 0.0 0.0
4   0.0 0.0 0.0 0.0 0.0 0.0
5   0.0 0.0 0.0 0.0 0.0 -1.0
$ir_spectrum
6
   0.0   0.0   0.0   0.0 0.0 0.0
   0.0   0.0   0.0   0.0 0.0 0.0
   0.0   0.0   0.0   0.0 0.0 0.0
   0.0   0.0   0.0   0.0 0.0 0.0
   0.0   0.0   0.0   0.0 0.0 0.0
2100.0   0.1  50.0   0.0 0.0 1.0
$end
";

#[test]
fn linear_molecule_five_zeros_is_ok_not_an_error() {
    let ref_geom = ReferenceGeometry {
        z: vec![6, 8],
        xyz_angstrom: vec![[0.0, 0.0, 0.0], [0.0, 0.0, 2.1]],
        ids: derived_identity_ids(2),
    };
    let v = HessFile::parse(LINEAR_HESS)
        .verify(&ref_geom, &map_of(&ref_geom))
        .expect("a linear molecule with 5 trans/rot zeros is legal");
    let f = v.frequencies().unwrap();
    assert_eq!(f.zero_count, 5);
    assert!(f.is_linear);
    assert_eq!(f.imaginary_count, 0);
}

#[test]
fn a_bad_zero_count_is_malformed() {
    // Same synthetic, but with a 4th trans/rot mode turned real → 4 zeros, which is
    // neither 5 (linear) nor 6 (non-linear).
    let broken = LINEAR_HESS.replace("   3     0.0", "   3    99.0");
    let ref_geom = ReferenceGeometry {
        z: vec![6, 8],
        xyz_angstrom: vec![[0.0, 0.0, 0.0], [0.0, 0.0, 2.1]],
        ids: derived_identity_ids(2),
    };
    let err = HessFile::parse(&broken).verify(&ref_geom, &map_of(&ref_geom)).unwrap_err();
    assert!(matches!(err, ParseError::Malformed { .. }), "{err:?}");
}
