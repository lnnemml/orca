//! Tests for step A, step C, the reconciliation and the `submit_job` guard (ADR-027 decision 5;
//! M1b controls).

use std::cell::RefCell;
use std::sync::{Arc, Barrier};

use super::classify::{GrantProblem, ProposedReason, Target};
use super::grant::{create_grant_conn, grant_snapshot_conn, in_flight_conn, now_utc_conn, revoke_grant_conn};
use super::signature::MethodSignature;
use super::steps::{
    charge_and_insert_conn, charge_and_insert_with, finish_submit_conn, insert_proposed_conn, reconcile_submitting_conn,
    submit_refusal_for_mcp_state, McpJobDraft, McpReason, Reconciled, StepA, StepC, SUBMITTING,
};
use super::test_db::{grant, new_grant, TestDb, EXPIRES, LEVEL, NOW};
use crate::commands::jobs::get_job_conn;
use crate::error::AppError;
use crate::models::job::{JobOrigin, JobStatus};

const INPUT: &str = "! wB97X-3c CPCM(DMF) Opt Freq\n%maxcore 2000\n# charge/mult from job:x\n* xyz 0 1\nH 0 0 0\nH 0 0 0.74\n*\n";

fn sig() -> MethodSignature {
    MethodSignature::of_input(LEVEL)
}

/// The tests' clock: the fixed [`NOW`], so no test depends on the machine's date.
fn fixed_clock(_: &rusqlite::Connection) -> Result<String, AppError> {
    Ok(NOW.to_string())
}

/// Step A on `draft` at the fixed clock.
fn step_a_draft(conn: &rusqlite::Connection, draft: McpJobDraft<'_>) -> Result<StepA, AppError> {
    charge_and_insert_with(conn, draft, || Ok(()), fixed_clock)
}

/// Step A for the study level on Local.
fn step_a(conn: &rusqlite::Connection, sig: &MethodSignature) -> StepA {
    step_a_draft(conn, McpJobDraft { title: "t", input_content: INPUT, target: &Target::Local, signature: sig }).unwrap()
}

fn charged(a: StepA) -> (String, String) {
    match a {
        StepA::Charged { job, grant_id } => (job.id, grant_id),
        other => panic!("expected Charged, got {other:?}"),
    }
}

fn proposed(a: StepA) -> (String, Vec<ProposedReason>) {
    match a {
        StepA::Proposed { job, reasons } => (job.id, reasons),
        other => panic!("expected Proposed, got {other:?}"),
    }
}

fn reasons_of(db: &TestDb, id: &str) -> Vec<McpReason> {
    serde_json::from_str(get_job_conn(&db.conn, id).unwrap().mcp_reasons.as_deref().unwrap()).unwrap()
}

#[test]
fn step_a_charges_a_covered_job_as_a_submitting_draft() {
    let db = TestDb::new("step-a");
    let g = grant(&db.conn, 3, 2);
    let (id, gid) = charged(step_a(&db.conn, &sig()));
    assert_eq!(gid, g.id);
    let job = get_job_conn(&db.conn, &id).unwrap();
    assert_eq!(job.status, JobStatus::Draft);
    assert_eq!(job.origin, JobOrigin::Mcp);
    assert_eq!(job.grant_id.as_deref(), Some(g.id.as_str()));
    assert_eq!(job.mcp_state.as_deref(), Some(SUBMITTING));
    assert_eq!(job.mcp_reasons, None);
    assert_eq!(job.input_content, INPUT);
    assert_eq!(db.charged(&g.id), 1);
    assert_eq!(in_flight_conn(&db.conn).unwrap(), 1, "the step-B draft is in flight");
}

#[test]
fn step_a_proposes_what_the_grant_no_longer_covers() {
    let db = TestDb::new("step-a-proposed");
    // No grant at all.
    let (id, reasons) = proposed(step_a(&db.conn, &sig()));
    assert_eq!(reasons, vec![ProposedReason::Grant(GrantProblem::NoGrant)]);
    let job = get_job_conn(&db.conn, &id).unwrap();
    assert_eq!((job.origin, job.grant_id, job.mcp_state), (JobOrigin::Mcp, None, None));
    assert_eq!(reasons_of(&db, &id), vec![McpReason { row: "P5".into(), message: "there is no grant".into() }]);
    // Another level.
    let g = grant(&db.conn, 1, 1);
    let other = MethodSignature::of_input("! r2SCAN-3c\n");
    let (_, reasons) = proposed(step_a(&db.conn, &other));
    assert!(matches!(reasons.as_slice(), [ProposedReason::Grant(GrantProblem::LevelNotGranted { .. })]), "{reasons:?}");
    // Used up.
    let (done, _) = charged(step_a(&db.conn, &sig()));
    db.conn.execute("UPDATE jobs SET status = 'completed', mcp_state = NULL WHERE id = ?1", [&done]).unwrap();
    let (_, reasons) = proposed(step_a(&db.conn, &sig()));
    assert_eq!(reasons, vec![ProposedReason::Grant(GrantProblem::MaxJobsUsed { charged: 1, max: 1 })]);
    // Revoked.
    revoke_grant_conn(&db.conn, &g.id, NOW).unwrap();
    let (_, reasons) = proposed(step_a(&db.conn, &sig()));
    assert!(reasons.contains(&ProposedReason::Grant(GrantProblem::Revoked)), "{reasons:?}");
    assert_eq!(db.charged(&g.id), 1, "a proposed job is never charged");
}

