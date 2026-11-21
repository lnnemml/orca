//! The ten read tools end to end (ADR-027 M2b Part B): registered in the `rmcp` service, served on
//! `tauri::async_runtime` over the real socket, and read by a raw NDJSON client, so every assertion
//! is on what the model would receive. The database is a scratch one seeded with the Part A
//! fixtures (`tools::test_support`); the service reaches it through a recording `DbSource`.

use std::os::unix::fs::OpenOptionsExt;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};

use super::lifecycle::{Hooks, LifecycleEnv, McpLifecycle};
use super::listener::{kernel_peer_uid, own_uid};
use super::read_tools::{error_text, output_request, respond, served_text, ReadOutputArgs};
use super::service::{bounded_error_text, ConnState, HostPaths, ARGUMENT_ERROR_PREFIX, BUSY, RECOVERY_SENTENCE};
use super::test_support::{eventually, tool_text, Client, Scratch};
use super::{DbSource, REQUESTS_PER_CONNECTION};
use crate::commands::settings::DbState;
use crate::mcp::steps::reconcile_submitting_conn;
use crate::mcp::test_db::TestDb;
use crate::mcp::tools::output::OutputRequest;
use crate::mcp::tools::test_support::{
    grid_molecule, index_manual, largest_get_job, largest_groups, largest_list_jobs, largest_manual, largest_profiles,
    largest_search_lines, largest_tail_lines, molecule, output_job, results_json, store_results, JobRow, WATER_COORDS, WATER_INPUT,
};
use crate::mcp::tools::{ToolError, ECHO_MAX_BYTES, REFUSAL_MAX_BYTES, RESULT_CAP_BYTES};

const SOON: Duration = Duration::from_secs(5);

/// The ten read tools of decision 7.
const READ_TOOLS: [&str; 10] = [
    "list_jobs",
    "get_job",
    "get_results",
    "get_geometry",
    "measure",
    "read_output",
    "search_manual",
    "list_run_targets",
    "memory_estimate",
    "list_groups",
];

const MEMINFO: &str = "MemTotal:       15988408 kB\nMemFree:         2683044 kB\nMemAvailable:    9000000 kB\n";
const WATER_XYZ: &str = "3\nwater\nO 0.0 0.0 0.117790\nH 0.0 0.755453 -0.471161\nH 0.0 -0.755453 -0.471161\n";

// ----- the recording database -----

/// Where a database touch ran.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ran {
    /// An async worker of the runtime: `Handle::block_on` panics there.
    AsyncWorker,
    /// Tauri's blocking pool: inside the runtime, where `Handle::block_on` is allowed.
    BlockingPool,
    /// Outside the runtime (the test thread, the lifecycle's switch-on).
    Outside,
}

fn where_am_i() -> Ran {
    match tokio::runtime::Handle::try_current() {
        Err(_) => Ran::Outside,
        Ok(handle) => match catch_unwind(AssertUnwindSafe(|| handle.block_on(async {}))) {
            Ok(()) => Ran::BlockingPool,
            Err(_) => Ran::AsyncWorker,
        },
    }
}

#[derive(Debug, Clone, Copy)]
struct Touch {
    ran: Ran,
    /// `DbState` was locked (by anyone) when the service reached for it.
    lock_held: bool,
}

/// The service's `DbSource`, recording each touch's thread and whether the lock was held.
struct RecordingDb {
    db: Arc<DbState>,
    touches: Mutex<Vec<Touch>>,
}

impl DbSource for RecordingDb {
    fn db(&self) -> &DbState {
        let lock_held = self.db.0.try_lock().is_err();
        self.touches.lock().unwrap().push(Touch { ran: where_am_i(), lock_held });
        &self.db
    }
}

impl RecordingDb {
    fn take(&self) -> Vec<Touch> {
        std::mem::take(&mut *self.touches.lock().unwrap())
    }
}

// ----- the served app -----

