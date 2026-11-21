//! The audit view (ADR-027 M4 amendment, item 4; unit M4a Part A): the limit's clamp, keyset paging
//! with no row skipped or repeated, `args` cut and marked, unreadable `reasons` shown raw without
//! failing the page, and the whole row on demand.
//!
//! Each control has a tagged assertion; `m4a_parta_controls.py` (the session scratchpad) applies
//! each mutation and expects its tag red.

use rusqlite::params;

use super::audit::{append_audit_conn, AuditEntry};
use super::audit_view::*;
use super::steps::McpReason;
use super::test_db::TestDb;
use crate::error::AppError;

fn append(db: &TestDb, args: &str, reasons: &[McpReason]) -> i64 {
    append_audit_conn(
        &db.conn,
        &AuditEntry { tool: "create_job", args, disposition: None, reasons, job_id: Some("j"), grant_id: None },
    )
    .unwrap();
    db.conn.last_insert_rowid()
}

fn reason(row: &str, message: &str) -> McpReason {
    McpReason { row: row.into(), message: message.into() }
}

/// Control LIMIT (item 8: "an audit `limit` of 0 or above 200 honoured").
#[test]
fn the_limit_defaults_to_50_and_is_clamped_to_1_through_200() {
    assert_eq!(page_limit(None), 50);
    assert_eq!(page_limit(Some(0)), 1, "LIMIT: 0 honoured");
    assert_eq!(page_limit(Some(1)), 1);
    assert_eq!(page_limit(Some(200)), 200);
    assert_eq!(page_limit(Some(201)), 200, "LIMIT: above 200 honoured");
    assert_eq!(page_limit(Some(u32::MAX)), 200, "LIMIT: above 200 honoured");

    let db = TestDb::new("audit-limit");
    for i in 0..205 {
        append(&db, &format!("{{\"n\":{i}}}"), &[]);
    }
    assert_eq!(list_audit_conn(&db.conn, None, Some(0)).unwrap().rows.len(), 1, "LIMIT: a page of 0 rows");
    assert_eq!(list_audit_conn(&db.conn, None, Some(500)).unwrap().rows.len(), 200, "LIMIT: a page above 200");
    assert_eq!(list_audit_conn(&db.conn, None, None).unwrap().rows.len(), 50);
}

/// Control PAGES (item 8: "a row skipped or repeated at a page boundary"): paging through every row
/// at several limits, rows appended meanwhile included, yields each older row exactly once, newest
/// first, and the last page says no older one exists.
#[test]
fn paging_yields_every_row_once_newest_first() {
    let db = TestDb::new("audit-pages");
    let mut ids: Vec<i64> = (0..23).map(|i| append(&db, &format!("{{\"n\":{i}}}"), &[])).collect();
    ids.reverse();
    for limit in [1u32, 2, 5, 7, 22, 23, 24, 200] {
        let (mut seen, mut before, mut pages) = (Vec::new(), None, 0);
        loop {
            let page = list_audit_conn(&db.conn, before, Some(limit)).unwrap();
            assert!(page.rows.len() <= limit as usize);
            seen.extend(page.rows.iter().map(|r| r.id));
            pages += 1;
            match page.next_before_id {
                Some(next) => {
                    assert_eq!(Some(next), page.rows.last().map(|r| r.id), "PAGES: the cursor is the last row shown");
                    before = Some(next);
                }
                None => break,
            }
            assert!(pages <= ids.len() + 1, "PAGES: paging does not end");
        }
        assert_eq!(seen, ids, "PAGES: limit {limit}: a row skipped or repeated");
    }

    // A row appended between two pages shows on the next "Refresh", never inside the older pages.
    let first = list_audit_conn(&db.conn, None, Some(10)).unwrap();
    let newer = append(&db, "{}", &[]);
    let second = list_audit_conn(&db.conn, first.next_before_id, Some(10)).unwrap();
    assert_eq!(second.rows.first().map(|r| r.id), Some(ids[10]), "PAGES: the next page starts below the last row shown");
    assert!(!second.rows.iter().any(|r| r.id == newer));
    assert_eq!(list_audit_conn(&db.conn, None, Some(1)).unwrap().rows[0].id, newer);

    assert_eq!(list_audit_conn(&TestDb::new("audit-empty").conn, None, None).unwrap(), AuditPage { rows: Vec::new(), next_before_id: None });
}

