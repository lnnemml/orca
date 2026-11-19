//! The header grammar (ADR-027 decision 3; clarifications C7 and C12). The model's header is
//! **closed**: every line is a `!` line, part of a `%name … end` block, a `#` comment, or blank.
//! [`parse_header`] never fails: it returns what it recognised plus every [`Violation`], so the
//! classifier can list all REFUSED reasons at once.
//!
//! **The scanner (C7).** Each line is lexed with `#` comments stripped and `"…"` strings and
//! `{…}` constraint entries kept as single tokens, so a quoted `"end"` or a `# end` comment never
//! closes a block. Depth is tracked: inside `%geom`, `Constraints` and `Scan` open a sub-block
//! that its own `end` closes (the sub-block set of `src/scene/geomBlock.ts`). A first-`end`
//! scanner would read `%geom Constraints … end end` wrongly.
//!
//! **Fail closed.** The lexer refuses what it cannot read unambiguously: an unterminated quote or
//! brace, a `#` inside a quoted string or a brace (ORCA's reading of it has no run record), text
//! glued to a quote or brace, a control character, or non-ASCII whitespace (which ORCA might not
//! split on where we do). Each of those is a [`Violation`], which the classifier maps to R1.

/// What kind of token the lexer produced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenKind {
    /// A bare whitespace-delimited word.
    Word,
    /// A `"…"` string; [`Token::text`] keeps the quotes.
    Quoted,
    /// A `{…}` group (a `%geom Constraints` entry); [`Token::text`] keeps the braces.
    Braced,
}

/// One token, with the 1-based line it came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    pub text: String,
    pub kind: TokenKind,
    pub line: usize,
}

impl Token {
    /// True for a bare word equal to `word`, ignoring ASCII case.
    pub fn is_word(&self, word: &str) -> bool {
        self.kind == TokenKind::Word && self.text.eq_ignore_ascii_case(word)
    }
}

/// One `!` line's keywords, the leading `!` removed, as written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeywordLine {
    pub line: usize,
    pub tokens: Vec<String>,
}

/// A sub-block inside a block (`Constraints … end` or `Scan … end` inside `%geom`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubBlock {
    /// Lowercased, e.g. `constraints`.
    pub name: String,
    pub line: usize,
    pub tokens: Vec<Token>,
}

/// One item of a block body, in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlockItem {
    Token(Token),
    SubBlock(SubBlock),
}

/// A `%name … end` block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Block {
    /// Lowercased, without the `%`, e.g. `geom`.
    pub name: String,
    pub line: usize,
    pub items: Vec<BlockItem>,
    /// False when the header ended before the block's `end` (also a [`Violation`]).
    pub closed: bool,
}

/// Why a line falls outside the grammar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Violation {
    /// A line, or part of one, outside the closed grammar (R1).
    OutsideGrammar { line: usize, excerpt: String, why: String },
    /// A `*` line: a geometry block in the text (R2; also R1).
    Coordinates { line: usize, excerpt: String },
}

/// The header as parsed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ParsedHeader {
    pub keyword_lines: Vec<KeywordLine>,
    /// Every top-level block, closed or not, in order.
    pub blocks: Vec<Block>,
    /// `%name` tokens found **inside** a block (lowercased, without `%`), with their line. Blocks
    /// do not nest, so each is also a violation; the classifier still reads the name for R2/R3.
    pub nested_openers: Vec<(usize, String)>,
    pub violations: Vec<Violation>,
}

impl ParsedHeader {
    /// Every `!` keyword, in order of appearance.
    pub fn keywords(&self) -> impl Iterator<Item = &str> {
        self.keyword_lines.iter().flat_map(|l| l.tokens.iter().map(String::as_str))
    }

    /// The names of every block opener, top-level and nested, with their lines.
    pub fn all_block_names(&self) -> Vec<(usize, &str)> {
        let mut out: Vec<(usize, &str)> = self.blocks.iter().map(|b| (b.line, b.name.as_str())).collect();
        out.extend(self.nested_openers.iter().map(|(l, n)| (*l, n.as_str())));
        out
    }
}

/// The sub-block keywords of a block: only `%geom` has any here (`src/scene/geomBlock.ts`).
fn sub_block_keywords(block: &str) -> &'static [&'static str] {
    match block {
        "geom" => &["constraints", "scan"],
        _ => &[],
    }
}

