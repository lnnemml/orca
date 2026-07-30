//! Tests for the `.property.txt` reader, against **real** ORCA 6.1.0 artifacts
//! (the author's own runs — rule #7 is about not redistributing ORCA itself/its
//! manual; these are our own calculation outputs).

use super::{verify_geometry_atoms, GeomAtom, PropertyFile};
use crate::parse::units::Angstrom;
use crate::parse::ParseError;

const FIX: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/");

macro_rules! fixture {
    ($name:literal) => {
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/",
            $name
        ))
    };
}

const OPTFREQ: &str = fixture!("property_optfreq_ethane.property.txt");
const OPTFREQ_INP: &str = fixture!("property_optfreq_ethane.input.inp");
const SP: &str = fixture!("property_sp.property.txt");
const SP_INP: &str = fixture!("property_sp.input.inp");
const GOAT: &str = fixture!("property_goat.property.txt");
const GOAT_INP: &str = fixture!("property_goat.input.inp");
const SCAN: &str = fixture!("property_scan_ethane.property.txt");

/// Parse the `* xyz … *` block of an `.inp` into Å coordinates (the reference the
/// caller supplies to `verify`).
fn inp_xyz_angstrom(inp: &str) -> Vec<[f64; 3]> {
    let mut out = Vec::new();
    let mut inside = false;
    for line in inp.lines() {
        let t = line.trim();
        if t.to_lowercase().starts_with("* xyz") {
            inside = true;
            continue;
        }
        if inside {
            if t.starts_with('*') {
                break;
            }
            let toks: Vec<&str> = t.split_whitespace().collect();
            if toks.len() >= 4 {
                out.push([
                    toks[1].parse().unwrap(),
                    toks[2].parse().unwrap(),
                    toks[3].parse().unwrap(),
                ]);
            }
        }
    }
    out
}

#[test]
fn fixtures_are_present_on_disk() {
    assert!(std::path::Path::new(FIX)
        .join("property_optfreq_ethane.property.txt")
        .exists());
}

#[test]
fn tokenizes_blocks_and_all_are_known() {
    // works on the UNVERIFIED handle — rule-#10 diagnostics need no reference.
    let pf = PropertyFile::parse(OPTFREQ);
    assert!(pf.blocks.len() > 10, "expected many blocks, got {}", pf.blocks.len());
    assert_eq!(pf.unknown_block_names(), Vec::<String>::new());
}

#[test]
fn unverified_handle_verifies_then_exposes_values() {
    // the typestate in one test: parse → verify(reference) → read.
    let reference = inp_xyz_angstrom(OPTFREQ_INP);
    assert_eq!(reference.len(), 8);
    let v = PropertyFile::parse(OPTFREQ)
        .verify(&reference)
        .expect("Opt+Freq verifies against its own input xyz");

    // el energy: measured cross-check with the .out FINAL SINGLE POINT ENERGY.
    let thermo = v.thermochemistry().expect("Opt+Freq has thermochemistry");
    assert!(
        (thermo.el_energy_eh - (-79.7918513760713)).abs() < 1e-9,
        "elEnergy = {}",
        thermo.el_energy_eh
    );

    let geoms = v.geometries().unwrap();
    let elements: Vec<&str> = geoms[0].atoms.iter().map(|a| a.element.as_str()).collect();
    assert_eq!(elements, ["C", "C", "H", "H", "H", "H", "H", "H"]);
}

#[test]
fn missed_bohr_conversion_fails_loudly() {
    // Simulate a reader that FORGOT ×0.529: it treated the Bohr number as if it
    // were already Å (from_angstrom on a Bohr magnitude). The geometry
    // post-condition must reject it at ≈1.889×, not accept a plausible molecule.
    let reference = inp_xyz_angstrom(OPTFREQ_INP);
    let wrong: Vec<GeomAtom> = reference
        .iter()
        .map(|r| {
            let bohr = |a: f64| a / Angstrom::BOHR_TO_ANGSTROM; // the raw Bohr number
            GeomAtom {
                element: "C".into(),
                z: 6,
                xyz: [
                    Angstrom::from_angstrom(bohr(r[0])),
                    Angstrom::from_angstrom(bohr(r[1])),
                    Angstrom::from_angstrom(bohr(r[2])),
                ],
            }
        })
        .collect();

    match verify_geometry_atoms(&wrong, &reference) {
        Err(ParseError::GeometryMismatch { max_delta }) => {
            assert!(max_delta > 0.5, "max_delta = {max_delta}");
        }
        other => panic!("expected GeometryMismatch, got {other:?}"),
    }
}

