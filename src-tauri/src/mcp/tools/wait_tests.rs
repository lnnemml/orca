//! Tests for `wait_for_jobs` (ADR-027 decision 8; M0 amendment, items 5, 6 and 10; unit M3b Part A):
//! the answer at once, the tick loop, `closed`, the clamp, the argument bounds, and no lock held
//! while it sleeps. The clock is a fake that advances by each sleep at once (no real waiting), and
//! runs a hook at each sleep — the moment the wait is asleep. Each control in the session's
//! `m3b_parta_controls.py` names the test it turns red by its tag.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::wait::*;
use super::{result_text, ToolError};
use crate::commands::settings::DbState;
use crate::mcp::test_db::TestDb;
use crate::models::job::JobStatus;

type Hook = Box<dyn FnMut(usize) + Send>;

/// Time moves only when the wait sleeps; `on_sleep` runs with the sleep's index (0-based).
#[derive(Default)]
struct FakeClock {
    now_ms: AtomicU64,
    sleeps: Mutex<Vec<Duration>>,
    on_sleep: Mutex<Option<Hook>>,
}

impl FakeClock {
    fn with_hook(hook: impl FnMut(usize) + Send + 'static) -> FakeClock {
        FakeClock { on_sleep: Mutex::new(Some(Box::new(hook))), ..Default::default() }
    }

    fn sleeps(&self) -> Vec<Duration> {
        self.sleeps.lock().unwrap().clone()
    }

    fn slept(&self) -> Duration {
        self.sleeps().iter().sum()
    }
}

impl WaitClock for FakeClock {
    fn now(&self) -> Duration {
        Duration::from_millis(self.now_ms.load(Ordering::SeqCst))
    }

    fn sleep(&self, d: Duration) -> SeamFuture<()> {
        let index = {
            let mut sleeps = self.sleeps.lock().unwrap();
            sleeps.push(d);
            sleeps.len() - 1
        };
        self.now_ms.fetch_add(d.as_millis() as u64, Ordering::SeqCst);
        if let Some(hook) = self.on_sleep.lock().unwrap().as_mut() {
            hook(index);
        }
        Box::pin(std::future::ready(()))
    }
}

/// A reader whose answer is a function of the read's index (0-based).
struct Scripted {
    reads: AtomicUsize,
    script: Box<dyn Fn(usize) -> StatusReading + Send + Sync>,
}

impl Scripted {
    fn new(script: impl Fn(usize) -> StatusReading + Send + Sync + 'static) -> Scripted {
        Scripted { reads: AtomicUsize::new(0), script: Box::new(script) }
    }

    fn reads(&self) -> usize {
        self.reads.load(Ordering::SeqCst)
    }
}

impl StatusReader for Scripted {
    fn read(&self, _ids: Vec<String>) -> SeamFuture<Result<StatusReading, ToolError>> {
        let n = self.reads.fetch_add(1, Ordering::SeqCst);
        Box::pin(std::future::ready(Ok((self.script)(n))))
    }
}

fn reading(pairs: &[(&str, JobStatus)]) -> StatusReading {
    pairs.iter().map(|(id, s)| (id.to_string(), *s)).collect()
}

fn args(known: &[(&str, &str)], timeout_s: Option<u64>) -> WaitArgs {
    WaitArgs { known: known.iter().map(|(id, s)| (id.to_string(), s.to_string())).collect(), timeout_s }
}

fn run(a: &WaitArgs, closed: &AtomicBool, reader: &dyn StatusReader, clock: &dyn WaitClock) -> Result<WaitEnd, ToolError> {
    tauri::async_runtime::block_on(wait_for_jobs(a, closed, reader, clock, WAIT_TICK))
}

fn answered(end: Result<WaitEnd, ToolError>) -> WaitResult {
    match end {
        Ok(WaitEnd::Answered(r)) => r,
        other => panic!("not answered: {other:?}"),
    }
}