/// A short excerpt of a line for a reason string.
fn excerpt(raw: &str) -> String {
    const MAX: usize = 60;
    let t = raw.trim();
    if t.chars().count() <= MAX {
        t.to_string()
    } else {
        format!("{}…", t.chars().take(MAX).collect::<String>())
    }
}

/// Why a line with two or more `#` is refused (R1). The ORCA manual says a comment "can also be
/// closed by a second `#`" (`resources/manual/6.1/contents/essentialelements/input.md.txt`, the
/// `TolE=1e-5; #Energy conv.# TolMaxP=1e-6;` example). That is a source, not a run (rule #10): how
/// ORCA 6.1.1 reads such a line is unmeasured, so no reader here guesses.
pub(crate) const UNMEASURED_COMMENT: &str =
    "two or more `#` on one line (the manual says a second `#` closes a comment; ORCA's reading is unmeasured)";

/// True for a line holding two or more `#`: every comment-stripping reader in `mcp` refuses it
/// rather than read past the second `#` as comment.
pub(crate) fn has_unmeasured_comment(line: &str) -> bool {
    line.matches('#').count() >= 2
}

/// A character the lexer refuses outright: a control character other than tab, or whitespace
/// outside ASCII.
fn forbidden_char(c: char) -> bool {
    (c.is_control() && c != '\t') || (c.is_whitespace() && !c.is_ascii())
}

/// Lex one line (its `\r` already removed) into tokens, or say why it cannot be read.
pub(crate) fn lex_line(line: &str, line_no: usize) -> Result<Vec<Token>, String> {
    if let Some(c) = line.chars().find(|c| forbidden_char(*c)) {
        return Err(format!("unsupported character U+{:04X}", c as u32));
    }
    if has_unmeasured_comment(line) {
        return Err(UNMEASURED_COMMENT.into());
    }
    let chars: Vec<char> = line.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    // After a quoted or braced token, the next character must end the token.
    let ends_token = |c: Option<&char>| matches!(c, None | Some(' ') | Some('\t') | Some('#'));
    while i < chars.len() {
        let c = chars[i];
        if c == ' ' || c == '\t' {
            i += 1;
            continue;
        }
        if c == '#' {
            break; // the rest of the line is a comment
        }
        if c == '"' || c == '{' {
            let close = if c == '"' { '"' } else { '}' };
            let start = i;
            let mut j = i + 1;
            while j < chars.len() && chars[j] != close {
                if chars[j] == '#' {
                    return Err(format!("a `#` inside {}", if c == '"' { "a quoted string" } else { "braces" }));
                }
                if c == '{' && (chars[j] == '{' || chars[j] == '"') {
                    return Err("nested braces or a quote inside braces".into());
                }
                j += 1;
            }
            if j == chars.len() {
                return Err(if c == '"' { "an unterminated quoted string" } else { "an unterminated `{`" }.into());
            }
            if !ends_token(chars.get(j + 1)) {
                return Err("text glued to a quoted string or brace".into());
            }
            out.push(Token {
                text: chars[start..=j].iter().collect(),
                kind: if c == '"' { TokenKind::Quoted } else { TokenKind::Braced },
                line: line_no,
            });
            i = j + 1;
            continue;
        }
        if c == '}' {
            return Err("a stray `}`".into());
        }
        let start = i;
        while i < chars.len() && !matches!(chars[i], ' ' | '\t' | '#') {
            if matches!(chars[i], '"' | '{' | '}') {
                return Err("a quote or brace glued to a word".into());
            }
            i += 1;
        }
        out.push(Token { text: chars[start..i].iter().collect(), kind: TokenKind::Word, line: line_no });
    }
    Ok(out)
}

/// A `%name` opener's name, lowercased, when the token is `%` + an identifier.
fn opener_name(tok: &Token) -> Option<String> {
    if tok.kind != TokenKind::Word {
        return None;
    }
    let name = tok.text.strip_prefix('%')?;
    let mut cs = name.chars();
    let first = cs.next()?;
    if !(first.is_ascii_alphabetic() || first == '_') || !cs.all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return None;
    }
    Some(name.to_ascii_lowercase())
}

/// The block being parsed, with its open sub-block if any.
struct Open {
    block: Block,
    sub: Option<SubBlock>,
}

