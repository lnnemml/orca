//! `get_job`'s header, **built from M1a's parsed structure, never filtered from raw text** (decided
//! by Anton after two verifier FAILs on filtering, M2b; ADR-027's M2 amendment, item 8). The input's
//! text outside its main `* xyz` block (`header_of_input_in_place`: the block and the app's
//! `%maxcore` line blanked, so line numbers stay the input's) is parsed by [`parse_header`], and
//! only parsed parts are emitted:
//!
//! | construct | shown as |
//! |---|---|
//! | a `!` line | `! ` and its parsed keyword tokens, each numeric-looking one or one holding a decimal number replaced by [`HIDDEN_TOKEN`] ([`hides_bang_token`]) |
//! | the app's `%maxcore N` | from the strict reader (`memory::read_maxcore`), at its line |
//! | an allowlisted key with an admitted value (`allowlist::value_rule`: `%geom Calc_Hess`, `MaxIter`; `%cpcm smd`, `SMDsolvent`) | the key and the value |
//! | an allowlisted key with any other value | the key, `# value not shown` |
//! | a key on the **display-only** list ([`DISPLAY_INTEGER_KEYS`]: `%pal nprocs`, `%scf MaxIter`, `%neb NImages`, `%geom Recalc_Hess`) with exactly one integer value | the key and the value; any other value: `# value not shown`. M1a's SUBMITTED allowlist is unchanged |
//! | a `%geom Constraints` entry `{T i … [v] [C]}` (T = B/A/D/C with its index count, ASCII-digit indices, an optional finite number, an optional `C`) | re-emitted from its parsed parts |
//! | any other key, in any block | its name only (an identifier), `# values not shown` |
//! | any other entry or sub-block | `# … not shown` with its line |
//! | a geometry block (`%coords`, `%pointcharges`), a block holding a nested `%` opener, a `*` line | a marker naming the kind and the line |
//! | a second job (`$…` line) | a marker; nothing after it is read |
//! | a line the parser refuses | a marker: the parser's reason kind and the line, never the line |
//!
//! The main `* xyz` block is never shown; `get_job` gives its atom count and the charge and
//! multiplicity with their source. A key name is shown only when it is an identifier
//! ([`is_identifier`]); no value outside the two lists is ever emitted. The rule-#9 tripwire
//! (`jobs.rs`) checks every field with [`coordinate_row`]: [`Field::Header`] for this header (a `{`
//! constraint line and a `=` line skipped), [`Field::Text`] for everything else (nothing skipped).
//!
//! **Known limitation.** The shared grammar (`header.rs`) knows only `Constraints` and `Scan` as
//! `%geom` sub-blocks, so `TS_Mode {B 0 1} end` closes `%geom` at that `end`: the key is named without
//! its value and the later `end` becomes a refused-line marker. M1a's grammar is not changed for it.

use crate::mcp::allowlist::{atom_count, brace_inner, parse_index, value_rule};
use crate::mcp::header::{parse_header, Block, BlockItem, ParsedHeader, SubBlock, Token, TokenKind, Violation};
use crate::mcp::resolve::{header_of_input_in_place, HeaderOfInputError};
use crate::memory::{maxcore_lines, read_maxcore};

/// Blocks that carry geometry: shown as a marker only.
pub const GEOMETRY_BLOCKS: [&str; 2] = ["coords", "pointcharges"];
/// Blocks whose admitted keys show their values (`allowlist::value_rule`'s blocks).
const ALLOWLISTED_BLOCKS: [&str; 2] = ["geom", "cpcm"];
/// A key name longer than this is not shown.
const KEY_MAX_BYTES: usize = 40;
/// A parser reason in a marker is cut to this many bytes.
const REASON_MAX_BYTES: usize = 100;

