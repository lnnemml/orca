//! `create_job` over the wire (ADR-027 decision 7; M0 amendment, item 5; M3a Part B): registered in
//! the `rmcp` service, served on `tauri::async_runtime` over the real socket and called by a raw
//! NDJSON client, so every assertion is on what the model would receive. The database is the
//! `create_job` test world (a grant on `uni`, a converged source job); step B is injected.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use super::create_job_task::CreateJobSeams;
use super::create_job_tool::CREATE_JOB_DESCRIPTION;
use super::lifecycle::{Hooks, LifecycleEnv, McpLifecycle};
use super::listener::{kernel_peer_uid, own_uid};
use super::service::{HostPaths, BUSY, RECOVERY_SENTENCE};
use super::test_support::{eventually, tool_text, Client, Scratch};
use super::REQUESTS_PER_CONNECTION;
use crate::commands::settings::DbState;
use crate::mcp::classify::Target;
use crate::mcp::create_job::{StepB, StepBOutcome, HEADER_MAX_BYTES, SOURCE_MAX_BYTES, TARGET_MAX_BYTES};
use crate::mcp::create_job_test_support::{NoSidecar, World, HEADER, PROFILE};
use crate::mcp::steps::reconcile_submitting_conn;

const SOON: Duration = Duration::from_secs(10);

/// A step B that holds every call until released (at most 30 s, so a failing test cannot hang), then
/// hands the job over (`queued`). Counts the calls that reached it.
struct HeldStepB {
    db: Arc<DbState>,
    entered: AtomicUsize,
    open: Mutex<bool>,
    cv: Condvar,
}

impl HeldStepB {
    fn new(db: Arc<DbState>, open: bool) -> Arc<HeldStepB> {
        Arc::new(HeldStepB { db, entered: AtomicUsize::new(0), open: Mutex::new(open), cv: Condvar::new() })
    }

    fn release(&self) {
        *self.open.lock().unwrap() = true;
        self.cv.notify_all();
    }

    fn entered(&self) -> usize {
        self.entered.load(Ordering::SeqCst)
    }
}

impl StepB for HeldStepB {
    fn submit(&self, job_id: &str, _: &Target) -> StepBOutcome {
        self.entered.fetch_add(1, Ordering::SeqCst);
        let deadline = Instant::now() + Duration::from_secs(30);
        let mut open = self.open.lock().unwrap();
        while !*open && Instant::now() < deadline {
            open = self.cv.wait_timeout(open, Duration::from_millis(100)).unwrap().0;
        }
        drop(open);
        self.db.lock().unwrap().execute("UPDATE jobs SET status = 'queued' WHERE id = ?1", [job_id]).unwrap();
        StepBOutcome::Handed("queued".into())
    }
}

struct Served {
    w: World,
    scratch: Scratch,
    lc: Arc<McpLifecycle>,
}

impl Served {
    fn client(&self) -> Client {
        let mut c = Client::connect(&self.scratch.paths().socket);
        c.initialize(1);
        c
    }
}

/// A served app over the `create_job` world (`max_in_flight` as given), with the step B `step_b`
/// builds over the world's `DbState`.
fn served<S: StepB + 'static>(tag: &str, max_in_flight: u32, step_b: impl FnOnce(&Arc<DbState>) -> Arc<S>) -> Served {
    let w = World::new(tag, max_in_flight);
    let step_b: Arc<dyn StepB> = step_b(&w.state);
    let scratch = Scratch::new(tag);
    let env = LifecycleEnv {
        paths: Ok(scratch.paths()),
        db: w.state.clone(),
        host: Arc::new(HostPaths { meminfo: w.db.dir.join("meminfo"), data_dir: w.db.dir.clone() }),
        reconcile: Box::new(reconcile_submitting_conn),
        peer_uid: kernel_peer_uid(),
        own_uid: own_uid(),
        hooks: Hooks::default(),
        create_job: CreateJobSeams { smiles: Arc::new(NoSidecar), step_b, emit: w.emit.clone(), cancel: Arc::new(super::test_support::NoCancel) },
    };
    let lc = McpLifecycle::new(env);
    assert!(lc.set_enabled(true).unwrap().serving);
    Served { w, scratch, lc }
}

fn args() -> Value {
    json!({ "header": HEADER, "source": "job:opt", "target": PROFILE })
}

fn tools(c: &mut Client) -> Vec<Value> {
    c.request(2, "tools/list", json!({}));
    c.response(2).unwrap()["result"]["tools"].as_array().unwrap().clone()
}