/// Control ARGS-CLIP (item 8: "`args` over 1,024 characters not clipped, or clipped without its
/// mark"): a page cuts `args` to 1,024 characters (not bytes) and marks it; the whole row returns it
/// uncut; a short `args` is neither cut nor marked.
#[test]
fn a_pages_args_are_cut_to_1024_characters_and_marked() {
    let db = TestDb::new("audit-args");
    let long = format!("{{\"x\":\"{}\"}}", "ж".repeat(3000)); // 2 bytes a character
    let long_id = append(&db, &long, &[]);
    let exact = "a".repeat(AUDIT_ARGS_PAGE_CHARS);
    let exact_id = append(&db, &exact, &[]);

    let page = list_audit_conn(&db.conn, None, None).unwrap();
    let row = |id: i64| page.rows.iter().find(|r| r.id == id).unwrap().clone();
    let cut = row(long_id);
    assert_eq!(cut.args.chars().count(), AUDIT_ARGS_PAGE_CHARS, "ARGS-CLIP: not cut to 1,024 characters");
    assert!(long.starts_with(&cut.args));
    assert!(cut.args_clipped, "ARGS-CLIP: cut without its mark");
    assert_eq!(cut.args_chars as usize, long.chars().count());
    let whole = row(exact_id);
    assert_eq!((whole.args.as_str(), whole.args_clipped), (exact.as_str(), false), "ARGS-CLIP: 1,024 characters exactly are not cut");

    let full = audit_row_conn(&db.conn, long_id).unwrap();
    assert_eq!((full.args.as_str(), full.args_clipped), (long.as_str(), false), "the whole row's args");
    assert_eq!(full.reasons, cut.reasons);
}

/// Control REASONS (item 8: "unreadable `reasons` failing the page"): a row whose `reasons` is not a
/// reasons array is shown as unreadable with its raw text cut to 1,024 characters, and the rest of
/// the page reads normally. A NULL `reasons` is an empty list.
#[test]
fn unreadable_reasons_are_shown_raw_and_never_fail_the_page() {
    let db = TestDb::new("audit-reasons");
    let good = append(&db, "{}", &[reason("R1", "no grant"), reason("P2", "unknown key")]);
    let bad = append(&db, "{}", &[]);
    let raw = format!("not json {}", "x".repeat(2000));
    db.conn.execute("UPDATE mcp_audit SET reasons = ?1 WHERE id = ?2", params![raw, bad]).unwrap();
    let wrong_shape = append(&db, "{}", &[]);
    db.conn.execute("UPDATE mcp_audit SET reasons = '{\"row\":\"R1\"}' WHERE id = ?1", params![wrong_shape]).unwrap();
    let null = append(&db, "{}", &[]);
    db.conn.execute("UPDATE mcp_audit SET reasons = NULL WHERE id = ?1", params![null]).unwrap();

    let page = list_audit_conn(&db.conn, None, None).expect("REASONS: unreadable reasons failed the page");
    assert_eq!(page.rows.len(), 4, "REASONS: every row on the page");
    let row = |id: i64| page.rows.iter().find(|r| r.id == id).unwrap().reasons.clone();
    assert_eq!(row(good), AuditReasons::Read { list: vec![reason("R1", "no grant"), reason("P2", "unknown key")] });
    let AuditReasons::Unreadable { raw: shown, raw_clipped } = row(bad) else { panic!("REASONS: {:?}", row(bad)) };
    assert_eq!(shown.chars().count(), AUDIT_REASONS_RAW_CHARS);
    assert!(raw.starts_with(&shown) && raw_clipped);
    assert_eq!(row(wrong_shape), AuditReasons::Unreadable { raw: "{\"row\":\"R1\"}".into(), raw_clipped: false });
    assert_eq!(row(null), AuditReasons::Read { list: Vec::new() });
    assert!(audit_row_conn(&db.conn, bad).is_ok(), "REASONS: the whole row fails on unreadable reasons");
}

/// `mcp_audit_row` of an unknown id is `NotFound`.
#[test]
fn an_unknown_row_is_not_found() {
    let db = TestDb::new("audit-missing");
    let id = append(&db, "{}", &[]);
    assert!(matches!(audit_row_conn(&db.conn, id + 1), Err(AppError::NotFound(_))));
}

/// The wire shape the TS types mirror.
#[test]
fn a_row_serialises_with_its_marks_and_tagged_reasons() {
    let db = TestDb::new("audit-wire");
    let id = append(&db, "{\"a\":1}", &[reason("R1", "m")]);
    let json = serde_json::to_value(audit_row_conn(&db.conn, id).unwrap()).unwrap();
    for key in ["id", "at", "tool", "args", "args_clipped", "args_chars", "disposition", "reasons", "job_id", "grant_id"] {
        assert!(json.get(key).is_some(), "`{key}`: {json}");
    }
    assert_eq!(json["reasons"], serde_json::json!({ "kind": "read", "list": [{ "row": "R1", "message": "m" }] }));
    let page = serde_json::to_value(list_audit_conn(&db.conn, None, None).unwrap()).unwrap();
    assert_eq!(page["next_before_id"], serde_json::Value::Null);
    let unreadable = serde_json::to_value(read_reasons(Some("x"))).unwrap();
    assert_eq!(unreadable, serde_json::json!({ "kind": "unreadable", "raw": "x", "raw_clipped": false }));
}
