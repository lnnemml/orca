//! Tests for the MCP grant commands' cores (ADR-027 decision 5, M1b Part B). The cores read `now`
//! from SQLite's real clock, so every grant here expires in 2999; an expired grant is made through
//! the grant core with a fake `now`.

use super::{create_grant_core, current_grant_core, level_from_job_conn, revoke_grant_core, GrantState};
use crate::commands::jobs::create_job_conn;
use crate::error::AppError;
use crate::mcp::grant::{create_grant_conn, get_grant_conn};
use crate::mcp::signature::MethodSignature;
use crate::mcp::test_db::{new_grant, TestDb, LEVEL};

const FAR: &str = "2999-12-31 00:00:00";

fn create(db: &TestDb, levels: &[&str]) -> Result<super::GrantView, AppError> {
    create_grant_core(&db.conn, vec!["local".into()], levels.iter().map(|l| l.to_string()).collect(), 5, 2, FAR.into())
}

#[test]
fn create_returns_the_active_grant_with_its_counters() {
    let db = TestDb::new("cmd-create");
    let v = create(&db, &[LEVEL]).unwrap();
    assert_eq!(v.state, GrantState::Active);
    assert_eq!(v.targets, vec!["local".to_string()]);
    assert_eq!(v.levels, vec!["CPCM(DMF) WB97X-3C".to_string()]);
    assert_eq!((v.max_jobs, v.max_in_flight, v.charged_jobs, v.in_flight), (5, 2, 0, 0));
    assert_eq!(v.expires_at, FAR);
    assert_eq!(v.revoked_at, None);
    assert_eq!(current_grant_core(&db.conn).unwrap(), Some(v), "get reads back what create made");
}

#[test]
fn the_view_serializes_lowercase_state_and_snake_case_fields() {
    let db = TestDb::new("cmd-wire");
    let v = create(&db, &[LEVEL]).unwrap();
    let json = serde_json::to_value(&v).unwrap();
    assert_eq!(json["state"], "active");
    for key in ["id", "created_at", "expires_at", "revoked_at", "targets", "levels", "max_jobs", "max_in_flight", "charged_jobs", "in_flight"] {
        assert!(json.get(key).is_some(), "the view has `{key}`: {json}");
    }
}

/// A level outside the grammar is the grant core's refusal, mapped to a clear `Invalid` that names
/// the level and the reason; nothing is written.
#[test]
fn an_invalid_level_is_refused_as_invalid_and_nothing_is_written() {
    let db = TestDb::new("cmd-bad-level");
    let e = create(&db, &[LEVEL, "! wB97X-3c\n* xyz 0 1\n"]).unwrap_err();
    assert!(matches!(&e, AppError::Invalid(m) if m.starts_with("level 2:") && m.contains("coordinates")), "{e:?}");
    let e = create(&db, &["! Opt Freq"]).unwrap_err();
    assert!(matches!(&e, AppError::Invalid(m) if m.contains("level 1") && m.contains("names no method keyword")), "{e:?}");
    assert_eq!(current_grant_core(&db.conn).unwrap(), None, "a refused create writes nothing");
}

#[test]
fn other_refusals_map_to_clear_errors() {
    let db = TestDb::new("cmd-refusals");
    let past = create_grant_core(&db.conn, vec!["local".into()], vec![LEVEL.into()], 5, 2, "2000-01-01 00:00:00".into());
    assert!(matches!(&past, Err(AppError::Invalid(m)) if m.contains("not after now")), "{past:?}");
    let zero = create_grant_core(&db.conn, vec!["local".into()], vec![LEVEL.into()], 0, 2, FAR.into());
    assert!(matches!(&zero, Err(AppError::Invalid(m)) if m.contains("max_jobs")), "{zero:?}");
    let form = create_grant_core(&db.conn, vec!["local".into()], vec![LEVEL.into()], 5, 2, "2999-12-31T00:00:00Z".into());
    assert!(matches!(&form, Err(AppError::Invalid(m)) if m.contains("YYYY-MM-DD HH:MM:SS")), "{form:?}");
    let server = create_grant_core(&db.conn, vec!["no-such-profile".into()], vec![LEVEL.into()], 5, 2, FAR.into());
    assert!(matches!(&server, Err(AppError::NotFound(m)) if m.contains("no-such-profile")), "{server:?}");
    assert_eq!(current_grant_core(&db.conn).unwrap(), None);
}

