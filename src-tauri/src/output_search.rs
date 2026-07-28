//! Streaming, context-aware search over an ORCA `output.out`.
//!
//! Outputs reach tens of MB, so the file is **never read whole** (domain rule
//! #5): [`search_output`] streams it line by line through a `BufReader`, holding
//! only a small ring buffer of context lines, the handful of matches still
//! awaiting their trailing context, and the (capped) result list.
//!
//! On top of raw search sit **chemistry-aware presets** ([`SEARCH_PRESETS`]) —
//! one-click chips for what a chemist actually hunts for in an ORCA output
//! (warnings, aborts, non-convergence, imaginary modes, …). A bare search box
//! would force the user to remember ORCA's exact banner wording, which defeats
//! the point; the presets are also a learning aid (see `orca/output-files.md`).

use std::collections::VecDeque;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

use regex::RegexBuilder;
use serde::{Deserialize, Serialize};
use tauri::State;

use crate::commands::jobs::get_job_conn;
use crate::commands::settings::DbState;
use crate::error::AppError;

/// Lines of context kept either side of a match.
const CONTEXT_LINES: usize = 2;
/// Hard cap on returned matches (the `total` still counts every hit). A search
/// that hits thousands of lines would otherwise flood the UI and the IPC.
const MAX_MATCHES: usize = 500;

/// One match: the line itself plus a few lines either side, so the user sees
/// what the hit means without leaving the results list.
#[derive(Clone, Debug, Serialize)]
pub struct OutputMatch {
    /// 1-indexed line number in output.out.
    pub line_no: usize,
    pub line: String,
    pub context_before: Vec<String>,
    pub context_after: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct SearchResult {
    pub matches: Vec<OutputMatch>,
    /// Total matches in the file — may exceed `matches.len()` when capped.
    pub total: usize,
    pub truncated: bool,
    pub lines_scanned: usize,
}

impl SearchResult {
    fn empty() -> Self {
        SearchResult {
            matches: Vec::new(),
            total: 0,
            truncated: false,
            lines_scanned: 0,
        }
    }
}

#[derive(Deserialize)]
pub struct SearchOptions {
    pub query: String,
    /// Treat `query` as a regular expression instead of a literal substring.
    pub regex: bool,
    pub case_sensitive: bool,
}

/// A compiled matcher; the case-insensitive literal lowercases its needle once
/// up front rather than per line.
enum Matcher {
    Regex(regex::Regex),
    LiteralCaseSensitive(String),
    LiteralCaseInsensitive(String),
}

impl Matcher {
    fn build(opts: &SearchOptions) -> Result<Self, AppError> {
        if opts.regex {
            let re = RegexBuilder::new(&opts.query)
                .case_insensitive(!opts.case_sensitive)
                .build()
                .map_err(|e| AppError::Backend(format!("invalid regular expression: {e}")))?;
            Ok(Matcher::Regex(re))
        } else if opts.case_sensitive {
            Ok(Matcher::LiteralCaseSensitive(opts.query.clone()))
        } else {
            Ok(Matcher::LiteralCaseInsensitive(opts.query.to_lowercase()))
        }
    }

