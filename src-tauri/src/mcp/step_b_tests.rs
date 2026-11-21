//! Tests for `create_job`'s real step B (ADR-027 decision 5; M3 amendment, item 1; unit M3a Part B):
//! the whole pipeline (`run_create_job`) over [`RealStepB`] and a scratch [`SubmitWorld`], so steps A
//! and C are the real ones. The local path runs the app's queueing step (no ORCA is started: the
//! world records the `after_local_queue` call that would emit and start it); the remote path runs
//! `submit_remote` over the `ssh_backend` fake runners (no ssh).

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::classify::Target;
use super::create_job::{run_create_job, CreateJobArgs, CreateJobEnv, CreateJobResult, Outcome, StepB, StepBOutcome};
use super::create_job_test_support::World;
use super::grant::{create_grant_conn, now_utc_conn, NewGrant};
use super::server::gate::AdmissionGate;
use super::step_b::*;
use super::test_db::LEVEL;
use crate::commands::settings::DbState;
use crate::in_flight::InFlight;
use crate::poller::plan::PollerMemory;
use crate::poller::remote_state::RemoteStates;
use crate::models::job::JobStatus;
use crate::poller::{Event, RecordingSink};
use crate::remote::contact::Contact;
use crate::remote::ssh::{CommandRunner, ProcessOutput, TransportError};
use crate::ssh_backend::tests::FirstAttempt;

/// A runner that counts its calls (every ssh and rsync) and hands each to `inner`.
struct Counting {
    inner: Box<dyn CommandRunner + Send + Sync>,
    calls: AtomicUsize,
}

impl CommandRunner for Counting {
    fn run(&self, program: &str, args: &[String], stdin: &[u8], timeout: Duration) -> Result<ProcessOutput, TransportError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.inner.run(program, args, stdin, timeout)
    }
}

/// A server that cannot be reached: every call fails to start.
struct Unreachable;

impl CommandRunner for Unreachable {
    fn run(&self, program: &str, _: &[String], _: &[u8], _: Duration) -> Result<ProcessOutput, TransportError> {
        Err(TransportError::Spawn { program: program.into(), message: "no route to host".into() })
    }
}

/// The scratch world step B runs in: the exit flag and the gate it reads, the remote path's state, a
/// counting runner, and the local queue's tail recorded.
struct Scene {
    db: Arc<DbState>,
    exiting: AtomicBool,
    gate: AdmissionGate,
    in_flight: InFlight,
    remote: RemoteStates,
    memory: PollerMemory,
    sink: RecordingSink,
    data_dir: PathBuf,
    runner: Counting,
    after_queue: Mutex<Vec<String>>,
    /// At each `after_local_queue`: whether `DbState` was free. In the app the tail runs
    /// `try_start_next` → `claim_next`, which locks `DbState` again: held there, it would self-deadlock.
    lock_free_after_queue: Mutex<Vec<bool>>,
}

impl Scene {
    fn new(w: &World, runner: impl CommandRunner + Send + Sync + 'static) -> Arc<Scene> {
        let gate = AdmissionGate::default();
        gate.open();
        Arc::new(Scene {
            db: w.state.clone(),
            exiting: AtomicBool::new(false),
            gate,
            in_flight: InFlight::default(),
            remote: RemoteStates::new(),
            memory: PollerMemory::new(),
            sink: RecordingSink::default(),
            data_dir: w.db.dir.join("data"),
            runner: Counting { inner: Box::new(runner), calls: AtomicUsize::new(0) },
            after_queue: Mutex::new(Vec::new()),
            lock_free_after_queue: Mutex::new(Vec::new()),
        })
    }

    fn ssh_calls(&self) -> usize {
        self.runner.calls.load(Ordering::SeqCst)
    }
}

