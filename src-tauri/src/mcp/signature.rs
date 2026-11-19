//! The method signature (ADR-027 decision 6; clarification C3): the case-folded, sorted, de-duplicated
//! union of every `!` token, minus [`NON_METHOD`] and minus `^PAL\d*$`, plus the normalised text of
//! each `%cpcm` block. A grant's level is one of these; a job matches a level only on equality.
//!
//! **Not the ΔΔ table's `methodSignature`** (`src/reactions/compare.ts`). That one adds only `SMD`
//! for a `%cpcm … smd true` block; this one adds the block's whole normalised text, so SMD(DMF) and
//! SMD(THF) differ. A divergence between the two can only send a job to Anton, never weaken this
//! guard. The non-method list itself is shared: [`NON_METHOD`] is a copy of the TS set, pinned
//! equal by `signature_tests::non_method_matches_compare_ts`.

use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::header::{parse_header, BlockItem, ParsedHeader, Violation};

/// The `!` tokens that do not identify the electronic-structure method: run types and the `*Opt`
/// presets, SCF thresholds, and print control. A copy of `NON_METHOD` in
/// `src/reactions/compare.ts` (C2); the parity test fails if the two differ.
pub const NON_METHOD: &[&str] = &[
    // run types / job control
    "OPT", "LOOSEOPT", "NORMALOPT", "TIGHTOPT", "VERYTIGHTOPT", "OPTTS", "COPT", "OPTH",
    "FREQ", "NUMFREQ", "ANFREQ", "SP", "ENGRAD", "GRAD", "MD", "GOAT",
    "NEB", "NEB-TS", "NEB-CI", "IRC", "SCANTS",
    // SCF-convergence thresholds
    "TIGHTSCF", "VERYTIGHTSCF", "EXTREMESCF", "NORMALSCF", "LOOSESCF", "SLOPPYSCF", "STRONGSCF",
    // print / output control
    "LARGEPRINT", "NORMALPRINT", "MINIPRINT", "PRINTBASIS", "XYZFILE", "PRINTMOS", "NOPRINTMOS",
];

/// True for a parallelism token, `^PAL\d*$` in any case (`PAL`, `PAL0`, `PAL16`). R3 refuses these;
/// the signature drops them, as the TS one does.
pub fn is_pal_token(token: &str) -> bool {
    let up = token.to_ascii_uppercase();
    up.strip_prefix("PAL").is_some_and(|rest| rest.bytes().all(|b| b.is_ascii_digit()))
}

/// A method signature. Equality is the comparison; [`MethodSignature::display`] is for reasons.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MethodSignature {
    /// The method `!` tokens, ASCII upper-cased, sorted and de-duplicated.
    pub keywords: Vec<String>,
    /// Each `%cpcm` block's normalised text, in order: `%cpcm` + its tokens lowercased and joined
    /// by one space (quotes kept) + `end`.
    pub solvation: Vec<String>,
}

impl MethodSignature {
    /// One line for a human: `"DEF2-QZVP ΩB97X-3C"`, then any solvation block.
    pub fn display(&self) -> String {
        let mut parts = self.keywords.clone();
        parts.extend(self.solvation.iter().cloned());
        if parts.is_empty() {
            "(no method keywords)".into()
        } else {
            parts.join(" ")
        }
    }

    /// The signature of a text, **lenient**: a line the parser refuses is left out of it. Kept for
    /// the tests only; production code reads a level with [`MethodSignature::of_level_text`], which
    /// refuses such a line instead of dropping it.
    #[cfg(test)]
    pub fn of_input(text: &str) -> MethodSignature {
        method_signature(&parse_header(text))
    }

    /// The signature of a level's text, **strict** (ADR-027 decision 5; the M1a verifier's LOW
    /// finding): a grant level pasted by Anton, or a source job's header. The text must parse with
    /// no violation at all — a refused line would otherwise be dropped from the signature without a
    /// word, and the level would silently differ from what Anton wrote — and must name at least one
    /// method token (a level of `! Opt Freq` alone, or a lone `%cpcm` block, matches nothing).
    pub fn of_level_text(text: &str) -> Result<MethodSignature, LevelError> {
        let header = parse_header(text);
        if let Some(v) = header.violations.first() {
            let more = header.violations.len() - 1;
            return Err(match v {
                Violation::OutsideGrammar { line, excerpt, why } => {
                    LevelError::OutsideGrammar { line: *line, excerpt: excerpt.clone(), why: why.clone(), more }
                }
                Violation::Coordinates { line, excerpt } => {
                    LevelError::Coordinates { line: *line, excerpt: excerpt.clone(), more }
                }
            });
        }
        let sig = method_signature(&header);
        if sig.keywords.is_empty() {
            return Err(LevelError::NoMethodTokens);
        }
        Ok(sig)
    }
}

/// Why a level's text gives no signature. Every variant refuses the level; none drops a line.
#[derive(Debug, Clone, PartialEq, Eq, Error, Serialize)]
pub enum LevelError {
    #[error("line {line} of the level is outside the header grammar: {why}: `{excerpt}` (and {more} more)")]
    OutsideGrammar { line: usize, excerpt: String, why: String, more: usize },
    #[error("line {line} of the level carries coordinates (`{excerpt}`); a level is a `!` line and a solvation block (and {more} more)")]
    Coordinates { line: usize, excerpt: String, more: usize },
    #[error("the level names no method keyword")]
    NoMethodTokens,
}

/// The normalised text of one `%cpcm` block (C3): every token lowercased, joined by one space,
/// quotes kept, comments already gone (the lexer dropped them).
fn normalised_cpcm(items: &[BlockItem]) -> String {
    let mut parts = vec!["%cpcm".to_string()];
    for item in items {
        match item {
            BlockItem::Token(t) => parts.push(t.text.to_ascii_lowercase()),
            // `%cpcm` has no sub-blocks in the grammar; kept for completeness, never silent.
            BlockItem::SubBlock(s) => {
                parts.push(s.name.clone());
                parts.extend(s.tokens.iter().map(|t| t.text.to_ascii_lowercase()));
                parts.push("end".into());
            }
        }
    }
    parts.push("end".into());
    parts.join(" ")
}

/// The method signature of a parsed header (C3).
pub fn method_signature(header: &ParsedHeader) -> MethodSignature {
    let mut keywords: Vec<String> = header
        .keywords()
        .map(|k| k.to_ascii_uppercase())
        .filter(|up| !NON_METHOD.contains(&up.as_str()) && !is_pal_token(up))
        .collect();
    keywords.sort();
    keywords.dedup();
    // Every `%cpcm` block counts, admitted or not: a non-admitted one is P2 anyway, and counting it
    // can only make two signatures differ, never collapse them.
    let solvation = header
        .blocks
        .iter()
        .filter(|b| b.name == "cpcm")
        .map(|b| normalised_cpcm(&b.items))
        .collect();
    MethodSignature { keywords, solvation }
}

/// True when any `!` token is exactly `word` in any case.
pub(crate) fn has_keyword(header: &ParsedHeader, words: &[&str]) -> bool {
    header.keywords().any(|k| words.iter().any(|w| k.eq_ignore_ascii_case(w)))
}
