//! The method signature (ADR-027 decision 6; clarification C3): the case-folded, sorted, de-duplicated
//! union of every `!` token, minus [`NON_METHOD`] and minus `^PAL\d*$`, plus the normalised text of
//! each `%cpcm` block. A grant's level is one of these; a job matches a level only on equality.
//!
//! **Not the ΔΔ table's `methodSignature`** (`src/reactions/compare.ts`). That one adds only `SMD`
//! for a `%cpcm … smd true` block; this one adds the block's whole normalised text, so SMD(DMF) and
//! SMD(THF) differ. A divergence between the two can only send a job to Anton, never weaken this
//! guard. The non-method list itself is shared: [`NON_METHOD`] is a copy of the TS set, pinned
//! equal by `signature_tests::non_method_matches_compare_ts`.

use serde::Serialize;

use super::header::{parse_header, BlockItem, ParsedHeader};

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
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
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

    /// The signature of a whole input text (a grant level pasted by Anton, or a source job's
    /// input): only its `!` lines and `%cpcm` blocks count, so a `* xyz` block in it is harmless.
    pub fn of_input(text: &str) -> MethodSignature {
        method_signature(&parse_header(text))
    }
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
