//! Tests for the `orca_2json` MO reader, against a REAL gbw→JSON fixture (198 KB,
//! ethane) that DOES contain `MOCoefficients` — so the streaming skip is exercised.

use std::io::Write;

use super::{classify_geometry, GeometryVerdict, MoJson};
use crate::parse::elements::z_of;
use crate::parse::{derived_identity_ids, identity_map_for, ParseError, ReferenceGeometry};
use orcastudio_core::ids::{IndexMap, OrcaIndex};

/// The identity map for a reference — the derived (unit 1d) case every green test uses.
fn map_of(r: &ReferenceGeometry) -> IndexMap<OrcaIndex> {
    identity_map_for(r)
}

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
    let ids = derived_identity_ids(z.len());
    ReferenceGeometry { z, xyz_angstrom: xyz, ids }
}

#[test]
fn ethane_orbitals_and_homo_lumo() {
    let r = reference();
    let v = MoJson::from_path(&ethane_json_path())
        .unwrap()
        .verify(&r, &map_of(&r))
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
    // disagree by ~0.529× → the ratio signature must reject it as a UNIT error.
    // BITE: the old single-threshold code returned GeometryMismatch here, conflating
    // a real unit error with any over-tolerance mismatch — the ratio test separates
    // them so benign staleness (below) can pass without masking this.
    let bohr = 0.529_177_210_903_f64;
    let mut r = reference();
    for c in &mut r.xyz_angstrom {
        for x in c {
            *x /= bohr;
        }
    }
    match MoJson::from_path(&ethane_json_path()).unwrap().verify(&r, &map_of(&r)) {
        Err(ParseError::GeometryUnitError { .. }) => {}
        other => panic!("expected GeometryUnitError, got {:?}", other.err()),
    }
}

// --- classify_geometry: the three-way verdict, pure (rule #11) ------------------ //
// Reference distances of a small molecule (Å); the classifier is fed (json, ref)
// pairs directly so each verdict is exercised without a full MoJson.
const REF_DISTS: [f64; 5] = [1.09, 1.53, 2.18, 2.50, 3.00];

#[test]
fn unit_error_caught() {
    // json distances = reference × 1.889 (a skipped Bohr→Å) → the ratio signature
    // must classify this as a UNIT error, NOT a generic mismatch. BITE: the old
    // single-threshold code called any over-tolerance delta a GeometryMismatch,
    // conflating a unit error with a mere mismatch.
    let pairs: Vec<(f64, f64)> = REF_DISTS.iter().map(|&r| (r * 1.8897, r)).collect();
    match classify_geometry(&pairs) {
        GeometryVerdict::UnitError { ratio } => assert!((ratio - 1.8897).abs() < 1e-2, "{ratio}"),
        other => panic!("expected UnitError, got {other:?}"),
    }
}

#[test]
fn staleness_passes() {
    // Reference perturbed by ~0.03 Å (SAME unit, small — measured `.gbw` staleness on
    // the plain-Opt MeNH₂+EtI jobs) → PASS. BITE: the old 1e-4 single threshold FAILS
    // at this magnitude, which is exactly the false ParseFailed this unit fixes.
    let pairs: Vec<(f64, f64)> = REF_DISTS.iter().map(|&r| (r + 0.03, r)).collect();
    assert_eq!(classify_geometry(&pairs), GeometryVerdict::Pass);
}

#[test]
fn real_mismatch_fails() {
    // Same unit (ratio ≈ 1) but a genuinely different structure (one pair off by
    // ~1 Å) → GeometryMismatch, NOT UnitError and NOT pass.
    let pairs = [(2.5_f64, 1.5), (2.6, 2.5), (3.1, 3.0), (4.2, 4.1), (1.2, 1.1)];
    match classify_geometry(&pairs) {
        GeometryVerdict::Mismatch { max_delta } => assert!(max_delta > 0.5, "{max_delta}"),
        other => panic!("expected Mismatch, got {other:?}"),
    }
}

#[test]
fn fast_path_still_passes() {
    // Δ < 1e-4 (Opt+Freq / scan common case) → PASS unchanged (no regression).
    let pairs: Vec<(f64, f64)> = REF_DISTS.iter().map(|&r| (r + 1e-6, r)).collect();
    assert_eq!(classify_geometry(&pairs), GeometryVerdict::Pass);
}

#[test]
fn wrong_element_order_is_rejected() {
    let mut r = reference();
    r.z.swap(0, 2); // C↔H — the energies would sit on the wrong atoms
    match MoJson::from_path(&ethane_json_path()).unwrap().verify(&r, &map_of(&r)) {
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
