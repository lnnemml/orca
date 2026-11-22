//! "Cancel Claude's jobs" (ADR-027 M4 amendment, item 3; unit M4a Part A): the selection, its order,
//! the summary's parity with it, the per-job results, and no audit row. The per-job body is injected
//! here; the command's own wiring (the blocking pool, the shared body) is pinned in
//! `commands::mcp_tests`.
//!
//! Each control has a tagged assertion; `m4a_parta_controls.py` (the session scratchpad) applies
//! each mutation and expects its tag red.

use std::sync::Mutex;

use super::claude_jobs::*;
use super::test_db::{grant, TestDb};
use crate::commands::jobs::CancelResult;
use crate::commands::settings::DbState;
use crate::error::AppError;
use crate::models::job::JobStatus;

/// A job row with every field the selection reads. A step-B flag needs a charged job (the v24
/// CHECK), so a flagged row is charged to a grant made here.
fn job(db: &TestDb, id: &str, status: &str, origin: &str, mcp_state: Option<&str>) {
    let remote = status == "cancelling"; // the v21 CHECK: a cancelling job is remote
    let grant_id = mcp_state.map(|_| grant(&db.conn, 50, 50).id);
    db.conn
        .execute(
            "INSERT INTO jobs (id, title, input_content, status, origin, mcp_state, grant_id, remote_host, remote_job_dir, remote_socket) \
             VALUES (?1, 't', '! SP', ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            rusqlite::params![
                id,
                status,
                origin,
                mcp_state,
                grant_id,
                remote.then_some("uni"),
                remote.then(|| format!("/r/jobs/{id}")),
                remote.then_some("/r/tsp/slot0.sock")
            ],
        )
        .unwrap();
}

/// Every (origin, status, step-B flag) a job row can have, inserted in an order that puts a running
/// job before a queued one (so an unordered selection would cancel it first).
fn every_kind_of_job(db: &TestDb) {
    job(db, "mcp-running", "running", "mcp", None);
    job(db, "user-queued", "queued", "user", None);
    job(db, "mcp-cancelling", "cancelling", "mcp", None);
    job(db, "mcp-queued", "queued", "mcp", None);
    job(db, "mcp-queued-flagged", "queued", "mcp", Some("submitting")); // between persist_submit and step C
    job(db, "user-running", "running", "user", None);
    job(db, "user-cancelling", "cancelling", "user", None);
    job(db, "user-draft", "draft", "user", None);
    job(db, "mcp-proposed", "draft", "mcp", None);
    job(db, "mcp-submitting", "draft", "mcp", Some("submitting"));
    job(db, "mcp-submitting-2", "draft", "mcp", Some("submitting"));
    for (i, terminal) in ["completed", "parsed", "failed", "cancelled", "lost"].iter().enumerate() {
        job(db, &format!("mcp-{terminal}"), terminal, "mcp", None);
        job(db, &format!("user-{terminal}-{i}"), terminal, "user", None);
    }
}

const EXPECTED_ORDER: [&str; 4] = ["mcp-queued", "mcp-queued-flagged", "mcp-running", "mcp-cancelling"];

/// Controls SEL-USER (item 8: "touching a `user` job or a draft") and SEL-ORDER ("cancelling a
/// `running` job before a `queued` one"): exactly Claude's queued, running and cancelling jobs, queued
/// first; never a `user` job, a draft or a terminal job; the step-B drafts counted.
#[test]
fn the_selection_is_claudes_live_jobs_queued_first() {
    let db = TestDb::new("claude-select");
    every_kind_of_job(&db);
    let s = select_claude_jobs_conn(&db.conn).unwrap();
    let ids: Vec<&str> = s.to_cancel.iter().map(|j| j.id.as_str()).collect();
    for id in &ids {
        assert!(id.starts_with("mcp-"), "SEL-USER: a user job selected: {ids:?}");
        assert!(!id.contains("proposed") && !id.contains("submitting"), "SEL-USER: a draft selected: {ids:?}");
    }
    let mut sorted = ids.clone();
    sorted.sort();
    let mut expected = EXPECTED_ORDER.to_vec();
    expected.sort();
    assert_eq!(sorted, expected, "SEL-USER: the selected set");
    assert_eq!(ids, EXPECTED_ORDER, "SEL-ORDER: queued, then running, then cancelling");
    assert_eq!(
        s.to_cancel.iter().map(|j| j.status).collect::<Vec<_>>(),
        [JobStatus::Queued, JobStatus::Queued, JobStatus::Running, JobStatus::Cancelling]
    );
    assert_eq!(s.submitting, 2, "SKIP-COUNT: the drafts in step B");
}

/// Control POSTCOND-ORIGIN (M4a Part A verifier, LOW-1): the runtime post-condition refuses a row whose
/// origin is not `mcp`, whatever the statement returned. `m4a_partb_controls.py` also drops
/// `origin = 'mcp'` from `CLAUDE_JOBS_SQL` and expects the selection itself to fail with this message
/// (not just the SEL-USER assertions to go red).
#[test]
fn the_post_condition_refuses_a_row_that_is_not_claudes() {
    let row = |id: &str, status: &str, submitting: bool, origin: &str| SelectedRow {
        id: id.into(),
        status: status.into(),
        submitting,
        origin: origin.into(),
    };
    let ok = selection_of_rows(vec![row("a", "queued", false, "mcp"), row("b", "draft", true, "mcp")]).unwrap();
    assert_eq!((ok.to_cancel.len(), ok.submitting), (1, 1));
    for (status, submitting) in [("queued", false), ("running", false), ("cancelling", false), ("draft", true)] {
        let Err(err) = selection_of_rows(vec![row("u", status, submitting, "user")]) else {
            panic!("POSTCOND-ORIGIN: a {status}/{submitting} row of origin user was accepted");
        };
        assert!(err.to_string().contains("selected with origin user"), "POSTCOND-ORIGIN: {status}/{submitting}: {err}");
    }
    // The real statement over a table holding both origins returns no `user` row at all.
    let db = TestDb::new("claude-origin-postcond");
    every_kind_of_job(&db);
    assert!(select_claude_jobs_conn(&db.conn).is_ok());
}