#[test]
fn verify_rejects_a_wrong_reference() {
    // A reference of the wrong length is a caller error the post-condition catches.
    let bad_reference = vec![[0.0, 0.0, 0.0]]; // 1 atom vs ethane's 8
    let err = PropertyFile::parse(OPTFREQ).verify(&bad_reference).unwrap_err();
    assert!(matches!(err, ParseError::LengthMismatch { .. }), "{err:?}");
}

#[test]
fn entropy_field_is_t_times_s() {
    // measured: entropyS == enthalpyH − freeEnergyG (so it is T·S in Eh, not S).
    let v = PropertyFile::parse(OPTFREQ)
        .verify(&inp_xyz_angstrom(OPTFREQ_INP))
        .unwrap();
    let t = v.thermochemistry().unwrap();
    assert!(
        (t.t_times_s_eh - (t.enthalpy_h_eh - t.free_energy_g_eh)).abs() < 1e-9,
        "t_times_s_eh={} H-G={}",
        t.t_times_s_eh,
        t.enthalpy_h_eh - t.free_energy_g_eh
    );
}

#[test]
fn goat_verifies_and_absent_blocks_are_none() {
    // GOAT has only $Geometry + $Single_Point_Data — a reader that crashes here is
    // a bug (measured: no charges/dipole/thermo).
    let v = PropertyFile::parse(GOAT)
        .verify(&inp_xyz_angstrom(GOAT_INP))
        .expect("GOAT verifies (its first geometry == input)");
    assert!(!v.geometries().unwrap().is_empty());
    assert!(v.final_single_point_energy().is_some());
    let ch = v.charges();
    assert!(ch.mulliken.is_none() && ch.loewdin.is_none() && ch.mayer.is_none());
    assert!(v.dipole().is_none());
    assert!(v.thermochemistry().is_none());
    assert!(v.last_gradient().is_none());
}

#[test]
fn sp_has_charges_and_dipole_but_no_thermo() {
    let v = PropertyFile::parse(SP).verify(&inp_xyz_angstrom(SP_INP)).unwrap();
    assert!(v.charges().mulliken.is_some());
    assert!(v.dipole().is_some());
    assert!(v.thermochemistry().is_none(), "SP has no thermochemistry");
}

#[test]
fn mayer_charge_read_from_qa() {
    // measured: Mayer's charge field is &QA, not &AtomicCharges.
    let v = PropertyFile::parse(OPTFREQ)
        .verify(&inp_xyz_angstrom(OPTFREQ_INP))
        .unwrap();
    let mayer = v.charges().mayer.expect("Opt+Freq has a Mayer block");
    assert_eq!(mayer.charges.len(), 8);
    assert_eq!(mayer.atomic_numbers.len(), 8);
}

#[test]
fn scan_geometry_blocks_are_per_cycle_not_scan_points() {
    // measured: a 6-point relaxed scan has 26 $Geometry blocks (opt cycles), NOT 6
    // scan points. Structural count on the raw blocks — no verify needed (and the
    // scan's first geometry is already constrained, so it would not match the input
    // xyz anyway).
    let n = PropertyFile::parse(SCAN)
        .blocks
        .iter()
        .filter(|b| b.name == "Geometry")
        .count();
    assert_eq!(n, 26);
}

#[test]
fn unknown_block_is_surfaced_not_dropped() {
    let text = "\
$Geometry
   &GeometryIndex 1
   &NAtoms [&Type \"Integer\"] 1
   &CartesianCoordinates [&Type \"Coordinates\", &Dim(1,4), &Units \"Bohr\"]
              H      0.000000000000    0.000000000000    0.000000000000
$End
$Totally_New_ORCA_62_Block
   &something [&Type \"Double\"] 1.0
$End
";
    let pf = PropertyFile::parse(text);
    assert_eq!(
        pf.unknown_block_names(),
        vec!["Totally_New_ORCA_62_Block".to_string()]
    );
}

#[test]
fn refuses_a_pathological_size() {
    let bytes = std::fs::metadata(
        std::path::Path::new(FIX).join("property_scan_ethane.property.txt"),
    )
    .unwrap()
    .len();
    assert!(bytes < 1_000_000, "largest fixture {bytes} B is well under the 16 MB cap");
}
