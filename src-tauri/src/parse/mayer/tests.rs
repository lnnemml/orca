//! Tests for the Mayer bond-order reader (`mayer.rs`) against a real Menshutkin
//! SN2 excerpt (two blocks; the reader keeps the LAST = final structure) plus
//! synthetic bites for the bounds post-condition and absent-is-normal.

use std::path::{Path, PathBuf};

use super::*;
use crate::parse::ParseError;

fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

/// The Menshutkin structure has 15 atoms (indices 0..14).
const MENSHUTKIN_NATOMS: usize = 15;

fn find(bonds: &[MayerBond], i: usize, j: usize) -> Option<f64> {
    bonds
        .iter()
        .find(|b| (b.i == i && b.j == j) || (b.i == j && b.j == i))
        .map(|b| b.order)
}

#[test]
fn parses_the_last_block_including_partial_ts_bonds() {
    let path = fixtures_dir().join("mayer_menshutkin.out");
    let bonds = read_mayer(&path, MENSHUTKIN_NATOMS)
        .expect("parse ok")
        .expect("the file has a Mayer table");

    // 14 entries in the block.
    assert_eq!(bonds.len(), 14);

    // The LAST block wins (the final structure): its C–N is 0.9056, NOT the first
    // block's 0.8996 — proves last-block-wins, not first-seen.
    assert!((find(&bonds, 0, 1).unwrap() - 0.9056).abs() < 1e-9, "C–N from the LAST block");

    // The partial TS bonds parse: the FORMING N···C (1–8) ≈ 0.19 and the BREAKING
    // C–I (8–9) ≈ 0.57 in the final block — fractional, authoritative orders.
    assert!((find(&bonds, 1, 8).unwrap() - 0.1870).abs() < 1e-9, "forming N···C");
    assert!((find(&bonds, 8, 9).unwrap() - 0.5679).abs() < 1e-9, "breaking C–I");

    // A full single bond is ~0.94–0.97; every order is positive.
    assert!(bonds.iter().all(|b| b.order > 0.0));
}

#[test]
fn negative_bond_order_is_valid_data_stored_exactly() {
    // The real ground-truth defect (r²SCAN-3c CPCM codeine-cation re-opt): ORCA prints
    // every pair with |BO| > threshold, so a through-space, non-bonded O···C pair is
    // legitimately NEGATIVE. It must parse (not be rejected) and be stored EXACTLY —
    // no abs(), no flip, no drop.
    let lines = [
        "B(  9-C , 10-O ) :   0.7899 B( 10-O , 11-C ) :   1.1010 B( 10-O , 18-C ) :  -0.1016",
        "B( 11-C , 12-C ) :   1.5264",
    ];
    let bonds = parse_mayer_lines(&lines, 20).unwrap();
    assert_eq!(bonds.len(), 4);
    // The exact negative value is preserved bit-for-bit (honest data, rule #9).
    let neg = find(&bonds, 10, 18).expect("the negative O···C pair is kept");
    assert_eq!(neg, -0.1016, "the -0.1016 through-space order is stored exactly");
    // The positive siblings on the same lines are unaffected.
    assert_eq!(find(&bonds, 9, 10), Some(0.7899));
    assert_eq!(find(&bonds, 11, 12), Some(1.5264));
}

#[test]
fn a_non_numeric_order_matches_no_entry_nothing_is_kept() {
    // The entry regex only accepts a signed decimal for the order, so a non-numeric
    // token (`abc`) simply produces NO match — no bond, no panic, no silent bad pair.
    // (This documents the honest floor; the guard that LOUDLY bites on corrupt data is
    // the bounds check — see the two `negative_control_*` bounds tests below.)
    let lines = ["B(  1-C ,  2-C ) :   abc"];
    let bonds = parse_mayer_lines(&lines, 5).unwrap();
    assert!(bonds.is_empty(), "a non-numeric order matches no entry — nothing kept");
}

#[test]
fn absent_block_is_none_not_an_error() {
    // An xTB run prints no Mayer table — absent-is-normal, Ok(None), never an error.
    let path = fixtures_dir().join("xtb_success_dexketoprofen_bh4.out");
    assert_eq!(read_mayer(&path, 100).expect("parse ok"), None);
}

#[test]
fn a_missing_file_is_absent_not_a_failure() {
    let path = fixtures_dir().join("does_not_exist.out");
    assert_eq!(read_mayer(&path, 10).expect("parse ok"), None);
}

#[test]
fn parses_several_entries_per_line() {
    // One line, three entries → three bonds (formaldehyde-style, C=O at 2.02).
    let lines = ["B(  0-H ,  1-C ) :   0.9192 B(  1-C ,  2-H ) :   0.9192 B(  1-C ,  3-O ) :   2.0172"];
    let bonds = parse_mayer_lines(&lines, 4).unwrap();
    assert_eq!(bonds.len(), 3);
    assert!((find(&bonds, 1, 3).unwrap() - 2.0172).abs() < 1e-9, "C=O double is ~2.02");
}

#[test]
fn negative_control_index_out_of_range_is_an_error_not_a_silent_bad_pair() {
    // Index 20 in a 5-atom structure — the bite: a reader that skipped the bounds
    // check would silently keep B(0,20). It must be a LOUD Malformed error.
    let lines = ["B(  0-C , 20-N ) :   0.5"];
    let err = parse_mayer_lines(&lines, 5).unwrap_err();
    match err {
        ParseError::Malformed { field, detail } => {
            assert_eq!(field, "Mayer bond orders");
            assert!(detail.contains("out of range"), "{detail}");
        }
        other => panic!("expected Malformed, got {other:?}"),
    }
}

#[test]
fn negative_control_the_real_table_would_fail_a_too_small_atom_count() {
    // The decisive bite: the SAME real table that parses at natoms=15 is REFUSED at
    // natoms=10 (index 14 is now out of range) — proving the bounds check is
    // load-bearing, not decorative.
    let path = fixtures_dir().join("mayer_menshutkin.out");
    assert!(read_mayer(&path, MENSHUTKIN_NATOMS).is_ok(), "green at the real count");
    assert!(
        matches!(read_mayer(&path, 10), Err(ParseError::Malformed { .. })),
        "red at too-small a count"
    );
}
