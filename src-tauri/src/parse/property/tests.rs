//! Tests for the `.property.txt` reader, against **real** ORCA 6.1.0 artifacts
//! (the author's own runs — rule #7 is about not redistributing ORCA itself/its
//! manual; these are our own calculation outputs).

use super::{verify_geometry_atoms, GeomAtom, PropertyFile};
use crate::parse::elements::z_of;
use crate::parse::units::Angstrom;
use crate::parse::{derived_identity_ids, identity_map_for, ParseError, ReferenceGeometry};
use orcastudio_core::ids::{IndexMap, OrcaIndex};

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
const SCAN: &str = fixture!("scan-ethane-cc/input.property.txt");

/// Parse the `* xyz … *` block of an `.inp` into Å coordinates (kept for the
/// `verify_geometry_atoms` post-condition test, which takes bare coords).
fn inp_xyz_angstrom(inp: &str) -> Vec<[f64; 3]> {
    reference(inp).xyz_angstrom
}

/// The full reference (elements + coords + derived identity ids) the caller supplies
/// to `verify`, parsed from an `.inp` `* xyz … *` block.
fn reference(inp: &str) -> ReferenceGeometry {
    let (mut z, mut xyz) = (Vec::new(), Vec::new());
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
                z.push(z_of(toks[0]).unwrap());
                xyz.push([
                    toks[1].parse().unwrap(),
                    toks[2].parse().unwrap(),
                    toks[3].parse().unwrap(),
                ]);
            }
        }
    }
    let ids = derived_identity_ids(z.len());
    ReferenceGeometry { z, xyz_angstrom: xyz, ids }
}

/// The identity map for a reference — the derived (unit 1d) case every green test uses.
fn map_of(r: &ReferenceGeometry) -> IndexMap<OrcaIndex> {
    identity_map_for(r)
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
    let r = reference(OPTFREQ_INP);
    assert_eq!(r.z.len(), 8);
    let v = PropertyFile::parse(OPTFREQ)
        .verify(&r, &map_of(&r))
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
    let bad = ReferenceGeometry {
        z: vec![6],
        xyz_angstrom: vec![[0.0, 0.0, 0.0]], // 1 atom vs ethane's 8
        ids: derived_identity_ids(1),
    };
    let err = PropertyFile::parse(OPTFREQ).verify(&bad, &map_of(&bad)).unwrap_err();
    assert!(matches!(err, ParseError::LengthMismatch { .. }), "{err:?}");
}

#[test]
fn entropy_field_is_t_times_s() {
    // measured: entropyS == enthalpyH − freeEnergyG (so it is T·S in Eh, not S).
    let r = reference(OPTFREQ_INP);
    let v = PropertyFile::parse(OPTFREQ).verify(&r, &map_of(&r)).unwrap();
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
    let r = reference(GOAT_INP);
    let v = PropertyFile::parse(GOAT)
        .verify(&r, &map_of(&r))
        .expect("GOAT verifies (its first geometry == input)");
    assert!(!v.geometries().unwrap().is_empty());
    assert!(v.final_single_point_energy().is_some());
    let ch = v.charges();
    assert!(ch.mulliken.is_none() && ch.loewdin.is_none() && ch.mayer.is_none());
    // Absent block → Ok(None), not an error and not a fabricated dipole.
    assert!(v.dipole().unwrap().is_none());
    assert!(v.thermochemistry().is_none());
    assert!(v.last_gradient().is_none());
}

#[test]
fn sp_has_charges_and_dipole_but_no_thermo() {
    let r = reference(SP_INP);
    let v = PropertyFile::parse(SP).verify(&r, &map_of(&r)).unwrap();
    assert!(v.charges().mulliken.is_some());
    // A present, VALID block parses to the exact probe-confirmed value — the
    // regression that honest parsing didn't break the valid path (`wiki/debugging/024`).
    let d = v
        .dipole()
        .expect("valid block does not error")
        .expect("valid block is Some");
    assert!((d.magnitude_au - 1.1257352328353853).abs() < 1e-12);
    let expect = [-0.60312812185945486, -0.59037359853012172, 0.74496664168794879];
    for (got, want) in d.total_au.iter().zip(expect.iter()) {
        assert!((got - want).abs() < 1e-12, "total_au {got} != {want}");
    }
    assert!(v.thermochemistry().is_none(), "SP has no thermochemistry");
}

/// Isolate the `$SCF_Dipole_Moment` block of the SP fixture and let `f` mutate it,
/// then splice it back so the file still verifies (geometry/energy untouched). The
/// block spans from `$SCF_Dipole_Moment` to the next top-level `$` line.
fn sp_with_mangled_dipole_block(f: impl Fn(&str) -> String) -> String {
    let src = SP;
    let start = src.find("$SCF_Dipole_Moment").expect("SP has a dipole block");
    // The block ends at the next line beginning with `$` (the following top-level block).
    let rest = &src[start + 1..];
    let end_rel = rest.find("\n$").expect("a block follows the dipole block");
    let end = start + 1 + end_rel + 1; // keep the trailing newline, cut at the next `$`
    let block = &src[start..end];
    format!("{}{}{}", &src[..start], f(block), &src[end..])
}