struct Served {
    /// The seeding connection and the data dir (`meminfo`, `jobs/`).
    db: TestDb,
    /// The service's own `DbState`, another connection on the same file.
    state: Arc<DbState>,
    recording: Arc<RecordingDb>,
    scratch: Scratch,
    lc: Arc<McpLifecycle>,
}

/// A served app over a fresh database (seeded by `seed` before the server starts).
fn served(tag: &str, seed: impl FnOnce(&TestDb)) -> Served {
    let db = TestDb::new(tag);
    std::fs::write(db.dir.join("meminfo"), MEMINFO).unwrap();
    seed(&db);
    let state = Arc::new(DbState(Mutex::new(db.open_another())));
    let recording = Arc::new(RecordingDb { db: state.clone(), touches: Mutex::new(Vec::new()) });
    let scratch = Scratch::new(tag);
    let env = LifecycleEnv {
        paths: Ok(scratch.paths()),
        db: recording.clone(),
        host: Arc::new(HostPaths { meminfo: db.dir.join("meminfo"), data_dir: db.dir.clone() }),
        reconcile: Box::new(reconcile_submitting_conn),
        peer_uid: kernel_peer_uid(),
        own_uid: own_uid(),
        hooks: Hooks::default(),
    };
    let lc = McpLifecycle::new(env);
    assert!(lc.set_enabled(true).unwrap().serving);
    recording.take();
    Served { db, state, recording, scratch, lc }
}

impl Served {
    fn client(&self) -> Client {
        let mut c = Client::connect(&self.scratch.paths().socket);
        c.initialize(1);
        c
    }

    fn live_connection(&self) -> Arc<ConnState> {
        let live = || self.lc.connections().into_iter().filter(|c| !c.is_closed()).collect::<Vec<_>>();
        assert!(eventually(SOON, || live().len() == 1));
        live().remove(0)
    }
}

/// A small world in which every tool has something to read.
fn small_world(db: &TestDb) {
    JobRow { input: WATER_INPUT, energy: Some(-76.4089), ..JobRow::new("w") }.insert(&db.conn);
    JobRow::new("r").insert(&db.conn);
    store_results(&db.conn, "r", &results_json(3, 2, 1, 1, "Å"));
    molecule(&db.conn, "water", WATER_XYZ, 0, 1);
    output_job(&db.conn, &db.dir, "o", (0..5).map(|i| format!("line {i}")));
    index_manual(&db.conn, &[("contents/RI".into(), "RIJCOSX".into(), vec!["Essential".into()], "The RIJCOSX approximation.".into())]);
    db.conn.execute("INSERT INTO groups (id, name) VALUES ('g', 'study')", []).unwrap();
}

/// One valid call of each tool on [`small_world`].
fn small_calls() -> Vec<(&'static str, Value)> {
    vec![
        ("list_jobs", json!({})),
        ("get_job", json!({ "job_id": "w" })),
        ("get_results", json!({ "job_id": "r" })),
        ("get_geometry", json!({ "source": "molecule:water" })),
        ("measure", json!({ "source": "molecule:water", "atom_indices": [1, 0, 2] })),
        ("read_output", json!({ "job_id": "o", "lines": 3 })),
        ("search_manual", json!({ "query": "RIJCOSX" })),
        ("list_run_targets", json!({})),
        ("memory_estimate", json!({ "header": "! wB97X-3c Opt\n", "target": "local" })),
        ("list_groups", json!({})),
    ]
}

fn json_of(text: &str) -> Value {
    serde_json::from_str(text).unwrap_or_else(|e| panic!("not JSON ({e}): {text}"))
}

/// The longest run of `c` in `text`.
fn longest_run(text: &str, c: char) -> usize {
    text.split(|x| x != c).map(|run| run.chars().count()).max().unwrap_or(0)
}

// ----- registration -----

