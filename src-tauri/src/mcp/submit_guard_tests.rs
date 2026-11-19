//! The `submit_job` guard wired into the commands (ADR-027 decision 5, "the window"; M1b Part B).
//!
//! Every path that submits a job by hand starts in one of two commands: `submit_job` (the UI's Run,
//! Create & Run, and every bulk "run all" in the views, each one `invoke("submit_job")` per job) and
//! `retry_remote_submit` (Retry). Both need an `AppHandle`, so — as `cancel_job`'s test — the guard's
//! behaviour is tested on the core each one calls, and its place in each body is pinned in the
//! source. The backends carry no guard (pinned too): step B calls them on the very job the guard
//! refuses.

use super::steps::{refuse_while_mcp_submitting, SUBMITTING};
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