/// `create_job` is listed; its description carries the recovery sentence, stays ≤ 700 bytes and says
/// what the brief requires; its schema refuses unknown properties and carries the core's bounds.
/// NEGATIVE CONTROLS: the router left out of the service (`REGISTERED`), the description without the
/// sentence (`RECOVERY`) → red.
#[test]
fn create_job_is_listed_with_its_description_and_schema() {
    let s = served("cj-list", 1, |db| HeldStepB::new(db.clone(), true));
    let mut c = s.client();
    let tools = tools(&mut c);
    let tool = tools.iter().find(|t| t["name"] == "create_job");
    assert!(tool.is_some(), "REGISTERED: create_job is not listed: {:?}", tools.iter().map(|t| t["name"].clone()).collect::<Vec<_>>());
    let tool = tool.unwrap();
    let description = tool["description"].as_str().unwrap();
    assert!(description.contains(RECOVERY_SENTENCE), "RECOVERY: create_job's description lacks the recovery sentence: {description}");
    assert_eq!(description, CREATE_JOB_DESCRIPTION);
    assert!(description.len() <= 700, "{} bytes", description.len());
    for says in ["assembles the input from the source", "No coordinates are accepted", "SUBMITTED", "PROPOSED", "REFUSED", "with its reasons", "A grant Anton made governs spending"] {
        assert!(description.contains(says), "the description says `{says}`: {description}");
    }
    let schema = &tool["inputSchema"];
    assert_eq!(schema["additionalProperties"], json!(false), "{schema}");
    let max = |p: &str| schema["properties"][p]["maxLength"].as_u64().map(|n| n as usize);
    assert_eq!(
        (max("header"), max("source"), max("target")),
        (Some(HEADER_MAX_BYTES), Some(SOURCE_MAX_BYTES), Some(TARGET_MAX_BYTES)),
        "{schema}"
    );
    let mut required: Vec<&str> = schema["required"].as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect();
    required.sort();
    assert_eq!(required, ["header", "source", "target"]);
}

/// A SUBMITTED call end to end: the result is the core's value as compact JSON (disposition, job id,
/// what the app set, steps B and C), and the job is queued, charged and audited.
#[test]
fn a_submitted_call_returns_its_result_over_the_wire() {
    let s = served("cj-wire", 1, |db| HeldStepB::new(db.clone(), true));
    let mut c = s.client();
    let (text, is_error) = c.call_and_wait(10, "create_job", args());
    assert!(!is_error, "{text}");
    let r: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(r["disposition"], "submitted", "{r}");
    assert!(r["job_id"].as_str().is_some_and(|id| id.len() == 36), "{r}");
    assert_eq!(r["set_by_app"]["charge_mult_from"], "job:opt");
    assert_eq!((r["submit"]["step_b"].clone(), r["submit"]["job_status"].clone()), (json!("queued"), json!("queued")));
    assert_eq!(s.w.mcp_jobs()[0].1, "queued");
    assert_eq!(s.w.charged(), 1);
    assert_eq!(s.w.audit().len(), 2, "the SUBMITTED row and step C's outcome");
    assert!(!text.contains('\n') && !text.contains("  "), "compact JSON");
}

/// An unknown argument — a model passing coordinates — is a tool error, and nothing is written.
#[test]
fn coordinates_are_not_an_argument() {
    let s = served("cj-unknown", 1, |db| HeldStepB::new(db.clone(), true));
    let mut c = s.client();
    let mut a = args();
    a["coordinates"] = json!("O 0 0 0");
    let (text, is_error) = c.call_and_wait(10, "create_job", a);
    assert!(is_error && text.contains("unknown field `coordinates`"), "{text}");
    assert!(s.w.mcp_jobs().is_empty() && s.w.audit().is_empty());
}

/// Control (M0 amendment, item 5; M3a Part B): `call_tool` taking a second permit for `create_job`.
/// Four concurrent `create_job` calls on one connection each hold exactly one permit, inside their
/// tasks: all four reach step B, a fifth request is refused busy at once, and every permit returns
/// once step C has run. NEGATIVE CONTROL: `call_tool` takes its generic permit for `create_job` too →
/// only two calls reach step B (the third and fourth are refused busy) → red.
#[test]
fn a_create_job_call_holds_one_permit_not_two() {
    let held: Arc<Mutex<Option<Arc<HeldStepB>>>> = Arc::default();
    let keep = held.clone();
    let s = served("cj-permit", REQUESTS_PER_CONNECTION as u32, move |db| {
        let step_b = HeldStepB::new(db.clone(), false);
        *keep.lock().unwrap() = Some(step_b.clone());
        step_b
    });
    let held = held.lock().unwrap().clone().unwrap();
    let mut c = s.client();
    let conn = s.lc.connections().into_iter().next().expect("the connection");

    for id in 10..10 + REQUESTS_PER_CONNECTION as u64 {
        c.call(id, "create_job", args());
    }
    let all_reached = eventually(SOON, || held.entered() == REQUESTS_PER_CONNECTION);
    let reached = held.entered();
    let busy = if all_reached {
        c.set_read_timeout(Duration::from_secs(3));
        c.call_get_grant(99);
        c.response(99).map(|r| tool_text(&r))
    } else {
        None
    };
    held.release();
    c.set_read_timeout(SOON);
    let results: Vec<(String, bool)> =
        (10..10 + REQUESTS_PER_CONNECTION as u64).map(|id| tool_text(&c.response(id).expect("a reply"))).collect();
    assert!(
        all_reached,
        "PERMIT-ONCE: only {reached} of {REQUESTS_PER_CONNECTION} concurrent create_job calls reached step B: {results:?}"
    );
    assert_eq!(busy, Some((BUSY.to_string(), true)), "a fifth request is refused busy while four create_job calls run");
    for (text, is_error) in &results {
        assert!(!is_error && text.contains("\"disposition\":\"submitted\""), "{text}");
    }
    assert!(eventually(SOON, || conn.in_flight() == 0), "every permit returned");
    assert_eq!(s.w.charged(), REQUESTS_PER_CONNECTION as i64);
}
