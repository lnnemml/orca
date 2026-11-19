//! Tests for the decision-6 block-key allowlist.

use super::allowlist::{allowlist_findings, constraint_entries, parse_index, P2Finding};
use super::header::parse_header;

fn findings(text: &str) -> Vec<P2Finding> {
    allowlist_findings(&parse_header(text))
}

#[test]
fn the_four_admitted_rows_pass() {
    let text = "! OptTS\n%geom\n  Calc_Hess true\n  MaxIter 200\n  Constraints\n    {C 0 C}\n    {C 12 C}\n  end\nend\n\
                %cpcm smd true SMDsolvent \"N,N-dimethylformamide\" end\n";
    assert_eq!(findings(text), vec![]);
}

#[test]
fn irc_with_any_key_is_p2() {
    for text in ["%irc MaxIter 5 end\n", "%irc\n  Direction both\nend\n", "%irc end\n"] {
        let f = findings(text);
        assert!(f.iter().any(|f| f.block == "%irc" && f.key.is_none()), "{text:?}: {f:?}");
    }
}

#[test]
fn a_valueless_bond_constraint_is_p2() {
    let f = findings("%geom Constraints\n {B 3 17 C}\n end\nend\n");
    assert_eq!(f.len(), 1, "{f:?}");
    assert_eq!((f[0].block.as_str(), f[0].key.as_deref()), ("%geom", Some("Constraints")));
    assert!(f[0].detail.contains("{B 3 17 C}"));
}

#[test]
fn every_non_cartesian_or_valued_constraint_is_p2() {
    for entry in ["{B 0 1 1.5 C}", "{A 0 1 2 C}", "{D 0 1 2 3 180 C}", "{C 0 1.0 C}", "{c 0 c}", "{C 0}", "{C x C}"] {
        let f = findings(&format!("%geom Constraints {entry} end end\n"));
        assert_eq!(f.len(), 1, "{entry}: {f:?}");
    }
    // A bare (non-braced) entry is not admitted either.
    assert_eq!(findings("%geom Constraints C 0 C end end\n").len(), 3);
}

#[test]
fn any_scan_is_p2() {
    let f = findings("! Opt\n%geom Scan B 0 1 = 1.4, 2.4, 6 end end\n");
    assert_eq!(f.len(), 1, "{f:?}");
    assert_eq!(f[0].key.as_deref(), Some("scan"));
}

#[test]
fn values_outside_the_recorded_forms_are_p2() {
    for text in [
        "%geom Calc_Hess false end\n",
        "%geom Calc_Hess 1 end\n",
        "%geom MaxIter 0 end\n",
        "%geom MaxIter -5 end\n",
        "%geom MaxIter 1.5 end\n",
        "%geom MaxIter \"200\" end\n",
        "%geom MaxIter end\n",
        "%cpcm smd false end\n",
        "%cpcm SMDsolvent DMF end\n",
        "%cpcm SMDsolvent \"/home/anton/solvent.dat\" end\n",
        "%cpcm SMDsolvent \"my.solvent\" end\n",
        "%cpcm SMDsolvent \"\" end\n",
    ] {
        assert_eq!(findings(text).len(), 1, "{text:?}: {:?}", findings(text));
    }
}

#[test]
fn unknown_keys_and_blocks_name_the_block_and_key() {
    let f = findings("%geom Recalc_Hess 5 end\n");
    assert_eq!(f, vec![P2Finding {
        block: "%geom".into(),
        key: Some("Recalc_Hess".into()),
        detail: "`Recalc_Hess` is not on the %geom allowlist".into(),
    }]);
    // The values of an unknown key on its line are not reported as keys of their own.
    assert_eq!(findings("%geom TS_Mode {B 0 1} end\n").len(), 1);
    for block in ["%scf MaxIter 300 end\n", "%freq Temp 298 end\n", "%goat end\n", "%basis end\n"] {
        assert_eq!(findings(block).len(), 1, "{block:?}");
    }
    let f = findings("%cpcm epsilon 80 end\n");
    assert_eq!(f[0].key.as_deref(), Some("epsilon"));
}

#[test]
fn repeated_blocks_and_keys_are_p2() {
    assert_eq!(findings("%geom MaxIter 5 end\n%geom MaxIter 6 end\n").len(), 1);
    assert_eq!(findings("%geom MaxIter 5 MaxIter 500 end\n").len(), 1);
    assert_eq!(findings("%geom Constraints {C 0 C} end Constraints {C 1 C} end end\n").len(), 1);
}

#[test]
fn constraint_entries_read_every_type() {
    let e = constraint_entries(&parse_header("%geom Constraints\n{B 3 17 C}\n{C -1 C}\n{A 0 1 2 109.5 C}\nend end\n"));
    let got: Vec<(&str, Vec<String>)> = e.iter().map(|c| (c.text.as_str(), c.index_tokens.clone())).collect();
    let s = |v: &[&str]| v.iter().map(|t| t.to_string()).collect::<Vec<_>>();
    assert_eq!(
        got,
        [("{B 3 17 C}", s(&["3", "17"])), ("{C -1 C}", s(&["-1"])), ("{A 0 1 2 109.5 C}", s(&["0", "1", "2"]))]
    );
}

#[test]
fn an_index_past_u32_is_neither_admitted_nor_dropped() {
    for entry in ["{C 99999999999999999999 C}", "{C 9223372036854775808 C}", "{C 4294967296 C}"] {
        let text = format!("%geom Constraints {entry} end end\n");
        assert_eq!(findings(&text).len(), 1, "{entry} must not be admitted");
        let e = constraint_entries(&parse_header(&text));
        assert_eq!(e[0].index_tokens.len(), 1, "{entry}: the index token is kept");
        assert_eq!(parse_index(&e[0].index_tokens[0]), None);
    }
    assert_eq!(parse_index("4294967295"), Some(u32::MAX));
    for t in ["+1", "-0", "1.0", "0:5", ""] {
        assert_eq!(parse_index(t), None, "{t}");
    }
}

#[test]
fn a_leading_zero_index_is_not_admitted() {
    for entry in ["{C 007 C}", "{C 00 C}", "{C 01 C}"] {
        let f = findings(&format!("%geom Constraints {entry} end end\n"));
        assert_eq!(f.len(), 1, "{entry}: {f:?}");
        assert!(f[0].detail.ends_with("index form not recorded"), "{entry}: {}", f[0].detail);
    }
    assert_eq!(findings("%geom Constraints {C 0 C} {C 10 C} end end\n"), vec![]);
}

#[test]
fn a_key_and_its_value_on_separate_lines_are_not_admitted() {
    for text in [
        "%geom\n  Calc_Hess\n  true\nend\n",
        "%geom MaxIter\n 200 end\n",
        "%cpcm smd\ntrue end\n",
        "%cpcm SMDsolvent\n\"DMF\"\nend\n",
    ] {
        let f = findings(text);
        assert_eq!(f.len(), 1, "{text:?}: {f:?}");
        assert!(f[0].detail.ends_with("key and value on separate lines (not recorded)"), "{text:?}");
    }
    assert_eq!(findings("%geom\n  Calc_Hess true\n  MaxIter 200\nend\n"), vec![]);
}