/// True for a name worth showing as a key: an ASCII letter or `_`, then letters, digits or `_`.
pub fn is_identifier(text: &str) -> bool {
    let mut cs = text.chars();
    text.len() <= KEY_MAX_BYTES
        && cs.next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && cs.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// The **display-only** list (Anton, M2b round 4): integer-valued keys whose value `get_job` shows,
/// though they are outside M1a's SUBMITTED allowlist (`allowlist.rs`, unchanged). Showing a value
/// here admits nothing to SUBMITTED. `(block, key)`, lowercased.
pub const DISPLAY_INTEGER_KEYS: [(&str, &str); 4] = [("pal", "nprocs"), ("scf", "maxiter"), ("neb", "nimages"), ("geom", "recalc_hess")];

fn is_display_key(block: &str, key: &str) -> bool {
    let key = key.to_ascii_lowercase();
    DISPLAY_INTEGER_KEYS.iter().any(|(b, k)| *b == block && *k == key)
}

/// An integer as the display list shows it: 1 to 9 ASCII digits, a bare word.
fn is_display_integer(tok: &Token) -> bool {
    tok.kind == TokenKind::Word && (1..=9).contains(&tok.text.len()) && tok.text.bytes().all(|b| b.is_ascii_digit())
}

/// True for a `!` token that looks like a number (a digit, sign or `.` first, then only digits,
/// signs, `.`, `e`/`E`, Fortran `d`/`D`, `,` or `;`, with at least one digit). Such a token is not
/// shown: a `!` line is no place for a coordinate (`! O 0.0 0.0 0.117790`). `D3`, `def2-SVP`,
/// `6-31G*` and `PAL8` are not numbers and stay.
pub fn is_numeric_token(tok: &str) -> bool {
    let mut cs = tok.chars();
    cs.next().is_some_and(|c| c.is_ascii_digit() || matches!(c, '+' | '-' | '.'))
        && tok.chars().all(|c| c.is_ascii_digit() || matches!(c, '+' | '-' | '.' | 'e' | 'E' | 'd' | 'D' | ',' | ';'))
        && tok.chars().any(|c| c.is_ascii_digit())
}

/// True for a `!` token that holds a decimal number (`\d*\.\d+`: a `.` followed by a digit)
/// anywhere: `(0.000000,0.000000,0.117790)`, `x=0.117790` (decided by Anton, M2b Part B).
/// `D3`, `def2-SVP` and `6-31G*` hold none and stay.
pub fn has_decimal_number(tok: &str) -> bool {
    tok.as_bytes().windows(2).any(|w| w[0] == b'.' && w[1].is_ascii_digit())
}

/// Whether a `!` token is replaced by [`HIDDEN_TOKEN`]: it looks like a number
/// ([`is_numeric_token`]), or it holds a decimal number anywhere ([`has_decimal_number`]).
pub fn hides_bang_token(tok: &str) -> bool {
    is_numeric_token(tok) || has_decimal_number(tok)
}

/// The marker a hidden `!` token is replaced by.
pub const HIDDEN_TOKEN: &str = "[value not shown]";

/// A `%geom Constraints` entry re-emitted from its parsed parts, or `None` when it is not exactly
/// `{T i … [v] [C]}`.
fn constraint(tok: &Token) -> Option<String> {
    if tok.kind != TokenKind::Braced {
        return None;
    }
    let inner = brace_inner(tok);
    let (letter, rest) = inner.split_first()?;
    let n = atom_count(letter)?;
    let (indices, rest) = (rest.get(..n)?, rest.get(n..)?);
    let indices: Vec<u32> = indices.iter().map(|t| parse_index(t)).collect::<Option<_>>()?;
    // The target value is kept as written (the app writes the user's `90.0`, `constraints.ts`), once
    // it reads as a finite number spelled only in digits, a sign, `.` and an exponent.
    let plain_number = |v: &str| {
        v.bytes().all(|b| b.is_ascii_digit() || matches!(b, b'+' | b'-' | b'.' | b'e' | b'E'))
            && v.parse::<f64>().is_ok_and(f64::is_finite)
    };
    let (value, rest) = match rest.split_first() {
        Some((v, rest)) if !v.eq_ignore_ascii_case("c") => (Some(plain_number(v).then_some(*v)?), rest),
        _ => (None, rest),
    };
    let flag = match rest {
        [] => false,
        [c] if c.eq_ignore_ascii_case("c") => true,
        _ => return None,
    };
    let mut out = format!("{{{}", letter.to_ascii_uppercase());
    for i in indices {
        out.push_str(&format!(" {i}"));
    }
    if let Some(v) = value {
        out.push_str(&format!(" {v}"));
    }
    if flag {
        out.push_str(" C");
    }
    out.push('}');
    Some(out)
}

/// A key's display: its name when it is an identifier, else a note with its line.
fn key_name(tok: &Token) -> String {
    if tok.kind == TokenKind::Word && is_identifier(&tok.text) {
        tok.text.clone()
    } else {
        format!("# an entry at line {}", tok.line)
    }
}

/// Render a sub-block of an allowlisted block.
fn sub_block(block: &str, sub: &SubBlock, out: &mut Vec<String>) {
    if block == "geom" && sub.name == "constraints" {
        out.push("  Constraints".into());
        for tok in &sub.tokens {
            match constraint(tok) {
                Some(entry) => out.push(format!("    {entry}")),
                None => out.push(format!("    # a constraint entry at line {}, not shown", tok.line)),
            }
        }
        out.push("  end".into());
    } else {
        out.push(format!("  # a {} sub-block at line {}, values not shown", sub.name, sub.line));
    }
}

/// Render one block's body. An allowlisted key shows an admitted value; every other key shows its
/// name only, and the rest of its line is skipped (its values).
fn block_body(block: &Block, out: &mut Vec<String>) {
    let allowlisted = ALLOWLISTED_BLOCKS.contains(&block.name.as_str());
    let items = &block.items;
    let mut i = 0;
    while i < items.len() {
        let key = match &items[i] {
            BlockItem::SubBlock(sub) => {
                if allowlisted {
                    sub_block(&block.name, sub, out);
                } else {
                    out.push(format!("  # a {} sub-block at line {}, values not shown", sub.name, sub.line));
                }
                i += 1;
                continue;
            }
            BlockItem::Token(key) => key,
        };
        let same_line = |j: usize| matches!(items.get(j), Some(BlockItem::Token(t)) if t.line == key.line);
        let rule = if allowlisted { value_rule(&block.name, &key.text) } else { None };
        match (rule, items.get(i + 1)) {
            (Some((_, ok)), Some(BlockItem::Token(v))) if v.line == key.line => {
                if ok(v) {
                    out.push(format!("  {} {}", key.text, v.text));
                } else {
                    out.push(format!("  {}  # value not shown", key.text));
                }
                i += 2;
            }
            (Some(_), _) => {
                out.push(format!("  {}  # no value on its line", key.text));
                i += 1;
            }
            (None, _) => {
                let name = key_name(key);
                let has_values = same_line(i + 1);
                let first = i + 1;
                i += 1;
                while same_line(i) {
                    i += 1;
                }
                let values: Vec<&Token> =
                    items[first..i].iter().filter_map(|it| if let BlockItem::Token(t) = it { Some(t) } else { None }).collect();
                if is_display_key(&block.name, &key.text) {
                    match values.as_slice() {
                        [v] if is_display_integer(v) => out.push(format!("  {} {}", key.text, v.text)),
                        _ => out.push(format!("  {}  # value not shown", key.text)),
                    }
                } else if name.starts_with('#') {
                    out.push(format!("  {name}, not shown"));
                } else if has_values {
                    out.push(format!("  {name}  # values not shown"));
                } else {
                    out.push(format!("  {name}"));
                }
            }
        }
    }
}

/// One parsed block, rendered, or a marker.
fn block_lines(block: &Block, nested_opener_lines: &[usize]) -> Vec<String> {
    if GEOMETRY_BLOCKS.contains(&block.name.as_str()) {
        return vec![format!("# a geometry block (%{}) at line {}, not shown", block.name, block.line)];
    }
    if !nested_opener_lines.is_empty() {
        return vec![format!(
            "# block %{} at line {} holds a block opener inside it (line {}), not shown",
            block.name, block.line, nested_opener_lines[0]
        )];
    }
    let mut out = vec![format!("%{}", block.name)];
    block_body(block, &mut out);
    if block.closed {
        out.push("end".into());
    }
    out
}

/// The parser's reason, as a kind: its own fixed phrase, cut. It never holds the line's text (the
/// excerpt is a separate field, not read here).
fn reason_kind(why: &str) -> String {
    crate::mcp::tools::clip(why, REASON_MAX_BYTES).0
}

/// Every rendered piece of `parsed`, keyed by its first line.
fn pieces(parsed: &ParsedHeader) -> Vec<(usize, Vec<String>)> {
    let mut out: Vec<(usize, Vec<String>)> = Vec::new();
    for kl in &parsed.keyword_lines {
        let shown: Vec<&str> = kl.tokens.iter().map(|t| if hides_bang_token(t) { HIDDEN_TOKEN } else { t.as_str() }).collect();
        out.push((kl.line, vec![format!("! {}", shown.join(" "))]));
    }
    // A nested opener belongs to the block open at its line: the last block starting at or before it.
    for (b, block) in parsed.blocks.iter().enumerate() {
        let next_start = parsed.blocks.get(b + 1).map_or(usize::MAX, |n| n.line);
        let nested: Vec<usize> =
            parsed.nested_openers.iter().map(|(l, _)| *l).filter(|l| *l >= block.line && *l < next_start).collect();
        out.push((block.line, block_lines(block, &nested)));
    }
    // Refused lines: a run of consecutive lines with the same reason is one marker.
    let mut refused: Vec<(usize, usize, &str)> = Vec::new();
    for v in &parsed.violations {
        match v {
            Violation::Coordinates { line, .. } => out.push((*line, vec![format!("# a geometry line at line {line}, not shown")])),
            // A `*` line has both violations; its marker is the one above.
            Violation::OutsideGrammar { why, .. } if why == "a `*` geometry line" => {}
            Violation::OutsideGrammar { line, why, .. } => match refused.last_mut() {
                Some((_, end, w)) if *w == why.as_str() && *end + 1 == *line => *end = *line,
                _ => refused.push((*line, *line, why.as_str())),
            },
        }
    }
    for (start, end, why) in refused {
        let lines = if start == end { format!("line {start}") } else { format!("lines {start}–{end}") };
        out.push((start, vec![format!("# {lines}: outside the header grammar ({}), not shown", reason_kind(why))]));
    }
    out.sort_by_key(|(line, _)| *line);
    out
}

/// `get_job`'s header for `input`: built from the parsed structure (see the module docs). `Err` when
/// the header cannot be separated from the main coordinate block; the error is a kind and a line.
pub fn reconstruct(input: &str) -> Result<String, HeaderOfInputError> {
    let in_place = header_of_input_in_place(input)?;
    let lines: Vec<&str> = in_place.split('\n').collect();
    // A second job: nothing from its line on is read.
    let second = lines.iter().position(|l| l.trim_start().starts_with('$'));
    let first_job = lines[..second.unwrap_or(lines.len())].join("\n");
    let mut pieces = pieces(&parse_header(&first_job));
    // The app's `%maxcore`, blanked from the parsed text, comes back from the strict reader (it read
    // cleanly, or `header_of_input_in_place` would have failed) at its own line.
    let input_lines: Vec<&str> = input.lines().collect();
    if let (Ok(Some(mib)), [at]) = (read_maxcore(input), maxcore_lines(&input_lines).as_slice()) {
        if second.is_none_or(|cut| *at < cut) {
            pieces.push((*at + 1, vec![format!("%maxcore {mib}")]));
            pieces.sort_by_key(|(line, _)| *line);
        }
    }
    let mut rendered: Vec<String> = pieces.into_iter().flat_map(|(_, l)| l).collect();
    if let Some(i) = second {
        rendered.push(format!("# a second job at line {}, not shown", i + 1));
    }
    Ok(rendered.join("\n"))
}

/// Which field a line comes from: the built header, or any other text (a title, an error, a reason).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    /// The built header: a line starting with `{` is a constraint entry and a line with `=` a scan
    /// or parameter definition, the only places it shows numbers in a row. Both are skipped here.
    Header,
    /// Any other text: nothing is skipped.
    Text,
}

