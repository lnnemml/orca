//! Tests for `search_manual` (ADR-027 decision 7; M2 amendment, item 7).

use super::manual::{search_manual, FIELD_MAX_BYTES, MANUAL_MAX_HITS, SNIPPET_MAX_BYTES};
use super::test_support::{assert_fits, index_manual as index, largest_manual};
use super::ToolError;
use crate::mcp::test_db::TestDb;


fn section(file: &str, title: &str, body: &str) -> (String, String, Vec<String>, String) {
    (file.into(), title.into(), vec!["Essential elements".into()], body.into())
}

/// Hits keep their section ids; the snippet marks the match with `«` `»`; an empty query is empty.
#[test]
fn hits_keep_their_section_ids_and_readable_marks() {
    let db = TestDb::new("tools-manual");
    index(
        &db.conn,
        &[
            section("contents/RI", "RIJCOSX", "The RIJCOSX approximation accelerates hybrid DFT."),
            section("contents/solv", "CPCM", "The conductor-like polarizable continuum model."),
        ],
    );
    let r = search_manual(&db.conn, "RIJCOSX", None).unwrap();
    assert_eq!(r.hits.len(), 1);
    let h = &r.hits[0];
    let id: i64 = db.conn.query_row("SELECT id FROM manual_sections WHERE title = 'RIJCOSX'", [], |row| row.get(0)).unwrap();
    assert_eq!((h.section_id, h.title.as_str(), h.anchor.as_deref()), (id, "RIJCOSX", Some("sec-0")));
    assert!(h.snippet.contains("«RIJCOSX»"), "{}", h.snippet);
    assert!(!h.snippet.contains('\u{E000}'));
    assert!(!r.more_may_exist);
    assert!(search_manual(&db.conn, "", None).unwrap().hits.is_empty());
    assert!(matches!(search_manual(&db.conn, &"q".repeat(501), None), Err(ToolError::Refused(_))));
}

/// The largest result fits: 60 matching sections with 5,000-character titles, files and
/// breadcrumbs, and words of 3,000 characters in the body, asked for 1,000 hits. NEGATIVE
/// CONTROLS: lift the hit cap (`ctl:manual_cap_hits`) or drop the field cut
/// (`ctl:manual_cap_fields`) → red.
#[test]
fn the_largest_search_manual_fits_under_the_cap() {
    let db = TestDb::new("tools-manual-big");
    largest_manual(&db.conn);
    let r = search_manual(&db.conn, "basis", Some(1_000)).expect("[ctl:manual_cap] the largest result is served");
    assert_fits(&r, "[ctl:manual_cap]");
    assert_eq!((r.limit, r.hits.len()), (MANUAL_MAX_HITS, MANUAL_MAX_HITS), "[ctl:manual_cap_hits]");
    assert!(r.more_may_exist);
    for h in &r.hits {
        assert!(h.title.len() <= FIELD_MAX_BYTES && h.file.len() <= FIELD_MAX_BYTES, "[ctl:manual_cap_fields]");
        assert!(h.snippet.len() <= SNIPPET_MAX_BYTES && h.breadcrumb_truncated, "[ctl:manual_cap_fields]");
    }
}