/// `tools/list` lists `get_grant` and exactly the ten read tools; each description carries the
/// recovery sentence, and the descriptions state what the brief requires. NEGATIVE CONTROLS: a tool
/// left out of the registration (`ctl:registered`) or a description without the sentence
/// (`ctl:recovery`) → red.
#[test]
fn the_ten_read_tools_are_listed_with_the_recovery_sentence() {
    let s = served("rt-list", |_| {});
    let mut c = s.client();
    c.request(2, "tools/list", json!({}));
    let tools = c.response(2).unwrap()["result"]["tools"].as_array().unwrap().clone();
    let mut names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    names.sort();
    let mut expected: Vec<&str> = READ_TOOLS.iter().copied().chain(["get_grant"]).collect();
    expected.sort();
    assert_eq!(names, expected, "[ctl:registered]");

    let description = |name: &str| -> String {
        tools.iter().find(|t| t["name"] == name).unwrap()["description"].as_str().unwrap().to_string()
    };
    for name in READ_TOOLS {
        assert!(description(name).contains(RECOVERY_SENTENCE), "[ctl:recovery] {name}");
        assert!(description(name).len() < 700, "{name}: keep the description concise");
    }
    for name in ["measure", "get_geometry"] {
        assert!(description(name).contains("0-based"), "{name} says indices are 0-based");
    }
    let get_job = description("get_job");
    assert!(get_job.contains("rebuilt from the parsed input") && get_job.contains("values outside the display allowlist are not shown"));
    assert!(description("read_output").contains("bounded tail or search"));
    assert!(description("measure").contains("(Å)") && description("get_results").contains("(Eh)"));
}

/// Every read tool's input schema refuses unknown properties, and an unknown argument is a tool
/// error, never ignored. NEGATIVE CONTROL (`ctl:deny_unknown`): `list_jobs`' arguments without
/// `deny_unknown_fields` → red.
#[test]
fn unknown_arguments_are_refused() {
    let s = served("rt-unknown", small_world);
    let mut c = s.client();
    c.request(2, "tools/list", json!({}));
    let tools = c.response(2).unwrap()["result"]["tools"].as_array().unwrap().clone();
    for name in READ_TOOLS {
        let tool = tools.iter().find(|t| t["name"] == name).unwrap();
        assert_eq!(tool["inputSchema"]["additionalProperties"], json!(false), "[ctl:deny_unknown] {name}: {}", tool["inputSchema"]);
    }
    for (i, (name, mut args)) in small_calls().into_iter().enumerate() {
        args["no_such_argument"] = json!(1);
        let (text, is_error) = c.call_and_wait(100 + i as u64, name, args);
        assert!(is_error && text.contains("unknown field `no_such_argument`"), "[ctl:deny_unknown] {name}: {text}");
    }
}

/// The schema carries the cores' bounds.
#[test]
fn the_schemas_bound_strings_and_integers() {
    let s = served("rt-schema", |_| {});
    let mut c = s.client();
    c.request(2, "tools/list", json!({}));
    let tools = c.response(2).unwrap()["result"]["tools"].as_array().unwrap().clone();
    let prop = |tool: &str, p: &str| tools.iter().find(|t| t["name"] == tool).unwrap()["inputSchema"]["properties"][p].clone();
    assert_eq!(prop("list_jobs", "limit")["maximum"], json!(40));
    assert_eq!((prop("measure", "atom_indices")["minItems"].clone(), prop("measure", "atom_indices")["maxItems"].clone()), (json!(2), json!(4)));
    assert_eq!(prop("read_output", "query")["maxLength"], json!(200));
    assert_eq!(prop("read_output", "lines")["maximum"], json!(400));
    assert_eq!(prop("search_manual", "query")["maxLength"], json!(500));
    assert_eq!(prop("memory_estimate", "header")["maxLength"], json!(20000));
    assert_eq!(prop("list_groups", "limit")["maximum"], json!(100));
}

// ----- size, as the client receives it -----