/// A status that differs when the call arrives is answered at once: one read, no sleep. Tag
/// `KNOWN-AT-ONCE`.
#[test]
fn a_status_that_already_differs_is_answered_at_once() {
    let reader = Scripted::new(|_| reading(&[("a", JobStatus::Queued), ("b", JobStatus::Completed)]));
    let clock = FakeClock::default();
    let result = answered(run(&args(&[("a", "queued"), ("b", "running")], None), &AtomicBool::new(false), &reader, &clock));
    assert_eq!(
        result.changed,
        vec![StatusChange { job_id: "b".into(), known_status: "running".into(), current_status: "completed".into() }],
        "KNOWN-AT-ONCE: the differing job"
    );
    assert!(!result.timed_out);
    assert_eq!((reader.reads(), clock.sleeps().len(), result.waited_s), (1, 0, 0), "KNOWN-AT-ONCE: answered before any sleep");
    assert_eq!((result.watched_count, result.effective_timeout_s, result.timeout_clamped), (2, 600, false));
}

/// A job that no longer exists counts as differing: answered at once, its current status `"gone"`.
#[test]
fn a_gone_job_is_answered_at_once_as_gone() {
    let reader = Scripted::new(|_| reading(&[("a", JobStatus::Queued)]));
    let clock = FakeClock::default();
    let result = answered(run(&args(&[("a", "queued"), ("deleted", "running")], None), &AtomicBool::new(false), &reader, &clock));
    assert_eq!(
        result.changed,
        vec![StatusChange { job_id: "deleted".into(), known_status: "running".into(), current_status: GONE.into() }]
    );
    assert_eq!((reader.reads(), clock.sleeps().len()), (1, 0));
}

/// Unchanged statuses are re-read once per 2 s tick until one differs. Tag `KNOWN-TICK`.
#[test]
fn an_unchanged_wait_reads_every_tick_until_a_change() {
    let reader = Scripted::new(|n| reading(&[("a", if n < 3 { JobStatus::Running } else { JobStatus::Completed })]));
    let clock = FakeClock::default();
    let result = answered(run(&args(&[("a", "running")], None), &AtomicBool::new(false), &reader, &clock));
    assert_eq!(result.changed.first().map(|c| c.current_status.as_str()), Some("completed"), "KNOWN-TICK: the change was not seen");
    assert_eq!(clock.sleeps(), vec![WAIT_TICK; 3]);
    assert_eq!((reader.reads(), result.waited_s, result.timed_out), (4, 6, false));
}

/// The `closed` flag ends the wait at the next tick, before the next read; a wait that starts closed
/// reads nothing. Tag `CLOSED`.
#[test]
fn a_closed_connection_ends_the_wait_at_the_next_tick() {
    let closed = Arc::new(AtomicBool::new(false));
    let flag = closed.clone();
    let clock = FakeClock::with_hook(move |i| {
        if i == 1 {
            flag.store(true, Ordering::SeqCst);
        }
    });
    let reader = Scripted::new(|_| reading(&[("a", JobStatus::Running)]));
    let end = run(&args(&[("a", "running")], Some(600)), &closed, &reader, &clock).unwrap();
    assert_eq!(end, WaitEnd::ConnectionClosed, "CLOSED: the wait outlived its connection");
    assert_eq!((reader.reads(), clock.sleeps().len()), (2, 2), "CLOSED: no read after the flag was set");

    let reader = Scripted::new(|_| reading(&[("a", JobStatus::Running)]));
    let end = run(&args(&[("a", "running")], None), &AtomicBool::new(true), &reader, &FakeClock::default()).unwrap();
    assert_eq!((end, reader.reads()), (WaitEnd::ConnectionClosed, 0));
}

