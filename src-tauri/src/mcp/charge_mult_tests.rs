//! Tests for the strict charge/multiplicity locator (C1).

use super::charge_mult::{locate_charge_mult, ChargeMult, ChargeMultError};

/// The cases file shared with `src/scene/charge-mult-parity.test.ts`.
const CASES: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/mcp/charge_mult_cases.json"));

fn cases() -> serde_json::Value {
    serde_json::from_str(CASES).expect("the cases file is JSON")
}

#[test]
fn well_formed_cases_match_the_ts_locator() {
    let doc = cases();
    let well = doc["wellFormed"].as_array().expect("wellFormed");
    assert!(well.len() >= 5);
    for c in well {
        let name = c["name"].as_str().unwrap();
        let want = ChargeMult {
            charge: c["charge"].as_i64().unwrap(),
            multiplicity: c["multiplicity"].as_i64().unwrap(),
        };
        assert_eq!(locate_charge_mult(c["input"].as_str().unwrap()), Ok(want), "case `{name}`");
    }
}

#[test]
fn malformed_cases_are_refused_where_ts_defaults() {
    let doc = cases();
    let mal = doc["malformed"].as_array().expect("malformed");
    assert!(mal.len() >= 5);
    for c in mal {
        let name = c["name"].as_str().unwrap();
        assert!(locate_charge_mult(c["input"].as_str().unwrap()).is_err(), "case `{name}` must be refused");
    }
}

#[test]
fn errors_name_the_form() {
    assert_eq!(locate_charge_mult("! SP\n"), Err(ChargeMultError::NoXyzLine));
    assert!(matches!(
        locate_charge_mult("* xyzfile 0 1 a.xyz\n"),
        Err(ChargeMultError::NotInlineXyz { line: 1, ref form }) if form == "xyzfile"
    ));
    assert!(matches!(locate_charge_mult("* xyz 0 #1\n"), Err(ChargeMultError::Malformed { line: 1, .. })));
}

#[test]
fn a_commented_star_line_is_not_the_header() {
    assert_eq!(
        locate_charge_mult("# * xyz 5 5\n* xyz -2 1\nO 0 0 0\n*\n"),
        Ok(ChargeMult { charge: -2, multiplicity: 1 })
    );
}

#[test]
fn a_line_with_two_hashes_is_never_read_past() {
    // If ORCA closed the comment at the second `#` (the manual says so; unmeasured), this line would
    // be the header. The locator refuses rather than guess either way.
    assert_eq!(
        locate_charge_mult("# old # * xyz 1 2\n* xyz 0 1\nHe 0 0 0\n*\n"),
        Err(ChargeMultError::UnmeasuredComment { line: 1 })
    );
    assert_eq!(
        locate_charge_mult("* xyz 0 1 # singlet # really\nHe 0 0 0\n*\n"),
        Err(ChargeMultError::UnmeasuredComment { line: 1 })
    );
}