/// The largest realistic result of every tool (the Part A fixtures that drive each core to its cap),
/// read through the socket, is at most 40,000 UTF-8 bytes and says that it cut or paged; a geometry
/// above 600 atoms comes back as a bounded tool error.
#[test]
fn the_largest_result_of_every_tool_fits_as_the_client_receives_it() {
    let s = served("rt-largest", |db| {
        largest_get_job(&db.conn, "big");
        largest_list_jobs(&db.conn);
        JobRow::new("res").insert(&db.conn);
        store_results(&db.conn, "res", &results_json(5_000, 5_000, 3_000, 100, "Å"));
        grid_molecule(&db.conn, "at", 600);
        grid_molecule(&db.conn, "over", 601);
        output_job(&db.conn, &db.dir, "wide", largest_tail_lines());
        output_job(&db.conn, &db.dir, "hits", largest_search_lines());
        largest_manual(&db.conn);
        largest_profiles(&db.conn);
        largest_groups(&db.conn);
    });
    let mut c = s.client();
    let calls: Vec<(&str, Value, &str)> = vec![
        ("list_jobs", json!({ "limit": 40 }), "/jobs/39/title_truncated"),
        ("get_job", json!({ "job_id": "big" }), "/header_truncated"),
        ("get_results", json!({ "job_id": "res" }), "/scan/points_truncated"),
        ("get_geometry", json!({ "source": "molecule:at" }), ""),
        ("measure", json!({ "source": "molecule:over", "atom_indices": [0, 300, 600] }), ""),
        ("read_output", json!({ "job_id": "wide", "lines": 400 }), "/truncated"),
        ("read_output", json!({ "job_id": "hits", "query": "HIT", "context_lines": 3, "limit": 50 }), ""),
        ("search_manual", json!({ "query": "basis", "limit": 20 }), "/more_may_exist"),
        ("list_run_targets", json!({}), "/profiles_truncated"),
        ("memory_estimate", json!({ "header": "! wB97X-3c CPCM(DMF) Opt Freq\n%pal nprocs 8 end\n", "target": "local" }), ""),
        ("list_groups", json!({ "limit": 100 }), ""),
    ];
    let mut seen: Vec<&str> = Vec::new();
    for (i, (name, args, flag)) in calls.into_iter().enumerate() {
        let (text, is_error) = c.call_and_wait(10 + i as u64, name, args);
        assert!(!is_error, "{name}: {text}");
        assert!(text.len() <= RESULT_CAP_BYTES, "{name}: {} bytes reach the client", text.len());
        if !flag.is_empty() {
            assert_eq!(json_of(&text).pointer(flag), Some(&json!(true)), "{name}: {flag} reaches the client");
        }
        seen.push(name);
    }
    seen.dedup();
    assert_eq!(seen, READ_TOOLS);
    let next = json_of(&c.call_and_wait(40, "list_groups", json!({ "limit": 100 })).0);
    assert_eq!(next["next_offset"], json!(100), "the page says where the next one starts");

    let (refused, is_error) = c.call_and_wait(41, "get_geometry", json!({ "source": "molecule:over" }));
    assert!(is_error && refused.contains("601 atoms") && refused.contains("measure"), "{refused}");
    assert!(refused.len() <= REFUSAL_MAX_BYTES);
}

// ----- the coordinate guard, as the client receives it -----

const ECHO_ROW: &str = "4.10220628";