/// `timeout_s` above 600 is clamped to 600, and the result says so. Tag `CLAMP`.
#[test]
fn a_timeout_above_600_is_clamped() {
    for asked in [601, 10_000, u64::MAX] {
        let reader = Scripted::new(|_| reading(&[("a", JobStatus::Running)]));
        let clock = FakeClock::default();
        let result = answered(run(&args(&[("a", "running")], Some(asked)), &AtomicBool::new(false), &reader, &clock));
        assert!(result.timed_out && result.changed.is_empty());
        assert_eq!((result.effective_timeout_s, result.timeout_clamped), (600, true), "CLAMP: {asked}");
        assert_eq!(clock.slept(), Duration::from_secs(600), "CLAMP: waited past 600 s for {asked}");
        assert_eq!((result.waited_s, reader.reads()), (600, 301));
    }
}

/// Omitted = 600 (not clamped); 0 = one read and no sleep; a timeout that is not a multiple of the
/// tick ends exactly on it.
#[test]
fn the_timeout_default_zero_and_a_partial_last_tick() {
    let still = || Scripted::new(|_| reading(&[("a", JobStatus::Queued)]));

    let (reader, clock) = (still(), FakeClock::default());
    let result = answered(run(&args(&[("a", "queued")], None), &AtomicBool::new(false), &reader, &clock));
    assert_eq!((result.effective_timeout_s, result.timeout_clamped, clock.slept()), (600, false, Duration::from_secs(600)));

    let (reader, clock) = (still(), FakeClock::default());
    let result = answered(run(&args(&[("a", "queued")], Some(0)), &AtomicBool::new(false), &reader, &clock));
    assert!(result.timed_out);
    assert_eq!((reader.reads(), clock.sleeps().len(), result.waited_s), (1, 0, 0), "a one-shot check");

    let (reader, clock) = (still(), FakeClock::default());
    let result = answered(run(&args(&[("a", "queued")], Some(5)), &AtomicBool::new(false), &reader, &clock));
    assert_eq!(clock.sleeps(), vec![WAIT_TICK, WAIT_TICK, Duration::from_secs(1)]);
    assert_eq!((result.waited_s, reader.reads(), result.timed_out), (5, 4, true));
}

/// The argument bounds, each refused before any read. Tags `EMPTY-KNOWN`, `KNOWN-BOUND`.
#[test]
fn the_arguments_are_bounded_and_refused_before_any_read() {
    // The refusal's text, or `None` when the call was not refused.
    let refused = |a: WaitArgs| {
        let reader = Scripted::new(|_| StatusReading::new());
        let err = run(&a, &AtomicBool::new(false), &reader, &FakeClock::default()).err()?;
        assert_eq!(reader.reads(), 0, "refused before any read: {err}");
        Some(err.to_string())
    };
    let empty = refused(args(&[], None));
    assert!(empty.as_deref().is_some_and(|e| e.contains("`known` is empty")), "EMPTY-KNOWN: {empty:?}");

    let many = |n: usize| WaitArgs {
        known: (0..n).map(|i| (format!("job-{i:02}"), "queued".to_string())).collect::<BTreeMap<_, _>>(),
        timeout_s: Some(0),
    };
    let over = refused(many(KNOWN_MAX + 1));
    assert!(over.as_deref().is_some_and(|e| e.contains("names 51 jobs; at most 50")), "KNOWN-BOUND: {over:?}");
    let reader = Scripted::new(|_| StatusReading::new());
    let at = answered(run(&many(KNOWN_MAX), &AtomicBool::new(false), &reader, &FakeClock::default()));
    assert_eq!((at.watched_count, at.changed.len()), (50, 50), "50 is accepted (all gone here)");

    assert!(refused(args(&[(&"j".repeat(JOB_ID_MAX_BYTES + 1), "queued")], None)).unwrap().contains("at most 64"));
    assert!(refused(args(&[("", "queued")], None)).unwrap().contains("empty job id"));
    for bad in ["Running", "done", "", GONE] {
        assert!(refused(args(&[("a", bad)], None)).unwrap().contains("is not a job status"), "{bad:?}");
    }
}

