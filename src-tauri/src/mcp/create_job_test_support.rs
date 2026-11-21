//! Shared fixture for `create_job`'s tests (the pipeline's, and the task's in
//! `server::create_job_task_tests`): a migrated scratch database with a server profile whose memory
//! facts are known (so the verdict is `Fits` without the laptop's own disk), an active grant on that
//! profile at the study level, a converged source optimization, and recording fakes for step B and
//! the `job:status` emit.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

use rusqlite::Connection;

use super::audit::{audit_rows_conn, AuditRow};
use super::classify::Target;
use super::create_job::{CreateJobArgs, CreateJobEnv, StatusEmit, StepB, StepBOutcome};
use super::grant::{create_grant_conn, now_utc_conn, NewGrant};
use super::resolve::{SmilesStructure, SmilesTo3d};
use super::server::service::HostPaths;
use super::test_db::{TestDb, LEVEL};
use crate::commands::settings::DbState;
use crate::models::job::JobStatus;

/// The study's Opt at the granted level.
pub const HEADER: &str = "! wB97X-3c CPCM(DMF) Opt\n";
/// The profile the grant covers; its memory facts are known (`preflight_tests`' `uni`).
pub const PROFILE: &str = "uni";
/// The source: water, optimized at the study level.
pub const SOURCE_INPUT: &str =
    "! wB97X-3c CPCM(DMF) Opt TightSCF\n%maxcore 2000\n* xyz 0 1\nO 0.0 0.0 0.0\nH 0.0 0.76 0.59\nH 0.0 -0.76 0.59\n*\n";
const WATER: &str = r#"{"elements":["O","H","H"],"xyz_angstrom":[[0.0,0.0,0.1],[0.0,0.75,0.6],[0.0,-0.75,0.6]]}"#;

/// A SMILES seam that is never reached in these tests (every source is a job).
pub struct NoSidecar;

impl SmilesTo3d for NoSidecar {
    fn smiles_to_3d(&self, _: &str) -> Result<SmilesStructure, String> {
        Err("no sidecar in this test".into())
    }
}

/// Records every `job:status` emit; `on_emit` runs on each (a test sets `closed` or signals there).
#[derive(Default)]
pub struct RecordingEmit {
    pub emitted: Mutex<Vec<(String, JobStatus)>>,
    pub on_emit: Mutex<Option<Box<dyn Fn() + Send>>>,
}

impl StatusEmit for RecordingEmit {
    fn job_status(&self, job_id: &str, status: JobStatus) {
        self.emitted.lock().unwrap().push((job_id.to_string(), status));
        if let Some(f) = self.on_emit.lock().unwrap().as_ref() {
            f();
        }
    }
}

impl RecordingEmit {
    pub fn statuses(&self) -> Vec<JobStatus> {
        self.emitted.lock().unwrap().iter().map(|(_, s)| *s).collect()
    }
}

/// What a fake step B does to the row before it answers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hand {
    /// The path took the job: the row becomes `queued` (as `local_backend::submit` or
    /// `persist_submit` leave it).
    Over,
    /// The path refused before it took the job: the row stays a `draft`.
    Refuse,
    /// A panic inside step B.
    Panic,
}

/// A fake step B over the world's `DbState`. It checks that it runs with the lock free (step B holds
/// no lock), that step A has committed (the row is a charged `submitting` draft), and, when gated,
/// signals `entered` and waits for `release`.
pub struct FakeStepB {
    pub db: Arc<DbState>,
    pub hand: Hand,
    pub calls: AtomicUsize,
    /// Each call: (the lock was free, the row was a submitting draft at entry).
    pub seen: Mutex<Vec<(bool, bool)>>,
    pub entered: Mutex<Option<mpsc::Sender<()>>>,
    pub release: Mutex<Option<mpsc::Receiver<()>>>,
}

impl FakeStepB {
    pub fn new(db: Arc<DbState>, hand: Hand) -> FakeStepB {
        FakeStepB { db, hand, calls: AtomicUsize::new(0), seen: Mutex::new(Vec::new()), entered: Mutex::new(None), release: Mutex::new(None) }
    }

    /// A step B that, on entry, signals and blocks until the returned sender sends or is dropped.
    pub fn gated(db: Arc<DbState>, hand: Hand) -> (Arc<FakeStepB>, mpsc::Receiver<()>, mpsc::Sender<()>) {
        let s = FakeStepB::new(db, hand);
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        *s.entered.lock().unwrap() = Some(entered_tx);
        *s.release.lock().unwrap() = Some(release_rx);
        (Arc::new(s), entered_rx, release_tx)
    }