impl SubmitWorld for Arc<Scene> {
    fn closing(&self) -> Option<&'static str> {
        closing_reason(self.exiting.load(Ordering::SeqCst), self.gate.is_open())
    }

    fn db(&self) -> Result<&DbState, &'static str> {
        Ok(&self.db)
    }

    /// Records the call and whether the lock was free; never locks (a held lock is recorded, not
    /// waited on, so the breach goes red instead of hanging, and a panic here would be swallowed by
    /// step B's `catch_unwind`).
    fn after_local_queue(&self, job_id: &str) {
        self.lock_free_after_queue.lock().unwrap().push(self.db.0.try_lock().is_ok());
        self.after_queue.lock().unwrap().push(job_id.to_string());
    }

    fn with_remote(&self, work: &mut dyn FnMut(&RemoteParts<'_>) -> StepBOutcome) -> Result<StepBOutcome, &'static str> {
        let parts = RemoteParts {
            db: &self.db,
            in_flight: &self.in_flight,
            remote: &self.remote,
            memory: &self.memory,
            sink: &self.sink,
            data_dir: &self.data_dir,
            runner: &self.runner,
        };
        Ok(work(&parts))
    }
}

fn env(w: &World, scene: &Arc<Scene>) -> Arc<CreateJobEnv> {
    w.env(Arc::new(RealStepB(scene.clone())))
}

fn run(w: &World, scene: &Arc<Scene>, args: CreateJobArgs) -> CreateJobResult {
    run_create_job(&env(w, scene), args).expect("create_job")
}

/// The world's profile `uni` made a run target (verified, a core count, a real scratch root), as a
/// remote submit requires.
fn verified_uni(w: &World) {
    w.db.conn
        .execute_batch(
            "UPDATE server_profiles SET verified_at = datetime('now'), orca_version = '6.1.1', core_count = 48, \
             remote_scratch_dir = '/home/anton/.orcastudio' WHERE id = 'uni'",
        )
        .unwrap();
}

/// A grant on this machine at the study level (it revokes the world's `uni` grant), and a
/// `/proc/meminfo` with room, so a local job's memory verdict is `Fits`. Returns the grant's id.
fn local_grant(w: &World) -> String {
    std::fs::write(w.db.dir.join("meminfo"), "MemTotal:       65795232 kB\nMemFree:        60000000 kB\nMemAvailable:   60000000 kB\n").unwrap();
    let grant = NewGrant {
        targets: vec![Target::Local],
        levels: vec![LEVEL.into()],
        max_jobs: 5,
        max_in_flight: 1,
        expires_at: "2099-12-31 00:00:00".into(),
    };
    create_grant_conn(&w.db.conn, &grant, &now_utc_conn(&w.db.conn).unwrap()).unwrap().id
}

fn local_args(w: &World) -> CreateJobArgs {
    CreateJobArgs { target: "local".into(), ..w.args() }
}

/// The MCP job's (status, remote_host, grant_id, mcp_state).
fn row(w: &World) -> (String, Option<String>, Option<String>, Option<String>) {
    w.db.conn
        .query_row("SELECT status, remote_host, grant_id, mcp_state FROM jobs WHERE origin = 'mcp'", [], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
        })
        .unwrap()
}

#[test]
fn the_closing_reason_is_the_exit_first_then_the_gate() {
    assert_eq!(closing_reason(false, true), None);
    assert_eq!(closing_reason(true, true), Some(APP_CLOSING));
    assert_eq!(closing_reason(true, false), Some(APP_CLOSING));
    assert_eq!(closing_reason(false, false), Some(MCP_SWITCHING_OFF));
}

// ----- this machine -----

/// The real local step B queues the charged draft through the app's queueing step and hands the rest
/// (the emit, `try_start_next`) to the world with `DbState` released; step C reads `queued` and keeps
/// the charge. NEGATIVE CONTROL (verifier MED-1a): hold the lock across `after_queued` in
/// `local_step_b` → red, not a hang.
#[test]
fn a_local_step_b_queues_the_job_and_step_c_keeps_the_charge() {
    let w = World::new("sb-local", 1);
    let grant = local_grant(&w);
    let scene = Scene::new(&w, FirstAttempt::new("enqueued 3\n"));
    let r = run(&w, &scene, local_args(&w));
    let job_id = r.job_id.clone().expect("a job");
    assert_eq!(r.disposition, Outcome::Submitted, "{r:?}");
    assert_eq!(row(&w), ("queued".into(), None, Some(grant.clone()), None), "a local job: no coordinates");
    assert_eq!(w.db.charged(&grant), 1);
    assert_eq!(
        *scene.lock_free_after_queue.lock().unwrap(),
        vec![true],
        "LOCK-FREE-QUEUE: after_queued ran with DbState held (try_start_next would self-deadlock)"
    );
    assert_eq!(*scene.after_queue.lock().unwrap(), vec![job_id], "the emit and try_start_next ran once");
    assert_eq!(r.submit.unwrap().step_b, "queued on this machine");
    assert_eq!(scene.ssh_calls(), 0);
}