#[test]
fn a_second_create_revokes_the_first() {
    let db = TestDb::new("cmd-second");
    let first = create(&db, &[LEVEL]).unwrap();
    let second = create(&db, &["! r2SCAN-3c"]).unwrap();
    assert_eq!(current_grant_core(&db.conn).unwrap().map(|v| v.id), Some(second.id));
    assert!(get_grant_conn(&db.conn, &first.id).unwrap().revoked_at.is_some(), "the first grant is revoked");
}

#[test]
fn revoke_is_shown_and_idempotent() {
    let db = TestDb::new("cmd-revoke");
    let g = create(&db, &[LEVEL]).unwrap();
    let r1 = revoke_grant_core(&db.conn, &g.id).unwrap();
    assert_eq!(r1.state, GrantState::Revoked);
    let at = r1.revoked_at.clone().expect("revoked_at set");
    let r2 = revoke_grant_core(&db.conn, &g.id).unwrap();
    assert_eq!(r2.revoked_at, Some(at), "the first revoked_at is kept");
    assert_eq!(current_grant_core(&db.conn).unwrap().map(|v| v.state), Some(GrantState::Revoked));
    assert!(matches!(revoke_grant_core(&db.conn, "nope"), Err(AppError::NotFound(_))));
}

#[test]
fn get_is_none_before_any_grant_and_shows_an_expired_one() {
    let db = TestDb::new("cmd-get");
    assert_eq!(current_grant_core(&db.conn).unwrap(), None);
    let mut g = new_grant(3, 1);
    g.expires_at = "2000-01-02 00:00:00".into();
    create_grant_conn(&db.conn, &g, "2000-01-01 00:00:00").unwrap();
    assert_eq!(current_grant_core(&db.conn).unwrap().map(|v| v.state), Some(GrantState::Expired));
}

#[test]
fn the_view_counts_every_charged_job_in_flight() {
    let db = TestDb::new("cmd-in-flight");
    let g = create(&db, &[LEVEL]).unwrap();
    db.job("a", "queued", Some(&g.id));
    db.job("b", "completed", Some(&g.id));
    db.job("c", "running", None);
    assert_eq!(current_grant_core(&db.conn).unwrap().map(|v| v.in_flight), Some(1));
}

// --- "this job's level" -------------------------------------------------------------------------

fn job(db: &TestDb, input: &str) -> String {
    create_job_conn(&db.conn, "t", input, None, None).unwrap().id
}

#[test]
fn a_jobs_level_is_its_header_signature_and_reads_back() {
    let db = TestDb::new("cmd-level");
    let id = job(&db, "! wB97X-3c def2-TZVP CPCM(DMF) Opt Freq TightSCF\n%maxcore 2000\n* xyz 0 1\nH 0 0 0\nH 0 0 0.74\n*\n");
    let text = level_from_job_conn(&db.conn, &id).unwrap();
    assert_eq!(text, "! CPCM(DMF) DEF2-TZVP WB97X-3C", "run-type, SCF and %maxcore are not the level");
    // The grant made from it holds the job's own signature, so the job's level is granted.
    let v = create(&db, &[&text]).unwrap();
    assert_eq!(v.levels, vec!["CPCM(DMF) DEF2-TZVP WB97X-3C".to_string()]);
}

#[test]
fn a_jobs_level_keeps_its_cpcm_block() {
    let db = TestDb::new("cmd-level-cpcm");
    let header = "! r2SCAN-3c Opt\n%cpcm\n  smd true\n  SMDsolvent \"DMF\"\nend\n";
    let id = job(&db, &format!("{header}* xyz 0 1\nH 0 0 0\nH 0 0 0.74\n*\n"));
    let text = level_from_job_conn(&db.conn, &id).unwrap();
    assert_eq!(text, "! R2SCAN-3C\n%cpcm smd true smdsolvent \"dmf\" end");
    assert_eq!(MethodSignature::of_level_text(&text).unwrap(), MethodSignature::of_level_text(header).unwrap());
}

