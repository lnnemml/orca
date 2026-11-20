//! Tests for `list_jobs` and `get_job` (ADR-027 decision 7; M2 amendment, items 7 and 8).

use serde_json::Value;

use super::jobs::{get_job, list_jobs, JobFilter, ERROR_EXCERPT_BYTES, LIST_JOBS_MAX, TITLE_MAX_BYTES};
use super::test_support::{assert_fits, id36, unitless_numeric_keys, JobRow};
use super::{result_text, ToolError};
use crate::mcp::test_db::TestDb;
use crate::models::job::{JobOrigin, JobStatus};

/// A real input: the app's `%maxcore`, a `%pal` block and an inline `* xyz` block.
const WATER_INPUT: &str = "! B3LYP def2-SVP Opt\n%maxcore 2000\n%pal nprocs 4 end\n* xyz 0 1\n\
O 0.000000 0.000000 0.117790\nH 0.000000 0.755453 -0.471161\nH 0.000000 -0.755453 -0.471161\n*\n";

/// Every coordinate value of [`WATER_INPUT`] that a leak would carry.
const WATER_COORDS: [&str; 3] = ["0.117790", "0.755453", "0.471161"];

fn json_of<T: serde::Serialize>(v: &T) -> String {
    result_text(v).unwrap()
}

/// No coordinate of the input appears anywhere in the serialised view, and no `* xyz 0 1` line does
/// (the words "`* xyz` line" in the charge source are not the line).
fn assert_no_coordinates(json: &str, tag: &str) {
    for c in WATER_COORDS {
        assert!(!json.contains(c), "{tag}: the view carries the coordinate {c}: {json}");
    }
    assert!(!json.contains("* xyz 0 1"), "{tag}: the view carries the `* xyz` line: {json}");
}

/// The projection carries the built header (`%maxcore` from the strict reader, `%pal nprocs` from the
/// display-only list) and
/// the atom count, never the block. NEGATIVE CONTROLS: the view carries `input_content`
/// (`ctl:get_job_input`), or the header is built from the whole input (`ctl:get_job_header`) → red.
#[test]
fn get_job_never_carries_the_coordinate_block() {
    let db = TestDb::new("tools-get-job");
    JobRow { input: WATER_INPUT, energy: Some(-76.4089), ..JobRow::new("w") }.insert(&db.conn);
    // Served, never refused: a refusal here means the tripwire caught a leak in the view.
    let view = get_job(&db.conn, "w").expect("[ctl:get_job_input] the view is served with no coordinate");
    let json = json_of(&view);
    assert_no_coordinates(&json, "[ctl:get_job_input]");
    assert_eq!(
        view.header.as_deref(),
        Some("! B3LYP def2-SVP Opt\n%maxcore 2000\n%pal\n  nprocs 4\nend"),
        "[ctl:get_job_header] the header without the block"
    );
    assert_eq!(view.header_withheld, None);
    assert_eq!(view.atom_count, Some(3));
    assert_eq!((view.charge_mult.charge, view.charge_mult.multiplicity), (Some(0), Some(1)));
    assert!(view.charge_mult.source.contains("`* xyz` line of job w"), "{}", view.charge_mult.source);
    assert_eq!(view.energy_eh, Some(-76.4089));
    assert!(!json.contains("scene"), "no scene field: {json}");
}