/// A failed job whose title is a coordinate row and whose error ends in ORCA's echo of an input row:
/// `get_job` and `list_jobs` withhold exactly those fields, name them, and serve the rest; no
/// coordinate reaches the client. NEGATIVE CONTROL (`ctl:withheld_dropped`): the mapping drops
/// `withheld_fields` → red.
#[test]
fn get_job_and_list_jobs_withhold_coordinate_rows_end_to_end() {
    let error = format!("ORCA finished by error termination in SCF\n|  6> C     {ECHO_ROW}   -0.56008055    0.58413357\n");
    let s = served("rt-withheld", |db| {
        JobRow { title: "O 0.000000 0.000000 0.117790", input: WATER_INPUT, status: "failed", error: Some(&error), ..JobRow::new("leak") }
            .insert(&db.conn);
    });
    let mut c = s.client();
    let (text, is_error) = c.call_and_wait(2, "get_job", json!({ "job_id": "leak" }));
    assert!(!is_error, "{text}");
    let view = json_of(&text);
    assert_eq!(view["withheld_fields"], json!(["title", "error"]), "[ctl:withheld_dropped] {text}");
    let header = view["header"].as_str().unwrap_or_default();
    assert!(header.starts_with("! B3LYP def2-SVP Opt\n%maxcore 2000") && header.contains("nprocs 4"), "the rest is served: {header}");
    assert_eq!(view["atom_count"], json!(3));
    for secret in WATER_COORDS.iter().chain([&ECHO_ROW]) {
        assert!(!text.contains(secret), "get_job carries {secret}: {text}");
    }

    let (text, is_error) = c.call_and_wait(3, "list_jobs", json!({}));
    assert!(!is_error, "{text}");
    let row = &json_of(&text)["jobs"][0];
    assert_eq!(row["withheld_fields"], json!(["title", "error_excerpt"]), "[ctl:withheld_dropped] {text}");
    for secret in WATER_COORDS.iter().chain([&ECHO_ROW]) {
        assert!(!text.contains(secret), "list_jobs carries {secret}: {text}");
    }
}

/// An input with a `%coords` block and a second job reaches the client as markers only.
#[test]
fn geometry_constructs_reach_the_client_as_markers_only() {
    let coords = format!("! SP\n%coords\n Coords\n  H 0.000000 0.755453 -0.471161\n end\nend\n{}", &WATER_INPUT[WATER_INPUT.find("* xyz").unwrap()..]);
    let second = format!("{WATER_INPUT}\n$new_job\n! B3LYP def2-SVP SP\n* xyz 0 1\nO 0.000000 0.000000 0.117790\n*\n");
    let s = served("rt-markers", |db| {
        JobRow { input: &coords, ..JobRow::new("coords") }.insert(&db.conn);
        JobRow { input: &second, ..JobRow::new("second") }.insert(&db.conn);
    });
    let mut c = s.client();
    for (i, (id, marker)) in [("coords", "# a geometry block (%coords) at line 2, not shown"), ("second", "# a second job at line 10, not shown")]
        .into_iter()
        .enumerate()
    {
        let (text, is_error) = c.call_and_wait(2 + i as u64, "get_job", json!({ "job_id": id }));
        assert!(!is_error, "{text}");
        let header = json_of(&text)["header"].as_str().unwrap_or_default().to_string();
        assert!(header.contains(marker), "{id}: {header}");
        for secret in WATER_COORDS {
            assert!(!text.contains(secret), "{id} carries {secret}: {text}");
        }
    }
}

// ----- error mapping -----

