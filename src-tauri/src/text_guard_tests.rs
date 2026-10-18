//! The user-facing text guard, Rust half (ADR-024 o22.14.11, last control): no **user-facing
//! string** — a non-comment string literal in `src-tauri/src`, tests excluded — says "withdraw"
//! (the command is gone, o22.7/o22.14.7), names a later unit as "unit 5.4" without its letter (the
//! split of o22.1: matched as `unit 5\.4(?![a-c])`), or tells the user "Cancel sweeps again" (the
//! button is Retry cancel, o22.14.2). The TypeScript half is `src/jobs/text-guard.test.ts`.
//!
//! The scan lexes each file just far enough to tell code from comments and string literals: `//`
//! and nested `/* */` comments are skipped; `"…"`, `b"…"`, raw `r#"…"#` strings are collected; a
//! `'` opens a char literal only when it closes one or two characters later (so a lifetime is not
//! mistaken for one). A file named `tests.rs` or `*_tests.rs` is a test file, and a file's own
//! `#[cfg(test)] mod tests` block (always its tail here) is cut off.

use std::path::{Path, PathBuf};

/// The string literals of Rust source `src`, comments skipped.
fn string_literals(src: &str) -> Vec<String> {
    let chars: Vec<char> = src.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    let ident = |c: char| c.is_alphanumeric() || c == '_';
    while i < chars.len() {
        let c = chars[i];
        let next = chars.get(i + 1).copied();
        if c == '/' && next == Some('/') {
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
        } else if c == '/' && next == Some('*') {
            let mut depth = 0;
            while i < chars.len() {
                if chars[i] == '/' && chars.get(i + 1) == Some(&'*') {
                    depth += 1;
                    i += 2;
                } else if chars[i] == '*' && chars.get(i + 1) == Some(&'/') {
                    depth -= 1;
                    i += 2;
                    if depth == 0 {
                        break;
                    }
                } else {
                    i += 1;
                }
            }
        } else if (c == 'r' || (c == 'b' && next == Some('r'))) && (i == 0 || !ident(chars[i - 1])) && raw_start(&chars, i).is_some() {
            let (hashes, body) = raw_start(&chars, i).unwrap_or((0, i));
            let close: Vec<char> = std::iter::once('"').chain(std::iter::repeat('#').take(hashes)).collect();
            let mut j = body;
            while j < chars.len() && !chars[j..].starts_with(&close) {
                j += 1;
            }
            out.push(chars[body..j.min(chars.len())].iter().collect());
            i = j + close.len();
        } else if c == '"' {
            let mut j = i + 1;
            let mut s = String::new();
            while j < chars.len() && chars[j] != '"' {
                if chars[j] == '\\' {
                    j += 1;
                }
                if let Some(&ch) = chars.get(j) {
                    s.push(ch);
                }
                j += 1;
            }
            out.push(s);
            i = j + 1;
        } else if c == '\'' {
            if next == Some('\\') {
                let mut j = i + 2;
                while j < chars.len() && chars[j] != '\'' {
                    j += 1;
                }
                i = j + 1;
            } else if chars.get(i + 2) == Some(&'\'') {
                i += 3;
            } else {
                i += 1;
            }
        } else {
            i += 1;
        }
    }
    out
}

/// At `chars[i]` = `r` or `br`: the raw string's `#` count and where its body starts, if it is one.
fn raw_start(chars: &[char], i: usize) -> Option<(usize, usize)> {
    let mut j = if chars[i] == 'b' { i + 2 } else { i + 1 };
    let mut hashes = 0;
    while chars.get(j) == Some(&'#') {
        hashes += 1;
        j += 1;
    }
    (chars.get(j) == Some(&'"')).then_some((hashes, j + 1))
}

/// What a user-facing string must not say, or `None`.
fn forbidden(text: &str) -> Option<&'static str> {
    if text.to_lowercase().contains("withdraw") {
        return Some("withdraw");
    }
    let mut rest = text;
    while let Some(at) = rest.find("unit 5.4") {
        let after = rest[at + "unit 5.4".len()..].chars().next();
        if !matches!(after, Some('a'..='c')) {
            return Some("unit 5.4 without its letter");
        }
        rest = &rest[at + 1..];
    }
    text.contains("Cancel sweeps again").then_some("Cancel sweeps again")
}

fn rust_sources(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            rust_sources(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            let name = path.file_name().unwrap().to_string_lossy().to_string();
            if name != "tests.rs" && !name.ends_with("_tests.rs") {
                out.push(path);
            }
        }
    }
}

/// The file's production part: everything before its `#[cfg(test)] mod tests`.
fn production(src: &str) -> &str {
    ["#[cfg(test)]\nmod tests", "#[cfg(test)]\npub(crate) mod tests", "#[cfg(test)]\npub(super) mod tests"]
        .iter()
        .filter_map(|marker| src.find(marker))
        .min()
        .map_or(src, |at| &src[..at])
}

/// The guard (o22.14.11). NEGATIVE CONTROLS: put "withdraw" back into a user-facing string (e.g.
/// `refuse_while_live`'s text) → red; set `shown_message`'s `LATER` back to "handled in unit 5.4" →
/// red; `cancelling_message`'s "Retry cancel sweeps again" back to "Cancel sweeps again" → red.
#[test]
fn no_user_facing_string_says_withdraw_an_unlettered_unit_5_4_or_cancel_sweeps_again() {
    let mut files = Vec::new();
    rust_sources(&Path::new(env!("CARGO_MANIFEST_DIR")).join("src"), &mut files);
    assert!(files.len() > 50, "the scan found the sources: {}", files.len());
    let mut hits = Vec::new();
    for file in &files {
        let src = std::fs::read_to_string(file).unwrap();
        for literal in string_literals(production(&src)) {
            if let Some(why) = forbidden(&literal) {
                hits.push(format!("{}: {why}: {literal:?}", file.display()));
            }
        }
    }
    assert!(hits.is_empty(), "user-facing strings that must not stay:\n{}", hits.join("\n"));
}

/// The lexer tells strings from comments, raw strings and lifetimes, and the matcher keeps a
/// lettered unit. (Its own self-test, so a lexer that collects nothing cannot pass the guard.)
#[test]
fn the_scan_finds_strings_and_skips_comments() {
    let src = r##"
        // "withdraw in a line comment"
        /* "withdraw in a /* nested */ block" */
        fn f<'a>(x: &'a str) -> char { let _ = "Withdraw the job"; let _ = r#"handled in unit 5.4"#; 'x' }
        const OK: &str = "handled in unit 5.4b";
        const Q: char = '"';
    "##;
    let literals = string_literals(src);
    assert_eq!(literals, ["Withdraw the job", "handled in unit 5.4", "handled in unit 5.4b"]);
    let hits: Vec<_> = literals.iter().filter_map(|l| forbidden(l)).collect();
    assert_eq!(hits, ["withdraw", "unit 5.4 without its letter"]);
    assert_eq!(forbidden("automatic sweeps stopped — Cancel sweeps again."), Some("Cancel sweeps again"));
    assert_eq!(forbidden("automatic sweeps stopped — Retry cancel sweeps again."), None);
}