/// The round-2 shapes (from the manual: `coordinates.md.txt:228`, `counterpoise.md.txt:126`,
/// `population.md.txt:904`) under the built header: a second job, and `%coords` blocks with
/// Cartesian, internal, fragment-labelled, ghost and comma-separated rows, plus a stray ghost row
/// after the main block. Each is a marker; no coordinate is served. (The round-2 filter controls
/// `get_job_postcondition`, `get_job_classifier` and `coord_line_widened` are retired with the filter;
/// `ctl:build_bypass` in `header_view_tests.rs` covers these inputs too.)
#[test]
fn geometry_constructs_never_reach_the_header() {
    let db = TestDb::new("tools-get-job-second");
    let tail = "* xyz 0 1\nH 0 0 0\nH 0 0 0.74\n*\n";
    let inputs = [
        ("two", format!("{WATER_INPUT}\n$new_job\n! B3LYP def2-SVP SP\n* xyz 0 1\nO 0.000000 0.000000 0.117790\n*\n")),
        ("cart", format!("! SP\n%coords\n Coords\n  H 0.000000 0.755453 -0.471161\n end\nend\n{tail}")),
        ("internal", format!("! SP\n%coords\n CTyp internal\n Coords\n  O 0 0 0 0.0 0.0 0.0\n  H 1 2 0 0.755453 104.471280 0\n end\nend\n{tail}")),
        ("fragment", format!("! SP\n%coords\n Coords\n  O(1) 0.0 0.0 0.117790\n end\nend\n{tail}")),
        ("ghost", format!("! SP\n%coords\n Coords\n  H: 0.0 0.755453 -0.471161\n  O : 0.0 0.0 0.117790\n end\nend\n{tail}")),
        ("comma", format!("! SP\n%coords\n Coords\n  H 0.000000,0.755453,-0.471161\n end\nend\n{tail}")),
        ("stray", "! SP\n* xyz 0 1\nH 0 0 0\nH 0 0 0.74\n*\nH: 0.0 0.755453 -0.471161\n".to_string()),
    ];
    for (id, input) in &inputs {
        JobRow { input, ..JobRow::new(id) }.insert(&db.conn);
        let view = get_job(&db.conn, id).unwrap();
        assert_no_coordinates(&json_of(&view), id);
        let header = view.header.unwrap_or_else(|| panic!("{id}: a built header"));
        assert!(header.contains("not shown"), "{id}: a marker in:\n{header}");
    }
}

