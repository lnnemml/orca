//! The one-delete-path pin (ADR-027 M4 amendment, item 6; the M4b verifier's LOW-1 and its round-2
//! MED). A job row is deleted in exactly one place, `commands::jobs::delete_job_conn`, and that is
//! where the `mcp_state = 'submitting'` refusal sits (`mcp::steps::refuse_delete_while_mcp_submitting`).
//! A second statement that removed a `jobs` row would bypass it, so this scan fails the build the day
//! one appears. In the non-test sources it asserts:
//! - exactly one `DELETE FROM jobs`, inside `delete_job_conn`;
//! - no `REPLACE INTO jobs` (`INSERT OR REPLACE` deletes the conflicting row) and no
//!   `UPDATE OR REPLACE jobs` (the same);
//! - no `jobs` column or table definition (`ALTER TABLE jobs …`, `CREATE TABLE [IF NOT EXISTS] jobs`)
//!   carrying `ON DELETE CASCADE`, which would make deleting a parent row delete jobs. (A *child* table
//!   that references `jobs` with `ON DELETE CASCADE`, as `results` does, is the reverse direction and
//!   is allowed.)
//!
//! **What the SQL matcher sees.** Only string literals are read (the text guard's lexer: comments are
//! skipped, so a doc comment that *names* the statement is no hit), after these steps, each pinned by
//! the self-test: the literals of a `concat!(…)` are joined into one; the escapes `\n \x0a \u{a}` become
//! a real newline and `\r \t \x20 \x09 \x0d \u{20} \u{9} \u{d}` a space; `/* … */` and `-- …` SQL
//! comments are removed; the quote and bracket characters `"` `` ` `` `'` `[` `]` are dropped; a schema
//! qualifier (`main.`, `temp.`, any `name.`) before `jobs` is dropped; case is folded and whitespace
//! collapsed. A production `include_str!` / `include_bytes!` / `include!` must name a plain literal
//! path, and the file it names is scanned too (an include the scan cannot read fails the pin).
//!
//! **How test code is told apart from the app's** (the scan must neither miss a production statement
//! nor trip over a test's own `DELETE FROM jobs`):
//! - A file declared by a `#[cfg(test)] mod name;` line is test-only and skipped — found from the
//!   declarations themselves (`tests.rs`, `*_tests.rs`, `test_db.rs`, `race_model.rs`, …), so there is
//!   no list of names to drift. As a cross-check, every file *named* `tests.rs` or `*_tests.rs` must be
//!   among them: a test-named file that is not `cfg(test)` fails the scan instead of being trusted.
//! - A file's own `#[cfg(test)] mod tests { … }` is cut off with the text guard's `production`; the
//!   cut is only sound if everything after it is test code, which is asserted (`after_the_cut`: in
//!   the comment- and string-blanked tail, each top-level item carries its own `#[cfg(test)]`, wherever
//!   on its line it starts, so code after a module's closing `}` on the same line is refused too).
//!
//! **Not covered, and said so** (the pin is lexical; the ADR and `modules/tauri-core.md` repeat this):
//! - SQL assembled at run time or by another macro (`format!("DELETE FROM {table}")`,
//!   `concat!("DELETE FROM ", stringify!(jobs))`, a `const` joined by `+`, a statement read from disk
//!   or the network);
//! - a delete issued outside `src-tauri/src` (the repo has no other crate or script that touches `jobs`);
//! - an `include!` of a Rust file is scanned as text, not lexed; a binary include is scanned lossily
//!   as text; `include!(concat!(env!("OUT_DIR"), …))` and any other computed path is **refused on
//!   purpose** (the scan cannot know what it reads), so a build-script-generated include would need
//!   the pin taught about it;
//! - a char literal inside `concat!` (`concat!("DELETE FROM ", 'j', "obs")`);
//! - a letter written as a non-whitespace escape (`\x6a`, `\u{6a}`): only the whitespace escapes are
//!   folded;
//! - a comment marker (`--`, `/*`) inside an SQL *string value*, which the comment folding takes for
//!   a comment and can so hide a later delete in the same literal;
//! - the cascade rule reads `jobs` itself only (`ALTER`/`CREATE TABLE jobs`), not other tables that
//!   might reach `jobs` rows some other way;
//! - a trigger or a schema change in a *migration* that deletes `jobs` rows indirectly beyond the
//!   cascade rule above (no such migration exists; a migration is reviewed as one).
//!
//! **False positive, and said so:** the dot folding joins any `word . word`, so a prose string
//! such as "…nothing to delete from Settings. Jobs stay…" reads as `delete from jobs` and fails the
//! pin (it blocks, it never hides); reword such a string.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::text_guard_tests::{production, raw_start, string_literals};

