//! The block-key allowlist (ADR-027 decision 6), exactly the four rows of its table:
//!
//! | block | admitted | record |
//! |---|---|---|
//! | `%geom` | `Calc_Hess true` | `wiki/orca/optts.md` |
//! | `%geom` | `MaxIter <positive integer>` | `wiki/orca/gotchas.md` |
//! | `%geom` | `Constraints` with Cartesian `{C n C}` entries only | `wiki/orca/constraints.md` |
//! | `%cpcm` | `smd true`, `SMDsolvent "<name>"` | `wiki/orca/solvation.md` (form 2) |
//!
//! Anything else — another block, key, sub-block or value, any `Scan`, a constraint with a value,
//! a bond/angle/dihedral constraint, a file path — is a [`P2Finding`] naming the block and key. Each
//! admitted value is the form a run recorded (rule #10): `Calc_Hess true`, not `false`; `smd true`;
//! a quoted solvent name. A key that appears twice, and a block that appears twice, are findings
//! too, because which one ORCA honours has no run record.

use serde::Serialize;

use super::header::{Block, BlockItem, ParsedHeader, Token, TokenKind};

/// One reason a header is not fully on the allowlist (P2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct P2Finding {
    /// The block, e.g. `%geom`.
    pub block: String,
    /// The key, sub-block or entry, when the finding is about one.
    pub key: Option<String>,
    pub detail: String,
}

impl P2Finding {
    fn new(block: &str, key: Option<&str>, detail: impl Into<String>) -> Self {
        P2Finding { block: format!("%{block}"), key: key.map(str::to_string), detail: detail.into() }
    }
}

/// One constraint entry in a `%geom Constraints` sub-block, whatever its type, for R6 (C10).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConstraintEntry {
    /// The entry as written, e.g. `{B 3 17 C}`.
    pub text: String,
    /// The atom-index tokens as written, every one of them (0-based, ORCA's `%geom` base —
    /// `wiki/orca/constraints.md`): the first n after the type letter, n = 2/3/4/1 for B/A/D/C,
    /// once a trailing `C` flag is set aside (as `parseConstraintToken` in `constraints.ts`). A token
    /// that is not an index is kept, never dropped: [`parse_index`] refuses it and R6 refuses the job.
    pub index_tokens: Vec<String>,
}

/// An atom index as ORCA's `%geom` writes it: ASCII digits only, within `u32`. Anything else —
/// a sign, a decimal, a range, a number past `u32::MAX` — is `None`.
pub fn parse_index(token: &str) -> Option<u32> {
    if token.is_empty() || !token.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    token.parse::<u32>().ok()
}

/// The number of atom indices for a constraint type letter (any case), as `constraints.ts`.
pub(crate) fn atom_count(letter: &str) -> Option<usize> {
    match letter.to_ascii_uppercase().as_str() {
        "B" => Some(2),
        "A" => Some(3),
        "D" => Some(4),
        "C" => Some(1),
        _ => None,
    }
}

/// The tokens inside a `{…}` entry.
pub(crate) fn brace_inner(tok: &Token) -> Vec<&str> {
    tok.text.trim_start_matches('{').trim_end_matches('}').split_whitespace().collect()
}

/// Every constraint entry of every `%geom` block, B/A/D/C alike (C10: R6 checks them all).
pub fn constraint_entries(header: &ParsedHeader) -> Vec<ConstraintEntry> {
    let mut out = Vec::new();
    for block in header.blocks.iter().filter(|b| b.name == "geom") {
        for item in &block.items {
            let BlockItem::SubBlock(sub) = item else { continue };
            if sub.name != "constraints" {
                continue;
            }
            for tok in sub.tokens.iter().filter(|t| t.kind == TokenKind::Braced) {
                let inner = brace_inner(tok);
                let n = inner.first().and_then(|l| atom_count(l)).unwrap_or(0);
                let mut rest = inner.get(1..).unwrap_or(&[]);
                if rest.last().is_some_and(|t| t.eq_ignore_ascii_case("C")) {
                    rest = &rest[..rest.len() - 1];
                }
                let index_tokens = rest.iter().take(n).map(|t| t.to_string()).collect();
                out.push(ConstraintEntry { text: tok.text.clone(), index_tokens });
            }
        }
    }
    out
}

/// True for an index written the way the recorded runs wrote one: `0`, or digits not starting with
/// `0`. `007` reads as 7 in any integer parser, but no run recorded how ORCA reads it, so the
/// allowlist does not admit it (R6 still range-checks its value).
fn is_recorded_index_form(token: &str) -> bool {
    token == "0" || !token.starts_with('0')
}

/// True for exactly `{C n C}`: an upper-case `C`, an index [`parse_index`] reads, an upper-case `C`.
fn is_cartesian_entry(tok: &Token) -> bool {
    tok.kind == TokenKind::Braced
        && matches!(brace_inner(tok).as_slice(), ["C", n, "C"] if parse_index(n).is_some())
}

