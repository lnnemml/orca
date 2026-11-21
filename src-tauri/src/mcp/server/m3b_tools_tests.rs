//! The M3b tools over the wire (ADR-027 decisions 5, 7 and 8; M0 amendment, items 5 and 6; M3b Part
//! B): `cancel_job`, `create_group` and `wait_for_jobs` registered in the `rmcp` service, served on
//! `tauri::async_runtime` over the real socket and called by a raw NDJSON client, so every assertion
//! is on what the model would receive. `cancel_job` runs in a scratch [`CancelWorld`]: a real
//! `Poller` over the served database and a recording dispatch (the app's world, `AppCancelWorld`,
//! needs an `AppHandle`; its body is pinned in `wiring_tests`). Each control in the session's
//! `m3b_partb_controls.py` names the test it turns red by its tag.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};

use super::create_job_task::CreateJobSeams;
use super::lifecycle::{Hooks, LifecycleEnv, McpLifecycle};
use super::listener::{kernel_peer_uid, own_uid};
use super::m3b_tools::{CancelCall, CancelWorld, CANCEL_JOB_DESCRIPTION, CREATE_GROUP_DESCRIPTION, WAIT_FOR_JOBS_DESCRIPTION};
use super::service::{ConnState, HostPaths, INSTRUCTIONS, RECOVERY_SENTENCE};
use super::test_support::{eventually, where_am_i, Client, NoEmit, NoStepB, Ran, RecordingDb, Scratch};
use super::CONNECTIONS;
use crate::commands::settings::DbState;
use crate::error::AppError;
use crate::execution_backend::FetchPolicy;
use crate::in_flight::InFlight;
use crate::mcp::audit::audit_rows_conn;
use crate::mcp::cancel::{CancelJobResult, SUBMITTING_REFUSED};
use crate::mcp::create_job_test_support::NoSidecar;
use crate::mcp::steps::reconcile_submitting_conn;
use crate::mcp::test_db::{grant, TestDb};
use crate::mcp::tools::wait::{JOB_ID_MAX_BYTES, KNOWN_MAX};
use crate::mcp::tools::{ToolError, RESULT_CAP_BYTES};
use crate::models::job::{Job, JobStatus};
use crate::poller::live_log::LiveLog;
use crate::poller::plan::PollerMemory;
use crate::poller::remote_state::RemoteStates;
use crate::poller::{Poller, RecordingSink, SystemClock};
use crate::remote::ssh::{CommandRunner, ProcessOutput, TransportError};

const SOON: Duration = Duration::from_secs(10);

/// The three M3b tools.
const M3B_TOOLS: [&str; 3] = ["cancel_job", "create_group", "wait_for_jobs"];

/// No ssh or rsync here: every job is local.
struct NoSsh;

impl CommandRunner for NoSsh {
    fn run(&self, program: &str, _: &[String], _: &[u8], _: Duration) -> Result<ProcessOutput, TransportError> {
        panic!("no {program} call in the M3b tool tests");
    }
}

/// `cancel_job`'s world over the served database: a real `Poller` and a dispatch that records the job
/// and marks it `cancelled`, as a local backend's cancel of a queued job does.
struct ScratchCancel {
    db: Arc<DbState>,
    in_flight: InFlight,
    memory: PollerMemory,
    live: LiveLog,
    remote: RemoteStates,
    sink: RecordingSink,
    data_dir: PathBuf,
    dispatched: Mutex<Vec<String>>,
    /// Where each `run` ran: the tool must call it on the blocking pool (the cancel core takes
    /// `DbState`, a sync mutex, and may make ssh calls).
    ran: Mutex<Vec<Ran>>,
}

impl CancelWorld for ScratchCancel {
    fn run(&self, call: CancelCall<'_>) -> Result<CancelJobResult, ToolError> {
        self.ran.lock().unwrap().push(where_am_i());
        let poller = Poller {
            db: &self.db,
            runner: &NoSsh,
            in_flight: &self.in_flight,
            memory: &self.memory,
            live: &self.live,
            remote: &self.remote,
            sink: &self.sink,
            policy: FetchPolicy::SMALL_ONLY,
            data_dir: &self.data_dir,
            clock: &SystemClock,
        };
        call(&poller, &|job: Job| -> Result<(), AppError> {
            self.dispatched.lock().unwrap().push(job.id.clone());
            let conn = self.db.0.try_lock().map_err(|_| AppError::Internal("DbState held during the dispatch".into()))?;
            conn.execute("UPDATE jobs SET status = 'cancelled' WHERE id = ?1", [&job.id])?;
            Ok(())
        })
    }
}