/// Every `.rs` file under `dir`.
fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// The files `src` declares as test-only: `#[cfg(test)]` (other attributes may sit between) then
/// `[pub(..)] mod name;`. Each is the file `name.rs` or `name/mod.rs` beside a `mod.rs`/`lib.rs`/
/// `main.rs`, or under `<stem>/` beside any other file; the result holds the directory-or-file stems
/// to compare by prefix.
fn cfg_test_modules(file: &Path, src: &str) -> Vec<PathBuf> {
    let parent = file.parent().unwrap();
    let stem = file.file_stem().unwrap().to_string_lossy().to_string();
    let base = if matches!(stem.as_str(), "mod" | "lib" | "main") { parent.to_path_buf() } else { parent.join(&stem) };
    let lines: Vec<&str> = src.lines().collect();
    let mut out = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        // `#[cfg(test)]`, or `#[cfg(all(test, unix))]` (a conjunction with test is test-only too).
        let attr = line.trim();
        if attr != "#[cfg(test)]" && !attr.starts_with("#[cfg(all(test,") {
            continue;
        }
        // Attributes between: a `#[path = "x.rs"]` names the file (relative to this file's directory).
        let mut path_attr: Option<String> = None;
        let mut item = None;
        for l in lines[i + 1..].iter().map(|l| l.trim()) {
            if let Some(rest) = l.strip_prefix("#[path = \"") {
                path_attr = rest.split_once('"').map(|(p, _)| p.to_string());
            } else if !l.starts_with("#[") {
                item = Some(l);
                break;
            }
        }
        let Some(item) = item else { continue };
        let decl = item.strip_prefix("pub").map_or(item, |rest| {
            let rest = rest.trim_start();
            if rest.starts_with('(') { rest.split_once(')').map_or(rest, |(_, after)| after.trim_start()) } else { rest }
        });
        if let Some(name) = decl.strip_prefix("mod ").and_then(|r| r.strip_suffix(';')) {
            out.push(match &path_attr {
                Some(p) => parent.join(p),
                None => base.join(name.trim()),
            });
        }
    }
    out
}

/// Whether `path` is one of the declared test modules (`<decl>.rs`, or any file under `<decl>/`).
fn is_declared_test(path: &Path, declared: &BTreeSet<PathBuf>) -> bool {
    declared.iter().any(|d| path == d.with_extension("rs") || path.starts_with(d))
}

/// `src` with every comment and string/char literal blanked to spaces (newlines kept), by the text
/// guard's lexing rules (`//`, nested `/* */`, `"…"`, `b"…"`, raw `r#"…"#`, a `'` opens a char literal
/// only if it closes one or two characters on). What is left is code, so brackets can be counted.
fn blanked(src: &str) -> String {
    let chars: Vec<char> = src.chars().collect();
    let mut out: Vec<char> = chars.clone();
    let blank = |out: &mut Vec<char>, from: usize, to: usize| {
        for c in out.iter_mut().take(to.min(chars.len())).skip(from) {
            if *c != '\n' {
                *c = ' ';
            }
        }
    };
    let ident = |c: char| c.is_alphanumeric() || c == '_';
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let next = chars.get(i + 1).copied();
        if c == '/' && next == Some('/') {
            let start = i;
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
            blank(&mut out, start, i);
        } else if c == '/' && next == Some('*') {
            let (start, mut depth) = (i, 0);
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
            blank(&mut out, start, i);
        } else if (c == 'r' || (c == 'b' && next == Some('r'))) && (i == 0 || !ident(chars[i - 1])) && raw_start(&chars, i).is_some() {
            let (hashes, body) = raw_start(&chars, i).unwrap_or((0, i));
            let close: Vec<char> = std::iter::once('"').chain(std::iter::repeat('#').take(hashes)).collect();
            let mut j = body;
            while j < chars.len() && !chars[j..].starts_with(&close) {
                j += 1;
            }
            let end = (j + close.len()).min(chars.len());
            blank(&mut out, i, end);
            i = end;
        } else if c == '"' {
            let start = i;
            i += 1;
            while i < chars.len() && chars[i] != '"' {
                i += if chars[i] == '\\' { 2 } else { 1 };
            }
            i = (i + 1).min(chars.len());
            blank(&mut out, start, i);
        } else if c == '\'' {
            let start = i;
            if next == Some('\\') {
                i += 2;
                while i < chars.len() && chars[i] != '\'' {
                    i += 1;
                }
                i += 1;
                blank(&mut out, start, i);
            } else if chars.get(i + 2) == Some(&'\'') {
                i += 3;
                blank(&mut out, start, i);
            } else {
                i += 1;
            }
        } else {
            i += 1;
        }
    }
    out.into_iter().collect()
}