/// A label: a token with a letter that is not itself a number (`O`, `O(1)`, `H:`, `geometry:`).
fn is_label(tok: &str) -> bool {
    tok.chars().any(|c| c.is_ascii_alphabetic()) && tok.parse::<f64>().is_err()
}

/// `line` without ORCA's input-echo prefix (`|  4> `, the format measured in a real `output.out`).
fn strip_echo(line: &str) -> &str {
    let t = line.trim_start();
    if let Some(rest) = t.strip_prefix('|') {
        let rest = rest.trim_start();
        if let Some(i) = rest.find('>') {
            if i > 0 && rest[..i].bytes().all(|b| b.is_ascii_digit()) {
                return rest[i + 1..].trim_start();
            }
        }
    }
    t
}

/// A geometry opener: `*`, any spacing, then `xyz`, `int`, `gzmt` or `xyzfile` as a whole word, in
/// any case, also after the input-echo prefix (`|  4> * xyz 0 1`). A bare `***` banner is not one.
fn geometry_opener(line: &str) -> bool {
    let Some(rest) = strip_echo(line).strip_prefix('*') else { return false };
    let word: String = rest.trim_start().chars().take_while(|c| c.is_ascii_alphanumeric()).collect();
    matches!(word.to_ascii_lowercase().as_str(), "xyz" | "int" | "gzmt" | "xyzfile")
}

