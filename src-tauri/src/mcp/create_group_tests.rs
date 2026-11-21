//! Tests for `create_group`'s core (ADR-027 decision 7; M3 amendment, items 1 and 3; unit M3b Part
//! A): the bounds, the trimmed name, and the insert with its audit row in one transaction. Each
//! control in the session's `m3b_parta_controls.py` names the test it turns red by its tag.

use super::audit::audit_rows_conn;
use super::create_group::*;
use super::test_db::TestDb;
use crate::commands::groups::list_groups_conn;

fn args(name: &str, parent: Option<&str>) -> CreateGroupArgs {
    CreateGroupArgs { name: name.into(), parent_id: parent.map(str::to_string) }
}

/// The group is stored trimmed, and its audit row (NULL disposition, the new id) is written with it.
/// Tag `GROUP-ROW`.
#[test]
fn a_group_is_created_trimmed_with_its_audit_row() {
    let db = TestDb::new("group-new");
    let root = run_create_group(&db.conn, &args("  SN2 study  ", None)).unwrap();
    assert_eq!((root.name.as_str(), root.parent_id.as_deref()), ("SN2 study", None));
    let child = run_create_group(&db.conn, &args("TS search", Some(&root.group_id))).unwrap();
    assert_eq!(child.parent_id.as_deref(), Some(root.group_id.as_str()));

    let groups = list_groups_conn(&db.conn).unwrap();
    let mut names: Vec<&str> = groups.iter().map(|g| g.name.as_str()).collect();
    names.sort(); // `list_groups_conn` orders by `created_at, id`, and both share a second
    assert_eq!(names, ["SN2 study", "TS search"]);
    let rows = audit_rows_conn(&db.conn).unwrap();
    assert_eq!(rows.len(), 2, "GROUP-ROW: one audit row per created group");
    for (row, group) in rows.iter().zip([&root, &child]) {
        assert_eq!((row.tool.as_str(), row.disposition.as_deref()), (CREATE_GROUP, None));
        assert_eq!((row.job_id.as_deref(), row.grant_id.as_deref()), (None, None));
        assert_eq!(row.reasons.len(), 1);
        assert_eq!((row.reasons[0].row.as_str(), row.reasons[0].message.clone()), ("group", format!("created {}", group.group_id)));
    }
    assert_eq!(rows[0].args, r#"{"name":"  SN2 study  ","parent_id":null}"#, "the argument as given");
}

/// No group commits without its row: an audit insert that fails rolls the group back. Tag
/// `GROUP-ONE-TX`.
#[test]
fn a_failed_audit_row_leaves_no_group() {
    let db = TestDb::new("group-onetx");
    db.conn
        .execute_batch("CREATE TEMP TRIGGER refuse_audit BEFORE INSERT ON mcp_audit BEGIN SELECT RAISE(ABORT, 'audit refused'); END;")
        .unwrap();
    let err = run_create_group(&db.conn, &args("orphan", None)).unwrap_err();
    assert!(err.to_string().contains("audit refused"), "{err}");
    assert!(list_groups_conn(&db.conn).unwrap().is_empty(), "GROUP-ONE-TX: a group committed without its audit row");
}

/// A parent that does not exist is the core's `NotFound`: refused, nothing written, no row.
#[test]
fn an_unknown_parent_is_refused_with_no_row() {
    let db = TestDb::new("group-parent");
    let err = run_create_group(&db.conn, &args("child", Some("no-such-group"))).unwrap_err();
    assert!(err.to_string().contains("does not exist"), "{err}");
    assert!(list_groups_conn(&db.conn).unwrap().is_empty());
    assert!(audit_rows_conn(&db.conn).unwrap().is_empty());
}

/// The name bound (200 characters, counted as given; multi-byte characters count once) and the
/// empty-after-trim refusal; the `parent_id` bound. Each refusal writes nothing. Tag `NAME-BOUND`.
#[test]
fn the_bounds_refuse_before_anything_is_written() {
    let db = TestDb::new("group-bounds");
    let at = "é".repeat(GROUP_NAME_MAX_CHARS);
    assert_eq!(run_create_group(&db.conn, &args(&at, None)).unwrap().name, at, "200 two-byte characters fit");

    let over = "a".repeat(GROUP_NAME_MAX_CHARS + 1);
    let result = run_create_group(&db.conn, &args(&over, None));
    assert!(
        matches!(&result, Err(e) if e.to_string().contains("201 characters; at most 200")),
        "NAME-BOUND: an over-long name was not refused: {result:?}"
    );
    for empty in ["", "   ", "\t\n"] {
        let err = run_create_group(&db.conn, &args(empty, None)).unwrap_err();
        assert!(err.to_string().contains("`name` is empty"), "{empty:?}: {err}");
    }
    assert!(run_create_group(&db.conn, &args("g", Some(""))).is_err());
    let err = run_create_group(&db.conn, &args("g", Some(&"p".repeat(PARENT_ID_MAX_BYTES + 1)))).unwrap_err();
    assert!(err.to_string().contains("at most 64"), "{err}");

    assert_eq!(list_groups_conn(&db.conn).unwrap().len(), 1, "only the in-bound group");
    assert_eq!(audit_rows_conn(&db.conn).unwrap().len(), 1, "a refused argument has no row");
}