/// The text after the `#[cfg(test)] mod tests` cut must be test code only. In the blanked tail,
/// walk the items: an attribute `#[…]` is read whole (and `#[cfg(test)]` / `#[cfg(all(test, …))]`
/// arms the next item); anything else starts an item, which must be armed, and runs to its closing
/// `}` or its `;` at bracket depth zero. The walk is by character, not by line, so code that follows a
/// module's closing `}` on the **same line** is an unarmed item and is refused. A file may keep
/// several test modules at its tail (`connection_test.rs`: `tests` and `conntest_tests`) or end in
/// `mod tests;`; a test's multi-line string with text at column 0 is blanked first, so it is no item.
fn after_the_cut(src: &str) -> bool {
    let cut = production(src).len();
    let tail: Vec<char> = blanked(&src[cut..]).chars().collect();
    let (mut i, mut armed) = (0, false);
    while i < tail.len() {
        if tail[i].is_whitespace() {
            i += 1;
            continue;
        }
        if tail[i] == '#' && tail.get(i + 1) == Some(&'[') {
            let (start, mut depth) = (i, 0);
            while i < tail.len() {
                match tail[i] {
                    '[' => depth += 1,
                    ']' => {
                        depth -= 1;
                        if depth == 0 {
                            i += 1;
                            break;
                        }
                    }
                    _ => {}
                }
                i += 1;
            }
            let attr: String = tail[start..i.min(tail.len())].iter().filter(|c| !c.is_whitespace()).collect();
            if attr == "#[cfg(test)]" || attr.starts_with("#[cfg(all(test,") {
                armed = true;
            }
            continue;
        }
        if !armed {
            return false;
        }
        armed = false;
        let mut depth = 0i64;
        while i < tail.len() {
            let c = tail[i];
            i += 1;
            match c {
                '{' | '(' | '[' => depth += 1,
                ')' | ']' => depth -= 1,
                '}' => {
                    depth -= 1;
                    if depth <= 0 {
                        break;
                    }
                }
                ';' if depth == 0 => break,
                _ => {}
            }
        }
    }
    true
}

// ---------------------------------------------------------------------------------------------
// The SQL matcher
// ---------------------------------------------------------------------------------------------

/// What a source text does to the `jobs` table, lexically.
#[derive(Debug, Default, PartialEq, Eq)]
struct Counts {
    /// `DELETE FROM jobs`.
    deletes: usize,
    /// `REPLACE INTO jobs` (also `INSERT OR REPLACE INTO jobs`) and `OR REPLACE jobs` (`UPDATE OR REPLACE`).
    replaces: usize,
    /// A `jobs` column or table definition carrying `ON DELETE CASCADE`.
    cascades: usize,
    /// An include the scan could not read (a non-literal path, a missing or non-UTF-8 file).
    problems: Vec<String>,
}

impl Counts {
    fn add(&mut self, other: Counts) {
        self.deletes += other.deletes;
        self.replaces += other.replaces;
        self.cascades += other.cascades;
        self.problems.extend(other.problems);
    }
    /// `(deletes, replaces, cascades, problems)`, for compact asserts.
    fn tuple(&self) -> (usize, usize, usize, usize) {
        (self.deletes, self.replaces, self.cascades, self.problems.len())
    }
}

/// The whitespace escapes of Rust source as the characters they stand for. The text guard's lexer
/// keeps an escape's letter and drops its backslash, so `"DELETE FROM\njobs"` would read `deleten`.
/// A newline stays a newline (it ends an SQL `--` comment); the rest become spaces.
fn unescape_whitespace(code: &str) -> String {
    let mut text = code.to_string();
    for escape in ["\\n", "\\x0a", "\\x0A", "\\u{a}", "\\u{A}", "\\u{0a}", "\\u{0A}"] {
        text = text.replace(escape, "\n");
    }
    for escape in ["\\r", "\\t", "\\x20", "\\x09", "\\x0d", "\\x0D", "\\u{20}", "\\u{9}", "\\u{09}", "\\u{d}", "\\u{0d}"] {
        text = text.replace(escape, " ");
    }
    text
}