/// The tripwire's detector (rule #9 on the read tools' own output): a coordinate-like row is a
/// geometry line, or **an element-or-label token followed by at least three finite numbers, at any
/// position on the line** (a lone `:` token skipped). So ORCA's input echo `|  6> C 4.10 -0.56 0.58`
/// (measured in a real `output.out`), `geometry: O x y z`, fragment and ghost rows all match. SCF
/// iteration rows (numbers only) do not.
///
/// A geometry line is, in the built header ([`Field::Header`]), any line starting with `*`; in any
/// other text ([`Field::Text`]), only a real geometry opener (`* xyz`, `* int`, `* gzmt`,
/// `* xyzfile`, any case and spacing, also echoed), so ORCA's `***ERROR***` and `*****` banners in a
/// failed job's error do not withhold it (Anton, round 5). The `{` and `=` skips apply to
/// [`Field::Header`] only.
pub fn coordinate_row(line: &str, field: Field) -> bool {
    let line = line.trim();
    let geometry_line = match field {
        Field::Header => line.starts_with('*'),
        Field::Text => geometry_opener(line),
    };
    if geometry_line {
        return true;
    }
    if field == Field::Header && line.starts_with('{') {
        return false;
    }
    if field == Field::Header && line.contains('=') {
        return false;
    }
    let toks: Vec<&str> = line.split_whitespace().filter(|t| *t != ":").collect();
    let number = |t: &str| t.parse::<f64>().is_ok_and(f64::is_finite);
    toks.windows(4).any(|w| is_label(w[0]) && w[1..].iter().all(|t| number(t)))
}
