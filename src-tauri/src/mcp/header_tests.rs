//! Tests for the header grammar and its depth-tracked block scanner (C7, C12).

use super::header::{parse_header, BlockItem, ParsedHeader, TokenKind, Violation};

fn outside_whys(h: &ParsedHeader) -> Vec<String> {
    h.violations
        .iter()
        .filter_map(|v| match v {
            Violation::OutsideGrammar { why, .. } => Some(why.clone()),
            Violation::Coordinates { .. } => None,
        })
        .collect()
}

fn item_texts(items: &[BlockItem]) -> Vec<String> {
    items
        .iter()
        .map(|i| match i {
            BlockItem::Token(t) => t.text.clone(),
            BlockItem::SubBlock(s) => format!("[{}: {}]", s.name, s.tokens.iter().map(|t| t.text.as_str()).collect::<Vec<_>>().join(" ")),
        })
        .collect()
}

#[test]
fn a_well_formed_header_has_no_violations() {
    let h = parse_header("# Opt+Freq at the study level\n! wB97X-3c CPCM(DMF) Opt Freq\n\n! TightSCF\n");
    assert!(h.violations.is_empty(), "{:?}", h.violations);
    assert_eq!(h.keywords().collect::<Vec<_>>(), ["wB97X-3c", "CPCM(DMF)", "Opt", "Freq", "TightSCF"]);
    assert!(h.blocks.is_empty());
}

#[test]
fn nested_geom_constraints_parse_correctly() {
    let h = parse_header("! Opt\n%geom\n  Constraints\n    {C 0 C}\n    {C 3 C}\n  end\n  MaxIter 200\nend\n");
    assert!(h.violations.is_empty(), "{:?}", h.violations);
    assert_eq!(h.blocks.len(), 1);
    let g = &h.blocks[0];
    assert_eq!((g.name.as_str(), g.closed), ("geom", true));
    assert_eq!(item_texts(&g.items), ["[constraints: {C 0 C} {C 3 C}]", "MaxIter", "200"]);
}

#[test]
fn nested_sub_block_on_one_line() {
    let h = parse_header("%geom Constraints {C 0 C} end end\n");
    assert!(h.violations.is_empty(), "{:?}", h.violations);
    assert_eq!(item_texts(&h.blocks[0].items), ["[constraints: {C 0 C}]"]);
    let h = parse_header("%geom Scan B 0 1 = 1.4, 2.4, 6 end end\n");
    assert!(h.violations.is_empty(), "{:?}", h.violations);
    assert_eq!(item_texts(&h.blocks[0].items), ["[scan: B 0 1 = 1.4, 2.4, 6]"]);
}

#[test]
fn a_one_line_cpcm_block_with_a_quoted_value() {
    let h = parse_header("%cpcm smd true SMDsolvent \"methanol\" end\n");
    assert!(h.violations.is_empty(), "{:?}", h.violations);
    let b = &h.blocks[0];
    assert_eq!((b.name.as_str(), b.closed), ("cpcm", true));
    assert_eq!(item_texts(&b.items), ["smd", "true", "SMDsolvent", "\"methanol\""]);
    assert!(matches!(&b.items[3], BlockItem::Token(t) if t.kind == TokenKind::Quoted));
}

#[test]
fn a_quoted_end_does_not_close_a_block() {
    let h = parse_header("%cpcm SMDsolvent \"end\"\nend\n");
    assert!(h.violations.is_empty(), "{:?}", h.violations);
    assert_eq!(h.blocks.len(), 1);
    assert_eq!(item_texts(&h.blocks[0].items), ["SMDsolvent", "\"end\""]);
}

#[test]
fn a_commented_end_does_not_close_a_block() {
    let h = parse_header("%geom\n  MaxIter 5 # end\n  # end\nend\n");
    assert!(h.violations.is_empty(), "{:?}", h.violations);
    assert_eq!(h.blocks.len(), 1);
    assert_eq!(item_texts(&h.blocks[0].items), ["MaxIter", "5"]);
}

#[test]
fn lines_outside_the_grammar_are_violations() {
    for (text, why) in [
        ("$new_job\n", "a `$` line (a second job)"),
        ("end\n", "a line that is not `!`, a `%` block, a `#` comment or blank"),
        ("Opt\n", "a line that is not `!`, a `%` block, a `#` comment or blank"),
        ("%geom MaxIter 5 end ! Opt\n", "text after a block's `end`"),
        ("%geom MaxIter 5\n", "block %geom never closes"),
        ("%geom\n ! PAL4\nend\n", "a `!` line inside a block"),
        ("%cpcm SMDsolvent \"dmf\n", "an unterminated quoted string"),
        ("%cpcm SMDsolvent \"a#b\" end\n", "a `#` inside a quoted string"),
        ("%geom Constraints {C 0 C end end\n", "an unterminated `{`"),
        ("! Opt\u{a0}Freq\n", "unsupported character U+00A0"),
        ("! Opt\u{b}Freq\n", "unsupported character U+000B"),
        ("! Opt \"x\"\n", "a quoted, braced or second `!` token on a `!` line"),
    ] {
        let h = parse_header(text);
        assert!(outside_whys(&h).iter().any(|w| w == why), "{text:?}: {:?}", h.violations);
    }
}

#[test]
fn a_star_line_is_coordinates_and_outside_the_grammar() {
    let h = parse_header("! SP\n* xyz 0 1\nH 0 0 0\n*\n");
    assert!(h.violations.iter().any(|v| matches!(v, Violation::Coordinates { line: 2, .. })));
    assert!(outside_whys(&h).iter().any(|w| w == "a `*` geometry line"));
    // A `*` line inside a block is still coordinates.
    let h = parse_header("%geom\n* xyz 0 1\nend\n");
    assert!(h.violations.iter().any(|v| matches!(v, Violation::Coordinates { line: 2, .. })));
}

#[test]
fn a_nested_opener_is_recorded_by_name() {
    let h = parse_header("%geom %pal nprocs 4 end end\n");
    assert_eq!(h.nested_openers, vec![(1, "pal".to_string())]);
    assert!(outside_whys(&h).iter().any(|w| w == "a `%` block inside a block"));
}

#[test]
fn crlf_and_case_are_handled() {
    let h = parse_header("! Opt\r\n%GEOM\r\n  CONSTRAINTS\r\n    {C 1 C}\r\n  END\r\nEnd\r\n");
    assert!(h.violations.is_empty(), "{:?}", h.violations);
    assert_eq!(item_texts(&h.blocks[0].items), ["[constraints: {C 1 C}]"]);
}

#[test]
fn a_line_with_two_hashes_is_outside_the_grammar() {
    for text in [
        "! wB97X-3c CPCM(DMF) Opt Freq #x# PAL16\n",
        "%geom MaxIter 5 #x# Recalc_Hess 1\nend\n",
        "# x # %maxcore 99999\n",
        "## a banner\n",
    ] {
        let h = parse_header(text);
        assert!(
            outside_whys(&h).iter().any(|w| w.starts_with("two or more `#` on one line")),
            "{text:?}: {:?}",
            h.violations
        );
    }
    // One `#` is still an ordinary comment.
    assert!(parse_header("! Opt # a note\n").violations.is_empty());
}

#[test]
fn a_non_ascii_character_on_a_bang_line_is_outside_the_grammar() {
    for text in ["! wB97X-3c Opt Freq ſp\n", "! ωB97X-3c Opt\n"] {
        let h = parse_header(text);
        assert!(outside_whys(&h).iter().any(|w| w == "a non-ASCII character on a `!` line"), "{text:?}");
        assert!(h.keyword_lines.is_empty());
    }
}