/// A refusal for an oversized or malicious input comes back bounded: at most 600 bytes, quoting at
/// most 200 bytes of the input, never a job's source text. Covers a core refusal (an unknown id), a
/// reused reader's refusal (the resolver), `rmcp`'s argument errors (an unknown field's name, a
/// string for a number) and an input refused for its length. NEGATIVE CONTROLS
/// (`ctl:error_quotes_input`): the `get_job` error quotes the id it was given; the source reference's
/// length bound removed (`ctl:source_bound`, the resolver quotes it) → red.
#[test]
fn a_refusal_quotes_at_most_an_echo_of_the_input() {
    let s = served("rt-refusal", |db| {
        JobRow { input: WATER_INPUT, status: "failed", ..JobRow::new("failed") }.insert(&db.conn);
        output_job(&db.conn, &db.dir, "out", (0..3).map(|i| format!("line {i}")));
    });
    let mut c = s.client();
    let a = "A".repeat(100_000);
    let mut unknown = json!({});
    unknown["B".repeat(10_000)] = json!(1);
    let cases: Vec<(&str, Value, char)> = vec![
        ("get_job", json!({ "job_id": a }), 'A'),
        ("get_geometry", json!({ "source": format!("job:{a}") }), 'A'),
        ("measure", json!({ "source": format!("molecule:{a}"), "atom_indices": [0, 1] }), 'A'),
        ("list_jobs", unknown, 'B'),
        ("list_groups", json!({ "limit": "C".repeat(10_000) }), 'C'),
        ("read_output", json!({ "job_id": "out", "query": "D".repeat(10_000) }), 'D'),
        ("memory_estimate", json!({ "header": "E".repeat(30_000), "target": "local" }), 'E'),
        ("search_manual", json!({ "query": "F".repeat(1_000) }), 'F'),
    ];
    for (i, (name, args, c_in)) in cases.into_iter().enumerate() {
        let (text, is_error) = c.call_and_wait(10 + i as u64, name, args);
        assert!(is_error, "{name}: refused, not served: {}", &text[..text.len().min(300)]);
        assert!(text.len() <= REFUSAL_MAX_BYTES, "[ctl:error_quotes_input] {name}: {} bytes", text.len());
        assert!(longest_run(&text, c_in) <= ECHO_MAX_BYTES, "[ctl:error_quotes_input] {name}: {} bytes of input quoted", longest_run(&text, c_in));
    }
    // A job without a geometry: the refusal names the source, never the job's input.
    let (text, is_error) = c.call_and_wait(30, "get_geometry", json!({ "source": "job:failed" }));
    assert!(is_error && text.contains("without success"), "{text}");
    for secret in WATER_COORDS {
        assert!(!text.contains(secret), "{text}");
    }
}

/// The mapping's own size post-condition: a value over the cap is a tool error, never served (the
/// cores cap themselves, so only this test reaches it). NEGATIVE CONTROL (`ctl:service_cap`): the
/// check removed → red.
#[test]
fn the_mapping_refuses_a_result_over_the_cap() {
    let at_cap = "x".repeat(RESULT_CAP_BYTES - 2); // with its quotes, exactly the cap
    assert_eq!(served_text(&at_cap).unwrap().len(), RESULT_CAP_BYTES);
    let over = "x".repeat(RESULT_CAP_BYTES);
    let result = respond("list_jobs", Ok(over));
    assert_eq!(result.is_error, Some(true), "[ctl:service_cap] a result over the cap must not be served");
    let text = result.content[0].as_text().unwrap().text.clone();
    assert!(text.contains("over this tool's cap") && text.len() <= REFUSAL_MAX_BYTES, "[ctl:service_cap] {text}");
    let served = respond("list_jobs", Ok(at_cap));
    assert_eq!(served.is_error, Some(false));
}

/// Error text is the error's own message, bounded, whatever built it; `rmcp`'s argument error keeps
/// at most an echo of the input after its prefix. NEGATIVE CONTROLS: the mapping's cut dropped
/// (`ctl:error_clip`), or the argument error's echo cut dropped (`ctl:argument_echo`) → red.
#[test]
fn error_text_is_bounded_whatever_built_it() {
    let raw = ToolError::Refused("r".repeat(5_000)); // built without `ToolError::refused`
    assert!(error_text("get_job", &raw).len() <= REFUSAL_MAX_BYTES, "[ctl:error_clip]");
    let app = ToolError::App(crate::error::AppError::Internal("i".repeat(5_000)));
    let text = error_text("get_job", &app);
    assert!(text.starts_with("get_job failed: ") && text.len() <= REFUSAL_MAX_BYTES, "[ctl:error_clip] {text}");
    let argument = format!("{ARGUMENT_ERROR_PREFIX} unknown field `{}`, expected `job_id`", "x".repeat(5_000));
    let bounded = bounded_error_text(&argument);
    assert!(bounded.starts_with(ARGUMENT_ERROR_PREFIX) && longest_run(&bounded, 'x') <= ECHO_MAX_BYTES, "[ctl:argument_echo] {bounded}");
    assert_eq!(bounded_error_text("short"), "short");
}