/// `text` without the spans from `open` to `close` (each replaced by one space); an unterminated one
/// takes the rest.
fn without_spans(text: &str, open: &str, close: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(at) = rest.find(open) {
        out.push_str(&rest[..at]);
        out.push(' ');
        let after = &rest[at + open.len()..];
        match after.find(close) {
            Some(end) => rest = &after[end + close.len()..],
            None => return out,
        }
    }
    out.push_str(rest);
    out
}

/// `text` with every `<ident>.jobs` (a schema qualifier: `main.jobs`, `temp.jobs`, `other.jobs`)
/// reduced to `jobs`.
fn without_qualifiers(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let ident = |c: char| c.is_alphanumeric() || c == '_';
    let mut out: Vec<char> = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let is_dot_jobs = chars[i] == '.'
            && chars[i + 1..].starts_with(&['j', 'o', 'b', 's'])
            && !chars.get(i + 5).is_some_and(|c| ident(*c));
        if is_dot_jobs {
            while out.last().is_some_and(|c| ident(*c)) {
                out.pop();
            }
            i += 1; // the dot; `jobs` follows
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }
    out.into_iter().collect()
}

/// A statement's text normalised for matching: lowercase, `/* */` and `--` comments removed, quote
/// and bracket characters dropped, schema qualifiers dropped, whitespace collapsed to single spaces.
fn normalized(sql: &str) -> String {
    let text = without_spans(&sql.to_lowercase(), "/*", "*/");
    let text = text.lines().map(|l| l.split("--").next().unwrap_or("")).collect::<Vec<_>>().join(" ");
    let text: String = text.chars().filter(|c| !matches!(c, '"' | '`' | '\'' | '[' | ']')).collect();
    without_qualifiers(&tight_dots(&text)).split_whitespace().collect::<Vec<_>>().join(" ")
}

/// `text` with the whitespace around every `.` removed, so `main . jobs` and `main .jobs` read as
/// `main.jobs` (SQL allows whitespace on either side of a qualifier's dot).
fn tight_dots(text: &str) -> String {
    let mut out = String::new();
    let mut pending = String::new(); // whitespace seen since the last non-space character
    for c in text.chars() {
        if c.is_whitespace() {
            pending.push(c);
        } else if c == '.' {
            pending.clear();
            out.push('.');
        } else {
            if !out.ends_with('.') {
                out.push_str(&pending);
            }
            pending.clear();
            out.push(c);
        }
    }
    out.push_str(&pending);
    out
}

/// How many times `needle` occurs in `text` as a whole table name (`jobs`, not `jobs_x`).
fn statements(text: &str, needle: &str) -> usize {
    let mut n = 0;
    let mut rest = text;
    while let Some(at) = rest.find(needle) {
        let after = rest[at + needle.len()..].chars().next();
        if !after.is_some_and(|c| c.is_alphanumeric() || c == '_') {
            n += 1;
        }
        rest = &rest[at + needle.len()..];
    }
    n
}

/// The counts of one piece of SQL text (a literal, or an included file).
fn sql_counts(sql: &str) -> Counts {
    let sql = normalized(sql);
    let cascades = sql
        .split(';')
        .map(str::trim)
        .filter(|s| {
            ["alter table jobs", "create table jobs", "create table if not exists jobs", "create temp table jobs", "create temporary table jobs"]
                .iter()
                .any(|p| s.strip_prefix(p).is_some_and(|after| !after.chars().next().is_some_and(|c| c.is_alphanumeric() || c == '_')))
                && s.contains("on delete cascade")
        })
        .count();
    Counts {
        deletes: statements(&sql, "delete from jobs"),
        replaces: statements(&sql, "replace into jobs") + statements(&sql, "or replace jobs"),
        cascades,
        problems: Vec::new(),
    }
}