/// The detail of a constraint entry the allowlist does not admit.
fn constraint_detail(tok: &Token) -> Option<String> {
    if !is_cartesian_entry(tok) {
        return Some(format!("constraint `{}` is not admitted: only Cartesian `{{C n C}}` entries are", tok.text));
    }
    match brace_inner(tok).as_slice() {
        [_, n, _] if !is_recorded_index_form(n) => Some(format!("constraint `{}`: index form not recorded", tok.text)),
        _ => None,
    }
}

/// True for a quoted solvent name: non-empty, letters, digits, space, `-`, `,`, `(`, `)` or `'`.
/// No `/`, `\` or `.`, so no file path passes.
fn is_solvent_name(tok: &Token) -> bool {
    if tok.kind != TokenKind::Quoted {
        return false;
    }
    let inner = &tok.text[1..tok.text.len() - 1];
    !inner.trim().is_empty()
        && inner.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, ' ' | '-' | ',' | '(' | ')' | '\''))
}

/// The admitted value test for an admitted key of `block`, or `None` when the key is not admitted.
pub(crate) fn value_rule(block: &str, key: &str) -> Option<(&'static str, fn(&Token) -> bool)> {
    let is_true: fn(&Token) -> bool = |t| t.is_word("true");
    let positive_int: fn(&Token) -> bool = |t| {
        t.kind == TokenKind::Word
            && !t.text.is_empty()
            && t.text.bytes().all(|b| b.is_ascii_digit())
            && t.text.parse::<u32>().is_ok_and(|v| v >= 1)
    };
    match (block, key.to_ascii_lowercase().as_str()) {
        ("geom", "calc_hess") => Some(("`true`", is_true)),
        ("geom", "maxiter") => Some(("a positive integer", positive_int)),
        ("cpcm", "smd") => Some(("`true`", is_true)),
        ("cpcm", "smdsolvent") => Some(("a quoted solvent name", is_solvent_name)),
        _ => None,
    }
}

/// The findings for one admitted block (`%geom` or `%cpcm`).
fn judge_block(block: &Block, out: &mut Vec<P2Finding>) {
    let name = block.name.as_str();
    let mut seen: Vec<String> = Vec::new();
    let mut seen_twice = |key: &str, out: &mut Vec<P2Finding>| {
        let k = key.to_ascii_lowercase();
        if seen.contains(&k) {
            out.push(P2Finding::new(name, Some(key), format!("`{key}` appears more than once in %{name}")));
        } else {
            seen.push(k);
        }
    };
    let mut i = 0;
    while i < block.items.len() {
        match &block.items[i] {
            BlockItem::SubBlock(sub) => {
                seen_twice(&sub.name, out);
                if name == "geom" && sub.name == "constraints" {
                    for tok in &sub.tokens {
                        if let Some(detail) = constraint_detail(tok) {
                            out.push(P2Finding::new(name, Some("Constraints"), detail));
                        }
                    }
                } else {
                    out.push(P2Finding::new(name, Some(&sub.name), format!("the `{}` sub-block is not on the allowlist", sub.name)));
                }
                i += 1;
            }
            BlockItem::Token(key) => {
                let Some((wanted, ok)) = value_rule(name, &key.text) else {
                    out.push(P2Finding::new(name, Some(&key.text), format!("`{}` is not on the %{name} allowlist", key.text)));
                    // Its values (the rest of the key's line) are not keys: skip them.
                    i += 1;
                    while let Some(BlockItem::Token(t)) = block.items.get(i) {
                        if t.line != key.line {
                            break;
                        }
                        i += 1;
                    }
                    continue;
                };
                seen_twice(&key.text, out);
                match block.items.get(i + 1) {
                    // Every recorded run wrote the value on the key's line; another layout's
                    // reading is not recorded.
                    Some(BlockItem::Token(v)) if v.line != key.line => out.push(P2Finding::new(
                        name,
                        Some(&key.text),
                        format!("`{}`: key and value on separate lines (not recorded)", key.text),
                    )),
                    Some(BlockItem::Token(v)) if ok(v) => {}
                    Some(BlockItem::Token(v)) => out.push(P2Finding::new(
                        name,
                        Some(&key.text),
                        format!("`{} {}` is not admitted: the value must be {wanted}", key.text, v.text),
                    )),
                    _ => {
                        out.push(P2Finding::new(name, Some(&key.text), format!("`{}` has no value", key.text)));
                        i += 1;
                        continue;
                    }
                }
                i += 2;
            }
        }
    }
}

/// Every P2 finding of a header: blocks outside the allowlist, keys and values outside their
/// block's allowlist, and repeated blocks or keys. Empty means fully admitted.
pub fn allowlist_findings(header: &ParsedHeader) -> Vec<P2Finding> {
    let mut out = Vec::new();
    let mut seen: Vec<&str> = Vec::new();
    for block in &header.blocks {
        if seen.contains(&block.name.as_str()) {
            out.push(P2Finding::new(&block.name, None, format!("block %{} appears more than once", block.name)));
        }
        seen.push(&block.name);
        match block.name.as_str() {
            "geom" | "cpcm" => judge_block(block, &mut out),
            other => out.push(P2Finding::new(other, None, format!("block %{other} is not on the allowlist"))),
        }
    }
    out
}
