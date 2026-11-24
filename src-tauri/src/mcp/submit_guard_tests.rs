//! The `submit_job` guard wired into the commands (ADR-027 decision 5, "the window"; M1b Part B).
//!
//! Every path that submits a job by hand starts in one of two commands: `submit_job` (the UI's Run,
//! Create & Run, and every bulk "run all" in the views, each one `invoke("submit_job")` per job) and
//! `retry_remote_submit` (Retry). Both need an `AppHandle`, so — as `cancel_job`'s test — the guard's
//! behaviour is tested on the core each one calls, and its place in each body is pinned in the
//! source. The backends carry no guard (pinned too): step B calls them on the very job the guard
//! refuses.

use super::steps::{
    refuse_off_target_conn, refuse_while_mcp_submitting, DELETED_PROFILE_NOTE, LEGACY_DRAFT_NOTE, SUBMITTING,
};
use super::test_db::{grant, TestDb};
use crate::commands::jobs::{get_job_conn, submittable_job_conn};
use crate::error::AppError;

/// A draft charged to a grant, in step B (`mcp_state = 'submitting'`), and a user draft.
fn world(tag: &str) -> TestDb {
    let db = TestDb::new(tag);
    let g = grant(&db.conn, 5, 5);
    db.job("mcp", "draft", Some(&g.id));
    db.conn.execute("UPDATE jobs SET mcp_state = ?1 WHERE id = 'mcp'", [SUBMITTING]).unwrap();
    db.job("user", "draft", None);
    db
}

/// `submit_job`'s read refuses a job Claude is submitting, on both arms (the read precedes the
/// dispatch), and hands a user draft through unchanged.
#[test]
fn submit_jobs_read_refuses_a_submitting_job_and_passes_a_user_draft() {
    let db = world("guard-submit");
    let refused = submittable_job_conn(&db.conn, "mcp");
    assert!(matches!(&refused, Err(AppError::Conflict(m)) if m.contains("Claude is submitting job mcp")), "{refused:?}");
    let user = submittable_job_conn(&db.conn, "user").expect("a job with mcp_state NULL submits as before");
    assert_eq!(user.mcp_state, None);
    assert_eq!(user.id, get_job_conn(&db.conn, "user").unwrap().id);
    assert!(matches!(submittable_job_conn(&db.conn, "nope"), Err(AppError::NotFound(_))), "a missing job is still NotFound");
}

/// Once step C clears the flag, the same job submits by hand like any draft.
#[test]
fn the_job_submits_by_hand_once_step_c_cleared_the_flag() {
    let db = world("guard-cleared");
    db.conn.execute("UPDATE jobs SET mcp_state = NULL WHERE id = 'mcp'", []).unwrap();
    assert!(submittable_job_conn(&db.conn, "mcp").is_ok());
}

/// The retry's guard, as the retry calls it on the row it read under the job's in-flight guard.
#[test]
fn the_retrys_guard_refuses_a_submitting_job_and_passes_a_user_job() {
    let db = world("guard-retry");
    let mcp = get_job_conn(&db.conn, "mcp").unwrap();
    let refused = refuse_while_mcp_submitting(&mcp);
    assert!(matches!(&refused, Err(AppError::Conflict(m)) if m.contains("Claude is submitting")), "{refused:?}");
    assert!(refuse_while_mcp_submitting(&get_job_conn(&db.conn, "user").unwrap()).is_ok());
}

/// The target world (ADR-027 M4c amendment, items 3 and 4): the profile `p-uni` (named "uni"), and
/// PROPOSED MCP drafts stored for it, for `local`, for a profile that is gone, and from before v25
/// (NULL); a user draft; and a queued MCP job.
fn target_world(tag: &str) -> TestDb {
    let db = TestDb::new(tag);
    db.conn
        .execute_batch(
            "INSERT INTO server_profiles (id, name, host, remote_orca_path, remote_scratch_dir) \
                 VALUES ('p-uni', 'uni', 'uni', '/opt/orca/orca', '/s');
             INSERT INTO jobs (id, title, input_content, status, origin, mcp_target) VALUES
                 ('uni', 't', '! SP', 'draft', 'mcp', 'p-uni'),
                 ('local', 't', '! SP', 'draft', 'mcp', 'local'),
                 ('gone', 't', '! SP', 'draft', 'mcp', 'p-gone'),
                 ('legacy', 't', '! SP', 'draft', 'mcp', NULL),
                 ('queued', 't', '! SP', 'queued', 'mcp', 'p-uni');
             INSERT INTO jobs (id, title, input_content, status) VALUES ('user', 't', '! SP', 'draft');",
        )
        .unwrap();
    db
}