#[test]
fn get_job_shows_notices_reasons_and_mcp_fields() {
    let db = TestDb::new("tools-get-job-mcp");
    let g = crate::mcp::test_db::grant(&db.conn, 5, 2);
    db.job("m", "queued", Some(&g.id));
    db.conn
        .execute(
            "UPDATE jobs SET mcp_state = 'submitting', mcp_reasons = ?1, error_message = ?2 WHERE id = 'm'",
            rusqlite::params![r#"[{"row":"P4","message":"memory unknown"}]"#, "e".repeat(10_000)],
        )
        .unwrap();
    let view = get_job(&db.conn, "m").unwrap();
    assert_eq!(view.origin, JobOrigin::Mcp);
    assert_eq!(view.grant_id.as_deref(), Some(g.id.as_str()));
    assert_eq!(view.mcp_state.as_deref(), Some("submitting"));
    assert_eq!(view.mcp_reasons.len(), 1);
    assert_eq!(view.mcp_reasons[0].row, "P4");
    assert!(view.notices.iter().any(|n| n.contains("Claude is submitting")), "{:?}", view.notices);
    assert!(view.error_truncated);
    // `! SP` with no block: no header, no atom count, a charge error — each a kind, no source text.
    assert_eq!((view.header, view.atom_count), (None, None));
    assert_eq!(view.charge_mult.error.as_deref(), Some("no `* xyz <charge> <multiplicity>` line"));
    assert_eq!(view.header_withheld.as_deref(), Some("the coordinate block: no `* xyz <charge> <multiplicity>` line"));

    match get_job(&db.conn, "nope") {
        Err(ToolError::Refused(why)) => assert!(why.contains("nope"), "{why}"),
        other => panic!("a missing job is refused: {other:?}"),
    }
}

/// A line of control characters is a parser refusal: a marker with its line, served, small.
#[test]
fn a_line_of_control_characters_is_a_marker() {
    let db = TestDb::new("tools-get-job-control");
    let input = format!("! SP\n# {}\n* xyz 0 1\nH 0 0 0\nH 0 0 0.74\n*\n", "\u{1}".repeat(20_000));
    JobRow { input: &input, ..JobRow::new("ctl") }.insert(&db.conn);
    let view = get_job(&db.conn, "ctl").expect("served");
    assert_eq!(view.header.as_deref(), Some("! SP\n# line 2: outside the header grammar (unsupported character U+0001), not shown"));
    assert_fits(&view, "control characters");
}

/// The largest `get_job` fits: a built header of 1,500 named keys (about 45,000 bytes before its
/// cut), a 50,000-character error and 100 long reasons. NEGATIVE CONTROL (`ctl:get_job_cap`): drop
/// the header cut → red.
#[test]
fn the_largest_get_job_fits_under_the_cap() {
    let db = TestDb::new("tools-get-job-big");
    let keys: String = (0..1_500).map(|i| format!(" Key{i:04} 1\n")).collect();
    let input = format!("! B3LYP def2-SVP\n%scf\n{keys}end\n{}", &WATER_INPUT[WATER_INPUT.find("* xyz").unwrap()..]);
    let reasons: Vec<Value> = (0..100).map(|i| serde_json::json!({"row": format!("P{i}"), "message": "r".repeat(5_000)})).collect();
    JobRow { input: &input, title: &"T".repeat(5_000), error: Some(&"e".repeat(50_000)), ..JobRow::new("big") }.insert(&db.conn);
    db.conn
        .execute("UPDATE jobs SET origin = 'mcp', mcp_reasons = ?1 WHERE id = 'big'", [Value::Array(reasons).to_string()])
        .unwrap();
    let view = get_job(&db.conn, "big").expect("[ctl:get_job_cap] the largest get_job is served");
    assert_fits(&view, "[ctl:get_job_cap]");
    assert!(view.header_truncated && view.error_truncated && view.title_truncated);
    assert!(view.mcp_reasons_note.as_deref().is_some_and(|n| n.contains("80 more")));
}

/// Four jobs across statuses, origins and groups.
fn four(db: &TestDb) {
    db.conn.execute_batch("INSERT INTO groups (id, name) VALUES ('g1', 'one'), ('g2', 'two');").unwrap();
    for (id, status, origin, group, at) in [
        ("a", "parsed", "user", Some("g1"), "2026-11-20 10:00:01"),
        ("b", "failed", "mcp", None, "2026-11-20 10:00:02"),
        ("c", "failed", "user", Some("g1"), "2026-11-20 10:00:03"),
        ("d", "parsed", "mcp", Some("g2"), "2026-11-20 10:00:04"),
    ] {
        JobRow { status, origin, group_id: group, created_at: at, ..JobRow::new(id) }.insert(&db.conn);
    }
}

fn ids(db: &TestDb, filter: JobFilter) -> Vec<String> {
    list_jobs(&db.conn, &filter, None, None).unwrap().jobs.into_iter().map(|j| j.id).collect()
}

/// Each filter narrows the list, and all given must match. NEGATIVE CONTROLS: ignore the status
/// filter (`ctl:list_jobs_status`), the origin filter (`ctl:list_jobs_origin`) or the group filter
/// (`ctl:list_jobs_group`) → red.
#[test]
fn list_jobs_filters_by_status_origin_and_group() {
    let db = TestDb::new("tools-list-filter");
    four(&db);
    let f = |status: Option<&str>, origin: Option<&str>, group: Option<&str>| JobFilter {
        status: status.map(str::to_string),
        origin: origin.map(str::to_string),
        group_id: group.map(str::to_string),
    };
    assert_eq!(ids(&db, f(None, None, None)), ["d", "c", "b", "a"], "newest first");
    assert_eq!(ids(&db, f(Some("failed"), None, None)), ["c", "b"], "[ctl:list_jobs_status]");
    assert_eq!(ids(&db, f(None, Some("mcp"), None)), ["d", "b"], "[ctl:list_jobs_origin]");
    assert_eq!(ids(&db, f(None, None, Some("g1"))), ["c", "a"], "[ctl:list_jobs_group]");
    assert_eq!(ids(&db, f(Some("parsed"), Some("mcp"), None)), ["d"]);
    let page = list_jobs(&db.conn, &f(Some("failed"), None, None), None, None).unwrap();
    assert_eq!(page.total_count, 2);
    assert_eq!(page.jobs[0].status, JobStatus::Failed);

    for bad in [f(Some("done"), None, None), f(None, Some("claude"), None)] {
        assert!(matches!(list_jobs(&db.conn, &bad, None, None), Err(ToolError::Refused(_))), "{bad:?}");
    }
}

/// Paging: `limit` rows from `offset`, `next_offset` until the last page; the limit is clamped.
/// NEGATIVE CONTROLS: ignore the offset (`ctl:list_jobs_offset`) or the limit
/// (`ctl:list_jobs_limit`) → red.
#[test]
fn list_jobs_pages_with_offset_and_next_offset() {
    let db = TestDb::new("tools-list-page");
    for i in 0..45 {
        let id = format!("j{i:02}");
        let at = format!("2026-11-20 10:{:02}:00", i);
        JobRow { created_at: &at, ..JobRow::new(&id) }.insert(&db.conn);
    }
    let first = list_jobs(&db.conn, &JobFilter::default(), None, Some(20)).unwrap();
    assert_eq!(first.jobs.len(), 20, "[ctl:list_jobs_limit]");
    assert_eq!((first.jobs[0].id.as_str(), first.next_offset, first.total_count), ("j44", Some(20), 45));
    let second = list_jobs(&db.conn, &JobFilter::default(), first.next_offset, Some(20)).unwrap();
    assert_eq!(second.jobs[0].id, "j24", "[ctl:list_jobs_offset] the second page starts after the first");
    let last = list_jobs(&db.conn, &JobFilter::default(), Some(40), Some(20)).unwrap();
    assert_eq!(last.jobs.iter().map(|j| j.id.as_str()).collect::<Vec<_>>(), ["j04", "j03", "j02", "j01", "j00"], "[ctl:list_jobs_offset]");
    assert_eq!(last.next_offset, None);
    let clamped = list_jobs(&db.conn, &JobFilter::default(), None, Some(1000)).unwrap();
    assert_eq!((clamped.limit, clamped.jobs.len()), (LIST_JOBS_MAX, LIST_JOBS_MAX as usize), "[ctl:list_jobs_limit]");
}

/// The largest page fits: 80 jobs with 36-character ids, group and profile ids, 5,000-character
/// titles and errors, asked for 1000 at once. NEGATIVE CONTROLS: lift the page cap
/// (`ctl:list_jobs_cap_page`), drop the title cut (`ctl:list_jobs_cap_title`) or the error cut
/// (`ctl:list_jobs_cap_error`) → red.
#[test]
fn the_largest_list_jobs_page_fits_under_the_cap() {
    let db = TestDb::new("tools-list-big");
    let (group, profile) = (id36("group"), id36("profile"));
    db.conn.execute("INSERT INTO groups (id, name) VALUES (?1, 'g')", [&group]).unwrap();
    db.conn
        .execute(
            "INSERT INTO server_profiles (id, name, host, remote_orca_path, remote_scratch_dir) VALUES (?1, 'p', 'uni', '/opt/orca/orca', '/s')",
            [&profile],
        )
        .unwrap();
    let (title, error) = ("T".repeat(5_000), "E".repeat(5_000));
    for i in 0..80 {
        let id = id36(&format!("job{i:02}"));
        JobRow {
            title: &title,
            error: Some(&error),
            status: "cancelled",
            group_id: Some(&group),
            backend_id: Some(&profile),
            energy: Some(-1234.567_890_123_456),
            ..JobRow::new(&id)
        }
        .insert(&db.conn);
    }
    let page = list_jobs(&db.conn, &JobFilter::default(), None, Some(1000)).expect("[ctl:list_jobs_cap] the largest page is served");
    assert_fits(&page, "[ctl:list_jobs_cap]");
    assert_eq!(page.jobs.len(), LIST_JOBS_MAX as usize);
    let row = &page.jobs[0];
    assert!(row.title.len() <= TITLE_MAX_BYTES && row.title_truncated, "[ctl:list_jobs_cap_title]");
    assert!(row.error_excerpt.as_ref().is_some_and(|e| e.len() <= ERROR_EXCERPT_BYTES) && row.error_truncated, "[ctl:list_jobs_cap_error]");
}

/// Rule #11 on the job tools: every numeric field names its unit, or is a count or an index.
#[test]
fn job_tool_numbers_carry_their_units() {
    let db = TestDb::new("tools-jobs-units");
    JobRow { input: WATER_INPUT, energy: Some(-76.4), ..JobRow::new("w") }.insert(&db.conn);
    db.conn.execute("UPDATE jobs SET wall_time = 12.5 WHERE id = 'w'", []).unwrap();
    let allow = ["charge", "multiplicity", "offset", "limit", "next_offset"];
    let view = serde_json::to_value(get_job(&db.conn, "w").unwrap()).unwrap();
    assert_eq!(unitless_numeric_keys(&view, &allow), Vec::<String>::new());
    assert!(view["wall_time_s"].is_number() && view["energy_eh"].is_number());
    let page = serde_json::to_value(list_jobs(&db.conn, &JobFilter::default(), None, None).unwrap()).unwrap();
    assert_eq!(unitless_numeric_keys(&page, &allow), Vec::<String>::new());
}