struct Served {
    db: TestDb,
    cancel: Arc<ScratchCancel>,
    /// The service's `DbSource`, recording where each touch ran.
    recording: Arc<RecordingDb>,
    scratch: Scratch,
    lc: Arc<McpLifecycle>,
}

/// A served app over a scratch database with a grant, Claude's queued job `m1`, Anton's queued job
/// `u1`, a running job `r1`, `seed`'s rows, and (after the switch-on) Claude's charged draft in step
/// B `d2`.
fn served(tag: &str, seed: impl FnOnce(&TestDb)) -> Served {
    let db = TestDb::new(tag);
    let g = grant(&db.conn, 5, 2).id;
    db.job("m1", "queued", Some(&g));
    db.job("u1", "queued", None);
    db.job("r1", "running", None);
    seed(&db);
    let state = Arc::new(DbState(Mutex::new(db.open_another())));
    let recording = RecordingDb::new(state.clone());
    let cancel = Arc::new(ScratchCancel {
        db: state.clone(),
        in_flight: InFlight::default(),
        memory: PollerMemory::new(),
        live: LiveLog::new(),
        remote: RemoteStates::new(),
        sink: RecordingSink::default(),
        data_dir: db.dir.clone(),
        dispatched: Mutex::new(Vec::new()),
        ran: Mutex::new(Vec::new()),
    });
    let scratch = Scratch::new(tag);
    let env = LifecycleEnv {
        paths: Ok(scratch.paths()),
        db: recording.clone(),
        host: Arc::new(HostPaths { meminfo: db.dir.join("meminfo"), data_dir: db.dir.clone() }),
        reconcile: Box::new(reconcile_submitting_conn),
        peer_uid: kernel_peer_uid(),
        own_uid: own_uid(),
        hooks: Hooks::default(),
        create_job: CreateJobSeams {
            smiles: Arc::new(NoSidecar),
            step_b: Arc::new(NoStepB),
            emit: Arc::new(NoEmit),
            cancel: cancel.clone(),
        },
    };
    let lc = McpLifecycle::new(env);
    let status = lc.set_enabled(true).unwrap();
    assert!(status.serving, "{status:?}");
    // Created after the switch-on's reconciliation, which would refund it: a charged draft in step B
    // exists only while a `create_job` task runs.
    db.job("d2", "draft", Some(&g));
    db.conn.execute("UPDATE jobs SET mcp_state = 'submitting' WHERE id = 'd2'", []).unwrap();
    db.conn.execute("UPDATE mcp_grants SET charged_jobs = 1 WHERE id = ?1", [&g]).unwrap();
    recording.take();
    Served { db, cancel, recording, scratch, lc }
}

impl Served {
    fn client(&self) -> Client {
        let mut c = Client::connect(&self.scratch.paths().socket);
        c.initialize(1);
        c
    }

    fn status(&self, id: &str) -> String {
        self.db.conn.query_row("SELECT status FROM jobs WHERE id = ?1", [id], |r| r.get(0)).unwrap()
    }

    fn dispatched(&self) -> Vec<String> {
        self.cancel.dispatched.lock().unwrap().clone()
    }

    fn live_connection(&self) -> Arc<ConnState> {
        let live = || self.lc.connections().into_iter().filter(|c| !c.is_closed()).collect::<Vec<_>>();
        assert!(eventually(SOON, || live().len() == 1));
        live().remove(0)
    }
}

fn json_of(text: &str) -> Value {
    serde_json::from_str(text).unwrap_or_else(|e| panic!("not JSON ({e}): {text}"))
}

fn tools(c: &mut Client) -> Vec<Value> {
    c.request(2, "tools/list", json!({}));
    c.response(2).unwrap()["result"]["tools"].as_array().unwrap().clone()
}