/// `submit_job`'s target guard, as `submit_job` calls it: on the job its guarded read returned, with
/// its own `target` argument.
fn submit_check(db: &TestDb, id: &str, target: Option<&str>) -> Result<(), AppError> {
    let job = submittable_job_conn(&db.conn, id)?;
    refuse_off_target_conn(&db.conn, &job, target)
}

fn conflict(r: Result<(), AppError>) -> String {
    match r {
        Err(AppError::Conflict(m)) => m,
        other => panic!("expected a Conflict refusal, got {other:?}"),
    }
}

/// An MCP draft runs only where Claude assembled it (ADR-027 M4c amendment, item 3): the uni draft on
/// `p-uni`, the local draft on `None` (this machine). Every other request is refused with the stored
/// target's name — `None` for the uni draft (the old row's "Run locally"), a profile for the local
/// draft, and `Some("local")`, which `Backend::for_submit` would read as a profile named "local".
/// NEGATIVE CONTROL (`ctl:submit_mismatch`): make the comparison never refuse → red.
#[test]
fn an_mcp_draft_is_refused_on_any_target_but_its_own() {
    let db = target_world("target-mismatch");
    submit_check(&db, "uni", Some("p-uni")).expect("the uni draft on uni");
    submit_check(&db, "local", None).expect("the local draft on this machine");
    for (id, target, named) in [
        ("uni", None, "the server profile \"uni\""),
        ("uni", Some("local"), "the server profile \"uni\""),
        ("local", Some("p-uni"), "this machine"),
        ("local", Some("local"), "this machine"),
    ] {
        let r = submit_check(&db, id, target);
        assert!(r.is_err(), "[ctl:submit_mismatch] {id} on {target:?} was accepted");
        let m = conflict(r);
        assert_eq!(m, format!("Claude assembled this job for {named}; run it there, or delete it"), "{id} on {target:?}");
    }
}

/// A legacy MCP draft (no stored target, ADR-027 M4c amendment, item 4) is refused on every target,
/// and a draft whose stored profile was deleted is refused even on that very id (item 3), each with
/// the note's words. NEGATIVE CONTROLS: `ctl:submit_legacy` — let a NULL target through → red;
/// `ctl:submit_deleted` — treat a missing profile as present (fall through to the comparison) → the
/// request for `p-gone` is accepted → red.
#[test]
fn a_legacy_draft_and_a_deleted_profiles_draft_are_refused() {
    let db = target_world("target-legacy");
    for target in [None, Some("p-uni"), Some("local")] {
        let r = submit_check(&db, "legacy", target);
        assert!(r.is_err(), "[ctl:submit_legacy] the legacy draft on {target:?} was accepted");
        assert_eq!(conflict(r), format!("Claude's draft legacy: {LEGACY_DRAFT_NOTE}"));
    }
    for target in [Some("p-gone"), None, Some("p-uni")] {
        let r = submit_check(&db, "gone", target);
        assert!(r.is_err(), "[ctl:submit_deleted] the deleted profile's draft on {target:?} was accepted");
        assert_eq!(conflict(r), format!("Claude's draft gone: {DELETED_PROFILE_NOTE}"));
    }
    assert_eq!(
        LEGACY_DRAFT_NOTE, "assembled before targets were stored — delete it and ask Claude again",
        "the amendment's words (item 4), shared with the Jobs screen's note"
    );
    assert_eq!(DELETED_PROFILE_NOTE, "its target profile was deleted — delete it and ask Claude again");
}

/// A user draft is unchanged (ADR-027 M4c amendment, item 4): every target passes the guard, as
/// before v25. A non-draft MCP job is not this guard's (the backends refuse a non-draft on their own).
/// NEGATIVE CONTROL (`ctl:submit_user`): drop the `origin` test, so the user draft's NULL target reads
/// as a legacy MCP draft → red.
#[test]
fn a_user_draft_submits_on_any_target_as_before() {
    let db = target_world("target-user");
    for target in [None, Some("p-uni"), Some("p-gone")] {
        let r = submit_check(&db, "user", target);
        assert!(r.is_ok(), "[ctl:submit_user] the user draft on {target:?}: {r:?}");
    }
    assert!(submit_check(&db, "queued", None).is_ok(), "a queued MCP job is the backends' to refuse");
}