    pub fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl StepB for FakeStepB {
    fn submit(&self, job_id: &str, _target: &Target) -> StepBOutcome {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let lock_free = self.db.0.try_lock().is_ok();
        let submitting = {
            let conn = self.db.lock().unwrap();
            conn.query_row("SELECT status = 'draft' AND mcp_state = 'submitting' FROM jobs WHERE id = ?1", [job_id], |r| r.get(0))
                .unwrap_or(false)
        };
        self.seen.lock().unwrap().push((lock_free, submitting));
        if let Some(tx) = self.entered.lock().unwrap().take() {
            let _ = tx.send(());
        }
        if let Some(rx) = self.release.lock().unwrap().take() {
            let _ = rx.recv();
        }
        match self.hand {
            Hand::Over => {
                self.db.lock().unwrap().execute("UPDATE jobs SET status = 'queued' WHERE id = ?1", [job_id]).unwrap();
                StepBOutcome::Handed("queued".into())
            }
            Hand::Refuse => StepBOutcome::Refused("the slot is busy".into()),
            Hand::Panic => panic!("a step B that panics"),
        }
    }
}

/// The world `create_job` runs in.
pub struct World {
    pub db: TestDb,
    pub state: Arc<DbState>,
    pub grant_id: String,
    pub emit: Arc<RecordingEmit>,
}

impl World {
    /// A grant on [`PROFILE`] at the study level (`max_in_flight` as given, five jobs, far expiry, made
    /// at the real `now`: step A reads the real clock), and the source job `opt`.
    pub fn new(tag: &str, max_in_flight: u32) -> World {
        let db = TestDb::new(tag);
        std::fs::write(db.dir.join("meminfo"), "MemTotal:       15988408 kB\nMemFree:         2683044 kB\n").unwrap();
        db.conn
            .execute_batch(
                "INSERT INTO server_profiles (id, name, host, remote_orca_path, remote_scratch_dir, core_mask, mem_total_kib, disk_avail_bytes) VALUES
                    ('uni', 'uni', 'uni', '/opt/orca/orca', '/s', '0-23', 65795232, 78360924160),
                    ('dark', 'dark', 'lab', '/opt/orca/orca', '/s', '0-7', NULL, NULL);",
            )
            .unwrap();
        let grant = NewGrant {
            targets: vec![Target::Server { profile_id: PROFILE.into() }],
            levels: vec![LEVEL.into()],
            max_jobs: 5,
            max_in_flight,
            expires_at: "2099-12-31 00:00:00".into(),
        };
        let g = create_grant_conn(&db.conn, &grant, &now_utc_conn(&db.conn).unwrap()).unwrap();
        source_job(&db.conn, &db.dir, "opt", SOURCE_INPUT);
        let state = Arc::new(DbState(Mutex::new(db.open_another())));
        World { db, state, grant_id: g.id, emit: Arc::new(RecordingEmit::default()) }
    }

    pub fn env(&self, step_b: Arc<dyn StepB>) -> Arc<CreateJobEnv> {
        Arc::new(CreateJobEnv {
            db: self.state.clone(),
            smiles: Arc::new(NoSidecar),
            host: Arc::new(HostPaths { meminfo: self.db.dir.join("meminfo"), data_dir: self.db.dir.clone() }),
            step_b,
            emit: self.emit.clone(),
        })
    }

    pub fn args(&self) -> CreateJobArgs {
        CreateJobArgs { header: HEADER.into(), source: "job:opt".into(), target: PROFILE.into(), group_id: None, title: None }
    }

    /// Another connection on the world's file (for a thread or a hook).
    pub fn connection(&self) -> Connection {
        self.db.open_another()
    }

    pub fn audit(&self) -> Vec<AuditRow> {
        audit_rows_conn(&self.db.conn).unwrap()
    }

    pub fn charged(&self) -> i64 {
        self.db.charged(&self.grant_id)
    }

    /// The MCP jobs (every job but the source), oldest first: (id, status, grant_id, mcp_state).
    pub fn mcp_jobs(&self) -> Vec<(String, String, Option<String>, Option<String>)> {
        mcp_jobs(&self.db.conn)
    }
}

/// The MCP jobs on `conn`: (id, status, grant_id, mcp_state).
pub fn mcp_jobs(conn: &Connection) -> Vec<(String, String, Option<String>, Option<String>)> {
    let mut stmt = conn.prepare("SELECT id, status, grant_id, mcp_state FROM jobs WHERE origin = 'mcp' ORDER BY rowid").unwrap();
    stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))).unwrap().collect::<rusqlite::Result<Vec<_>>>().unwrap()
}

/// A parsed source job `id` with `input` and the water results.
pub fn source_job(conn: &Connection, dir: &std::path::Path, id: &str, input: &str) {
    let job_dir = dir.join("jobs").join(id);
    std::fs::create_dir_all(&job_dir).unwrap();
    conn.execute(
        "INSERT INTO jobs (id, title, input_content, status, job_dir) VALUES (?1, 't', ?2, 'parsed', ?3)",
        rusqlite::params![id, input, job_dir.to_str().unwrap()],
    )
    .unwrap();
    let results = format!(r#"{{"parser_version":1,"charges":[],"final_geometry":{WATER},"unknown_blocks":[],"converged":true}}"#);
    conn.execute("INSERT INTO results (job_id, data_json, parser_version) VALUES (?1, ?2, 1)", rusqlite::params![id, results]).unwrap();
}

/// Poll `cond` every 10 ms until it holds or `timeout` passes.
pub fn eventually(timeout: Duration, mut cond: impl FnMut() -> bool) -> bool {
    let start = std::time::Instant::now();
    loop {
        if cond() {
            return true;
        }
        if start.elapsed() > timeout {
            return false;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}