/// The stored counter (decided by Anton): deleting a charged job never re-opens the budget.
/// NEGATIVE CONTROL: derive `charged_jobs` in `grant_snapshot_conn` from a `COUNT(*)` of the grant's
/// jobs → the deleted job frees its charge, the third step A is charged, and this is red.
#[test]
fn deleting_a_charged_job_does_not_reopen_the_budget() {
    let db = TestDb::new("step-a-delete");
    let g = grant(&db.conn, 1, 5);
    let (id, _) = charged(step_a(&db.conn, &sig()));
    db.conn.execute("DELETE FROM jobs WHERE id = ?1", [&id]).unwrap();
    assert_eq!(db.charged(&g.id), 1);
    assert_eq!(grant_snapshot_conn(&db.conn, NOW).unwrap().unwrap().snapshot.charged_jobs, 1);
    let (_, reasons) = proposed(step_a(&db.conn, &sig()));
    assert_eq!(reasons, vec![ProposedReason::Grant(GrantProblem::MaxJobsUsed { charged: 1, max: 1 })]);
}

/// A new grant does not get around jobs still running under an old one (`in_flight` spans grants).
#[test]
fn an_old_grants_running_job_holds_the_new_grants_cap() {
    let db = TestDb::new("step-a-old-grant");
    let g1 = grant(&db.conn, 5, 1);
    let (id, _) = charged(step_a(&db.conn, &sig()));
    db.conn.execute("UPDATE jobs SET status = 'running', mcp_state = NULL WHERE id = ?1", [&id]).unwrap();
    let g2 = create_grant_conn(&db.conn, &new_grant(5, 1), NOW).unwrap();
    assert_ne!(g1.id, g2.id);
    let (_, reasons) = proposed(step_a(&db.conn, &sig()));
    assert_eq!(reasons, vec![ProposedReason::Grant(GrantProblem::MaxInFlightReached { in_flight: 1, max: 1 })]);
}

/// Two step As with `max_in_flight = 1`, interleaved deterministically: both callers classified
/// against the same empty snapshot; the rival commits its step A just before this one's `BEGIN
/// IMMEDIATE`. Because the grant is re-read inside the transaction, this one is PROPOSED. NEGATIVE
/// CONTROL: read the snapshot before the seam (outside `BEGIN IMMEDIATE`) → this one is charged too
/// (or trips the post-condition) and this is red.
#[test]
fn a_rival_step_a_committed_first_makes_this_one_proposed() {
    let db = TestDb::new("step-a-rival");
    let g = grant(&db.conn, 5, 1);
    let rival = db.open_another();
    let s = sig();
    let draft = McpJobDraft { title: "t", input_content: INPUT, target: &Target::Local, signature: &s };
    let outcome = charge_and_insert_with(
        &db.conn,
        draft,
        || {
            charged(step_a_draft(&rival, draft)?);
            Ok(())
        },
        fixed_clock,
    )
    .expect("step A");
    let (_, reasons) = proposed(outcome);
    assert_eq!(reasons, vec![ProposedReason::Grant(GrantProblem::MaxInFlightReached { in_flight: 1, max: 1 })]);
    assert_eq!(db.charged(&g.id), 1);
    assert_eq!(in_flight_conn(&db.conn).unwrap(), 1);
}

