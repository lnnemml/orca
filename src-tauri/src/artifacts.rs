//! The one list of a job's result artifacts (ADR-024 o item 6).
//!
//! [`ARTIFACT_PATTERNS`] is the single source of "which files of a job dir are its results":
//! - the curated group export copies exactly the files it matches
//!   (`commands::export_group::curated_match`);
//! - the remote download hands the same patterns to rsync as `--include` filters
//!   (`remote::sync::download_filter_args`), plus the download-only extras named there.
//!
//! Each pattern is an rsync-style glob over a **leaf name** (no `/`): `*` matches any run of
//! characters, `?` one character, `[a-z]` one character of a class. The same text is given to
//! rsync verbatim and matched here by [`glob_match`]; a test runs the real rsync over example
//! names of every pattern and checks the two agree (`remote::sync` tests).
//!
//! The patterns were pinned from real COMPLETED, SCAN and NEB job dirs (rule #10;
//! `wiki/modules/group-export.md`): the files OrcaStudio's own readers parse, plus the run's
//! input, geometry and completion marker — never `.gbw`, `.densities`, scratch `.tmp` or cubes.

/// The result artifacts of a job dir, as leaf-name globs.
///
/// - `*_trj.xyz` covers every trajectory (`input_trj.xyz`, `input_MEP_trj.xyz`,
///   `input_MEP_ALL_trj.xyz`, `input_initial_path_trj.xyz`).
/// - `*.property.txt` covers fragment outputs such as `input_atom53.property.txt` too.
/// - `*_converged.xyz` keeps every converged geometry, including the climbing-image
///   `input_NEB-CI_converged.xyz`; nothing scratch ends in `_converged.xyz`.
/// - `*.relaxscan*.dat` is the relaxed-scan curve (`input.relaxscanact.dat`,
///   `input.relaxscanscf.dat`).
/// - `*.finalensemble.xyz` is a GOAT run's conformer ensemble (`read_job_ensemble`); added
///   2026-10-05 (Anton) — before that the curated export omitted it.
/// - `input.[0-9]*.xyz` is the per-step scan geometry (`input.001.xyz` …); the digit right after
///   `input.` keeps `input.xyz` and a would-be `input.foo.xyz` out of this rule.
pub const ARTIFACT_PATTERNS: &[&str] = &[
    "input.inp",
    "output.out",
    "input.xyz",
    ".exit_code",
    "*.property.txt",
    "*.hess",
    "*_trj.xyz",
    "*.NEB.log",
    "*.final.interp",
    "*_converged.xyz",
    "*.relaxscan*.dat",
    "input.[0-9]*.xyz",
    "*.finalensemble.xyz",
];

/// Whether the leaf name `name` is one of a job's result artifacts.
pub fn is_artifact(name: &str) -> bool {
    ARTIFACT_PATTERNS.iter().any(|pattern| glob_match(pattern, name))
}

/// Match `text` against a glob `pattern` with rsync's wildcard meaning for a pattern without a
/// slash: `*` is any run of characters other than `/`, `?` is one character other than `/`,
/// `[...]` is one character of a class (ranges `a-z`, a leading `!` or `^` negates). Anything
/// else matches itself. A malformed class (no closing `]`) matches nothing.
pub fn glob_match(pattern: &str, text: &str) -> bool {
    let pattern: Vec<char> = pattern.chars().collect();
    let text: Vec<char> = text.chars().collect();
    match_from(&pattern, &text)
}

fn match_from(pattern: &[char], text: &[char]) -> bool {
    match pattern.first() {
        None => text.is_empty(),
        Some('*') => {
            // Try every split point of the star, stopping at a slash it cannot cross.
            let rest = &pattern[1..];
            for skip in 0..=text.len() {
                if match_from(rest, &text[skip..]) {
                    return true;
                }
                if text.get(skip) == Some(&'/') {
                    return false;
                }
            }
            false
        }
        Some('?') => match text.first() {
            Some(c) if *c != '/' => match_from(&pattern[1..], &text[1..]),
            _ => false,
        },
        Some('[') => {
            let Some(c) = text.first() else { return false };
            match class_match(&pattern[1..], *c) {
                Some((true, after)) => match_from(&pattern[1 + after..], &text[1..]),
                _ => false,
            }
        }
        Some(literal) => text.first() == Some(literal) && match_from(&pattern[1..], &text[1..]),
    }
}

/// Match one character against a class whose text starts right after the `[`. Returns whether it
/// matched and how many pattern characters the class used (through the closing `]`), or `None`
/// for a class that never closes.
fn class_match(class: &[char], c: char) -> Option<(bool, usize)> {
    let negated = matches!(class.first(), Some('!' | '^'));
    let mut i = usize::from(negated);
    let mut matched = false;
    let mut first = true;
    loop {
        let start = *class.get(i)?;
        if start == ']' && !first {
            return Some((matched != negated, i + 1));
        }
        first = false;
        if class.get(i + 1) == Some(&'-') && class.get(i + 2).is_some_and(|end| *end != ']') {
            let end = class[i + 2];
            matched |= start <= c && c <= end;
            i += 3;
        } else {
            matched |= start == c;
            i += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glob_star_question_and_class() {
        assert!(glob_match("*.hess", "input.hess"));
        assert!(glob_match("*.hess", ".hess"));
        assert!(!glob_match("*.hess", "input.hess.bak"));
        assert!(!glob_match("*.hess", "sub/input.hess"), "a star does not cross a slash");
        assert!(glob_match("a?c", "abc"));
        assert!(!glob_match("a?c", "a/c"));
        assert!(glob_match("input.[0-9]*.xyz", "input.001.xyz"));
        assert!(glob_match("input.[0-9]*.xyz", "input.9.xyz"));
        assert!(!glob_match("input.[0-9]*.xyz", "input.xyz"));
        assert!(!glob_match("input.[0-9]*.xyz", "input.a01.xyz"));
        assert!(!glob_match("input.[0-9]*.xyz", "input.1xyz"));
        assert!(glob_match("[!0-9]x", "ax"));
        assert!(!glob_match("[!0-9]x", "1x"));
        assert!(!glob_match("[0-9", "1"), "an unclosed class matches nothing");
        assert!(glob_match("*.relaxscan*.dat", "input.relaxscan.dat"));
        assert!(!glob_match("*.relaxscan*.dat", "input.relaxscandat"));
    }

    /// The list holds leaf-name patterns only: a `/` or `**` would mean something different to
    /// rsync (anchored or crossing directories) than to [`glob_match`].
    #[test]
    fn patterns_are_leaf_globs() {
        for pattern in ARTIFACT_PATTERNS {
            assert!(!pattern.contains('/'), "{pattern} names a path, not a leaf");
            assert!(!pattern.contains("**"), "{pattern} would cross directories in rsync");
            assert!(!pattern.is_empty());
        }
    }
}