/// Control SUMMARY-PARITY (item 8: "the summary's predicate diverging from the command's"): over every
/// kind of row, the summary is the selection counted, and the cancel acts on exactly that many.
#[test]
fn the_summary_counts_what_the_cancel_selects() {
    let db = TestDb::new("claude-summary");
    assert_eq!(claude_jobs_summary_conn(&db.conn).unwrap(), ClaudeJobsSummary { to_cancel: 0, submitting: 0 });
    every_kind_of_job(&db);
    let summary = claude_jobs_summary_conn(&db.conn).unwrap();
    assert_eq!(summary, ClaudeJobsSummary { to_cancel: EXPECTED_ORDER.len() as u32, submitting: 2 }, "SUMMARY-PARITY");
    let selection = select_claude_jobs_conn(&db.conn).unwrap();
    assert_eq!(
        (summary.to_cancel as usize, summary.submitting),
        (selection.to_cancel.len(), selection.submitting),
        "SUMMARY-PARITY"
    );
    // One more of each live kind moves both numbers together.
    job(&db, "mcp-queued-2", "queued", "mcp", None);
    job(&db, "mcp-submitting-3", "draft", "mcp", Some("submitting"));
    let after = claude_jobs_summary_conn(&db.conn).unwrap();
    assert_eq!((after.to_cancel, after.submitting), (summary.to_cancel + 1, summary.submitting + 1), "SUMMARY-PARITY");
}

fn db_state(db: &TestDb) -> DbState {
    DbState(Mutex::new(db.open_another()))
}

fn audit_count(db: &TestDb) -> i64 {
    db.conn.query_row("SELECT count(*) FROM mcp_audit", [], |r| r.get(0)).unwrap()
}

/// The cancel: each selected job through `cancel_one`, in order, with the database lock released
/// (the body locks it itself); a failure is that job's error, and the loop goes on; the step-B
/// drafts are reported as skipped; no audit row. Controls SEL-ORDER, SKIP-COUNT (item 8: "not
/// reporting the skipped `submitting` count"), LOCK-FREE.
#[test]
fn each_job_goes_through_cancel_one_in_order_and_every_outcome_is_reported() {
    let db = TestDb::new("claude-cancel");
    every_kind_of_job(&db);
    let state = db_state(&db);
    let before = audit_count(&db);
    let mut seen = Vec::new();
    let report = cancel_claude_jobs(&state, |id| {
        assert!(state.0.try_lock().is_ok(), "LOCK-FREE: the database lock is held across a cancel");
        seen.push(id.to_string());
        if id == "mcp-running" {
            Err(AppError::Conflict("the job is busy".into()))
        } else {
            Ok(CancelResult::Dispatched)
        }
    })
    .unwrap();
    assert_eq!(seen, EXPECTED_ORDER, "SEL-ORDER: the cancel order");
    assert_eq!(report.jobs.iter().map(|j| j.job_id.as_str()).collect::<Vec<_>>(), EXPECTED_ORDER);
    assert_eq!(report.jobs[2].outcome, ClaudeJobOutcome::Error("conflict: the job is busy".into()));
    assert_eq!(report.jobs[3].outcome, ClaudeJobOutcome::Result(CancelResult::Dispatched), "the loop went on");
    assert_eq!(report.skipped_submitting, 2, "SKIP-COUNT");
    assert_eq!(audit_count(&db), before, "Anton's action writes no audit row");
}

/// The wire shape: `{job_id, result}` or `{job_id, error}`, never serde's `Ok`/`Err`.
#[test]
fn each_outcome_serialises_as_job_id_with_result_or_error() {
    let ok = ClaudeJobCancel { job_id: "a".into(), outcome: ClaudeJobOutcome::Result(CancelResult::Dispatched) };
    assert_eq!(serde_json::to_value(&ok).unwrap(), serde_json::json!({ "job_id": "a", "result": { "result": "dispatched" } }));
    let err = ClaudeJobCancel { job_id: "b".into(), outcome: ClaudeJobOutcome::Error("conflict: busy".into()) };
    assert_eq!(serde_json::to_value(&err).unwrap(), serde_json::json!({ "job_id": "b", "error": "conflict: busy" }));
    let report = ClaudeCancelReport { jobs: vec![ok], skipped_submitting: 3 };
    let json = serde_json::to_value(&report).unwrap();
    assert_eq!(json["skipped_submitting"], 3);
    assert!(json["jobs"].is_array());
    let summary = serde_json::to_value(ClaudeJobsSummary { to_cancel: 2, submitting: 1 }).unwrap();
    assert_eq!(summary, serde_json::json!({ "to_cancel": 2, "submitting": 1 }));
}

/// Nothing to cancel: no call, an empty report, the skipped count still reported.
#[test]
fn with_nothing_to_cancel_nothing_is_called() {
    let db = TestDb::new("claude-none");
    job(&db, "user-queued", "queued", "user", None);
    job(&db, "mcp-submitting", "draft", "mcp", Some("submitting"));
    let report = cancel_claude_jobs(&db_state(&db), |id| panic!("called for {id}")).unwrap();
    assert_eq!(report, ClaudeCancelReport { jobs: Vec::new(), skipped_submitting: 1 });
}