/// The same race on real threads, two connections on one file, through the production
/// `charge_and_insert_conn` (the real clock, read inside the transaction): exactly one is charged.
#[test]
fn two_concurrent_step_as_charge_exactly_one() {
    let db = TestDb::new("step-a-threads");
    let mut n = new_grant(5, 1);
    n.expires_at = "2099-12-31 00:00:00".into();
    let g = create_grant_conn(&db.conn, &n, &now_utc_conn(&db.conn).unwrap()).unwrap();
    let barrier = Arc::new(Barrier::new(2));
    let handles: Vec<_> = (0..2)
        .map(|_| {
            let conn = db.open_another();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                let s = sig();
                barrier.wait();
                charge_and_insert_conn(&conn, McpJobDraft { title: "t", input_content: INPUT, target: &Target::Local, signature: &s })
                    .map(|a| matches!(a, StepA::Charged { .. }))
            })
        })
        .collect();
    let outcomes: Vec<bool> = handles.into_iter().map(|h| h.join().unwrap().expect("step A")).collect();
    assert_eq!(outcomes.iter().filter(|c| **c).count(), 1, "{outcomes:?}");
    assert_eq!(db.charged(&g.id), 1);
}

/// Step A reads the time inside its transaction (LOW-4): the caller classified at [`NOW`], the grant
/// expired while step A waited on the lock (the fake clock advances in the seam before `BEGIN`), so
/// the job is PROPOSED as expired, not charged. NEGATIVE CONTROL: read the clock before the seam
/// (before `BEGIN IMMEDIATE`) → the job is charged and this is red.
#[test]
fn a_grant_that_expires_during_the_lock_wait_is_not_charged() {
    let db = TestDb::new("step-a-expiry");
    let g = grant(&db.conn, 5, 5);
    let clock = RefCell::new(NOW.to_string());
    let s = sig();
    let draft = McpJobDraft { title: "t", input_content: INPUT, target: &Target::Local, signature: &s };
    let outcome = charge_and_insert_with(
        &db.conn,
        draft,
        || {
            *clock.borrow_mut() = EXPIRES.to_string(); // the lock was granted at the expiry
            Ok(())
        },
        |_| Ok(clock.borrow().clone()),
    )
    .unwrap();
    let (_, reasons) = proposed(outcome);
    assert_eq!(reasons, vec![ProposedReason::Grant(GrantProblem::Expired)]);
    assert_eq!(db.charged(&g.id), 0);
}

#[test]
fn insert_proposed_writes_an_uncharged_mcp_draft_with_its_reasons() {
    let db = TestDb::new("proposed");
    let reasons = vec![ProposedReason::FreqWithoutConvergedOpt];
    let job = insert_proposed_conn(&db.conn, "t", INPUT, &reasons).unwrap();
    assert_eq!((job.status, job.origin, job.grant_id, job.mcp_state), (JobStatus::Draft, JobOrigin::Mcp, None, None));
    assert_eq!(reasons_of(&db, &job.id), vec![McpReason::proposed(&reasons[0])]);
    assert!(insert_proposed_conn(&db.conn, "t", INPUT, &[]).is_err(), "a proposal without a reason");
}

/// Step C on a row step B never handed over: refund. NEGATIVE CONTROL: make step C never refund →
/// red.
#[test]
fn step_c_refunds_a_job_still_in_draft() {
    let db = TestDb::new("step-c-refund");
    let g = grant(&db.conn, 3, 2);
    let (id, gid) = charged(step_a(&db.conn, &sig()));
    assert_eq!(finish_submit_conn(&db.conn, &id, &gid, "slot check failed").unwrap(), StepC::Refunded);
    let job = get_job_conn(&db.conn, &id).unwrap();
    assert_eq!((job.status, job.grant_id, job.mcp_state), (JobStatus::Draft, None, None));
    assert_eq!(job.origin, JobOrigin::Mcp, "a refunded job stays Claude's");
    assert_eq!(reasons_of(&db, &id), vec![McpReason::submit_refused("slot check failed")]);
    assert_eq!(db.charged(&g.id), 0);
    assert_eq!(in_flight_conn(&db.conn).unwrap(), 0);
}

/// Step C on a row that left `draft`: the charge stays, the flag goes. NEGATIVE CONTROL: drop the
/// status check (refund whenever `grant_id` matches) → red.
#[test]
fn step_c_keeps_the_charge_of_a_job_that_left_draft() {
    let db = TestDb::new("step-c-kept");
    let g = grant(&db.conn, 3, 3);
    for status in ["queued", "running", "failed"] {
        let (id, gid) = charged(step_a(&db.conn, &sig()));
        db.conn.execute(&format!("UPDATE jobs SET status = '{status}' WHERE id = ?1"), [&id]).unwrap();
        let st = JobStatus::from_db(status).unwrap();
        assert_eq!(finish_submit_conn(&db.conn, &id, &gid, "x").unwrap(), StepC::Kept { status: st });
        let job = get_job_conn(&db.conn, &id).unwrap();
        assert_eq!((job.grant_id.as_deref(), job.mcp_state), (Some(gid.as_str()), None));
        assert_eq!(job.mcp_reasons, None);
    }
    assert_eq!(db.charged(&g.id), 3);
}