/// Control (Anton's decision, M3a): a local step B queueing a job once the exit flow has begun or the
/// gate has closed. It refuses before queueing: the draft is refunded to PROPOSED with the reason, and
/// nothing reaches the queue. NEGATIVE CONTROL: drop the closing check from `local_step_b` → the job
/// is queued and kept → red.
#[test]
fn a_local_step_b_refuses_once_exit_has_begun_or_the_gate_has_closed() {
    for (case, why) in [("exit", APP_CLOSING), ("gate", MCP_SWITCHING_OFF)] {
        let w = World::new("sb-local-closing", 1);
        let grant = local_grant(&w);
        let scene = Scene::new(&w, FirstAttempt::new("enqueued 3\n"));
        match case {
            "exit" => scene.exiting.store(true, Ordering::SeqCst),
            _ => scene.gate.close(),
        }
        let r = run(&w, &scene, local_args(&w));
        let money = (r.disposition, w.db.charged(&grant), row(&w));
        assert_eq!(
            money,
            (Outcome::Proposed, 0, ("draft".to_string(), None, None, None)),
            "EXIT-LOCAL: {case}: a local step B queued a job while closing: {r:?}"
        );
        assert_eq!(r.submit.unwrap().step_b, why);
        assert!(scene.after_queue.lock().unwrap().is_empty(), "{case}: nothing reached the queue");
    }
}

// ----- a server -----

/// The real remote step B runs `submit_remote` (persist, prepare, upload, submit) through the guarded
/// path; step C reads `queued` with coordinates and keeps the charge. The message names the outcome.
/// The path's own side effects are pinned too: `announcing_change` emits `job:status` through the
/// world's sink, and `attempt_with_contact` writes the job's remote record with the last call's
/// contact (ADR-024 o23.9). NEGATIVE CONTROLS (verifier MED-1b): the emit sent to a discarded sink
/// (`ANNOUNCE`); the runner called directly, bypassing `attempt_with_contact` (`CONTACT`) → red.
#[test]
fn a_remote_step_b_enqueues_and_step_c_keeps_the_charge() {
    let w = World::new("sb-remote", 1);
    verified_uni(&w);
    let scene = Scene::new(&w, FirstAttempt::new("enqueued 3\n"));
    let r = run(&w, &scene, w.args());
    assert_eq!(r.disposition, Outcome::Submitted, "{r:?}");
    assert_eq!(row(&w), ("queued".into(), Some("uni".into()), Some(w.grant_id.clone()), None));
    assert_eq!(w.charged(), 1);
    assert_eq!(scene.ssh_calls(), 3, "prepare, upload, submit");
    let step_b = r.submit.unwrap().step_b;
    assert!(step_b.starts_with("enqueued on server profile uni"), "{step_b}");
    let job_id = r.job_id.clone().unwrap();
    assert!(!scene.in_flight.is_busy(&job_id), "the guard is released");
    let events = scene.sink.events();
    assert!(
        events.contains(&Event::Status(job_id.clone(), JobStatus::Queued)),
        "ANNOUNCE: announcing_change's job:status never reached the world's sink: {events:?}"
    );
    let record = scene.remote.read(&job_id).state;
    assert_eq!(
        record.as_ref().map(|r| r.last_contact.clone()),
        Some(Some(Contact::Ok)),
        "CONTACT: the attempt bypassed attempt_with_contact; the job's remote record has no contact: {record:?}"
    );
}

/// Decision 5's table: a remote failure after `persist_submit` — every ssh call fails — leaves the row
/// `queued` with its coordinates (nothing on the server, "not on the server" for the label call), and
/// step C keeps the charge. Never refunded once `persist_submit` committed.
#[test]
fn a_remote_failure_after_persist_submit_is_not_refunded() {
    let w = World::new("sb-remote-fail", 1);
    verified_uni(&w);
    let scene = Scene::new(&w, Unreachable);
    let r = run(&w, &scene, w.args());
    assert!(scene.ssh_calls() >= 1, "the attempt reached ssh");
    assert_eq!(
        (r.disposition, w.charged(), row(&w)),
        (Outcome::Submitted, 1, ("queued".to_string(), Some("uni".to_string()), Some(w.grant_id.clone()), None)),
        "{r:?}"
    );
    assert!(!r.submit.unwrap().refunded);
}

