//! Cross-version regression: ORCA **6.1.0** vs **6.1.1** must produce identical
//! *parsed* fields. The real readers run over matched fixture pairs under
//! `tests/fixtures/xver/<case>/{v610,v611}/`, each generated from the SAME input by
//! `/opt/orca-6.1.0/orca` and `/opt/orca-6.1.1/orca` (nprocs 4, `%maxcore 2000`,
//! isolated dir per run — rules #1/#3/#6). See `tests/fixtures/xver/README.md`.
//!
//! Tolerances are the project's, NOT loosened to pass:
//!   - energies   `|Δ| ≤ 1e-6` Eh
//!   - frequencies `|Δ| ≤ 0.1` cm⁻¹
//!   - geometries `|Δ| ≤ 1e-4` Å (per coordinate)
//!   - every non-numeric field: **strictly equal**
//!   - **Option presence must match** — a `null` where the other side has a value is
//!     a failure (honest-or-absent: a vanished value is a regression like a wrong one).
//!
//! NEB-TS is deliberately absent — HCN↔HNC at r2SCAN-3c does not finish within the
//! 10-minute laptop guard (the climbing-image TS + numerical Hessian tail), so no
//! clean 6.1.1 pair exists yet. It is not silently dropped: see
//! [`neb_ts_cross_version_deferred`] (an `#[ignore]` marker) and the session report.

use std::path::{Path, PathBuf};

use super::elements::z_of;
use super::hess::HessFile;
use super::mayer::read_mayer;
use super::property::PropertyFile;
use super::relaxscan::{parse_scan_spec, RelaxScan};
use super::xyz::XyzFile;
use super::{derived_identity_ids, identity_map_for, ReferenceGeometry};
use orcastudio_core::ids::{IndexMap, OrcaIndex};

const E_TOL: f64 = 1e-6; // Eh
const F_TOL: f64 = 0.1; // cm⁻¹
const G_TOL: f64 = 1e-4; // Å

// ── fixture plumbing ──────────────────────────────────────────────────────────

fn xver(case: &str, ver: &str, file: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/xver")
        .join(case)
        .join(ver)
        .join(file)
}

fn read(case: &str, ver: &str, file: &str) -> String {
    std::fs::read_to_string(xver(case, ver, file))
        .unwrap_or_else(|e| panic!("fixture {case}/{ver}/{file}: {e}"))
}

/// Parse the `* xyz … *` block of an `.inp` into the reference the readers verify
/// against (same helper the per-reader tests use).
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

/// Build a reference from an `.xyz` file (count, comment, then `El x y z` rows). The
/// `.hess` holds the **final** (optimized / TS) geometry, which differs from the input
/// block — so the hess post-condition must be checked against the final geometry ORCA
/// wrote to `<base>.xyz`, not the input coordinates.
fn reference_from_xyz(xyz: &str) -> ReferenceGeometry {
    let (mut z, mut coords) = (Vec::new(), Vec::new());
    for line in xyz.lines().skip(2) {
        let toks: Vec<&str> = line.split_whitespace().collect();
        if toks.len() >= 4 {
            if let Some(zi) = z_of(toks[0]) {
                z.push(zi);
                coords.push([
                    toks[1].parse().unwrap(),
                    toks[2].parse().unwrap(),
                    toks[3].parse().unwrap(),
                ]);
            }
        }
    }
    let ids = derived_identity_ids(z.len());
    ReferenceGeometry { z, xyz_angstrom: coords, ids }
}

fn map_of(r: &ReferenceGeometry) -> IndexMap<OrcaIndex> {
    identity_map_for(r)
}

/// The tolerance gate itself, as a pure predicate so the negative control can show
/// it both passes (identical) and bites (perturbed) without a process panic.
fn within(a: f64, b: f64, tol: f64) -> bool {
    (a - b).abs() <= tol
}

fn assert_close(label: &str, a: f64, b: f64, tol: f64) {
    assert!(
        within(a, b, tol),
        "{label}: 6.1.0={a} 6.1.1={b} Δ={} > tol {tol}",
        (a - b).abs()
    );
}

/// Verify + return the property handle for one version of a case.
fn property(case: &str, ver: &str, r: &ReferenceGeometry, m: &IndexMap<OrcaIndex>) -> super::property::Verified {
    PropertyFile::parse(&read(case, ver, "input.property.txt"))
        .verify(r, m)
        .unwrap_or_else(|e| panic!("{case}/{ver} property verify: {e:?}"))
}

/// Compare the final single-point energy of a case across both versions. Shared by
/// every property-bearing case; also asserts the value is present on BOTH sides.
fn assert_final_energy_identical(case: &str, r: &ReferenceGeometry, m: &IndexMap<OrcaIndex>) {
    let a = property(case, "v610", r, m).final_single_point_energy();
    let b = property(case, "v611", r, m).final_single_point_energy();
    assert!(
        a.is_some() && b.is_some(),
        "{case}: final energy must be present on both versions (got {a:?} / {b:?})"
    );
    assert_close(&format!("{case} final energy"), a.unwrap(), b.unwrap(), E_TOL);
}

