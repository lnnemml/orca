//! Tests for `read_output` (ADR-027 decision 7; M2 amendment, item 7; domain rule #5).

use std::path::{Path, PathBuf};

use super::output::{
    output_path, read_output_at, OutputRequest, OutputView, LINE_MAX_BYTES, SEARCH_MAX_MATCHES, TAIL_MAX_LINES, TAIL_WINDOW_BYTES,
};
use super::test_support::{assert_fits, largest_search_lines, largest_tail_lines, output_job, JobRow};
use crate::local_backend::{read_tail_window, TAIL_LINES_MAX_BYTES};
use super::ToolError;
use crate::mcp::test_db::TestDb;

/// A job `id` whose `output.out` holds `lines`; returns the file's path.
fn job_with_output(db: &TestDb, id: &str, lines: impl Iterator<Item = String>) -> PathBuf {
    output_job(&db.conn, &db.dir, id, lines)
}

fn read(db: &TestDb, id: &str, req: &OutputRequest) -> Result<OutputView, ToolError> {
    let path = output_path(&db.conn, id).unwrap();
    read_output_at(id, path.as_deref().map_err(String::as_str), req)
}

fn tail(lines: Option<usize>) -> OutputRequest {
    OutputRequest::Tail { lines }
}

fn search(query: &str, context: usize, offset: Option<usize>, limit: Option<usize>) -> OutputRequest {
    OutputRequest::Search { query: query.into(), regex: false, case_sensitive: true, context_lines: Some(context), offset, limit }
}

/// A 1 MiB output: the tail reads only the last [`TAIL_WINDOW_BYTES`], never the whole file, and
/// gives the last lines in order. NEGATIVE CONTROL (`ctl:output_window`): read from byte 0 → red.
#[test]
fn the_tail_reads_a_bounded_window_never_the_whole_file() {
    let db = TestDb::new("tools-output-tail");
    let path = job_with_output(&db, "big", (0..25_000).map(|i| format!("line {i:05} {}", "x".repeat(40))));
    let len = std::fs::metadata(&path).unwrap().len();
    assert!(len > 4 * TAIL_WINDOW_BYTES, "the file is larger than the window ({len} bytes)");
    let OutputView::Tail { file_bytes, window_start_byte, read_bytes, lines, truncated, .. } = read(&db, "big", &tail(Some(5))).unwrap()
    else {
        panic!("a tail")
    };
    assert_eq!(file_bytes, len);
    assert_eq!(window_start_byte, len - TAIL_WINDOW_BYTES, "[ctl:output_window] the window starts {TAIL_WINDOW_BYTES} bytes from the end");
    assert!(read_bytes <= TAIL_WINDOW_BYTES, "[ctl:output_bytes_read] {read_bytes} bytes were read, over the window");
    assert_eq!(lines.len(), 5);
    assert!(lines[0].starts_with("line 24995") && lines[4].starts_with("line 24999"), "{lines:?}");
    assert!(!truncated);
}

/// The largest tail fits: 400 lines of 1,000 characters are cut and stopped at the text budget, and
/// the result says so. NEGATIVE CONTROLS: drop the text budget (`ctl:output_char_cap`) or the line
/// cut (`ctl:output_line_cut`) → red.
#[test]
fn the_largest_tail_fits_under_the_cap() {
    let db = TestDb::new("tools-output-tail-big");
    job_with_output(&db, "wide", largest_tail_lines());
    let view = read(&db, "wide", &tail(Some(10_000))).expect("[ctl:output_char_cap] the largest tail is served");
    assert_fits(&view, "[ctl:output_char_cap]");
    let OutputView::Tail { requested_line_count, lines, truncated, truncation, cut_line_count, .. } = view else { panic!("a tail") };
    assert_eq!(requested_line_count, TAIL_MAX_LINES, "the line count is clamped");
    assert!(truncated && truncation.is_some_and(|t| t.contains("bytes of line text")));
    assert!(lines.iter().all(|l| l.len() <= LINE_MAX_BYTES), "[ctl:output_line_cut]");
    assert_eq!(cut_line_count, lines.len(), "[ctl:output_line_cut]");
    assert!(lines.len() > 50 && lines.last().unwrap().starts_with("1999"), "{} lines", lines.len());
}