    fn is_match(&self, line: &str) -> bool {
        match self {
            Matcher::Regex(re) => re.is_match(line),
            Matcher::LiteralCaseSensitive(n) => line.contains(n.as_str()),
            Matcher::LiteralCaseInsensitive(n) => line.to_lowercase().contains(n.as_str()),
        }
    }
}

/// Search `path` line by line, returning matches with surrounding context.
/// Streams the file — never loads it whole (domain rule #5).
pub fn search_output(path: &Path, opts: &SearchOptions) -> Result<SearchResult, AppError> {
    if opts.query.is_empty() {
        return Ok(SearchResult::empty());
    }
    let matcher = Matcher::build(opts)?;

    let reader = BufReader::new(File::open(path)?);

    // The previous `CONTEXT_LINES` lines (context_before for the next match).
    let mut before: VecDeque<String> = VecDeque::with_capacity(CONTEXT_LINES);
    // Matches whose trailing context is still being filled.
    let mut pending: Vec<OutputMatch> = Vec::new();
    let mut matches: Vec<OutputMatch> = Vec::new();
    let mut total = 0usize;
    let mut lines_scanned = 0usize;

    for (idx, line) in reader.lines().enumerate() {
        let line = line?;
        let line_no = idx + 1;
        lines_scanned = line_no;

        // 1. This line is trailing context for any still-pending match.
        for m in pending.iter_mut() {
            if m.context_after.len() < CONTEXT_LINES {
                m.context_after.push(line.clone());
            }
        }
        // 2. Finalize matches whose trailing context is now complete. They fill
        //    in line order, so `matches` stays sorted by line_no.
        let mut i = 0;
        while i < pending.len() {
            if pending[i].context_after.len() >= CONTEXT_LINES {
                matches.push(pending.remove(i));
            } else {
                i += 1;
            }
        }
        // 3. Is this line itself a match? (context_after starts empty and fills
        //    on subsequent iterations — the match line never includes itself.)
        if matcher.is_match(&line) {
            if total < MAX_MATCHES {
                pending.push(OutputMatch {
                    line_no,
                    line: line.clone(),
                    context_before: before.iter().cloned().collect(),
                    context_after: Vec::new(),
                });
            }
            // Count every hit even past the cap, so `total` is truthful.
            total += 1;
        }
        // 4. Slide the ring buffer.
        if before.len() == CONTEXT_LINES {
            before.pop_front();
        }
        before.push_back(line);
    }

    // EOF: matches near the end never filled their trailing context — flush them
    // (still in line order, since `pending` is ordered by line_no).
    matches.append(&mut pending);

    let truncated = total > matches.len();
    Ok(SearchResult {
        matches,
        total,
        truncated,
        lines_scanned,
    })
}

// --- ORCA-specific presets ---------------------------------------------------

/// Curated searches for what one actually looks for in an ORCA output. Exposed
/// to the UI as one-click chips — a search box alone makes the user remember
/// ORCA's exact banner wording, which defeats the point.
pub struct SearchPreset {
    pub id: &'static str,
    pub label: &'static str,
    pub query: &'static str,
    pub regex: bool,
    /// Some presets must match case-sensitively — see `errors`, where a
    /// case-insensitive `error` would hit the benign `DIIS Error` on every SCF
    /// line (verified against real output). Default is case-insensitive.
    pub case_sensitive: bool,
    pub description: &'static str,
}

pub const SEARCH_PRESETS: &[SearchPreset] = &[
    SearchPreset {
        id: "warnings",
        label: "Warnings",
        query: "WARNING",
        regex: false,
        case_sensitive: false,
        description: "ORCA advisory messages — often the reason a result is off.",
    },
    // Case-SENSITIVE and targeted: a case-insensitive `error` matches the benign
    // `DIIS Error` / `Startup error` printed on every SCF (12+ hits in a normal
    // run) — noise that buries real aborts. All-caps `ERROR`, ORCA's
    // `error termination` line, and `aborting` are the genuine fatal signatures.
    SearchPreset {
        id: "errors",
        label: "Errors",
        query: r"ERROR|error termination|aborting|ABORTING",
        regex: true,
        case_sensitive: true,
        description: "Fatal problems and abort messages.",
    },
    SearchPreset {
        id: "scf_trouble",
        label: "SCF not converged",
        query: "SCF NOT CONVERGED",
        regex: false,
        case_sensitive: false,
        description: "The SCF failed to converge — any energy after this is meaningless.",
    },
    // ORCA marks a negative vibrational frequency with `***imaginary mode***`;
    // the literal `imaginary mode` matches it. Deliberately NOT bare `imaginary`,
    // which would hit `imaginary perturbations` (a CPHF count present in every
    // Freq run, even at a true minimum).
    SearchPreset {
        id: "imaginary",
        label: "Imaginary modes",
        query: "imaginary mode",
        regex: false,
        case_sensitive: false,
        description: "A negative frequency: a saddle point, not a minimum.",
    },
    SearchPreset {
        id: "energies",
        label: "Final energies",
        query: "FINAL SINGLE POINT ENERGY",
        regex: false,
        case_sensitive: false,
        description: "One per optimization cycle; the last is the final energy.",
    },
    SearchPreset {
        id: "geom_conv",
        label: "Geometry convergence",
        query: "Geometry convergence",
        regex: false,
        case_sensitive: false,
        description: "The per-cycle convergence tables.",
    },
    SearchPreset {
        id: "timings",
        label: "Timings",
        query: "TOTAL RUN TIME|Sum of individual times",
        regex: true,
        case_sensitive: false,
        description: "Where the wall time went.",
    },
    SearchPreset {
        id: "basis",
        label: "Basis set info",
        query: "Basis Dimension|Number of basis functions",
        regex: true,
        case_sensitive: false,
        description: "How large the calculation actually was.",
    },
];

/// Serde-friendly copy of a [`SearchPreset`] for the UI.
#[derive(Clone, Serialize)]
pub struct SearchPresetInfo {
    pub id: String,
    pub label: String,
    pub query: String,
    pub regex: bool,
    pub case_sensitive: bool,
    pub description: String,
}

// --- Tauri commands ----------------------------------------------------------

/// Search a job's `output.out`. Returns an empty result — not an error — when
/// the job has no directory or output yet (it may not have started writing).
#[tauri::command]
pub fn search_job_output(
    db: State<'_, DbState>,
    id: String,
    opts: SearchOptions,
) -> Result<SearchResult, AppError> {
    let job_dir = {
        let conn = db.lock()?;
        get_job_conn(&conn, &id)?.job_dir
    };
    let Some(job_dir) = job_dir else {
        return Ok(SearchResult::empty());
    };
    let out_path = Path::new(&job_dir).join("output.out");
    if !out_path.exists() {
        return Ok(SearchResult::empty());
    }
    search_output(&out_path, &opts)
}

/// The curated ORCA search presets, for the UI's one-click chips.
#[tauri::command]
pub fn get_search_presets() -> Vec<SearchPresetInfo> {
    SEARCH_PRESETS
        .iter()
        .map(|p| SearchPresetInfo {
            id: p.id.to_string(),
            label: p.label.to_string(),
            query: p.query.to_string(),
            regex: p.regex,
            case_sensitive: p.case_sensitive,
            description: p.description.to_string(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::path::PathBuf;

    fn temp_file(content: &str) -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir()
            .join(format!("orcastudio-search-{}-{n}.txt", std::process::id()));
        let mut f = File::create(&path).unwrap();
        f.write_all(content.as_bytes()).unwrap();
        path
    }

    fn opts(query: &str, regex: bool, case_sensitive: bool) -> SearchOptions {
        SearchOptions {
            query: query.to_string(),
            regex,
            case_sensitive,
        }
    }

    #[test]
    fn finds_literal_matches_with_context() {
        let content: String = (1..=20).map(|i| format!("line {i}\n")).collect();
        let path = temp_file(&content);

        let r = search_output(&path, &opts("line 10", false, false)).unwrap();
        assert_eq!(r.total, 1);
        assert_eq!(r.matches.len(), 1);
        let m = &r.matches[0];
        assert_eq!(m.line_no, 10);
        assert_eq!(m.line, "line 10");
        assert_eq!(m.context_before, vec!["line 8", "line 9"]);
        assert_eq!(m.context_after, vec!["line 11", "line 12"]);
        assert!(!r.truncated);
        assert_eq!(r.lines_scanned, 20);

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn context_at_file_boundaries() {
        let content: String = (1..=5).map(|i| format!("row {i}\n")).collect();
        let path = temp_file(&content);

        // First line: no `before` context.
        let first = search_output(&path, &opts("row 1", false, false)).unwrap();
        assert_eq!(first.matches[0].line_no, 1);
        assert!(first.matches[0].context_before.is_empty());
        assert_eq!(first.matches[0].context_after, vec!["row 2", "row 3"]);

        // Last line: no `after` context, and it doesn't panic on EOF flush.
        let last = search_output(&path, &opts("row 5", false, false)).unwrap();
        assert_eq!(last.matches[0].line_no, 5);
        assert_eq!(last.matches[0].context_before, vec!["row 3", "row 4"]);
        assert!(last.matches[0].context_after.is_empty());

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn case_insensitive_by_default() {
        let path = temp_file("a line\nWARNING: something\nanother\n");
        let r = search_output(&path, &opts("warning", false, false)).unwrap();
        assert_eq!(r.total, 1);
        assert_eq!(r.matches[0].line_no, 2);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn case_sensitive_respected() {
        let path = temp_file("a line\nWARNING: something\nanother\n");
        let r = search_output(&path, &opts("warning", false, true)).unwrap();
        assert_eq!(r.total, 0);
        assert!(r.matches.is_empty());
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn regex_mode_works() {
        let path = temp_file("all good\nfatal ERROR here\nnow aborting the run\ndone\n");
        let r = search_output(&path, &opts("ERROR|aborting", true, false)).unwrap();
        assert_eq!(r.total, 2);
        assert_eq!(r.matches[0].line_no, 2);
        assert_eq!(r.matches[1].line_no, 3);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn invalid_regex_is_an_error() {
        let path = temp_file("whatever\n");
        let err = search_output(&path, &opts("[unclosed", true, false)).unwrap_err();
        assert!(matches!(err, AppError::Backend(_)));
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn caps_results_but_counts_all() {
        // 600 matching lines interleaved so context handling runs too.
        let content: String = (1..=600).map(|i| format!("hit {i}\n")).collect();
        let path = temp_file(&content);
        let r = search_output(&path, &opts("hit", false, false)).unwrap();
        assert_eq!(r.matches.len(), MAX_MATCHES);
        assert_eq!(r.total, 600);
        assert!(r.truncated);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn empty_query_returns_nothing() {
        let path = temp_file("some\ncontent\nhere\n");
        let r = search_output(&path, &opts("", false, false)).unwrap();
        assert_eq!(r.total, 0);
        assert!(r.matches.is_empty());
        assert!(!r.truncated);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn searches_real_fixture() {
        // The convergence fixture has two `|Geometry convergence|` blocks.
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/opt_output_excerpt.txt");
        let r = search_output(&path, &opts("Geometry convergence", false, false)).unwrap();
        assert_eq!(r.total, 2);
        assert_eq!(r.matches.len(), 2);
        // Each hit line actually contains the banner.
        assert!(r.matches.iter().all(|m| m.line.contains("Geometry convergence")));
    }
}