// ----- registration -----

/// The three tools are listed; each description carries the recovery sentence and says what the brief
/// requires (`wait_for_jobs`: a mistyped id is answered at once as "gone", `timeout_s` capped at 600,
/// the 2 s tick); each schema refuses unknown properties and carries the core's bounds. Tags
/// `REGISTERED-M3B`, `RECOVERY-M3B`.
#[test]
fn the_m3b_tools_are_listed_with_their_descriptions_and_schemas() {
    let s = served("m3b-list", |_| {});
    let mut c = s.client();
    let tools = tools(&mut c);
    let tool = |name: &str| tools.iter().find(|t| t["name"] == name).cloned();
    for name in M3B_TOOLS {
        let t = tool(name);
        assert!(t.is_some(), "REGISTERED-M3B: {name} is not listed");
        let t = t.unwrap();
        let description = t["description"].as_str().unwrap();
        assert!(description.contains(RECOVERY_SENTENCE), "RECOVERY-M3B: {name}'s description lacks the recovery sentence");
        assert!(description.len() <= 700, "{name}: {} bytes", description.len());
        assert_eq!(t["inputSchema"]["additionalProperties"], json!(false), "{name}: {}", t["inputSchema"]);
    }
    assert_eq!(tool("cancel_job").unwrap()["description"], CANCEL_JOB_DESCRIPTION);
    assert_eq!(tool("create_group").unwrap()["description"], CREATE_GROUP_DESCRIPTION);
    let wait = WAIT_FOR_JOBS_DESCRIPTION;
    assert_eq!(tool("wait_for_jobs").unwrap()["description"], wait);
    for says in ["answered at once as \"gone\"", "a mistyped one included", "capped at 600", "every 2 s", "answers at once"] {
        assert!(wait.contains(says), "wait_for_jobs' description says `{says}`: {wait}");
    }
    for says in ["origin mcp", "A job Anton made is refused", "audit log"] {
        assert!(CANCEL_JOB_DESCRIPTION.contains(says), "cancel_job's description says `{says}`");
    }

    let prop = |tool_name: &str, p: &str| tool(tool_name).unwrap()["inputSchema"]["properties"][p].clone();
    assert_eq!(prop("cancel_job", "job_id")["maxLength"], json!(64));
    assert_eq!(prop("create_group", "name")["maxLength"], json!(200));
    assert_eq!(prop("create_group", "parent_id")["maxLength"], json!(64));
    assert_eq!((prop("wait_for_jobs", "known")["minProperties"].clone(), prop("wait_for_jobs", "known")["maxProperties"].clone()), (json!(1), json!(50)));
    assert!(prop("wait_for_jobs", "timeout_s").get("maximum").is_none(), "a larger timeout is capped, not refused");
    let known = prop("wait_for_jobs", "known");
    assert_eq!(
        (known["propertyNames"]["minLength"].clone(), known["propertyNames"]["maxLength"].clone()),
        (json!(1), json!(64)),
        "SCHEMA-KNOWN: each id's bounds: {known}"
    );
    let statuses: Vec<&str> = JobStatus::ALL.iter().map(|s| s.as_str()).collect();
    assert_eq!(known["additionalProperties"]["enum"], json!(statuses), "SCHEMA-KNOWN: the values are the job statuses: {known}");
}

/// The server's instructions (the `initialize` result) name the M3b tools (the verifier's INFO-4).
#[test]
fn the_instructions_name_the_m3b_tools() {
    let s = served("m3b-instr", |_| {});
    let mut c = Client::connect(&s.scratch.paths().socket);
    let init = c.initialize(1);
    assert_eq!(init["instructions"], INSTRUCTIONS);
    for name in M3B_TOOLS {
        assert!(INSTRUCTIONS.contains(name), "the instructions name {name}: {INSTRUCTIONS}");
    }
}