/// The `(open, close)` char positions of the parenthesised arguments of every `name(` in `blank`
/// (the blanked source), outermost only.
fn macro_args(blank: &[char], name: &str) -> Vec<(usize, usize)> {
    let pat: Vec<char> = name.chars().collect();
    let mut out: Vec<(usize, usize)> = Vec::new();
    let mut i = 0;
    while i + pat.len() < blank.len() {
        let is_call = blank[i..].starts_with(&pat) && (i == 0 || !(blank[i - 1].is_alphanumeric() || blank[i - 1] == '_'));
        let mut j = i + pat.len();
        while is_call && blank.get(j).is_some_and(|c| c.is_whitespace()) {
            j += 1;
        }
        if is_call && blank.get(j) == Some(&'(') {
            let (open, mut depth) = (j + 1, 0i64);
            let mut k = j;
            while k < blank.len() {
                match blank[k] {
                    '(' | '[' | '{' => depth += 1,
                    ')' | ']' | '}' => {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                    }
                    _ => {}
                }
                k += 1;
            }
            out.push((open, k.min(blank.len())));
            i = k;
        } else {
            i += 1;
        }
    }
    out
}

/// The counts of one source file's production code. `dir` is the file's directory, to read the files
/// its production `include_str!` / `include_bytes!` / `include!` name.
fn counts(src: &str, dir: Option<&Path>) -> Counts {
    let code = unescape_whitespace(production(src));
    let chars: Vec<char> = code.chars().collect();
    let blank: Vec<char> = blanked(&code).chars().collect();
    let mut rest = chars.clone();
    let mut total = Counts::default();
    let erase = |rest: &mut Vec<char>, from: usize, to: usize| {
        for c in rest.iter_mut().take(to).skip(from) {
            *c = ' ';
        }
    };

    // `include_str!("x")` and friends: a plain literal path, and the named file is scanned too.
    for name in ["include_str!", "include_bytes!", "include!"] {
        for (open, close) in macro_args(&blank, name) {
            let args: String = chars[open..close].iter().collect();
            let literals = string_literals(&args);
            let only_a_literal = blank[open..close].iter().all(|c| c.is_whitespace()) && literals.len() == 1;
            match (only_a_literal, dir) {
                // Bytes, read lossily: an `include_bytes!` of an icon or a font is not UTF-8 and is no problem.
                (true, Some(dir)) => match std::fs::read(dir.join(&literals[0])) {
                    Ok(bytes) => total.add(sql_counts(&String::from_utf8_lossy(&bytes))),
                    Err(e) => total.problems.push(format!("{name}({:?}) cannot be read: {e}", literals[0])),
                },
                (true, None) => {}
                (false, _) => total.problems.push(format!("{name}({}) does not name a plain literal path", args.trim())),
            }
            erase(&mut rest, open, close);
        }
    }

    // `concat!("DELETE FROM ", "jobs …")`: its literals are one statement.
    for (open, close) in macro_args(&blank, "concat!") {
        if rest[open..close].iter().all(|c| *c == ' ') {
            continue; // inside an include (erased above)
        }
        let args: String = rest[open..close].iter().collect();
        total.add(sql_counts(&string_literals(&args).concat()));
        erase(&mut rest, open, close);
    }

    for literal in string_literals(&rest.iter().collect::<String>()) {
        total.add(sql_counts(&literal));
    }
    total
}

