//! Tests for the method signature (C3) and the `NON_METHOD` parity with TypeScript (C2).

use std::collections::BTreeSet;

use regex::Regex;

use super::header::{KeywordLine, ParsedHeader};
use super::signature::{is_pal_token, method_signature, LevelError, MethodSignature, NON_METHOD};

/// `src/reactions/compare.ts`, read as text (C2: no TS change, the set is extracted by regex).
const COMPARE_TS: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../src/reactions/compare.ts"));

/// The string literals of the `NON_METHOD = new Set([ … ])` literal, `//` comments removed.
fn ts_non_method() -> BTreeSet<String> {
    let block = Regex::new(r"(?s)const NON_METHOD = new Set\(\[(.*?)\]\);")
        .unwrap()
        .captures(COMPARE_TS)
        .expect("compare.ts declares `const NON_METHOD = new Set([ … ]);`")[1]
        .to_string();
    let code: String = block.lines().map(|l| l.split("//").next().unwrap_or("")).collect::<Vec<_>>().join("\n");
    Regex::new(r#""([^"]*)""#).unwrap().captures_iter(&code).map(|c| c[1].to_string()).collect()
}

#[test]
fn non_method_matches_compare_ts() {
    let ts = ts_non_method();
    let rust: BTreeSet<String> = NON_METHOD.iter().map(|s| s.to_string()).collect();
    assert_eq!(rust.len(), NON_METHOD.len(), "the Rust list has a duplicate");
    assert!(ts.len() >= 30, "the extraction found only {} tokens", ts.len());
    assert_eq!(rust, ts, "NON_METHOD differs from src/reactions/compare.ts");
}

fn sig(text: &str) -> MethodSignature {
    MethodSignature::of_input(text)
}

#[test]
fn a_foreign_basis_on_a_composite_does_not_match_the_composite() {
    // The thesis error ADR-027 names: a composite method with a foreign basis.
    assert_ne!(sig("! wB97X-3c def2-QZVP Opt\n"), sig("! wB97X-3c\n"));
    assert_eq!(sig("! wB97X-3c Opt Freq\n"), sig("! wb97x-3c\n"));
}

#[test]
fn run_types_thresholds_and_pal_are_not_method() {
    let a = sig("! r2SCAN-3c Opt Freq TightSCF LargePrint PAL8\n");
    assert_eq!(a.keywords, ["R2SCAN-3C"]);
    assert_eq!(a, sig("! r2scan-3c\n"));
    for t in ["PAL", "pal0", "PAL16", "Pal4"] {
        assert!(is_pal_token(t), "{t}");
    }
    for t in ["PALX", "PAL-4", "APAL4"] {
        assert!(!is_pal_token(t), "{t}");
    }
}

#[test]
fn the_union_over_every_bang_line_is_sorted_and_deduplicated() {
    let a = sig("! def2-TZVP B3LYP\n! D3BJ b3lyp Opt\n");
    assert_eq!(a.keywords, ["B3LYP", "D3BJ", "DEF2-TZVP"]);
}

#[test]
fn an_inline_cpcm_token_is_kept_as_written() {
    let a = sig("! wB97X-3c CPCM(DMF) Opt\n");
    assert_eq!(a.keywords, ["CPCM(DMF)", "WB97X-3C"]);
    assert_ne!(a, sig("! wB97X-3c CPCM(THF) Opt\n"));
}

#[test]
fn smd_dmf_and_smd_thf_differ() {
    let dmf = sig("! wB97X-3c CPCM Opt\n%cpcm smd true SMDsolvent \"DMF\" end\n");
    let thf = sig("! wB97X-3c CPCM Opt\n%cpcm smd true SMDsolvent \"THF\" end\n");
    assert_ne!(dmf, thf);
    assert_eq!(dmf.solvation, ["%cpcm smd true smdsolvent \"dmf\" end"]);
}

#[test]
fn cpcm_normalisation_folds_case_whitespace_lines_and_comments() {
    let a = sig("! CPCM\n%cpcm smd true SMDsolvent \"DMF\" end\n");
    let b = sig("! cpcm\n%CPCM   SMD TRUE   # solvation\n  SMDSOLVENT \"dmf\"\nEND\n");
    assert_eq!(a, b);
    assert_ne!(a, sig("! CPCM\n"), "the block is part of the signature");
}

#[test]
fn the_case_fold_is_ascii_only() {
    // Unicode upper-casing would fold `ſp` to `SP` and drop it as a run type; ASCII folding keeps it,
    // and the classifier refuses the line anyway (R1, a non-ASCII `!` line).
    let header = ParsedHeader {
        keyword_lines: vec![KeywordLine { line: 1, tokens: vec!["B3LYP".into(), "ſp".into()] }],
        ..ParsedHeader::default()
    };
    assert_eq!(method_signature(&header).keywords, ["B3LYP", "ſP"]);
    // Through the parser the line is not read at all.
    assert!(sig("! B3LYP ſp\n").keywords.is_empty());
}

// --- The strict level reader (M1b; the M1a verifier's LOW finding) -------------------------------

/// A clean level reads to the same signature the lenient reader gives, `%cpcm` block included.
#[test]
fn a_clean_level_reads_like_the_lenient_signature() {
    for text in ["! wB97X-3c CPCM(DMF)\n", "! r2SCAN-3c\n%cpcm smd true SMDsolvent \"DMF\" end\n", "! B3LYP D3BJ def2-SVP Opt PAL8\n"] {
        assert_eq!(MethodSignature::of_level_text(text), Ok(sig(text)), "{text}");
    }
}

/// A line the grammar refuses is an error naming it, never dropped from the level. The lenient
/// reader would give `B3LYP DEF2-SVP` here and silently lose `D3BJ`. NEGATIVE CONTROL: skip the
/// violations check in `of_level_text` → it returns `Ok(B3LYP DEF2-SVP)` and this is red.
#[test]
fn a_violating_level_line_is_refused_not_dropped() {
    let text = "! B3LYP def2-SVP\n! D3BJ #x# TightSCF\n";
    assert_eq!(sig(text).keywords, ["B3LYP", "DEF2-SVP"], "the lenient reader drops line 2 (the hazard)");
    match MethodSignature::of_level_text(text) {
        Err(LevelError::OutsideGrammar { line: 2, .. }) => {}
        other => panic!("expected line 2 refused, got {other:?}"),
    }
    let coords = "! B3LYP def2-SVP\n* xyz 0 1\nH 0 0 0\nH 0 0 0.74\n*\n";
    assert!(matches!(MethodSignature::of_level_text(coords), Err(LevelError::Coordinates { .. } | LevelError::OutsideGrammar { .. })));
}

/// A level with no method token matches nothing and is refused. NEGATIVE CONTROL: drop the
/// empty-keywords check in `of_level_text` → `! Opt Freq` is accepted and this is red.
#[test]
fn an_empty_level_is_refused() {
    for text in ["! Opt Freq\n", "%cpcm smd true SMDsolvent \"DMF\" end\n", "", "# only a comment\n", "! PAL8 TightSCF\n"] {
        assert_eq!(MethodSignature::of_level_text(text), Err(LevelError::NoMethodTokens), "{text:?}");
    }
}

/// The stored form round-trips: a level written to `mcp_grants.levels` reads back equal.
#[test]
fn a_signature_round_trips_through_its_stored_json() {
    let s = sig("! wB97X-3c\n%cpcm smd true SMDsolvent \"DMF\" end\n");
    let back: MethodSignature = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
    assert_eq!(back, s);
}