#[test]
fn a_short_output_is_read_whole_and_a_missing_one_is_said() {
    let db = TestDb::new("tools-output-short");
    job_with_output(&db, "s", (0..3).map(|i| format!("l{i}")));
    let OutputView::Tail { lines, window_start_byte, truncated, .. } = read(&db, "s", &tail(None)).unwrap() else { panic!() };
    assert_eq!((lines, window_start_byte, truncated), (vec!["l0".to_string(), "l1".into(), "l2".into()], 0, false));

    JobRow::new("none").insert(&db.conn);
    assert!(matches!(read(&db, "none", &tail(None)).unwrap(), OutputView::NoOutput { reason, .. } if reason.contains("no job directory")));
    assert!(matches!(output_path(&db.conn, "nope"), Err(ToolError::Refused(_))));
}

/// Search pages through the kept matches with context, and counts every match.
#[test]
fn search_pages_with_context_and_counts_every_match() {
    let db = TestDb::new("tools-output-search");
    job_with_output(&db, "s", (0..100).map(|i| if i % 10 == 0 { format!("WARNING {i}") } else { format!("ok {i}") }));
    let OutputView::Search { total_match_count, matches, next_offset, .. } = read(&db, "s", &search("WARNING", 1, None, Some(4))).unwrap() else {
        panic!()
    };
    assert_eq!((total_match_count, matches.len(), next_offset), (10, 4, Some(4)));
    assert_eq!((matches[1].line_no, matches[1].line.as_str()), (11, "WARNING 10"));
    assert_eq!((matches[1].context_before.as_slice(), matches[1].context_after.as_slice()), (&["ok 9".to_string()][..], &["ok 11".to_string()][..]));
    let OutputView::Search { matches, next_offset, .. } = read(&db, "s", &search("WARNING", 0, Some(8), Some(4))).unwrap() else { panic!() };
    assert_eq!((matches.iter().map(|m| m.line_no).collect::<Vec<_>>(), next_offset), (vec![81, 91], None));

    for bad in [search("", 0, None, None), search(&"q".repeat(201), 0, None, None)] {
        assert!(matches!(read(&db, "s", &bad), Err(ToolError::Refused(_))));
    }
    let bad_regex = OutputRequest::Search { query: "(".into(), regex: true, case_sensitive: false, context_lines: None, offset: None, limit: None };
    assert!(matches!(read(&db, "s", &bad_regex), Err(ToolError::Refused(why)) if why.contains("regular expression")));
}

/// The largest search page fits: 600 matching 1,000-character lines with 3 context lines each,
/// asked for 1,000 at once. NEGATIVE CONTROL (`ctl:search_cap`): drop the page's text budget → red.
#[test]
fn the_largest_search_page_fits_under_the_cap() {
    let db = TestDb::new("tools-output-search-big");
    job_with_output(&db, "s", largest_search_lines());
    let req = search("HIT", 10, None, Some(1_000));
    let view = read(&db, "s", &req).expect("[ctl:search_cap] the largest page is served");
    assert_fits(&view, "[ctl:search_cap]");
    let OutputView::Search { matches, next_offset, kept_match_count, total_match_count, .. } = view else { panic!() };
    assert!(matches.len() <= SEARCH_MAX_MATCHES && matches.iter().all(|m| m.context_before.len() <= 3));
    assert_eq!((total_match_count, kept_match_count), (600, 500), "search_output keeps 500 and counts all");
    assert_eq!(next_offset, Some(matches.len()), "the page says where the next one starts");
}

/// `read_tail_lines` (the log console's reader) still returns the same lines through the shared
/// window core.
#[test]
fn the_console_reader_still_reads_its_tail() {
    let db = TestDb::new("tools-output-console");
    let path = job_with_output(&db, "c", (0..10).map(|i| format!("c{i}")));
    let lines = crate::local_backend::read_tail_lines(Path::new(&path), 3).unwrap();
    assert_eq!(lines, ["c7", "c8", "c9"]);
}

/// The console's 8 MB window bounds what is read, not only the reported offset: a 9 MiB output
/// reads exactly the window. NEGATIVE CONTROL (`ctl:output_bytes_read`, with the tail test): read
/// the whole file from byte 0 → red.
#[test]
fn the_console_window_bounds_the_bytes_read() {
    let db = TestDb::new("tools-output-console-big");
    let path = job_with_output(&db, "c", (0..150_000).map(|i| format!("{i:06} {}", "c".repeat(56))));
    let len = std::fs::metadata(&path).unwrap().len();
    assert!(len > TAIL_LINES_MAX_BYTES, "{len} bytes");
    let w = read_tail_window(Path::new(&path), 3, TAIL_LINES_MAX_BYTES).unwrap();
    assert!(w.bytes_read <= TAIL_LINES_MAX_BYTES, "[ctl:output_bytes_read] {} bytes read", w.bytes_read);
    assert_eq!(w.lines.last().map(|l| &l[..6]), Some("149999"));
}
