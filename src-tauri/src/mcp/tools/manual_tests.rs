//! Tests for `search_manual` (ADR-027 decision 7; M2 amendment, item 7).

use rusqlite::{params, Connection};

use super::manual::{search_manual, FIELD_MAX_BYTES, MANUAL_MAX_HITS, SNIPPET_MAX_BYTES};
use super::test_support::assert_fits;
use super::ToolError;
use crate::mcp::test_db::TestDb;

/// Index `sections` (file, title, breadcrumb, body) as the ingest would, then rebuild the FTS table.
fn index(conn: &Connection, sections: &[(String, String, Vec<String>, String)]) {
    for (i, (file, title, crumbs, body)) in sections.iter().enumerate() {
        conn.execute(
            "INSERT INTO manual_sections (orca_version, file, level, title, breadcrumb, labels, anchor, anchor_source, body_md, line_start, line_end) \
             VALUES ('6.1', ?1, 2, ?2, ?3, '[]', ?4, 'undetermined', ?5, ?6, ?6)",
            params![file, title, serde_json::to_string(crumbs).unwrap(), format!("sec-{i}"), body, i as i64],
        )
        .unwrap();
    }
    conn.execute_batch("INSERT INTO manual_fts(manual_fts) VALUES('rebuild');").unwrap();
}

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
    let long = |c: &str| c.repeat(5_000);
    let body = format!("basis {} set {} basis", "b".repeat(3_000), "c".repeat(3_000));
    let sections: Vec<_> = (0..60)
        .map(|i| (format!("{i}{}", long("f")), format!("basis {i} {}", long("t")), vec![long("x"); 20], body.clone()))
        .collect();
    index(&db.conn, &sections);
    let r = search_manual(&db.conn, "basis", Some(1_000)).expect("[ctl:manual_cap] the largest result is served");
    assert_fits(&r, "[ctl:manual_cap]");
    assert_eq!((r.limit, r.hits.len()), (MANUAL_MAX_HITS, MANUAL_MAX_HITS), "[ctl:manual_cap_hits]");
    assert!(r.more_may_exist);
    for h in &r.hits {
        assert!(h.title.len() <= FIELD_MAX_BYTES && h.file.len() <= FIELD_MAX_BYTES, "[ctl:manual_cap_fields]");
        assert!(h.snippet.len() <= SNIPPET_MAX_BYTES && h.breadcrumb_truncated, "[ctl:manual_cap_fields]");
    }
}
