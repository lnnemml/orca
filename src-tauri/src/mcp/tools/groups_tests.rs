//! Tests for `list_groups` (ADR-027 decision 7; M2 amendment, item 7).

use super::groups::{list_groups, GROUPS_MAX, GROUP_NAME_MAX_BYTES};
use super::test_support::{assert_fits, id36};
use crate::mcp::test_db::TestDb;

#[test]
fn groups_page_in_creation_order_with_their_parents() {
    let db = TestDb::new("tools-groups");
    db.conn
        .execute_batch(
            "INSERT INTO groups (id, name, parent_id, created_at) VALUES
                ('a', 'study', NULL, '2026-11-20 10:00:00'),
                ('b', 'scans', 'a', '2026-11-20 10:00:01'),
                ('c', 'tss', 'a', '2026-11-20 10:00:02');",
        )
        .unwrap();
    let p = list_groups(&db.conn, None, Some(2)).unwrap();
    assert_eq!(p.groups.iter().map(|g| g.id.as_str()).collect::<Vec<_>>(), ["a", "b"]);
    assert_eq!((p.total_count, p.next_offset), (3, Some(2)));
    assert_eq!(p.groups[1].parent_id.as_deref(), Some("a"));
    let rest = list_groups(&db.conn, p.next_offset, Some(2)).unwrap();
    assert_eq!((rest.groups.len(), rest.groups[0].id.as_str(), rest.next_offset), (1, "c", None));
}

/// The largest page fits: 300 groups with 36-character ids and 5,000-character names, asked for
/// 1,000. NEGATIVE CONTROL (`ctl:groups_cap`): lift the page cap → red.
#[test]
fn the_largest_list_groups_page_fits_under_the_cap() {
    let db = TestDb::new("tools-groups-big");
    let parent = id36("root");
    // An explicit earlier time: `datetime('now')` has whole seconds, so the root could otherwise sort
    // after a child created in the same second (`created_at, id`).
    db.conn.execute("INSERT INTO groups (id, name, created_at) VALUES (?1, 'root', '2000-01-01 00:00:00')", [&parent]).unwrap();
    for i in 0..300 {
        db.conn
            .execute("INSERT INTO groups (id, name, parent_id) VALUES (?1, ?2, ?3)", rusqlite::params![id36(&format!("g{i}")), "N".repeat(5_000), parent])
            .unwrap();
    }
    let p = list_groups(&db.conn, None, Some(1_000)).expect("[ctl:groups_cap] the largest page is served");
    assert_fits(&p, "[ctl:groups_cap]");
    assert_eq!((p.groups.len(), p.limit), (GROUPS_MAX, GROUPS_MAX), "[ctl:groups_cap]");
    assert!(p.groups[1].name.len() <= GROUP_NAME_MAX_BYTES && p.groups[1].name_truncated);
}