/// Parse a header under the closed grammar. Never fails: violations are collected.
pub fn parse_header(text: &str) -> ParsedHeader {
    let mut h = ParsedHeader::default();
    let mut open: Option<Open> = None;

    for (idx, raw_line) in text.split('\n').enumerate() {
        let line_no = idx + 1;
        let raw = raw_line.strip_suffix('\r').unwrap_or(raw_line);
        let outside = |why: &str| Violation::OutsideGrammar { line: line_no, excerpt: excerpt(raw), why: why.to_string() };

        let tokens = match lex_line(raw, line_no) {
            Ok(t) => t,
            Err(why) => {
                h.violations.push(outside(&why));
                continue;
            }
        };
        let Some(first) = tokens.first() else { continue }; // blank or comment-only

        // A line that starts like a keyword line, a geometry block or a second job is outside
        // the grammar wherever it is; inside a block its structure is ambiguous, so it is skipped.
        let lead = if first.kind == TokenKind::Word { first.text.chars().next() } else { None };
        match lead {
            Some('*') => {
                h.violations.push(Violation::Coordinates { line: line_no, excerpt: excerpt(raw) });
                h.violations.push(outside("a `*` geometry line"));
                continue;
            }
            Some('$') => {
                h.violations.push(outside("a `$` line (a second job)"));
                continue;
            }
            Some('!') if open.is_some() => {
                h.violations.push(outside("a `!` line inside a block"));
                continue;
            }
            Some('!') if !raw.is_ascii() => {
                // The signature folds ASCII case only, so a non-ASCII token (`ſp`, `ωB97X-3c`)
                // could fold differently from what ORCA reads.
                h.violations.push(outside("a non-ASCII character on a `!` line"));
                continue;
            }
            Some('!') => {
                let mut kws = Vec::new();
                let mut bad = false;
                for (k, t) in tokens.iter().enumerate() {
                    let text = if k == 0 { &t.text[1..] } else { t.text.as_str() };
                    if t.kind != TokenKind::Word || (k > 0 && text.starts_with('!')) {
                        bad = true;
                    } else if !text.is_empty() {
                        kws.push(text.to_string());
                    }
                }
                if bad {
                    h.violations.push(outside("a quoted, braced or second `!` token on a `!` line"));
                } else {
                    h.keyword_lines.push(KeywordLine { line: line_no, tokens: kws });
                }
                continue;
            }
            _ => {}
        }

        let mut rest = &tokens[..];
        if open.is_none() {
            match opener_name(first) {
                Some(name) => {
                    open = Some(Open { block: Block { name, line: line_no, items: Vec::new(), closed: false }, sub: None });
                    rest = &tokens[1..];
                }
                None => {
                    h.violations.push(outside("a line that is not `!`, a `%` block, a `#` comment or blank"));
                    continue;
                }
            }
        }

        // The block body: tokens until the block's own `end`.
        for tok in rest {
            let Some(cur) = open.as_mut() else {
                // The block closed earlier on this line: anything after it is outside (C12).
                h.violations.push(outside("text after a block's `end`"));
                break;
            };
            if tok.is_word("end") {
                if let Some(sub) = cur.sub.take() {
                    cur.block.items.push(BlockItem::SubBlock(sub));
                } else if let Some(Open { mut block, .. }) = open.take() {
                    block.closed = true;
                    h.blocks.push(block);
                }
                continue;
            }
            if let Some(name) = opener_name(tok) {
                h.nested_openers.push((line_no, name));
                h.violations.push(outside("a `%` block inside a block"));
                continue;
            }
            let sub_kw = tok.kind == TokenKind::Word
                && sub_block_keywords(&cur.block.name).iter().any(|s| tok.text.eq_ignore_ascii_case(s));
            if sub_kw {
                if cur.sub.is_some() {
                    h.violations.push(outside("a sub-block inside a sub-block"));
                } else {
                    cur.sub = Some(SubBlock { name: tok.text.to_ascii_lowercase(), line: line_no, tokens: Vec::new() });
                }
                continue;
            }
            match cur.sub.as_mut() {
                Some(sub) => sub.tokens.push(tok.clone()),
                None => cur.block.items.push(BlockItem::Token(tok.clone())),
            }
        }
    }

    if let Some(Open { mut block, sub }) = open {
        if let Some(sub) = sub {
            block.items.push(BlockItem::SubBlock(sub));
        }
        h.violations.push(Violation::OutsideGrammar {
            line: block.line,
            excerpt: format!("%{}", block.name),
            why: format!("block %{} never closes", block.name),
        });
        h.blocks.push(block);
    }
    h
}
