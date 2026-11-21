//! Tests for `cancel_job`'s core (ADR-027 decisions 5 and 7; M3 amendment, items 1 and 3; unit M3b
//! Part A): only `origin = 'mcp'` jobs, never a `Draft`, a pre-row before the cancel core and an
//! outcome row after it, the cancel core run with no lock held. Each control in the session's
//! `m3b_parta_controls.py` names the test it turns red by its tag.

use std::cell::Cell;
use std::path::PathBuf;
use std::time::Duration;

use super::audit::{audit_rows_conn, AuditRow};
use super::cancel::*;
use super::test_db::{grant, TestDb};
use crate::commands::settings::DbState;
use crate::error::AppError;
use crate::execution_backend::FetchPolicy;
use crate::in_flight::InFlight;
use crate::models::job::{Job, JobStatus};
use crate::poller::live_log::LiveLog;
use crate::poller::plan::PollerMemory;
use crate::poller::remote_state::RemoteStates;
use crate::poller::{Poller, RecordingSink, SystemClock};
use crate::remote::ssh::{CommandRunner, ProcessOutput, TransportError};

/// No ssh or rsync in these tests: every job the core sends to the server path is held busy.
struct NoSsh;

impl CommandRunner for NoSsh {
    fn run(&self, program: &str, _: &[String], _: &[u8], _: Duration) -> Result<ProcessOutput, TransportError> {
        panic!("no {program} call in cancel_job's tests");
    }
}

/// A migrated scratch database, a second connection on it as the poller's `DbState`, and the rest
/// of a [`Poller`]'s state.
struct Scene {
    db: TestDb,
    state: DbState,
    in_flight: InFlight,
    memory: PollerMemory,
    live: LiveLog,
    remote: RemoteStates,
    sink: RecordingSink,
    data_dir: PathBuf,
    grant_id: String,
}

impl Scene {
    fn new(tag: &str) -> Scene {
        let db = TestDb::new(tag);
        let grant_id = grant(&db.conn, 5, 2).id;
        let state = DbState(std::sync::Mutex::new(db.open_another()));
        let data_dir = db.dir.clone();
        Scene {
            db,
            state,
            in_flight: InFlight::default(),
            memory: PollerMemory::new(),
            live: LiveLog::new(),
            remote: RemoteStates::new(),
            sink: RecordingSink::default(),
            data_dir,
            grant_id,
        }
    }

    fn poller(&self) -> Poller<'_> {
        Poller {
            db: &self.state,
            runner: &NoSsh,
            in_flight: &self.in_flight,
            memory: &self.memory,
            live: &self.live,
            remote: &self.remote,
            sink: &self.sink,
            policy: FetchPolicy::SMALL_ONLY,
            data_dir: &self.data_dir,
            clock: &SystemClock,
        }
    }

    /// A local job charged to the grant (`origin = 'mcp'`).
    fn claudes(&self, id: &str, status: &str) {
        self.db.job(id, status, Some(&self.grant_id));
    }

    /// A local job Anton made (`origin = 'user'`).
    fn antons(&self, id: &str, status: &str) {
        self.db.job(id, status, None);
    }

    fn rows(&self) -> Vec<AuditRow> {
        audit_rows_conn(&self.db.conn).unwrap()
    }

    fn status(&self, id: &str) -> String {
        self.db.conn.query_row("SELECT status FROM jobs WHERE id = ?1", [id], |r| r.get(0)).unwrap()
    }

    fn args(id: &str) -> CancelJobArgs {
        CancelJobArgs { job_id: id.into() }
    }
}

/// A dispatch that must never run.
fn never(job: Job) -> Result<(), AppError> {
    panic!("the cancel core dispatched job {} although the pre-check refused it", job.id);
}

/// `[(disposition, [(row, message)])]` of every row, for compact asserts.
fn shape(rows: &[AuditRow]) -> Vec<(Option<String>, Vec<(String, String)>)> {
    rows.iter()
        .map(|r| (r.disposition.clone(), r.reasons.iter().map(|x| (x.row.clone(), x.message.clone())).collect()))
        .collect()
}