/// `submit_job` checks the target **under the lock of its guarded read**, after that read and before
/// either arm dispatches, on its own `target` argument (ADR-027 M4c amendment, item 3). NEGATIVE
/// CONTROL (`ctl:submit_pin`): move the check after the lock's block, or drop it → red.
#[test]
fn submit_job_checks_the_target_under_the_reads_lock_before_either_arm() {
    let src = include_str!("../commands/jobs.rs");
    let body = fn_body(src, concat!("pub async fn ", "submit_job("));
    let lock = body.find("db.lock()?").expect("submit_job locks the database");
    let read = body.find(concat!("submittable_job_", "conn(&conn, &id)?")).expect("the guarded read");
    let check = body
        .find(concat!("refuse_off_target_", "conn(&conn, &job, target.as_deref())?"))
        .expect("[ctl:submit_pin] submit_job checks the target");
    let block_end = lock + body[lock..].find("};").expect("the lock's block ends");
    let dispatch = body.find(concat!("Backend::for_", "submit(")).expect("submit_job dispatches");
    assert!(lock < read && read < check && check < block_end && block_end < dispatch, "[ctl:submit_pin] {body}");
}

/// The body of `fn <signature>` in `src`, up to its closing brace at column 0.
fn fn_body<'a>(src: &'a str, signature: &str) -> &'a str {
    let start = src.find(signature).unwrap_or_else(|| panic!("{signature} exists"));
    &src[start..start + src[start..].find("\n}\n").expect("the fn ends")]
}

/// `submit_job` reads the job **once**, through the guarded read, **before** it dispatches to
/// either backend. NEGATIVE CONTROL: the read back to `get_job_conn` → red; the read moved into
/// the local arm (the remote arm unguarded) → red.
#[test]
fn submit_job_reads_through_the_guard_before_either_arm() {
    let src = include_str!("../commands/jobs.rs");
    let body = fn_body(src, concat!("pub async fn ", "submit_job("));
    let read = body.find(concat!("submittable_job_", "conn(&conn, &id)?")).expect("submit_job reads through the guard");
    let dispatch = body.find(concat!("Backend::for_", "submit(")).expect("submit_job dispatches");
    assert!(read < dispatch, "the guarded read precedes the dispatch, so both arms are covered");
    assert!(!body.contains(concat!("get_job_", "conn(")), "no unguarded read of the job in submit_job");

    let core = fn_body(src, concat!("pub(crate) fn ", "submittable_job_conn("));
    let get = core.find(concat!("get_job_", "conn(conn, id)?")).expect("the core reads the job");
    let guard = core.find(concat!("refuse_while_mcp_", "submitting(&job)?")).expect("and refuses a submitting one");
    assert!(get < guard, "{core}");
}

/// `retry_remote_submit` refuses before it builds the backend, inside the guarded work. NEGATIVE
/// CONTROL: the guard removed from the retry → red.
#[test]
fn retry_remote_submit_refuses_before_the_backend() {
    let src = include_str!("../commands/remote_jobs.rs");
    let body = fn_body(src, concat!("pub async fn ", "retry_remote_", "submit("));
    let guard = body.find(concat!("refuse_while_mcp_", "submitting(job).and_then(")).expect("the retry is guarded");
    let backend = body.find(concat!("remote_", "backend(app, job)")).expect("the retry builds the backend");
    let retry = body.find(concat!("ssh.", "retry(&job.id, runner)")).expect("the retry runs");
    assert!(guard < backend && backend < retry, "{body}");
}

/// The backends carry no `mcp_state` guard: M3's step B submits the job through them while the flag
/// is set, so a guard there would refuse Claude's own submit (decision 5: "a guard in the command,
/// not in the backends").
#[test]
fn the_backends_carry_no_mcp_guard() {
    for (name, src) in [
        ("local_backend.rs", include_str!("../local_backend.rs")),
        ("ssh_backend.rs", include_str!("../ssh_backend.rs")),
        ("execution_backend.rs", include_str!("../execution_backend.rs")),
    ] {
        let production = src.find("#[cfg(test)]").map_or(src, |i| &src[..i]);
        for needle in [concat!("mcp_", "state"), concat!("refuse_while_mcp_", "submitting"), concat!("submit_refusal_for_", "mcp_state")] {
            assert!(!production.contains(needle), "{name} carries `{needle}`");
        }
    }
}
