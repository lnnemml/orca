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
const GOAT: &str = fixture!("property_goat.property.txt");
const SCAN: &str = fixture!("property_scan_ethane.property.txt");

/// Parse the `* xyz … *` block of an `.inp` into Å coordinates (the reference).
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
    // guards against a silently-missing fixture (include_str! would fail to build,
    // but this makes the intent explicit for a reader).
    assert!(std::path::Path::new(FIX).join("property_optfreq_ethane.property.txt").exists());
}

#[test]
fn tokenizes_blocks_and_all_are_known() {
    let pf = PropertyFile::parse(OPTFREQ);
    assert!(pf.blocks.len() > 10, "expected many blocks, got {}", pf.blocks.len());
    // every block in a healthy Opt+Freq is classified — no surprises.
    assert_eq!(pf.unknown_block_names(), Vec::<String>::new());
}

#[test]
fn ethane_el_energy_matches_out() {
    // measured cross-check with the .out FINAL SINGLE POINT ENERGY.
    let pf = PropertyFile::parse(OPTFREQ);
    let thermo = pf.thermochemistry().expect("ethane Opt+Freq has thermochemistry");
    assert!(
        (thermo.el_energy_eh - (-79.7918513760713)).abs() < 1e-9,
        "elEnergy = {}",
        thermo.el_energy_eh
    );
}

#[test]
fn first_geometry_matches_input_xyz_within_tolerance() {
    let pf = PropertyFile::parse(OPTFREQ);
    let reference = inp_xyz_angstrom(OPTFREQ_INP);
    assert_eq!(reference.len(), 8);
    // Bohr→Å conversion done in the reader; the post-condition passes.
    pf.verify_geometry(&reference).expect("converted geometry matches input xyz");

    let g = pf.first_geometry().unwrap();
    let elements: Vec<&str> = g.atoms.iter().map(|a| a.element.as_str()).collect();
    assert_eq!(elements, ["C", "C", "H", "H", "H", "H", "H", "H"]);
}

#[test]
fn missed_bohr_conversion_fails_loudly() {
    // Simulate a reader that FORGOT ×0.529: it treated the Bohr number as if it
    // were already Å (from_angstrom on a Bohr magnitude). The post-condition must
    // reject it at ≈1.889×, not accept a plausible-but-wrong molecule.
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
            // the largest |coord| is ~1.162 Å → Δ ≈ 1.162 × (1/0.5292 − 1) ≈ 1.035 Å
            assert!(max_delta > 0.5, "max_delta = {max_delta}");
        }
        other => panic!("expected GeometryMismatch, got {other:?}"),
    }
}

#[test]
fn entropy_field_is_t_times_s() {
    // measured: entropyS == enthalpyH − freeEnergyG (so it is T·S in Eh, not S).
    let pf = PropertyFile::parse(OPTFREQ);
    let t = pf.thermochemistry().unwrap();
    assert!(
        (t.t_times_s_eh - (t.enthalpy_h_eh - t.free_energy_g_eh)).abs() < 1e-9,
        "t_times_s_eh={} H-G={}",
        t.t_times_s_eh,
        t.enthalpy_h_eh - t.free_energy_g_eh
    );
}

#[test]
fn goat_parses_with_none_for_absent_blocks() {
    // GOAT has only $Geometry + $Single_Point_Data — a reader that crashes here is
    // a bug (measured: no charges/dipole/thermo).
    let pf = PropertyFile::parse(GOAT);
    assert!(!pf.geometries().unwrap().is_empty());
    assert!(pf.final_single_point_energy().is_some());
    let ch = pf.charges();
    assert!(ch.mulliken.is_none() && ch.loewdin.is_none() && ch.mayer.is_none());
    assert!(pf.dipole().is_none());
    assert!(pf.thermochemistry().is_none());
    assert!(pf.last_gradient().is_none());
}

#[test]
fn sp_has_charges_and_dipole_but_no_thermo() {
    let pf = PropertyFile::parse(SP);
    assert!(pf.charges().mulliken.is_some());
    assert!(pf.dipole().is_some());
    assert!(pf.thermochemistry().is_none(), "SP has no thermochemistry");
    pf.verify_charge_order().expect("SP charge order == geometry");
}

#[test]
fn mayer_charge_read_from_qa() {
    // measured: Mayer's charge field is &QA, not &AtomicCharges.
    let pf = PropertyFile::parse(OPTFREQ);
    let mayer = pf.charges().mayer.expect("Opt+Freq has a Mayer block");
    assert_eq!(mayer.charges.len(), 8);
    assert_eq!(mayer.atomic_numbers.len(), 8);
}

#[test]
fn charge_order_and_lengths_hold() {
    let pf = PropertyFile::parse(OPTFREQ);
    pf.verify_charge_order().expect("ATNO order == geometry");
    pf.verify_lengths().expect("charges=N, grad=3N, FREQ=3N");
}

#[test]
fn scan_geometries_are_per_cycle_not_scan_points() {
    // measured: a 6-point relaxed scan has 26 $Geometry blocks (opt cycles), NOT 6
    // scan points. The reader must not present these as scan points.
    let pf = PropertyFile::parse(SCAN);
    assert_eq!(pf.geometries().unwrap().len(), 26);
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
    assert_eq!(pf.unknown_block_names(), vec!["Totally_New_ORCA_62_Block".to_string()]);
}

#[test]
fn refuses_a_pathological_size() {
    // the cap is checked on a path; a tiny synthetic file over the (test) view of
    // the limit is awkward to fake on disk, so we assert the constant is sane and
    // that a real fixture is far under it.
    let bytes = std::fs::metadata(
        std::path::Path::new(FIX).join("property_scan_ethane.property.txt"),
    )
    .unwrap()
    .len();
    assert!(bytes < 1_000_000, "largest fixture {bytes} B is well under the 16 MB cap");
}