/// The pin. NEGATIVE CONTROLS (`m4b_partb_controls.py`, one per form): a second `DELETE FROM jobs`
/// added to `delete_molecule_conn` (the verifier's mutation), and the same through `concat!`, quoted
/// and bracketed names, `main.`/`temp.` qualifiers, `/* */` and `--` comments, `\x20` and `\u{20}`,
/// a lower-case split one; `INSERT OR REPLACE INTO jobs` and `UPDATE OR REPLACE jobs`; an
/// `ON DELETE CASCADE` `jobs` column; an `include_str!` that names a file with a delete in it, and one
/// with a non-literal path; code on the same line after a test module's `}`.
#[test]
fn a_job_row_is_deleted_in_exactly_one_place_and_never_replaced() {
    let src_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    rust_files(&src_dir, &mut files);
    assert!(files.len() > 50, "the scan found the sources: {}", files.len());

    let mut declared = BTreeSet::new();
    for file in &files {
        declared.extend(cfg_test_modules(file, &std::fs::read_to_string(file).unwrap()));
    }
    let named_tests: Vec<&PathBuf> = files
        .iter()
        .filter(|f| {
            let name = f.file_name().unwrap().to_string_lossy();
            name == "tests.rs" || name.ends_with("_tests.rs")
        })
        .collect();
    assert!(named_tests.len() > 20, "the scan found the test files: {}", named_tests.len());
    for f in &named_tests {
        assert!(is_declared_test(f, &declared), "{} is named like a test file but is not a `#[cfg(test)] mod`", f.display());
    }

    let (mut deletes, mut replaces, mut cascades, mut problems) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for file in files.iter().filter(|f| !is_declared_test(f, &declared)) {
        let src = std::fs::read_to_string(file).unwrap();
        assert!(after_the_cut(&src), "{}: code follows the `#[cfg(test)] mod tests` block, so the cut would hide it", file.display());
        let found = counts(&src, file.parent());
        let name = file.strip_prefix(&src_dir).unwrap().display().to_string();
        if found.deletes > 0 {
            deletes.push(format!("{name} x{}", found.deletes));
        }
        if found.replaces > 0 {
            replaces.push(format!("{name} x{}", found.replaces));
        }
        if found.cascades > 0 {
            cascades.push(format!("{name} x{}", found.cascades));
        }
        problems.extend(found.problems.iter().map(|p| format!("{name}: {p}")));
    }
    assert!(problems.is_empty(), "includes the scan cannot read (it would hide SQL):\n{}", problems.join("\n"));
    assert_eq!(deletes, ["commands/jobs.rs x1"], "`DELETE FROM jobs` must appear once, in commands/jobs.rs: {deletes:?}");
    assert!(replaces.is_empty(), "`REPLACE INTO jobs` / `UPDATE OR REPLACE jobs` deletes the row it replaces: {replaces:?}");
    assert!(cascades.is_empty(), "a `jobs` column with ON DELETE CASCADE would let a parent's delete delete jobs: {cascades:?}");

    // ... and that one is inside `delete_job_conn`, where the refusal sits.
    let jobs_rs = std::fs::read_to_string(src_dir.join("commands/jobs.rs")).unwrap();
    let start = jobs_rs.find("pub(crate) fn delete_job_conn(").expect("delete_job_conn");
    let body = &jobs_rs[start..];
    let end = body.find("\n}\n").expect("the end of delete_job_conn");
    let body = &body[..end];
    assert_eq!(counts(body, None).tuple(), (1, 0, 0, 0), "the one DELETE FROM jobs is inside delete_job_conn");
    assert!(
        body.contains("refuse_delete_while_mcp_submitting(&job)?"),
        "delete_job_conn carries the submitting refusal"
    );
}

