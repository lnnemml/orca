//! `read_output` (ADR-027 decision 7; M2 amendment, item 7): a bounded tail of a job's `output.out`,
//! or a bounded search in it. **Never the whole file** (domain rule #5):
//! - **Tail** reads at most [`TAIL_WINDOW_BYTES`] from the file's end ([`read_tail_window`], the core
//!   `read_job_output` also uses), returns at most [`TAIL_MAX_LINES`] lines, cuts each to
//!   [`LINE_MAX_BYTES`], and stops at [`OUTPUT_TEXT_BYTES`] bytes of serialised line text. The result
//!   gives the file's size and where the window began, and `truncated` with the reason when fewer
//!   lines came back than were asked for.
//! - **Search** streams the file line by line ([`search_output`], which keeps at most 500 matches
//!   and counts all of them). One page holds at most [`SEARCH_MAX_MATCHES`] matches with at most
//!   [`SEARCH_MAX_CONTEXT`] context lines each side, lines cut to [`LINE_MAX_BYTES`], and stops
//!   before the matches pass [`OUTPUT_TEXT_BYTES`]; `next_offset` continues.
//!
//! The job's directory is read under the database lock ([`output_path`]); the file is read without
//! it ([`read_output_at`]), so the registered tool locks only for the first.

use std::path::{Path, PathBuf};

use rusqlite::Connection;
use serde::Serialize;

use super::{clip, result_text, size_of_text, within_cap, ToolError, RESULT_CAP_BYTES};
use crate::commands::jobs::get_job_conn;
use crate::local_backend::read_tail_window;
use crate::output_search::{search_output, SearchOptions, SearchResult};

/// The tail reads at most this many bytes from the end of `output.out`.
pub const TAIL_WINDOW_BYTES: u64 = 256 * 1024;
pub const TAIL_DEFAULT_LINES: usize = 100;
pub const TAIL_MAX_LINES: usize = 400;
/// Each returned line (tail, match or context) is cut to this many serialised bytes.
pub const LINE_MAX_BYTES: usize = 400;
/// The lines of one result stop before their serialised bytes pass this.
pub const OUTPUT_TEXT_BYTES: usize = 36_000;
pub const SEARCH_DEFAULT_MATCHES: usize = 20;
pub const SEARCH_MAX_MATCHES: usize = 50;
pub const SEARCH_MAX_CONTEXT: usize = 3;
pub const QUERY_MAX_CHARS: usize = 200;

/// What to read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OutputRequest {
    /// The last `lines` lines (default [`TAIL_DEFAULT_LINES`], at most [`TAIL_MAX_LINES`]).
    Tail { lines: Option<usize> },
    /// The matches of `query`, a literal substring or a regular expression.
    Search {
        query: String,
        regex: bool,
        case_sensitive: bool,
        context_lines: Option<usize>,
        offset: Option<usize>,
        limit: Option<usize>,
    },
}

/// One search match as `read_output` shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MatchView {
    /// 1-based line number in `output.out`.
    pub line_no: usize,
    pub line: String,
    pub context_before: Vec<String>,
    pub context_after: Vec<String>,
}

/// `read_output`'s answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum OutputView {
    /// The job has no `output.out` yet.
    NoOutput { job_id: String, reason: String },
    Tail {
        job_id: String,
        file_bytes: u64,
        /// The byte offset the read window began at; 0 when the whole file fit in it.
        window_start_byte: u64,
        /// The bytes actually read from the file: at most [`TAIL_WINDOW_BYTES`].
        read_bytes: u64,
        requested_line_count: usize,
        /// The file's last lines, in order.
        lines: Vec<String>,
        /// Lines cut to [`LINE_MAX_BYTES`].
        cut_line_count: usize,
        /// Fewer lines than asked for, and the file has more before them.
        truncated: bool,
        truncation: Option<String>,
    },
    Search {
        job_id: String,
        query: String,
        /// Every match in the file.
        total_match_count: usize,
        /// The matches kept for paging (`search_output` keeps the first 500).
        kept_match_count: usize,
        offset: usize,
        matches: Vec<MatchView>,
        /// The next page's offset; `None` when no kept match follows.
        next_offset: Option<usize>,
        lines_scanned_count: usize,
    },
}

/// The path of job `id`'s `output.out`, or `None` with why there is none. Reads the database only.
pub fn output_path(conn: &Connection, id: &str) -> Result<Result<PathBuf, String>, ToolError> {
    let job = get_job_conn(conn, id).map_err(ToolError::from_app)?;
    let Some(dir) = job.job_dir else {
        return Ok(Err(format!("job {id} has no job directory yet (status {})", job.status.as_str())));
    };
    let path = Path::new(&dir).join("output.out");
    if !path.exists() {
        return Ok(Err(format!("job {id} has no output.out yet (status {})", job.status.as_str())));
    }
    Ok(Ok(path))
}