/// A refusal before `persist_submit` (the profile is not a run target: not verified) is refunded.
#[test]
fn a_remote_refusal_before_persist_submit_is_refunded() {
    let w = World::new("sb-remote-refused", 1);
    let scene = Scene::new(&w, FirstAttempt::new("enqueued 3\n"));
    let r = run(&w, &scene, w.args());
    assert_eq!((r.disposition, w.charged(), row(&w)), (Outcome::Proposed, 0, ("draft".to_string(), None, None, None)), "{r:?}");
    let step_b = r.submit.unwrap().step_b;
    assert!(step_b.contains("is not a run target"), "{step_b}");
    assert_eq!(scene.ssh_calls(), 0);
}

/// Control (Anton's decision, M3a): a remote step B calling `persist_submit` once the exit flow has
/// begun or the gate has closed. It refuses before `persist_submit`: no ssh starts, no coordinates are
/// written, and the job is refunded to PROPOSED with the reason. NEGATIVE CONTROL: drop the closing
/// check from `remote_step_b` → the job is persisted, submitted and kept → red.
#[test]
fn a_remote_step_b_refuses_before_persist_submit_once_closing() {
    for (case, why) in [("exit", APP_CLOSING), ("gate", MCP_SWITCHING_OFF)] {
        let w = World::new("sb-remote-closing", 1);
        verified_uni(&w);
        let scene = Scene::new(&w, FirstAttempt::new("enqueued 3\n"));
        match case {
            "exit" => scene.exiting.store(true, Ordering::SeqCst),
            _ => scene.gate.close(),
        }
        let r = run(&w, &scene, w.args());
        let money = (r.disposition, w.charged(), row(&w), scene.ssh_calls());
        assert_eq!(
            money,
            (Outcome::Proposed, 0, ("draft".to_string(), None, None, None), 0),
            "EXIT-REMOTE: {case}: a remote step B reached persist_submit while closing: {r:?}"
        );
        assert_eq!(r.submit.as_ref().unwrap().step_b, why);
        assert_eq!(r.reasons[0].message, format!("submit refused: {why}"));
    }
}

/// Switch-off during the call: the gate closes at step A's emit (after step A's commit, before step
/// B). Step B sees it and refuses; the job is refunded and no ssh starts. NEGATIVE CONTROL: as
/// `EXIT-REMOTE` → red.
#[test]
fn a_gate_closed_after_step_a_makes_step_b_refuse() {
    let w = World::new("sb-gate-after-a", 1);
    verified_uni(&w);
    let scene = Scene::new(&w, FirstAttempt::new("enqueued 3\n"));
    let s = scene.clone();
    *w.emit.on_emit.lock().unwrap() = Some(Box::new(move || s.gate.close()));
    let r = run(&w, &scene, w.args());
    assert_eq!(
        (r.disposition, w.charged(), scene.ssh_calls()),
        (Outcome::Proposed, 0, 0),
        "EXIT-GATE-AFTER-A: a gate closed after step A did not stop step B: {r:?}"
    );
    assert_eq!(r.submit.unwrap().step_b, MCP_SWITCHING_OFF);
}

/// The remote path holds the job's in-flight guard, as `submit_to_server` does: a job whose guard is
/// taken (another remote operation on it) is refused before anything is read or written.
#[test]
fn a_remote_step_b_is_refused_while_the_job_is_in_flight() {
    let w = World::new("sb-in-flight", 1);
    verified_uni(&w);
    w.db.conn.execute("INSERT INTO jobs (id, title, input_content, status) VALUES ('j9', 't', '! SP\n', 'draft')", []).unwrap();
    let scene = Scene::new(&w, FirstAttempt::new("enqueued 3\n"));
    let _held = scene.in_flight.acquire("j9").unwrap();
    let outcome = RealStepB(scene.clone()).submit("j9", &Target::Server { profile_id: "uni".into() });
    assert!(matches!(&outcome, StepBOutcome::Refused(m) if m.contains("already in progress")), "{outcome:?}");
    assert_eq!(scene.ssh_calls(), 0);
    let status: String = w.db.conn.query_row("SELECT status FROM jobs WHERE id = 'j9'", [], |r| r.get(0)).unwrap();
    assert_eq!(status, "draft");
}
