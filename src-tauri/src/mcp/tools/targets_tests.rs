//! Tests for `list_run_targets` and the window (ADR-027 decision 7; M2 amendment, item 8;
//! ADR-024 g, n item 3).

use super::targets::{list_run_targets, list_run_targets_at, local_now, window_state, WindowState, PROFILES_MAX};
use super::test_support::{assert_fits, largest_profiles};
use crate::mcp::test_db::TestDb;

const fn at(h: u32, m: u32) -> u32 {
    h * 60 + m
}

/// A wrapping night window holds 23:00 and 07:59, excludes its end minute 08:00, includes its start
/// 22:00. NEGATIVE CONTROLS: drop the after-start half of the wrap (`ctl:window_wrap_2300`), the
/// before-end half (`ctl:window_wrap_0759`), or include the end minute (`ctl:window_end_wrap`) → red.
#[test]
fn a_wrapping_window_spans_midnight_and_excludes_its_end() {
    let w = Some("22:00-08:00");
    assert_eq!(window_state(w, at(23, 0)), WindowState::Inside, "[ctl:window_wrap_2300]");
    assert_eq!(window_state(w, at(7, 59)), WindowState::Inside, "[ctl:window_wrap_0759]");
    assert_eq!(window_state(w, at(8, 0)), WindowState::Outside, "[ctl:window_end_wrap] the end minute is outside");
    assert_eq!(window_state(w, at(22, 0)), WindowState::Inside, "the start minute is inside");
    assert_eq!(window_state(w, at(12, 0)), WindowState::Outside);
    assert_eq!(window_state(w, at(0, 0)), WindowState::Inside);
}

/// uni's day window. NEGATIVE CONTROL (`ctl:window_end_plain`): include the end minute → red.
#[test]
fn a_day_window_excludes_its_end_minute() {
    let w = Some("08:00-22:00");
    assert_eq!(window_state(w, at(22, 0)), WindowState::Outside, "[ctl:window_end_plain] 22:00 is outside");
    assert_eq!(window_state(w, at(21, 59)), WindowState::Inside);
    assert_eq!(window_state(w, at(8, 0)), WindowState::Inside);
    assert_eq!(window_state(w, at(7, 59)), WindowState::Outside);
    assert_eq!(window_state(None, at(3, 0)), WindowState::NoWindow);
    assert_eq!(window_state(Some("8-22"), at(3, 0)), WindowState::Unreadable);
    assert_eq!(window_state(Some("08:00-08:00"), at(3, 0)), WindowState::Unreadable);
}

fn profiles(db: &TestDb) {
    db.conn
        .execute_batch(
            "INSERT INTO server_profiles (id, name, host, remote_orca_path, remote_scratch_dir, core_mask, core_count, verified_at, availability_window, created_at) VALUES
                ('uni', 'uni', 'uni', '/opt/orca/orca', '/s', '0-23', 24, '2026-11-01 10:00:00', '08:00-22:00', '2026-11-01 09:00:00'),
                ('lab', 'lab', 'lab', '/opt/orca/orca', '/s', NULL, NULL, NULL, NULL, '2026-11-02 09:00:00');",
        )
        .unwrap();
}

/// This machine, then each profile (newest first), with validity, the window and "unknown"
/// reachability.
#[test]
fn run_targets_are_this_machine_and_each_profile() {
    let db = TestDb::new("tools-targets");
    profiles(&db);
    let t = list_run_targets_at(&db.conn, "2026-11-20 23:30:00".into(), at(23, 30)).unwrap();
    let names: Vec<&str> = t.targets.iter().map(|x| x.target.as_str()).collect();
    assert_eq!(names, ["local", "lab", "uni"]);
    assert_eq!((t.targets[0].is_run_target, t.targets[0].reachability), (true, "this_machine"));
    let lab = &t.targets[1];
    assert!(!lab.is_run_target && lab.not_run_target_reason.is_some());
    assert_eq!((lab.window_state, lab.reachability), (WindowState::NoWindow, "unknown"));
    let uni = &t.targets[2];
    assert!(uni.is_run_target && uni.not_run_target_reason.is_none(), "{uni:?}");
    assert_eq!((uni.window_state, uni.availability_window_local.as_deref()), (WindowState::Outside, Some("08:00-22:00")));
    assert_eq!(t.profile_count, 2);
    // Production reads the laptop's local time from SQLite.
    let (text, min) = local_now(&db.conn).unwrap();
    assert_eq!(text.len(), 19);
    assert!(min < 24 * 60);
    assert_eq!(list_run_targets(&db.conn).unwrap().targets.len(), 3);
}

/// The largest result fits: 150 profiles with 200-character hosts and 5,000-character names.
/// NEGATIVE CONTROL (`ctl:targets_cap`): lift the profile cap → red.
#[test]
fn the_largest_list_run_targets_fits_under_the_cap() {
    let db = TestDb::new("tools-targets-big");
    largest_profiles(&db.conn);
    let t = list_run_targets_at(&db.conn, "2026-11-20 12:00:00".into(), at(12, 0)).expect("[ctl:targets_cap] the largest result is served");
    assert_fits(&t, "[ctl:targets_cap]");
    assert_eq!((t.targets.len(), t.profile_count, t.profiles_truncated), (PROFILES_MAX + 1, 150, true), "[ctl:targets_cap]");
}