fn tail(job_id: &str, path: &Path, lines: Option<usize>) -> Result<OutputView, ToolError> {
    let requested = lines.unwrap_or(TAIL_DEFAULT_LINES).clamp(1, TAIL_MAX_LINES);
    let window = read_tail_window(path, requested, TAIL_WINDOW_BYTES).map_err(|e| ToolError::App(e.into()))?;
    // The last lines first, until the text budget is reached; then back into file order.
    let mut kept: Vec<String> = Vec::new();
    let mut used = 0usize;
    let mut cut_line_count = 0usize;
    let mut budget_hit = false;
    for line in window.lines.iter().rev() {
        let (text, cut) = clip(line, LINE_MAX_BYTES);
        let cost = size_of_text(&result_text(&text)?) + 1;
        if used + cost > OUTPUT_TEXT_BYTES {
            budget_hit = true;
            break;
        }
        used += cost;
        cut_line_count += usize::from(cut);
        kept.push(text);
    }
    kept.reverse();
    let window_short = kept.len() < requested && window.window_start_byte > 0;
    let truncation = if budget_hit {
        Some(format!("stopped at {OUTPUT_TEXT_BYTES} bytes of line text; ask for fewer lines or search"))
    } else if window_short {
        Some(format!("the read window is the last {TAIL_WINDOW_BYTES} bytes of the file; search for earlier text"))
    } else {
        None
    };
    let view = OutputView::Tail {
        job_id: job_id.to_string(),
        file_bytes: window.file_bytes,
        window_start_byte: window.window_start_byte,
        read_bytes: window.bytes_read,
        requested_line_count: requested,
        cut_line_count,
        truncated: truncation.is_some(),
        truncation,
        lines: kept,
    };
    within_cap(view, RESULT_CAP_BYTES)
}

fn search(job_id: &str, path: &Path, req: &OutputRequest) -> Result<OutputView, ToolError> {
    let OutputRequest::Search { query, regex, case_sensitive, context_lines, offset, limit } = req else {
        return Err(ToolError::refused("not a search"));
    };
    if query.trim().is_empty() {
        return Err(ToolError::refused("the search query is empty"));
    }
    if query.chars().count() > QUERY_MAX_CHARS {
        return Err(ToolError::refused(format!("the search query is longer than {QUERY_MAX_CHARS} characters")));
    }
    let opts = SearchOptions {
        query: query.clone(),
        regex: *regex,
        case_sensitive: *case_sensitive,
        context_lines: context_lines.unwrap_or(0).min(SEARCH_MAX_CONTEXT),
    };
    let SearchResult { matches, total, lines_scanned, .. } = search_output(path, &opts).map_err(|e| match e {
        crate::error::AppError::Backend(why) => ToolError::refused(why),
        other => ToolError::App(other),
    })?;
    let offset = offset.unwrap_or(0);
    let limit = limit.unwrap_or(SEARCH_DEFAULT_MATCHES).clamp(1, SEARCH_MAX_MATCHES);
    let clip_line = |l: &String| clip(l, LINE_MAX_BYTES).0;

    let mut page: Vec<MatchView> = Vec::new();
    let mut used = 0usize;
    for m in matches.iter().skip(offset).take(limit) {
        let view = MatchView {
            line_no: m.line_no,
            line: clip_line(&m.line),
            context_before: m.context_before.iter().map(clip_line).collect(),
            context_after: m.context_after.iter().map(clip_line).collect(),
        };
        let cost = size_of_text(&result_text(&view)?) + 1;
        if used + cost > OUTPUT_TEXT_BYTES {
            break;
        }
        used += cost;
        page.push(view);
    }
    let end = offset + page.len();
    let view = OutputView::Search {
        job_id: job_id.to_string(),
        query: query.clone(),
        total_match_count: total,
        kept_match_count: matches.len(),
        offset,
        next_offset: (end < matches.len()).then_some(end),
        matches: page,
        lines_scanned_count: lines_scanned,
    };
    within_cap(view, RESULT_CAP_BYTES)
}

/// `read_output` on `path` (from [`output_path`]). Takes no lock.
pub fn read_output_at(job_id: &str, path: Result<&Path, &str>, req: &OutputRequest) -> Result<OutputView, ToolError> {
    let path = match path {
        Ok(p) => p,
        Err(reason) => return Ok(OutputView::NoOutput { job_id: job_id.to_string(), reason: reason.to_string() }),
    };
    match req {
        OutputRequest::Tail { lines } => tail(job_id, path, *lines),
        OutputRequest::Search { .. } => search(job_id, path, req),
    }
}