/// `read_output` refuses an argument that does not belong to its mode rather than ignoring it.
#[test]
fn read_output_refuses_arguments_of_the_other_mode() {
    let args = |v: Value| -> ReadOutputArgs { serde_json::from_value(v).unwrap() };
    assert_eq!(output_request(&args(json!({ "job_id": "j" }))).unwrap(), OutputRequest::Tail { lines: None });
    assert!(matches!(output_request(&args(json!({ "job_id": "j", "limit": 5 }))), Err(ToolError::Refused(m)) if m.contains("limit")));
    assert!(matches!(output_request(&args(json!({ "job_id": "j", "query": "x", "lines": 5 }))), Err(ToolError::Refused(m)) if m.contains("lines")));
    let search = output_request(&args(json!({ "job_id": "j", "query": "SCF", "context_lines": 2 }))).unwrap();
    assert_eq!(
        search,
        OutputRequest::Search { query: "SCF".into(), regex: false, case_sensitive: false, context_lines: Some(2), offset: None, limit: None }
    );
}

// ----- threads and the lock -----

/// Every tool's database work runs on Tauri's blocking pool, never on an async worker. NEGATIVE
/// CONTROL (`ctl:db_on_async`): the shared `with_db` runs its read inline → red.
#[test]
fn database_work_runs_on_the_blocking_pool_never_an_async_worker() {
    let s = served("rt-threads", small_world);
    let mut c = s.client();
    for (i, (name, args)) in small_calls().into_iter().enumerate() {
        let (text, is_error) = c.call_and_wait(10 + i as u64, name, args);
        assert!(!is_error, "{name}: {text}");
        let touches = s.recording.take();
        assert!(!touches.is_empty(), "{name} reached the database");
        assert!(touches.iter().all(|t| t.ran == Ran::BlockingPool), "[ctl:db_on_async] {name}: {touches:?}");
    }
}

/// Every tool's answer reaches the client as `respond` writes it: compact JSON. A tool that built its
/// own reply (pretty JSON, no cap re-check) would skip the post-condition the size test relies on.
/// The observable: the served text is as long as its own compact re-serialisation (`serde_json`
/// without `preserve_order` sorts keys, which changes the order and not the length; indentation and
/// newlines change the length). NEGATIVE CONTROL (`ctl:respond`): `list_groups` replies with pretty
/// JSON instead of `respond` → red.
#[test]
fn every_tool_answers_through_respond() {
    let s = served("rt-respond", small_world);
    let mut c = s.client();
    let mut seen: Vec<&str> = Vec::new();
    for (i, (name, args)) in small_calls().into_iter().enumerate() {
        let (text, is_error) = c.call_and_wait(10 + i as u64, name, args);
        assert!(!is_error, "{name}: {text}");
        let value = json_of(&text);
        assert!(value.as_object().is_some_and(|o| !o.is_empty()), "{name}: a non-empty object, so indentation would show: {text}");
        let compact = serde_json::to_string(&value).unwrap();
        assert_eq!(text.len(), compact.len(), "[ctl:respond] {name} is not served as compact JSON: {}", &text[..text.len().min(120)]);
        seen.push(name);
    }
    assert_eq!(seen, READ_TOOLS, "every read tool was checked");
}

/// `memory_estimate` runs with no lock taken by the service: its core locks `DbState` itself.
#[test]
fn memory_estimate_runs_without_the_lock() {
    let s = served("rt-memory", |_| {});
    let mut c = s.client();
    let (text, is_error) = c.call_and_wait(2, "memory_estimate", json!({ "header": "! wB97X-3c Opt\n", "target": "local" }));
    assert!(!is_error, "{text}");
    assert_eq!(json_of(&text)["target"], json!("local"));
    let touches = s.recording.take();
    assert!(!touches.is_empty() && touches.iter().all(|t| !t.lock_held && t.ran == Ran::BlockingPool), "{touches:?}");
}