/// Anton's job is refused, the cancel core is not called, and the refusal is audited. Tag `USER-JOB`.
#[test]
fn a_user_job_is_refused_with_a_row_and_never_reaches_the_core() {
    let s = Scene::new("cancel-user");
    s.antons("u1", "queued");
    let dispatched = Cell::new(0);
    let result = run_cancel_job(&s.poller(), &Scene::args("u1"), |_| {
        dispatched.set(dispatched.get() + 1);
        Ok(())
    })
    .unwrap();

    let CancelJobResult::Refused { job_id, reasons } = &result else { panic!("USER-JOB: not refused: {result:?}") };
    assert_eq!(job_id, "u1");
    assert_eq!(reasons[0].row, "origin", "{reasons:?}");
    assert!(reasons[0].message.contains("not Claude's job") && reasons[0].message.contains("origin = user"), "{reasons:?}");
    assert_eq!(dispatched.get(), 0, "USER-JOB: the cancel core dispatched a user job");
    assert_eq!(s.status("u1"), "queued");
    let rows = s.rows();
    assert_eq!(shape(&rows), vec![(Some("refused".into()), vec![("origin".into(), reasons[0].message.clone())])]);
    assert_eq!((rows[0].tool.as_str(), rows[0].job_id.as_deref(), rows[0].grant_id.as_deref()), (CANCEL_JOB, Some("u1"), None));
    assert_eq!(rows[0].args, r#"{"job_id":"u1"}"#);
}

/// A `Draft` — PROPOSED, and a charged draft in step B (`mcp_state = 'submitting'`) — is refused in
/// the pre-check, with its row; the cancel core is not called. Tag `DRAFT`.
#[test]
fn a_draft_is_refused_with_a_row_and_never_reaches_the_core() {
    let s = Scene::new("cancel-draft");
    s.claudes("d1", "draft");
    s.db.conn.execute("UPDATE jobs SET grant_id = NULL WHERE id = 'd1'", []).unwrap(); // PROPOSED: uncharged
    s.claudes("d2", "draft");
    s.db.conn.execute("UPDATE jobs SET mcp_state = 'submitting' WHERE id = 'd2'", []).unwrap();

    for id in ["d1", "d2"] {
        let dispatched = Cell::new(0);
        let result = run_cancel_job(&s.poller(), &Scene::args(id), |_| {
            dispatched.set(dispatched.get() + 1);
            Ok(())
        })
        .unwrap();
        let CancelJobResult::Refused { reasons, .. } = &result else { panic!("DRAFT: {id} not refused: {result:?}") };
        assert_eq!((reasons[0].row.as_str(), reasons[0].message.as_str()), ("draft", DRAFT_REFUSED));
        assert_eq!(dispatched.get(), 0, "DRAFT: the cancel core was reached for {id}");
        assert_eq!(s.status(id), "draft");
    }
    let rows = s.rows();
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().all(|r| r.disposition.as_deref() == Some("refused")), "{rows:?}");
    assert_eq!(rows[0].grant_id, None, "a PROPOSED draft has no grant");
    assert_eq!(rows[1].grant_id.as_deref(), Some(s.grant_id.as_str()), "the charged draft's grant, informational");
}

