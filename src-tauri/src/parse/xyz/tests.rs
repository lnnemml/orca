//! Tests for the xyz-trajectory reader, against REAL ORCA 6.1.0 artifacts.

use super::XyzFile;
use crate::parse::elements::z_of;
use crate::parse::{derived_identity_ids, identity_map_for, ParseError, ReferenceGeometry};
use orcastudio_core::ids::{IndexMap, OrcaIndex};

/// The identity map for a reference — the derived (unit 1d) case every green test uses.
fn map_of(r: &ReferenceGeometry) -> IndexMap<OrcaIndex> {
    identity_map_for(r)
}

macro_rules! fixture {
    ($name:literal) => {
        include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/", $name))
    };
}

const TRJ_ETHANE: &str = fixture!("xyz_trj_ethane.xyz");
const FINAL_ETHANE: &str = fixture!("xyz_final_ethane.xyz");
const TRJ_SCAN: &str = fixture!("xyz_trj_scan.xyz");
const ETHANE_INP: &str = fixture!("property_optfreq_ethane.input.inp");

/// Reference from a `.inp` `* xyz *` block (the start geometry).
fn ref_from_inp(inp: &str) -> ReferenceGeometry {
    let (mut z, mut xyz) = (Vec::new(), Vec::new());
    let mut inside = false;
    for l in inp.lines() {
        let t = l.trim();
        if t.to_lowercase().starts_with("* xyz") {
            inside = true;
            continue;
        }
        if inside {
            if t.starts_with('*') {
                break;
            }
            let k: Vec<&str> = t.split_whitespace().collect();
            if k.len() >= 4 {
                if let Some(zz) = z_of(k[0]) {
                    z.push(zz);
                    xyz.push([k[1].parse().unwrap(), k[2].parse().unwrap(), k[3].parse().unwrap()]);
                }
            }
        }
    }
    let ids = derived_identity_ids(z.len());
    ReferenceGeometry { z, xyz_angstrom: xyz, ids }
}

/// Reference from the first frame of an xyz (for a `.xyz` final geometry).
fn ref_from_first_frame(xyz_text: &str) -> ReferenceGeometry {
    let lines: Vec<&str> = xyz_text.lines().collect();
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
fn ethane_trj_frames_and_comment_energy() {
    // _trj first frame == the input start geometry.
    let r = ref_from_inp(ETHANE_INP);
    let v = XyzFile::parse(TRJ_ETHANE)
        .unwrap()
        .verify(&r, &map_of(&r))
        .expect("ethane _trj verifies against the input start geometry");
    let frames = v.frames();
    assert_eq!(frames.len(), 5); // measured
    assert_eq!(frames[0].atoms.len(), 8);
    let elems: Vec<&str> = frames[0].atoms.iter().map(|a| a.element.as_str()).collect();
    assert_eq!(elems, ["C", "C", "H", "H", "H", "H", "H", "H"]);
    // comment energy: `Coordinates from ORCA-job input E -79.791800280837`.
    let e = frames[0].energy_eh.expect("frame comment carries an energy");
    assert!((e - (-79.791800280837)).abs() < 1e-9, "{e}");
}

#[test]
fn final_xyz_is_a_single_frame() {
    let r = ref_from_first_frame(FINAL_ETHANE);
    let v = XyzFile::parse(FINAL_ETHANE)
        .unwrap()
        .verify(&r, &map_of(&r))
        .unwrap();
    assert_eq!(v.frames().len(), 1); // .xyz = final geometry, one frame
}

#[test]
fn scan_trj_is_26_frames_not_scan_points() {
    // 26 opt cycles for a 6-point scan — the reader exposes *frames*, never "scan
    // points". Verified against the scan trajectory's own first frame (its first
    // frame is a constrained opt step, not the input start).
    let r = ref_from_first_frame(TRJ_SCAN);
    let v = XyzFile::parse(TRJ_SCAN)
        .unwrap()
        .verify(&r, &map_of(&r))
        .unwrap();
    assert_eq!(v.frames().len(), 26);
}

#[test]
fn every_frame_natom_is_constant_and_ordered() {
    // verify() enforces natom-constant + element order on every frame; the scan trj
    // (26 frames) is the strongest available test.
    let r = ref_from_first_frame(TRJ_SCAN);
    XyzFile::parse(TRJ_SCAN)
        .unwrap()
        .verify(&r, &map_of(&r))
        .expect("all 26 scan frames share natom and element order");
}

#[test]
fn missed_conversion_fails_loudly() {
    // Coords are Å (from_angstrom is correct). Feed a BOHR-magnitude reference (as
    // if the units disagreed) — the geometry post-condition must reject the ~1.889×.
    let bohr = 0.529_177_210_903_f64;
    let mut r = ref_from_inp(ETHANE_INP);
    for c in &mut r.xyz_angstrom {
        for x in c {
            *x /= bohr;
        }
    }
    match XyzFile::parse(TRJ_ETHANE).unwrap().verify(&r, &map_of(&r)) {
        Err(ParseError::GeometryMismatch { max_delta }) => assert!(max_delta > 0.3, "{max_delta}"),
        other => panic!("expected GeometryMismatch, got {other:?}"),
    }
}

#[test]
fn malformed_frame_is_an_error() {
    // declares 3 atoms, provides 2.
    let bad = "3\ncomment\nC 0 0 0\nH 0 0 1\n";
    assert!(matches!(XyzFile::parse(bad), Err(ParseError::Malformed { .. })));
}

#[test]
fn comment_without_energy_is_none_not_a_failure() {
    let text = "1\njust a comment, no energy\nH 0.0 0.0 0.0\n";
    let r = ReferenceGeometry { z: vec![1], xyz_angstrom: vec![[0.0, 0.0, 0.0]], ids: derived_identity_ids(1) };
    let v = XyzFile::parse(text).unwrap().verify(&r, &map_of(&r)).unwrap();
    assert_eq!(v.frames()[0].energy_eh, None);
}

#[test]
fn empty_has_no_frames_and_fails_verify() {
    let r = ReferenceGeometry { z: vec![1], xyz_angstrom: vec![[0.0, 0.0, 0.0]], ids: derived_identity_ids(1) };
    let err = XyzFile::parse("").unwrap().verify(&r, &map_of(&r)).unwrap_err();
    assert!(matches!(err, ParseError::MissingField(_)), "{err:?}");
}