/// Every M3b tool's answer reaches the client as `respond` writes it: compact JSON (see the read
/// tools' `every_tool_answers_through_respond`, which pins the ten read tools). Tag `RESPOND-M3B`.
#[test]
fn every_m3b_tool_answers_through_respond() {
    let s = served("m3b-respond", |_| {});
    let mut c = s.client();
    let calls = [
        ("cancel_job", json!({ "job_id": "m1" })),
        ("create_group", json!({ "name": "SN2 study" })),
        ("wait_for_jobs", json!({ "known": { "m1": "running" }, "timeout_s": 0 })),
    ];
    for (i, (name, args)) in calls.into_iter().enumerate() {
        let (text, is_error) = c.call_and_wait(10 + i as u64, name, args);
        assert!(!is_error, "{name}: {text}");
        let compact = serde_json::to_string(&json_of(&text)).unwrap();
        assert_eq!(text.len(), compact.len(), "RESPOND-M3B: {name} is not served as compact JSON: {}", &text[..text.len().min(120)]);
    }
}

// ----- cancel_job -----

/// Over the wire: Anton's job is refused and never dispatched; a draft in step B is refused with its
/// own reason; Claude's queued job is dispatched once, with its pre-row and outcome row. Tag
/// `ORIGIN-E2E`.
#[test]
fn cancel_job_refuses_antons_job_end_to_end_and_cancels_claudes() {
    let s = served("m3b-cancel", |_| {});
    let mut c = s.client();

    let (text, is_error) = c.call_and_wait(10, "cancel_job", json!({ "job_id": "u1" }));
    assert!(!is_error, "{text}");
    let refused = json_of(&text);
    assert_eq!(refused["outcome"], "refused", "ORIGIN-E2E: {text}");
    assert_eq!(s.dispatched(), Vec::<String>::new(), "ORIGIN-E2E: Anton's job was dispatched");
    assert_eq!(s.status("u1"), "queued", "ORIGIN-E2E: Anton's job was touched");

    let (text, _) = c.call_and_wait(11, "cancel_job", json!({ "job_id": "d2" }));
    let submitting = json_of(&text);
    assert_eq!((submitting["outcome"].clone(), submitting["reasons"][0]["message"].clone()), (json!("refused"), json!(SUBMITTING_REFUSED)));

    let (text, is_error) = c.call_and_wait(12, "cancel_job", json!({ "job_id": "m1" }));
    assert!(!is_error, "{text}");
    let done = json_of(&text);
    assert_eq!((done["outcome"].clone(), done["result"].clone(), done["status_before"].clone()), (json!("requested"), json!("dispatched"), json!("queued")));
    assert_eq!(s.dispatched(), vec!["m1".to_string()]);
    assert_eq!(s.status("m1"), "cancelled");

    let (text, is_error) = c.call_and_wait(13, "cancel_job", json!({ "job_id": "nope" }));
    assert!(is_error && text.contains("does not exist"), "{text}");

    let rows = audit_rows_conn(&s.db.conn).unwrap();
    let kinds: Vec<(Option<&str>, &str)> = rows.iter().map(|r| (r.disposition.as_deref(), r.reasons[0].row.as_str())).collect();
    assert_eq!(kinds, [(Some("refused"), "origin"), (Some("refused"), "submitting"), (None, "cancel"), (None, "cancel")]);
}

// ----- create_group -----

/// Over the wire: a group is created trimmed with its audit row; an unknown parent and a blank name
/// are tool errors with no row.
#[test]
fn create_group_creates_a_group_with_its_row() {
    let s = served("m3b-group", |_| {});
    let mut c = s.client();
    let (text, is_error) = c.call_and_wait(10, "create_group", json!({ "name": "  TS search  " }));
    assert!(!is_error, "{text}");
    let group = json_of(&text);
    assert_eq!(group["name"], "TS search");
    let (text, is_error) = c.call_and_wait(11, "create_group", json!({ "name": "child", "parent_id": group["group_id"] }));
    assert!(!is_error, "{text}");
    assert_eq!(json_of(&text)["parent_id"], group["group_id"]);
    let (text, is_error) = c.call_and_wait(12, "create_group", json!({ "name": "x", "parent_id": "no-such" }));
    assert!(is_error && text.contains("does not exist"), "{text}");
    let (text, is_error) = c.call_and_wait(13, "create_group", json!({ "name": "   " }));
    assert!(is_error && text.contains("empty"), "{text}");
    let rows = audit_rows_conn(&s.db.conn).unwrap();
    assert_eq!(rows.iter().map(|r| r.tool.as_str()).collect::<Vec<_>>(), ["create_group", "create_group"]);
}