/// Step C is idempotent (LOW-1): a second call on the same job refunds nothing more — the first
/// cleared the row's `grant_id`, so it is no longer charged to this grant. NEGATIVE CONTROL: drop the
/// `row.grant_id == Some(grant_id)` clause → the second call refunds again and this is red.
#[test]
fn step_c_twice_refunds_once() {
    let db = TestDb::new("step-c-twice");
    let g = grant(&db.conn, 3, 2);
    charged(step_a(&db.conn, &sig()));
    let (id, gid) = charged(step_a(&db.conn, &sig()));
    assert_eq!(db.charged(&g.id), 2);
    assert_eq!(finish_submit_conn(&db.conn, &id, &gid, "first").unwrap(), StepC::Refunded);
    assert_eq!(finish_submit_conn(&db.conn, &id, &gid, "second").unwrap(), StepC::Kept { status: JobStatus::Draft });
    assert_eq!(db.charged(&g.id), 1, "one refund, not two");
    assert_eq!(reasons_of(&db, &id), vec![McpReason::submit_refused("first")]);
}

#[test]
fn step_c_on_a_gone_row_keeps_the_charge() {
    let db = TestDb::new("step-c-gone");
    let g = grant(&db.conn, 3, 2);
    let (id, gid) = charged(step_a(&db.conn, &sig()));
    db.conn.execute("DELETE FROM jobs WHERE id = ?1", [&id]).unwrap();
    assert_eq!(finish_submit_conn(&db.conn, &id, &gid, "x").unwrap(), StepC::Gone);
    assert_eq!(db.charged(&g.id), 1);
}

/// The reconciliation: a `submitting` draft is refunded as "interrupted before submit"; a
/// `submitting` row that left draft only loses the flag. NEGATIVE CONTROL: clear the flag on every
/// row without refunding → the draft stays charged and this is red.
#[test]
fn the_reconciliation_refunds_submitting_drafts_and_clears_the_rest() {
    let db = TestDb::new("reconcile");
    let g = grant(&db.conn, 5, 5);
    let (draft, _) = charged(step_a(&db.conn, &sig()));
    let (queued, _) = charged(step_a(&db.conn, &sig()));
    let (settled, _) = charged(step_a(&db.conn, &sig()));
    db.conn.execute("UPDATE jobs SET status = 'queued' WHERE id = ?1", [&queued]).unwrap();
    db.conn.execute("UPDATE jobs SET status = 'running', mcp_state = NULL WHERE id = ?1", [&settled]).unwrap();
    db.job("user", "draft", None);
    assert_eq!(db.charged(&g.id), 3);

    assert_eq!(reconcile_submitting_conn(&db.conn).unwrap(), Reconciled { refunded: 1, cleared: 1 });
    let d = get_job_conn(&db.conn, &draft).unwrap();
    assert_eq!((d.status, d.grant_id, d.mcp_state), (JobStatus::Draft, None, None));
    assert_eq!(reasons_of(&db, &draft), vec![McpReason::interrupted()]);
    let q = get_job_conn(&db.conn, &queued).unwrap();
    assert_eq!((q.grant_id.as_deref(), q.mcp_state), (Some(g.id.as_str()), None));
    assert_eq!(db.charged(&g.id), 2);
    assert_eq!(reconcile_submitting_conn(&db.conn).unwrap(), Reconciled::default(), "idempotent");
}

/// The `submit_job` guard: a job in the step-B window is refused; any other is not. NEGATIVE
/// CONTROL: return `None` always → red.
#[test]
fn submit_is_refused_while_claude_is_submitting() {
    let db = TestDb::new("guard");
    grant(&db.conn, 3, 2);
    let (id, gid) = charged(step_a(&db.conn, &sig()));
    let job = get_job_conn(&db.conn, &id).unwrap();
    assert!(matches!(submit_refusal_for_mcp_state(&job), Some(AppError::Conflict(_))));
    finish_submit_conn(&db.conn, &id, &gid, "x").unwrap();
    assert!(submit_refusal_for_mcp_state(&get_job_conn(&db.conn, &id).unwrap()).is_none());
    db.job("user", "draft", None);
    assert!(submit_refusal_for_mcp_state(&get_job_conn(&db.conn, "user").unwrap()).is_none());
}
