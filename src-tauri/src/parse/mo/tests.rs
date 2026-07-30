//! Tests for the `orca_2json` MO reader, against a REAL gbw→JSON fixture (198 KB,
//! ethane) that DOES contain `MOCoefficients` — so the streaming skip is exercised.

use std::io::Write;

use super::MoJson;
use crate::parse::elements::z_of;
use crate::parse::{ParseError, ReferenceGeometry};

const FINAL_ETHANE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/xyz_final_ethane.xyz"
));

fn ethane_json_path() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/mo_ethane.json")
}

/// Reference (final geometry) from the ethane `.xyz`.
fn reference() -> ReferenceGeometry {
    let lines: Vec<&str> = FINAL_ETHANE.lines().collect();
    let n: usize = lines[0].trim().parse().unwrap();
    let (mut z, mut xyz) = (Vec::new(), Vec::new());
    for l in lines.iter().skip(2).take(n) {
        let k: Vec<&str> = l.split_whitespace().collect();
        z.push(z_of(k[0]).unwrap());
        xyz.push([k[1].parse().unwrap(), k[2].parse().unwrap(), k[3].parse().unwrap()]);
    }
    ReferenceGeometry { z, xyz_angstrom: xyz }
}

#[test]
fn ethane_orbitals_and_homo_lumo() {
    let v = MoJson::from_path(&ethane_json_path())
        .unwrap()
        .verify(&reference())
        .expect("ethane gbw-json verifies against its final geometry");
    let orbitals = v.orbitals();
    assert_eq!(orbitals.len(), 68); // measured nMO
    assert_eq!(v.energy_unit(), Some("Eh"));

    // HOMO occupied (2.0), LUMO virtual (0.0), gap positive.
    let (homo, lumo, gap) = v.homo_lumo().expect("closed-shell ethane has a HOMO/LUMO");
    assert!(homo < 0.0, "HOMO should be bound: {homo}");
    assert!(lumo > homo, "LUMO above HOMO");
    assert!(gap > 0.0 && (gap - (lumo - homo)).abs() < 1e-12);
}

#[test]
fn coefficients_are_skipped_not_a_field() {
    // The fixture contains MOCoefficients; the reader must parse fine without them
    // (they are streamed past as IgnoredAny). We can't observe memory here, but the
    // struct simply has no such field — this proves parsing succeeds regardless.
    let json = std::fs::read_to_string(ethane_json_path()).unwrap();
    assert!(json.contains("MOCoefficients"), "fixture must contain the heavy field");
    assert!(MoJson::from_path(&ethane_json_path()).is_ok());
}

#[test]
fn missed_conversion_fails_loudly() {
    // Coords are Å (from_angstrom). A BOHR-magnitude reference makes distances
    // disagree by ~1.889× → the geometry post-condition must reject it.
    let bohr = 0.529_177_210_903_f64;
    let mut r = reference();
    for c in &mut r.xyz_angstrom {
        for x in c {
            *x /= bohr;
        }
    }
    match MoJson::from_path(&ethane_json_path()).unwrap().verify(&r) {
        Err(ParseError::GeometryMismatch { .. }) => {}
        other => panic!("expected GeometryMismatch, got {:?}", other.err()),
    }
}

#[test]
fn wrong_element_order_is_rejected() {
    let mut r = reference();
    r.z.swap(0, 2); // C↔H — the energies would sit on the wrong atoms
    match MoJson::from_path(&ethane_json_path()).unwrap().verify(&r) {
        Err(ParseError::OrderMismatch { .. }) => {}
        other => panic!("expected OrderMismatch, got {:?}", other.err()),
    }
}

#[test]
fn malformed_json_is_an_error() {
    let mut tmp = tempfile();
    write!(tmp.0, "{{not valid json").unwrap();
    assert!(matches!(
        MoJson::from_path(&tmp.1),
        Err(ParseError::Malformed { .. })
    ));
}

/// A throwaway temp file (path auto-removed on drop).
struct TmpFile(std::fs::File, std::path::PathBuf);
impl Drop for TmpFile {
    fn drop(&mut self) {
        std::fs::remove_file(&self.1).ok();
    }
}
fn tempfile() -> TmpFile {
    let p = std::env::temp_dir().join(format!("mo-test-{}.json", std::process::id()));
    TmpFile(std::fs::File::create(&p).unwrap(), p)
}