/// Compare the LAST geometry (final optimized structure) across both versions.
fn assert_final_geometry_identical(case: &str, r: &ReferenceGeometry, m: &IndexMap<OrcaIndex>) {
    let ga = property(case, "v610", r, m).geometries().unwrap();
    let gb = property(case, "v611", r, m).geometries().unwrap();
    assert_eq!(ga.len(), gb.len(), "{case}: geometry count");
    let (la, lb) = (ga.last().unwrap(), gb.last().unwrap());
    assert_eq!(la.atoms.len(), lb.atoms.len(), "{case}: final atom count");
    for (x, y) in la.atoms.iter().zip(&lb.atoms) {
        assert_eq!(x.z, y.z, "{case}: element order");
        assert_eq!(x.element, y.element, "{case}: element symbol");
        for k in 0..3 {
            assert_close(
                &format!("{case} final geom coord[{k}] of Z={}", x.z),
                x.xyz[k].angstrom(),
                y.xyz[k].angstrom(),
                G_TOL,
            );
        }
    }
}

// ── per-case cross-version equivalence ──────────────────────────────────────────

#[test]
fn opt_freq_water_property_identical() {
    let r = reference(&read("opt_freq_water", "v610", "input.inp"));
    let m = map_of(&r);
    assert_final_energy_identical("opt_freq_water", &r, &m);
    assert_final_geometry_identical("opt_freq_water", &r, &m);
}

#[test]
fn opt_freq_water_frequencies_identical() {
    // the .hess carries the FINAL (optimized) geometry — verify against it, not input.
    let r = reference_from_xyz(&read("opt_freq_water", "v610", "input.xyz"));
    let m = map_of(&r);
    let load = |v: &str| {
        HessFile::parse(&read("opt_freq_water", v, "input.hess"))
            .verify(&r, &m)
            .unwrap()
    };
    let (a, b) = (load("v610"), load("v611"));
    let (fa, fb) = (a.frequencies().unwrap(), b.frequencies().unwrap());
    assert_eq!(fa.values_cm.len(), fb.values_cm.len(), "freq count");
    for (i, (x, y)) in fa.values_cm.iter().zip(&fb.values_cm).enumerate() {
        assert_close(&format!("opt_freq_water freq[{i}]"), *x, *y, F_TOL);
    }
    // non-numeric / structural fields: strictly equal.
    assert_eq!(fa.imaginary_count, fb.imaginary_count, "imaginary_count");
    assert_eq!(fa.zero_count, fb.zero_count, "zero_count");
    assert_eq!(fa.is_linear, fb.is_linear, "is_linear");
    // IR intensities (km/mol) within freq tolerance's sibling — compare as energies-ish:
    let (ia, ib) = (a.ir_spectrum().unwrap(), b.ir_spectrum().unwrap());
    assert_eq!(ia.len(), ib.len(), "ir row count");
    for (i, (x, y)) in ia.iter().zip(&ib).enumerate() {
        assert_close(&format!("opt_freq_water ir freq[{i}]"), x.frequency_cm, y.frequency_cm, F_TOL);
        // intensity is derived from the same dipole-derivative numerics; require tight equality.
        assert_close(
            &format!("opt_freq_water ir intensity[{i}]"),
            x.intensity_km_mol,
            y.intensity_km_mol,
            1e-3,
        );
    }
}

#[test]
fn opt_freq_water_trajectory_identical() {
    let r = reference(&read("opt_freq_water", "v610", "input.inp"));
    let m = map_of(&r);
    let load = |v: &str| {
        XyzFile::parse(&read("opt_freq_water", v, "input_trj.xyz"))
            .unwrap()
            .verify(&r, &m)
            .unwrap()
    };
    let (a, b) = (load("v610"), load("v611"));
    let (fa, fb) = (a.frames(), b.frames());
    assert_eq!(fa.len(), fb.len(), "trajectory frame count");
    // final frame geometry + its energy (Option presence must match).
    let (la, lb) = (fa.last().unwrap(), fb.last().unwrap());
    assert_eq!(
        la.energy_eh.is_some(),
        lb.energy_eh.is_some(),
        "final-frame energy presence"
    );
    if let (Some(ea), Some(eb)) = (la.energy_eh, lb.energy_eh) {
        assert_close("opt_freq_water trj final-frame energy", ea, eb, E_TOL);
    }
    assert_eq!(la.atoms.len(), lb.atoms.len(), "final-frame atom count");
    for (x, y) in la.atoms.iter().zip(&lb.atoms) {
        assert_eq!(x.z, y.z, "trj element order");
        for k in 0..3 {
            assert_close("opt_freq_water trj coord", x.xyz[k].angstrom(), y.xyz[k].angstrom(), G_TOL);
        }
    }
}