fn mkfifo(path: &Path) {
    let c = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).unwrap();
    // SAFETY: a valid NUL-terminated path and a plain mode.
    assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0, "mkfifo {}", path.display());
}

/// `read_output` reads `output.out` after the lock is released. The file is a FIFO, so the read
/// blocks in `open` until the test writes: while it is blocked (the call unanswered), `DbState` must
/// be free. NEGATIVE CONTROL (`ctl:lock_across_read`): the guard kept across the file read → red.
#[test]
fn read_output_reads_the_file_after_the_lock_is_released() {
    let s = served("rt-fifo", |db| {
        let dir = db.dir.join("jobs").join("pipe");
        std::fs::create_dir_all(&dir).unwrap();
        mkfifo(&dir.join("output.out"));
        JobRow { job_dir: dir.to_str(), ..JobRow::new("pipe") }.insert(&db.conn);
    });
    let fifo = s.db.dir.join("jobs").join("pipe").join("output.out");
    let mut c = s.client();
    c.call(2, "read_output", json!({ "job_id": "pipe" }));
    assert!(eventually(SOON, || !s.recording.take().is_empty()), "the call reached the database");
    // The path read takes microseconds; from here the call is blocked in `open` on the FIFO.
    std::thread::sleep(Duration::from_millis(300));
    let free_while_reading = eventually(Duration::from_secs(2), || s.state.0.try_lock().is_ok());
    c.set_read_timeout(Duration::from_millis(200));
    let unanswered = c.response(2).is_none();
    // Unblock the read: a writer opens (once the reader is in `open`) and closes.
    let opened = eventually(SOON, || std::fs::OpenOptions::new().write(true).custom_flags(libc::O_NONBLOCK).open(&fifo).is_ok());
    c.set_read_timeout(SOON);
    let reply = c.response(2);
    assert!(opened && unanswered, "the read was blocked on the FIFO (opened {opened}, unanswered {unanswered})");
    assert!(free_while_reading, "[ctl:lock_across_read] DbState was held while output.out was read");
    assert!(reply.is_some(), "the call ends once the FIFO is opened");
}

/// A fifth concurrent call of any read tool on one connection gets "busy, retry" at once: four
/// `list_jobs` calls hold the permits (blocked on `DbState`, which the test holds), and each tool's
/// fifth call is refused before the database is released. NEGATIVE CONTROL (`ctl:permit_skipped`):
/// `list_groups` takes no permit → red (its call waits on the database instead).
#[test]
fn a_fifth_concurrent_call_of_any_tool_is_refused_busy() {
    let s = served("rt-busy", small_world);
    let mut c = s.client();
    let conn = s.live_connection();
    let held = s.state.lock().unwrap();
    for id in 10..10 + REQUESTS_PER_CONNECTION as u64 {
        c.call(id, "list_jobs", json!({}));
    }
    assert!(eventually(SOON, || conn.in_flight() == REQUESTS_PER_CONNECTION), "four permits taken");
    c.set_read_timeout(Duration::from_secs(2));
    let mut refused = Vec::new();
    for (i, (name, args)) in small_calls().into_iter().enumerate() {
        c.call(100 + i as u64, name, args);
        let fifth = c.response(100 + i as u64).map(|r| tool_text(&r));
        refused.push((name, fifth == Some((BUSY.to_string(), true))));
    }
    drop(held);
    c.set_read_timeout(SOON);
    for id in 10..10 + REQUESTS_PER_CONNECTION as u64 {
        let (text, is_error) = tool_text(&c.response(id).unwrap());
        assert!(!is_error, "{text}");
    }
    let not_busy: Vec<&str> = refused.iter().filter(|(_, busy)| !busy).map(|(name, _)| *name).collect();
    assert!(not_busy.is_empty(), "[ctl:permit_skipped] a fifth call was not refused busy: {not_busy:?}");
    assert!(eventually(SOON, || conn.in_flight() == 0), "every permit returned");
}