/// Claude's queued local job: the pre-row is committed **before** the cancel core runs, the core runs
/// with `DbState` free, and the outcome row follows. Tags `PRE-ROW`, `OUTCOME-ROW`, `LOCK-FREE-CORE`.
#[test]
fn claudes_job_gets_a_pre_row_then_the_core_then_an_outcome_row() {
    let s = Scene::new("cancel-mcp");
    s.claudes("m1", "queued");
    let seen_in_dispatch: Cell<Option<(bool, Vec<AuditRow>)>> = Cell::new(None);
    let result = run_cancel_job(&s.poller(), &Scene::args("m1"), |job| {
        assert_eq!(job.id, "m1");
        let lock_free = s.state.0.try_lock().is_ok();
        seen_in_dispatch.set(Some((lock_free, s.rows())));
        s.db.conn.execute("UPDATE jobs SET status = 'cancelled' WHERE id = 'm1'", []).unwrap();
        Ok(())
    })
    .unwrap();

    let (lock_free, rows_at_dispatch) = seen_in_dispatch.take().expect("the cancel core dispatched");
    assert!(lock_free, "LOCK-FREE-CORE: DbState was held while the cancel core dispatched");
    assert_eq!(
        shape(&rows_at_dispatch),
        vec![(None, vec![("cancel".into(), "requested (job was queued)".into())])],
        "PRE-ROW: the pre-row must be committed before the cancel core runs"
    );
    assert_eq!(
        result,
        CancelJobResult::Requested {
            job_id: "m1".into(),
            status_before: "queued".into(),
            result: "dispatched".into(),
            detail: r#"{"result":"dispatched"}"#.into(),
            audit_error: None,
        }
    );
    let rows = s.rows();
    assert_eq!(
        shape(&rows),
        vec![
            (None, vec![("cancel".into(), "requested (job was queued)".into())]),
            (None, vec![("cancel".into(), "dispatched".into()), ("detail".into(), r#"{"result":"dispatched"}"#.into())]),
        ],
        "OUTCOME-ROW: the pre-row and the outcome row, in that order"
    );
    for r in &rows {
        assert_eq!((r.tool.as_str(), r.job_id.as_deref()), (CANCEL_JOB, Some("m1")));
        assert_eq!(r.grant_id.as_deref(), Some(s.grant_id.as_str()), "the job's grant, informational");
    }
}

/// A PROPOSED job Anton ran keeps `origin = 'mcp'` and is not charged: Claude may cancel it
/// (decision 5, "When Anton runs a PROPOSED job"); its rows carry no grant.
#[test]
fn a_proposed_job_anton_ran_may_be_cancelled() {
    let s = Scene::new("cancel-ran");
    s.claudes("p1", "running");
    s.db.conn.execute("UPDATE jobs SET grant_id = NULL WHERE id = 'p1'", []).unwrap();
    let dispatched = Cell::new(0);
    let result = run_cancel_job(&s.poller(), &Scene::args("p1"), |_| {
        dispatched.set(dispatched.get() + 1);
        Ok(())
    })
    .unwrap();
    assert!(matches!(result, CancelJobResult::Requested { ref result, .. } if result == "dispatched"), "{result:?}");
    assert_eq!(dispatched.get(), 1);
    let rows = s.rows();
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().all(|r| r.grant_id.is_none() && r.disposition.is_none()), "{rows:?}");
    assert_eq!(rows[0].reasons[0].message, "requested (job was running)");
}

/// The core's error is the outcome row (`cancel_error`, bounded) and the result says so.
#[test]
fn the_cores_error_is_audited_and_reported() {
    let s = Scene::new("cancel-err");
    s.claudes("e1", "running");
    let long = format!("the backend refused: {}", "x".repeat(2_000));
    let result = run_cancel_job(&s.poller(), &Scene::args("e1"), |_| Err(AppError::Backend(long.clone()))).unwrap();
    let CancelJobResult::Failed { status_before, error, audit_error, .. } = &result else { panic!("{result:?}") };
    assert_eq!((status_before.as_str(), audit_error), ("running", &None));
    assert!(error.starts_with("Backend error: the backend refused") || error.contains("the backend refused"), "{error}");
    assert!(error.len() <= super::audit::REASON_MESSAGE_MAX_BYTES, "{} bytes", error.len());
    let rows = s.rows();
    assert_eq!(rows.len(), 2);
    assert_eq!((rows[1].disposition.as_deref(), rows[1].reasons[0].row.as_str()), (None, "cancel_error"));
    assert!(rows[1].reasons[0].message.len() <= super::audit::REASON_MESSAGE_MAX_BYTES);
}

/// A remote job the poller holds busy: the core records the cancel and leaves it pending (no ssh);
/// the outcome row says `pending`.
#[test]
fn a_busy_remote_job_is_recorded_pending_and_audited() {
    let s = Scene::new("cancel-remote");
    s.db.conn
        .execute(
            "INSERT INTO jobs (id, title, input_content, status, origin, grant_id, remote_host, remote_job_dir, remote_socket) \
             VALUES ('r1', 't', '! SP', 'queued', 'mcp', ?1, 'uni', '/r/jobs/r1', '/r/tsp/slot0.sock')",
            [&s.grant_id],
        )
        .unwrap();
    let busy = s.in_flight.acquire("r1").unwrap();
    let result = run_cancel_job(&s.poller(), &Scene::args("r1"), never).unwrap();
    drop(busy);
    assert!(matches!(result, CancelJobResult::Requested { ref result, .. } if result == "pending"), "{result:?}");
    let requested: Option<String> =
        s.db.conn.query_row("SELECT cancel_requested_at FROM jobs WHERE id = 'r1'", [], |r| r.get(0)).unwrap();
    assert!(requested.is_some(), "the cancel is recorded for the poller");
    let rows = s.rows();
    assert_eq!(rows[1].reasons[0].message, "pending");
    assert_eq!(rows[1].reasons[1].message, r#"{"result":"pending","already_pending":false}"#);
}

/// An unknown job is an argument error: no row (M3 amendment, item 3). So is an id over its bound or
/// empty.
#[test]
fn an_unknown_or_unbounded_id_is_refused_with_no_row() {
    let s = Scene::new("cancel-args");
    let unknown = run_cancel_job(&s.poller(), &Scene::args("nope"), never).unwrap_err();
    assert!(unknown.to_string().contains("does not exist"), "{unknown}");
    let long = run_cancel_job(&s.poller(), &Scene::args(&"a".repeat(JOB_ID_MAX_BYTES + 1)), never).unwrap_err();
    assert!(long.to_string().contains("at most 64"), "{long}");
    assert!(run_cancel_job(&s.poller(), &Scene::args(""), never).is_err());
    assert!(s.rows().is_empty(), "an argument error is not a disposition");
    // At the bound, the id is looked up (and is unknown here).
    let at = run_cancel_job(&s.poller(), &Scene::args(&"a".repeat(JOB_ID_MAX_BYTES)), never).unwrap_err();
    assert!(at.to_string().contains("does not exist"), "{at}");
}

/// No cancel without its audit row: when the pre-row cannot be written, the cancel core is not
/// called and the job is untouched. Tag `NO-ROW-NO-CANCEL`.
#[test]
fn no_cancel_happens_when_the_pre_row_cannot_be_written() {
    let s = Scene::new("cancel-norow");
    s.claudes("m1", "queued");
    s.state
        .lock()
        .unwrap()
        .execute_batch("CREATE TEMP TRIGGER refuse_audit BEFORE INSERT ON mcp_audit BEGIN SELECT RAISE(ABORT, 'audit refused'); END;")
        .unwrap();
    let dispatched = Cell::new(0);
    let result = run_cancel_job(&s.poller(), &Scene::args("m1"), |_| {
        dispatched.set(dispatched.get() + 1);
        Ok(())
    });
    assert_eq!(dispatched.get(), 0, "NO-ROW-NO-CANCEL: the cancel core ran without its pre-row");
    assert!(result.is_err(), "{result:?}");
    assert_eq!(s.status("m1"), "queued");
    assert!(s.rows().is_empty());
}

/// A failed outcome-row write is reported in the result (`audit_error`), never hidden; the pre-row
/// stays (the accepted crash case's shape).
#[test]
fn a_failed_outcome_row_is_reported() {
    let s = Scene::new("cancel-outfail");
    s.claudes("m1", "queued");
    let result = run_cancel_job(&s.poller(), &Scene::args("m1"), |_| {
        // `try_lock`, so a lock held across the core fails here instead of deadlocking.
        s.state
            .0
            .try_lock()
            .expect("DbState is free while the core dispatches")
            .execute_batch("CREATE TEMP TRIGGER refuse_audit BEFORE INSERT ON mcp_audit BEGIN SELECT RAISE(ABORT, 'audit refused'); END;")
            .unwrap();
        Ok(())
    })
    .unwrap();
    let CancelJobResult::Requested { audit_error, .. } = &result else { panic!("{result:?}") };
    assert!(audit_error.as_deref().is_some_and(|e| e.contains("audit refused")), "{audit_error:?}");
    assert_eq!(shape(&s.rows()), vec![(None, vec![("cancel".into(), "requested (job was queued)".into())])]);
}

/// The pre-check's encoding over every status Claude's job can be cancelled in, and the outcome
/// reasons of each `CancelResult` variant.
#[test]
fn the_row_encodings() {
    for status in [JobStatus::Queued, JobStatus::Running, JobStatus::Cancelling, JobStatus::Completed] {
        let r = requested_reason(status);
        assert_eq!((r.row.as_str(), r.message), ("cancel", format!("requested (job was {})", status.as_str())));
    }
    use crate::commands::jobs::CancelResult;
    for (result, tag) in [
        (CancelResult::Dispatched, "dispatched"),
        (CancelResult::Pending { already_pending: true }, "pending"),
        (CancelResult::AlreadyOnServer, "already_on_server"),
        (CancelResult::MovedOn { status: JobStatus::Completed }, "moved_on"),
    ] {
        let reasons = outcome_reasons(&Ok(result.clone()));
        assert_eq!(reasons[0].message, tag);
        // The tag here is the one serde writes (`CancelResult`'s `#[serde(tag = "result")]`).
        assert!(reasons[1].message.starts_with(&format!(r#"{{"result":"{tag}""#)), "{}", reasons[1].message);
    }
}