#[test]
fn opt_freq_water_mayer_identical() {
    let natoms = 3; // water
    let a = read_mayer(&xver("opt_freq_water", "v610", "mayer_tail.out"), natoms)
        .unwrap()
        .expect("mayer block present in 6.1.0");
    let b = read_mayer(&xver("opt_freq_water", "v611", "mayer_tail.out"), natoms)
        .unwrap()
        .expect("mayer block present in 6.1.1");
    assert_eq!(a.len(), b.len(), "mayer bond count");
    for (x, y) in a.iter().zip(&b) {
        assert_eq!((x.i, x.j), (y.i, y.j), "mayer bond atom pair");
        assert_close(&format!("mayer order ({}-{})", x.i, x.j), x.order, y.order, 1e-4);
    }
}

#[test]
fn smd_water_energy_identical() {
    let r = reference(&read("smd_water", "v610", "input.inp"));
    let m = map_of(&r);
    assert_final_energy_identical("smd_water", &r, &m);
}

#[test]
fn dlpno_ccsdt_hcn_energy_identical() {
    let r = reference(&read("dlpno_hcn", "v610", "input.inp"));
    let m = map_of(&r);
    assert_final_energy_identical("dlpno_hcn", &r, &m);
}

#[test]
fn optts_hcn_property_and_frequencies_identical() {
    let r = reference(&read("optts_hcn", "v610", "input.inp"));
    let m = map_of(&r);
    assert_final_energy_identical("optts_hcn", &r, &m);
    assert_final_geometry_identical("optts_hcn", &r, &m);
    // the .hess carries the located-TS geometry — verify against it, not the guess.
    let rh = reference_from_xyz(&read("optts_hcn", "v610", "input.xyz"));
    let mh = map_of(&rh);
    let load = |v: &str| {
        HessFile::parse(&read("optts_hcn", v, "input.hess"))
            .verify(&rh, &mh)
            .unwrap()
    };
    let (a, b) = (load("v610"), load("v611"));
    let (fa, fb) = (a.frequencies().unwrap(), b.frequencies().unwrap());
    assert_eq!(fa.values_cm.len(), fb.values_cm.len(), "ts freq count");
    for (i, (x, y)) in fa.values_cm.iter().zip(&fb.values_cm).enumerate() {
        assert_close(&format!("optts_hcn freq[{i}]"), *x, *y, F_TOL);
    }
    // a TS: the imaginary-mode count is a structural fact that must agree.
    assert_eq!(fa.imaginary_count, fb.imaginary_count, "ts imaginary_count");
}

#[test]
fn scan_ethane_relaxscan_identical() {
    let spec_a =
        parse_scan_spec(&read("scan_ethane", "v610", "input.inp")).expect("scan line present");
    let spec_b =
        parse_scan_spec(&read("scan_ethane", "v611", "input.inp")).expect("scan line present");
    assert_eq!(spec_a, spec_b, "scan spec parsed identically");
    let load = |v: &str, spec: &super::relaxscan::ScanSpec| {
        RelaxScan::from_path(&xver("scan_ethane", v, "input.inp").parent().unwrap())
            .unwrap()
            .expect("scan present")
            .verify(spec)
            .unwrap()
    };
    let (a, b) = (load("v610", &spec_a), load("v611", &spec_b));
    assert_eq!(a.kind(), b.kind(), "scan kind");
    assert_eq!(a.atoms(), b.atoms(), "scan atoms");
    assert_eq!(a.coordinate_unit(), b.coordinate_unit(), "coordinate unit");
    assert_eq!(a.points().len(), b.points().len(), "scan point count");
    for (i, (p, q)) in a.points().iter().zip(b.points()).enumerate() {
        assert_close(&format!("scan coord[{i}]"), p.coordinate, q.coordinate, G_TOL);
        assert_close(&format!("scan act[{i}]"), p.energy_act_eh, q.energy_act_eh, E_TOL);
        assert_close(&format!("scan scf[{i}]"), p.energy_scf_eh, q.energy_scf_eh, E_TOL);
    }
}

// ── negative control: the tolerance gate provably bites ─────────────────────────

#[test]
fn negative_control_energy_gate_bites() {
    // A real 6.1.1 energy, then the same value perturbed by 1e-5 Eh (10× the 1e-6
    // gate). The gate must PASS identical and FAIL the perturbation — otherwise the
    // green cross-version tests above prove nothing.
    let r = reference(&read("smd_water", "v611", "input.inp"));
    let m = map_of(&r);
    let e = property("smd_water", "v611", &r, &m)
        .final_single_point_energy()
        .unwrap();
    assert!(within(e, e, E_TOL), "identical energy must pass the gate");
    assert!(
        !within(e, e + 1e-5, E_TOL),
        "a 1e-5 Eh perturbation (> 1e-6 tol) MUST fail the gate — else the gate is green for nothing"
    );
}

// ── NEB-TS: explicitly deferred, not silently dropped ───────────────────────────

#[test]
#[ignore = "NEB-TS HCN↔HNC at r2SCAN-3c does not finish within the 10-min laptop guard \
            (climbing-image TS + numerical Hessian tail); no clean 6.1.1 pair yet. \
            Candidate for a server run. See the session report + wiki."]
fn neb_ts_cross_version_deferred() {
    // Intentionally empty: this marker keeps the gap VISIBLE in `cargo test` output
    // (`ignored`) instead of pretending NEB was covered.
}