/// A present block whose `&dipoleTotal` is truncated to < 3 components is MALFORMED —
/// a loud `Err`, never a fabricated `(0,0,0)`. (Symmetry: total mandatory when present.)
#[test]
fn dipole_present_but_total_truncated_is_malformed() {
    // Drop the `2  <z>` component row of `&dipoleTotal` → only two components remain.
    let mangled = sp_with_mangled_dipole_block(|b| {
        b.lines()
            .filter(|l| !l.trim_start().starts_with("2 "))
            .collect::<Vec<_>>()
            .join("\n")
            + "\n"
    });
    let r = reference(SP_INP);
    let v = PropertyFile::parse(&mangled).verify(&r, &map_of(&r)).unwrap();
    let err = v.dipole().expect_err("truncated total is malformed, not None");
    match err {
        ParseError::Malformed { field, detail } => {
            assert_eq!(field, "SCF_Dipole_Moment");
            assert!(detail.contains("3 components"), "detail: {detail}");
            assert!(detail.contains("observed 2"), "detail: {detail}");
        }
        other => panic!("expected Malformed, got {other:?}"),
    }
}

/// A present block whose `&dipoleMagnitude` inline value is non-numeric is MALFORMED —
/// the magnitude branch bites the same way the total branch does.
#[test]
fn dipole_present_but_magnitude_garbage_is_malformed() {
    let mangled = sp_with_mangled_dipole_block(|b| {
        b.replace(
            "&dipoleMagnitude [&Type \"Double\", &Units \"a.u.\"]       1.1257352328353853e+00",
            "&dipoleMagnitude [&Type \"Double\", &Units \"a.u.\"]       not_a_number",
        )
    });
    let r = reference(SP_INP);
    let v = PropertyFile::parse(&mangled).verify(&r, &map_of(&r)).unwrap();
    let err = v.dipole().expect_err("garbage magnitude is malformed, not None");
    match err {
        ParseError::Malformed { field, detail } => {
            assert_eq!(field, "SCF_Dipole_Moment");
            assert!(detail.contains("dipoleMagnitude"), "detail: {detail}");
            assert!(detail.contains("not a number"), "detail: {detail}");
        }
        other => panic!("expected Malformed, got {other:?}"),
    }
}

/// A present block with NO `&dipoleMagnitude` line is MALFORMED — the probe established
/// magnitude is mandatory when the block exists (141 blocks / 91 files, always co-present),
/// so its absence is a corruption, not an optional-field `None`. This REPLACES the old
/// N/A "absent-optional magnitude → Ok(Some)" assumption (`wiki/debugging/024`).
#[test]
fn dipole_present_but_magnitude_key_absent_is_malformed() {
    let mangled = sp_with_mangled_dipole_block(|b| {
        b.lines()
            .filter(|l| !l.contains("&dipoleMagnitude"))
            .collect::<Vec<_>>()
            .join("\n")
            + "\n"
    });
    let r = reference(SP_INP);
    let v = PropertyFile::parse(&mangled).verify(&r, &map_of(&r)).unwrap();
    let err = v.dipole().expect_err("missing magnitude key is malformed, not None");
    match err {
        ParseError::Malformed { field, detail } => {
            assert_eq!(field, "SCF_Dipole_Moment");
            assert!(detail.contains("&dipoleMagnitude key is absent"), "detail: {detail}");
        }
        other => panic!("expected Malformed, got {other:?}"),
    }
}

#[test]
fn permuted_map_makes_verify_refuse() {
    // Negative control (a) at the reader level: a map that swaps two NON-equivalent
    // atoms (ethane position 0 = C, position 2 = H) must make verify() refuse with a
    // named order mismatch. The map is cross-checked against the artifact, never
    // trusted — this is what holds a mislabelled parse red.
    let r = reference(OPTFREQ_INP);
    let mut order = r.ids.clone();
    order.swap(0, 2);
    let permuted = IndexMap::<OrcaIndex>::from_emit_order(&order);
    let err = PropertyFile::parse(OPTFREQ).verify(&r, &permuted).unwrap_err();
    assert!(matches!(err, ParseError::OrderMismatch { index: 0, .. }), "{err:?}");
}

#[test]
fn wrong_atom_count_map_makes_verify_refuse() {
    // Negative control (b): a map with one fewer atom than the artifact (7 vs 8) is
    // refused with a named length mismatch.
    let r = reference(OPTFREQ_INP);
    let short = IndexMap::<OrcaIndex>::from_emit_order(&r.ids[..7]);
    let err = PropertyFile::parse(OPTFREQ).verify(&r, &short).unwrap_err();
    assert!(matches!(err, ParseError::LengthMismatch { .. }), "{err:?}");
}

#[test]
fn mayer_charge_read_from_qa() {
    // measured: Mayer's charge field is &QA, not &AtomicCharges.
    let r = reference(OPTFREQ_INP);
    let v = PropertyFile::parse(OPTFREQ).verify(&r, &map_of(&r)).unwrap();
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
        std::path::Path::new(FIX).join("scan-ethane-cc/input.property.txt"),
    )
    .unwrap()
    .len();
    assert!(bytes < 1_000_000, "largest fixture {bytes} B is well under the 16 MB cap");
}