/// The largest answer — 50 ids of 64 bytes, each changed — stays far under the cap.
#[test]
fn the_largest_answer_fits() {
    let known: BTreeMap<String, String> =
        (0..KNOWN_MAX).map(|i| (format!("{i:0>64}"), "cancelling".to_string())).collect();
    let reader = Scripted::new(|_| StatusReading::new());
    let result = answered(run(&WaitArgs { known, timeout_s: None }, &AtomicBool::new(false), &reader, &FakeClock::default()));
    let bytes = result_text(&result).unwrap().len();
    assert!(bytes < 10_000, "{bytes} bytes");
}

/// The app's reader over a real database: one query; a missing job is absent; a stored status
/// outside the known set is an error (rule #9), never read as gone.
#[test]
fn the_db_reader_reads_existing_jobs_and_refuses_an_unknown_status() {
    let db = TestDb::new("wait-read");
    db.job("a", "running", None);
    db.job("b", "completed", None);
    let got = read_statuses_conn(&db.conn, &["a".into(), "b".into(), "missing".into()]).unwrap();
    assert_eq!(got, reading(&[("a", JobStatus::Running), ("b", JobStatus::Completed)]));
    assert!(read_statuses_conn(&db.conn, &[]).unwrap().is_empty());
    db.conn.execute("UPDATE jobs SET status = 'weird' WHERE id = 'a'", []).unwrap();
    assert!(read_statuses_conn(&db.conn, &["a".into()]).is_err());
}

/// No lock is held while the wait sleeps: at every sleep `DbState` is free (`try_lock`), and a writer
/// there changes a status that the next tick sees. Over the app's reader (`DbStatusReader`, the
/// blocking pool). Tag `LOCK-ACROSS-SLEEP`. When the lock is not free, the hook sets `closed`, so a
/// held lock fails the test instead of deadlocking the next read.
#[test]
fn no_lock_is_held_while_the_wait_sleeps() {
    let db = TestDb::new("wait-lock");
    db.job("a", "running", None);
    let state = Arc::new(DbState(Mutex::new(db.open_another())));
    let closed = Arc::new(AtomicBool::new(false));
    let free_at_sleep = Arc::new(Mutex::new(Vec::new()));
    let clock = {
        let (state, closed, free) = (state.clone(), closed.clone(), free_at_sleep.clone());
        FakeClock::with_hook(move |i| match state.0.try_lock() {
            Ok(conn) => {
                free.lock().unwrap().push(true);
                if i == 1 {
                    conn.execute("UPDATE jobs SET status = 'completed' WHERE id = 'a'", []).unwrap();
                }
            }
            Err(_) => {
                free.lock().unwrap().push(false);
                closed.store(true, Ordering::SeqCst);
            }
        })
    };
    let reader = DbStatusReader(state.clone());
    let end = run(&args(&[("a", "running")], None), &closed, &reader, &clock);
    assert_eq!(*free_at_sleep.lock().unwrap(), vec![true, true], "LOCK-ACROSS-SLEEP: DbState held while the wait slept");
    let result = answered(end);
    assert_eq!(result.changed[0].current_status, "completed");
    assert_eq!(result.waited_s, 4);
}

/// The wait's future is `Send` (an `rmcp` handler awaits it on the multi-threaded runtime). Together
/// with the seams' `Send + 'static` futures and `Send + Sync` reader, this is what keeps a
/// `std::sync::MutexGuard` out of the wait at compile time.
#[test]
fn the_wait_future_is_send() {
    fn assert_send<T: Send>(_: &T) {}
    let (a, closed, reader, clock) = (args(&[("a", "queued")], Some(0)), AtomicBool::new(false), Scripted::new(|_| StatusReading::new()), FakeClock::default());
    let fut = wait_for_jobs(&a, &closed, &reader, &clock, WAIT_TICK);
    assert_send(&fut);
    drop(fut);
}