// ----- wait_for_jobs -----

/// Over the wire: a differing status and a mistyped id are answered at once; `timeout_s` 0 checks
/// once; above 600 the result says it was capped (answered at once here, so no real wait).
#[test]
fn wait_for_jobs_answers_at_once_and_reports_the_cap() {
    let s = served("m3b-wait", |_| {});
    let mut c = s.client();
    let (text, is_error) = c.call_and_wait(10, "wait_for_jobs", json!({ "known": { "m1": "running", "m1-typo": "queued" } }));
    assert!(!is_error, "{text}");
    let r = json_of(&text);
    assert_eq!(r["timed_out"], false);
    assert_eq!(
        r["changed"],
        json!([
            { "job_id": "m1", "known_status": "running", "current_status": "queued" },
            { "job_id": "m1-typo", "known_status": "queued", "current_status": "gone" }
        ])
    );
    let (text, _) = c.call_and_wait(11, "wait_for_jobs", json!({ "known": { "m1": "queued" }, "timeout_s": 0 }));
    let once = json_of(&text);
    assert_eq!((once["timed_out"].clone(), once["changed"].clone(), once["waited_s"].clone()), (json!(true), json!([]), json!(0)));
    let (text, _) = c.call_and_wait(12, "wait_for_jobs", json!({ "known": { "r1": "queued" }, "timeout_s": 5000 }));
    let capped = json_of(&text);
    assert_eq!((capped["effective_timeout_s"].clone(), capped["timeout_clamped"].clone()), (json!(600), json!(true)));
    let (text, is_error) = c.call_and_wait(13, "wait_for_jobs", json!({ "known": {} }));
    assert!(is_error && text.contains("`known` is empty"), "{text}");
}

/// A `wait_for_jobs` ends when its client hangs up, and its permit returns: four waits fill one
/// connection's permits; the client hangs up; within a tick the waits end, every permit returns, and
/// the connection's slot frees, so with the other three slots taken a new client is still served.
/// Tag `WAIT-CLOSED-PERMIT`.
#[test]
fn a_wait_ends_when_its_client_hangs_up_and_its_permit_returns() {
    let s = served("m3b-closed", |_| {});
    let mut waiter = s.client();
    let conn = s.live_connection();
    for id in 10..14 {
        waiter.call(id, "wait_for_jobs", json!({ "known": { "r1": "running" }, "timeout_s": 600 }));
    }
    assert!(eventually(SOON, || conn.in_flight() == 4), "four waits hold the four permits");
    let mut others: Vec<Client> = (0..CONNECTIONS - 1).map(|_| s.client()).collect();
    drop(waiter); // the client hangs up

    // `closed` at the read EOF, the waits' next tick (2 s), then rmcp's response drain (up to 5 s).
    assert!(
        eventually(Duration::from_secs(15), || conn.in_flight() == 0),
        "WAIT-CLOSED-PERMIT: the waits still hold {} permits after the client hung up",
        conn.in_flight()
    );
    let served_again = eventually(Duration::from_secs(15), || {
        let mut c = Client::connect(&s.scratch.paths().socket);
        c.set_read_timeout(Duration::from_millis(500));
        c.request(1, "initialize", json!({ "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": { "name": "t", "version": "0" } }));
        c.response(1).is_some_and(|r| r.get("result").is_some())
    });
    assert!(served_again, "WAIT-CLOSED-PERMIT: the waiter's slot never freed");
    for o in others.iter_mut() {
        o.request(2, "tools/list", json!({}));
        assert!(o.response(2).is_some(), "the other connections still work");
    }
}

// ----- size, as the client receives it -----