#[test]
fn a_job_with_no_readable_level_is_refused() {
    let db = TestDb::new("cmd-level-bad");
    let no_method = job(&db, "! Opt Freq\n* xyz 0 1\nH 0 0 0\nH 0 0 0.74\n*\n");
    let e = level_from_job_conn(&db.conn, &no_method).unwrap_err();
    assert!(matches!(&e, AppError::Invalid(m) if m.contains("names no method keyword")), "{e:?}");
    let xyzfile = job(&db, "! r2SCAN-3c\n* xyzfile 0 1 mol.xyz\n");
    let e = level_from_job_conn(&db.conn, &xyzfile).unwrap_err();
    assert!(matches!(&e, AppError::Invalid(m) if m.contains("coordinate block")), "{e:?}");
    assert!(matches!(level_from_job_conn(&db.conn, "nope"), Err(AppError::NotFound(_))));
}

// ----- the switch's commands (ADR-027 M2a Part B) -----

mod switch {
    use std::sync::{Arc, Mutex};
    use std::thread::{self, ThreadId};

    use super::super::{mcp_set_enabled_core, mcp_status_core};
    use crate::commands::settings::DbState;
    use crate::mcp::server::test_support::{no_write_seams, Scratch};
    use crate::mcp::server::wiring::wired_lifecycle;
    use crate::mcp::server::DbSource;

    /// The managed database, recording which threads reach it.
    struct RecordingDb {
        db: Arc<DbState>,
        threads: Mutex<Vec<ThreadId>>,
    }

    impl DbSource for RecordingDb {
        fn db(&self) -> &DbState {
            self.threads.lock().unwrap().push(thread::current().id());
            &self.db
        }
    }

    /// Control (Part B): the commands' work running on the caller's thread instead of
    /// `spawn_blocking`. The cores are driven with `block_on` from this thread, as an `async`
    /// command's future would be; every database touch (`status` reads the row, `set_enabled`
    /// writes it, reconciles and serves) must happen elsewhere. The commands themselves are thin
    /// `async` shells over these cores (pinned below).
    #[test]
    fn the_switch_commands_do_their_work_off_the_callers_thread() {
        let scratch = Scratch::new("cmd-switch");
        let recording = Arc::new(RecordingDb { db: scratch.db(), threads: Mutex::new(Vec::new()) });
        let lifecycle = wired_lifecycle(recording.clone(), Ok(scratch.paths()), scratch.host(), no_write_seams());
        let caller = thread::current().id();

        let status = tauri::async_runtime::block_on(mcp_status_core(lifecycle.clone())).unwrap();
        assert_eq!((status.enabled, status.serving), (Some(false), false));
        let on = tauri::async_runtime::block_on(mcp_set_enabled_core(lifecycle.clone(), true)).unwrap();
        assert!(on.serving && on.enabled == Some(true), "{on:?}");
        let off = tauri::async_runtime::block_on(mcp_set_enabled_core(lifecycle, false)).unwrap();
        assert!(!off.serving && off.enabled == Some(false), "{off:?}");

        let threads = recording.threads.lock().unwrap();
        assert!(!threads.is_empty());
        assert!(!threads.contains(&caller), "a switch command did its work on the caller's thread");
    }

    /// The two commands are `async` and call the cores above (no `AppHandle` harness exists, so
    /// this is a source pin, as the M1b guard pins).
    #[test]
    fn the_switch_commands_are_async_shells_over_the_cores() {
        let src = include_str!("mcp.rs");
        assert!(src.contains(
            "pub async fn mcp_status(app: AppHandle) -> Result<McpStatus, AppError> {\n    mcp_status_core(managed_lifecycle(&app)?).await\n}"
        ));
        assert!(src.contains(
            "pub async fn mcp_set_enabled(app: AppHandle, on: bool) -> Result<McpStatus, AppError> {\n    mcp_set_enabled_core(managed_lifecycle(&app)?, on).await\n}"
        ));
    }
}