/// The scan's own tests, so one that finds nothing cannot pass the pin: every form the pin claims.
#[test]
fn the_scan_reads_sql_strings_only_and_tells_test_modules_apart() {
    let t = |src: &str| counts(src, None).tuple();
    // Comments do not count; a statement split over lines and a lower-case one do; `jobs_x` is not `jobs`.
    let src = "/// a raw `DELETE FROM jobs` in a doc comment\n\
               fn f() { let _ = \"DELETE FROM\n   jobs WHERE id = 1\"; let _ = \"delete from jobs_archive\"; \
               let _ = \"INSERT OR REPLACE INTO jobs (id) VALUES (1)\"; }\n";
    assert_eq!(t(src), (1, 1, 0, 0));
    // `UPDATE OR REPLACE jobs` replaces a row too.
    assert_eq!(t(r#"fn f() { let _ = "UPDATE OR REPLACE jobs SET title = 'x'"; }"#), (0, 1, 0, 0));
    // concat!: the pieces are one statement, counted once; pieces alone say nothing.
    assert_eq!(t(r#"fn f() { c(concat!("DELETE FROM ", "jobs WHERE mcp_state = 'submitting'")); }"#), (1, 0, 0, 0));
    assert_eq!(t(r#"fn f() { c(concat!("DELETE ", "FROM ", "jobs")); let _ = "DELETE FROM jobs"; }"#), (2, 0, 0, 0));
    assert_eq!(t(r#"const D: &str = concat!("DELETE FROM ", "molecules");"#), (0, 0, 0, 0));
    // Quoted and bracketed names, in every SQLite spelling.
    for quoted in [r#""DELETE FROM \"jobs\"""#, r#""DELETE FROM `jobs`""#, r#""DELETE FROM [jobs]""#, r#""DELETE FROM 'jobs'""#] {
        assert_eq!(t(&format!("fn f() {{ let _ = {quoted}; }}")), (1, 0, 0, 0), "{quoted}");
    }
    // Schema-qualified names.
    for qualified in ["main.jobs", "temp.jobs", "\\\"main\\\".\\\"jobs\\\"", "other.jobs"] {
        assert_eq!(t(&format!("fn f() {{ let _ = \"DELETE FROM {qualified}\"; }}")), (1, 0, 0, 0), "{qualified}");
    }
    // Whitespace around the qualifier's dot.
    for qualified in ["main . jobs", "main .jobs", "main. jobs", "\\\"main\\\" . \\\"jobs\\\""] {
        assert_eq!(t(&format!("fn f() {{ let _ = \"DELETE FROM {qualified}\"; }}")), (1, 0, 0, 0), "{qualified}");
    }
    // SQL comments, block and line (a line comment ends at an escaped newline).
    assert_eq!(t(r#"fn f() { let _ = "DELETE /* purge */ FROM jobs"; }"#), (1, 0, 0, 0));
    assert_eq!(t(r#"fn f() { let _ = "DELETE -- purge\nFROM jobs"; }"#), (1, 0, 0, 0));
    assert_eq!(t(r#"fn f() { let _ = "SELECT 1 -- DELETE FROM jobs"; }"#), (0, 0, 0, 0), "a delete inside a comment is no statement");
    // Whitespace escapes: newline, tab, \x20, \u{20}.
    assert_eq!(t(r#"fn f() { let _ = "DELETE\nFROM\tjobs"; }"#), (1, 0, 0, 0));
    assert_eq!(t(r#"fn f() { let _ = "DELETE FROM\x20jobs"; }"#), (1, 0, 0, 0));
    assert_eq!(t(r#"fn f() { let _ = "DELETE FROM\u{20}jobs"; }"#), (1, 0, 0, 0));
    // ON DELETE CASCADE on a `jobs` definition; the reverse direction (a child of jobs) and SET NULL are fine.
    assert_eq!(t(r#"const M: &str = "ALTER TABLE jobs ADD COLUMN g TEXT REFERENCES groups(id) ON DELETE CASCADE;";"#), (0, 0, 1, 0));
    assert_eq!(t(r#"const M: &str = "CREATE TABLE IF NOT EXISTS jobs (id TEXT, g TEXT REFERENCES groups(id) ON DELETE CASCADE)";"#), (0, 0, 1, 0));
    assert_eq!(t(r#"const M: &str = "ALTER TABLE jobs ADD COLUMN g TEXT REFERENCES groups(id) ON DELETE SET NULL;";"#), (0, 0, 0, 0));
    assert_eq!(t(r#"const M: &str = "CREATE TABLE results (job_id TEXT REFERENCES jobs(id) ON DELETE CASCADE)";"#), (0, 0, 0, 0));
    assert_eq!(
        t(r#"const M: &str = "ALTER TABLE jobs ADD COLUMN a TEXT; CREATE TABLE r (j TEXT REFERENCES jobs(id) ON DELETE CASCADE);";"#),
        (0, 0, 0, 0),
        "a cascade in the NEXT statement is not the jobs definition's"
    );
    // The table name ends at a word boundary: `jobs_archive` / `jobs_meta` are other tables, whose
    // cascades (and their own children's) are not the `jobs` definition's.
    assert_eq!(t(r#"const M: &str = "CREATE TABLE jobs_archive (id TEXT, j TEXT REFERENCES jobs(id) ON DELETE CASCADE)";"#), (0, 0, 0, 0));
    assert_eq!(t(r#"const M: &str = "ALTER TABLE jobs_meta ADD COLUMN j TEXT REFERENCES jobs(id) ON DELETE CASCADE;";"#), (0, 0, 0, 0));
    assert_eq!(t(r#"const M: &str = "CREATE TABLE jobs (id TEXT, g TEXT REFERENCES groups(id) ON DELETE CASCADE)";"#), (0, 0, 1, 0));
    assert_eq!(t(r#"const M: &str = "ALTER TABLE jobs";"#), (0, 0, 0, 0), "a bare name at the end of the text still ends the word");
    // include_str!: the named file is scanned; a non-literal path (or a missing file) is a problem.
    let dir = std::env::temp_dir().join(format!("orcastudio-pin-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("purge.sql"), "-- purge\nDELETE FROM jobs WHERE mcp_state = 'submitting';\n").unwrap();
    std::fs::write(dir.join("clean.sql"), "SELECT 1;\n").unwrap();
    let inc = |src: &str| counts(src, Some(&dir)).tuple();
    assert_eq!(inc(r#"const Q: &str = include_str!("purge.sql");"#), (1, 0, 0, 0));
    assert_eq!(inc(r#"const Q: &str = include_str!("clean.sql");"#), (0, 0, 0, 0));
    assert_eq!(inc(r#"const Q: &str = include_str!(concat!("pur", "ge.sql"));"#), (0, 0, 0, 1), "a computed path cannot be read");
    assert_eq!(inc(r#"const Q: &str = include_str!("missing.sql");"#), (0, 0, 0, 1));
    std::fs::remove_dir_all(&dir).ok();
    // A binary asset (an icon, a font) is read as bytes: no UTF-8 error, no hit.
    let icons = Path::new(env!("CARGO_MANIFEST_DIR")).join("icons");
    assert_eq!(counts(r#"const ICON: &[u8] = include_bytes!("32x32.png");"#, Some(&icons)).tuple(), (0, 0, 0, 0));
    assert!(std::str::from_utf8(&std::fs::read(icons.join("32x32.png")).unwrap()).is_err(), "the fixture really is not UTF-8");
    // The cut: a test module's own statements are not counted; production after it is refused.
    let tail_ok = "fn f() {}\n#[cfg(test)]\nmod tests {\n    fn t() { let _ = \"DELETE FROM jobs\"; }\n}\n";
    assert_eq!(t(tail_ok), (0, 0, 0, 0));
    assert!(after_the_cut(tail_ok));
    assert!(after_the_cut("fn f() {}\n\n#[cfg(test)]\nmod tests;\n"));
    assert!(!after_the_cut("#[cfg(test)]\nmod tests;\nfn later() {}\n"));
    assert!(after_the_cut("fn f() {}\n#[cfg(test)]\nmod tests {\n}\n\n#[cfg(test)]\npub(crate) mod more_tests {\n    fn t() {}\n}\n"));
    assert!(after_the_cut("#[cfg(test)]\nmod tests {\n}\n#[cfg(test)]\n#[derive(Debug)]\nstruct S;\n"));
    // A test's multi-line string with text at column 0 is not code.
    assert!(after_the_cut("#[cfg(test)]\nmod tests {\n    const S: &str = \"\nOpen MPI\nfn x() {\n\";\n}\n"));
    let hidden = format!("{tail_ok}fn later() {{ let _ = \"DELETE FROM jobs\"; }}\n");
    assert!(!after_the_cut(&hidden), "production code after the test module is caught, not skipped");
    // ... also on the SAME line as the module's closing brace, and after a one-line module.
    assert!(!after_the_cut("#[cfg(test)]\nmod tests {\n    fn t() {}\n} fn hidden() { let _ = \"DELETE FROM jobs\"; }\n"));
    assert!(!after_the_cut("#[cfg(test)]\nmod tests { fn t() {} } fn hidden() {}\n"));
    assert!(after_the_cut("#[cfg(test)]\nmod tests { fn t() {} } #[cfg(test)] mod more_tests { fn u() {} }\n"));
    // `#[cfg(test)] mod x;` declarations, with an attribute between and a visibility.
    let file = Path::new("/s/mcp/mod.rs");
    let decl = cfg_test_modules(file, "#[cfg(test)]\nmod a_tests;\n#[cfg(test)]\n#[allow(dead_code)]\npub(crate) mod test_db;\nmod real;\n");
    assert_eq!(decl, [PathBuf::from("/s/mcp/a_tests"), PathBuf::from("/s/mcp/test_db")]);
    let declared: BTreeSet<PathBuf> = decl.into_iter().collect();
    assert!(is_declared_test(Path::new("/s/mcp/a_tests.rs"), &declared));
    assert!(!is_declared_test(Path::new("/s/mcp/real.rs"), &declared));
    let pathed = cfg_test_modules(Path::new("/s/commands/mcp.rs"), "#[cfg(test)]\n#[path = \"mcp_tests.rs\"]\nmod tests;\n");
    assert_eq!(pathed, [PathBuf::from("/s/commands/mcp_tests.rs")]);
    let beside = cfg_test_modules(Path::new("/s/ssh_backend.rs"), "#[cfg(test)]\npub(crate) mod tests;\n");
    assert_eq!(beside, [PathBuf::from("/s/ssh_backend/tests")]);
}