/// The largest answer of each tool, read through the socket, stays under the cap: a wait naming
/// [`KNOWN_MAX`] jobs whose ids are [`JOB_ID_MAX_BYTES`] control characters (6 bytes each once
/// escaped), all gone; a group name of 200 four-byte characters; a cancel of a job whose id is
/// [`JOB_ID_MAX_BYTES`] long (dispatched; the id is echoed twice, in `job_id` and in the bounded
/// `detail`). Tag `CAP-M3B`.
#[test]
fn the_largest_answer_of_each_m3b_tool_fits_as_the_client_receives_it() {
    let s = served("m3b-cap", |db| {
        let g: String = db.conn.query_row("SELECT id FROM mcp_grants", [], |r| r.get(0)).unwrap();
        db.job(&"q".repeat(JOB_ID_MAX_BYTES), "running", Some(&g));
    });
    let mut c = s.client();

    let known: serde_json::Map<String, Value> = (0..KNOWN_MAX)
        .map(|i| {
            let id: String = std::iter::once(char::from(b'A' + (i % 26) as u8))
                .chain(std::iter::once(char::from(b'a' + (i / 26) as u8)))
                .chain(std::iter::repeat('\u{1}').take(JOB_ID_MAX_BYTES - 2))
                .collect();
            (id, json!("cancelling"))
        })
        .collect();
    let (text, is_error) = c.call_and_wait(10, "wait_for_jobs", json!({ "known": known, "timeout_s": 0 }));
    assert!(!is_error, "CAP-M3B: wait_for_jobs at its bounds was not served: {}", &text[..text.len().min(300)]);
    assert!(text.len() <= RESULT_CAP_BYTES, "CAP-M3B: wait_for_jobs: {} bytes", text.len());
    assert_eq!(json_of(&text)["changed"].as_array().unwrap().len(), KNOWN_MAX);

    let (text, is_error) = c.call_and_wait(11, "create_group", json!({ "name": "𝄞".repeat(200) }));
    assert!(!is_error && text.len() <= RESULT_CAP_BYTES, "CAP-M3B: create_group: {text}");

    // A running job with the longest id, dispatched by the scratch world: the answer's texts are bounded.
    let (text, is_error) = c.call_and_wait(12, "cancel_job", json!({ "job_id": "q".repeat(JOB_ID_MAX_BYTES) }));
    assert!(!is_error && text.len() <= RESULT_CAP_BYTES, "CAP-M3B: cancel_job: {text}");
}

// ----- threads -----

/// Every M3b tool's work runs on Tauri's blocking pool, never on an async worker of the runtime:
/// `cancel_job`'s world (the cancel core takes `DbState`, a sync mutex, and may make ssh calls),
/// `create_group`'s database work, and every read of `wait_for_jobs`' reader. Tags `V-BLOCKING`,
/// `V-WITHDB`, `V-WAIT-READ` (the verifier's MED-1, M3b Part B).
#[test]
fn every_m3b_tools_work_runs_on_the_blocking_pool() {
    let s = served("m3b-threads", |_| {});
    let mut c = s.client();
    s.recording.take();

    let (text, is_error) = c.call_and_wait(10, "cancel_job", json!({ "job_id": "m1" }));
    assert!(!is_error, "{text}");
    let ran = s.cancel.ran.lock().unwrap().clone();
    assert_eq!(ran, vec![Ran::BlockingPool], "V-BLOCKING: cancel_job's world ran off the blocking pool: {ran:?}");

    let (text, is_error) = c.call_and_wait(11, "create_group", json!({ "name": "threads" }));
    assert!(!is_error, "{text}");
    let touches = s.recording.take();
    assert!(!touches.is_empty(), "create_group reached the database");
    assert!(touches.iter().all(|t| t.ran == Ran::BlockingPool), "V-WITHDB: create_group touched DbState off the blocking pool: {touches:?}");

    let (text, is_error) = c.call_and_wait(12, "wait_for_jobs", json!({ "known": { "r1": "running" }, "timeout_s": 0 }));
    assert!(!is_error, "{text}");
    let touches = s.recording.take();
    assert!(!touches.is_empty(), "wait_for_jobs read the database");
    assert!(touches.iter().all(|t| t.ran == Ran::BlockingPool), "V-WAIT-READ: wait_for_jobs read DbState off the blocking pool: {touches:?}");
}
