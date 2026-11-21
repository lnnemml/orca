//! The read count (ADR-027 M4 amendment, item 2; unit M4a Part A): the read/write table against the
//! service's tool list, the counters, the ledger's exactly-once fold, the count end to end over the
//! socket, and the status read that waits on nothing a handler or a transition can hold.
//!
//! Every control named in the M4 amendment's item 8 for the read count has a tagged assertion here;
//! `m4a_parta_controls.py` (the session scratchpad) applies each mutation and expects its tag red.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Barrier, Mutex};
use std::thread;
use std::time::Duration;

use serde_json::json;

use super::lifecycle::{Hooks, McpLifecycle};
use super::reads::*;
use super::service::{ConnState, McpService, BUSY};
use super::test_support::{eventually, lifecycle, tool_text, Client, Scratch};
use super::REQUESTS_PER_CONNECTION;
use crate::commands::settings::DbState;

const SOON: Duration = Duration::from_secs(3);

fn tools(pairs: &[(&str, u64)]) -> BTreeMap<String, u64> {
    pairs.iter().map(|(t, n)| (t.to_string(), *n)).collect()
}

// ----- the table -----

/// Control PARITY (item 8: "the read/write table diverging from the tool list"): every tool the
/// service lists is classified, and the table names no tool the service lacks. Decision 7's kinds:
/// twelve reads (the eleven read tools, `get_grant` among them, plus `wait_for_jobs`) and three
/// writes.
#[test]
fn the_read_write_table_is_the_services_tool_list() {
    let scratch = Scratch::new("reads-parity");
    let db = scratch.db();
    let (lc, _) = lifecycle(&scratch.paths(), db.clone(), Hooks::default());
    let service = McpService::new(Arc::new(ConnState::new()), db, Arc::new(scratch.host()), lc.write_wiring());
    let mut listed: Vec<String> = service.tools().iter().map(|t| t.name.to_string()).collect();
    listed.sort();
    let mut table: Vec<String> = TOOL_KINDS.iter().map(|(n, _)| n.to_string()).collect();
    table.sort();
    let unique = table.len();
    table.dedup();
    assert_eq!(table.len(), unique, "PARITY: the table names a tool twice");
    assert_eq!(listed, table, "PARITY: the table diverges from the service's tool list");

    let reads: Vec<&str> = TOOL_KINDS.iter().filter(|(_, k)| *k == ToolKind::Read).map(|(n, _)| *n).collect();
    let writes: Vec<&str> = TOOL_KINDS.iter().filter(|(_, k)| *k == ToolKind::Write).map(|(n, _)| *n).collect();
    assert_eq!(reads.len(), READ_TOOL_COUNT, "PARITY: twelve reads");
    assert_eq!(reads, TOOL_KINDS[..READ_TOOL_COUNT].iter().map(|(n, _)| *n).collect::<Vec<_>>(), "PARITY: the reads come first");
    assert_eq!(writes, ["create_job", "cancel_job", "create_group"], "PARITY: decision 7's three writes");
    assert!(reads.contains(&"get_grant") && reads.contains(&"wait_for_jobs"), "PARITY: {reads:?}");
}

/// The counters: a read counts, once, in the total and its tool; a write or unknown tool does not.
/// Control COUNT-UNIT (item 8: "a write tool or an unknown tool counted").
#[test]
fn a_read_counts_and_a_write_or_unknown_tool_does_not() {
    let counts = ReadCounts::default();
    for (name, kind) in TOOL_KINDS {
        assert_eq!(counts.count(name), kind == ToolKind::Read, "COUNT-UNIT: {name}");
    }
    assert!(!counts.count("no_such_tool"), "COUNT-UNIT: an unknown tool counted");
    assert!(!counts.count(""), "COUNT-UNIT: an empty name counted");
    assert_eq!(counts.total(), READ_TOOL_COUNT as u64, "COUNT-UNIT: each read once, nothing else");
    assert_eq!(counts.by_tool().len(), READ_TOOL_COUNT);
    assert!(counts.by_tool().values().all(|n| *n == 1));
    assert_eq!(kind_of("create_job"), Some(ToolKind::Write));
    assert_eq!(kind_of("wait_for_jobs"), Some(ToolKind::Read));
    assert_eq!(kind_of("server/discover"), None);
}

// ----- the ledger -----

fn counted_conn(reads: &[&str]) -> Arc<ConnState> {
    let conn = Arc::new(ConnState::new());
    for r in reads {
        assert!(conn.reads.count(r));
    }
    conn
}

/// A connection's reads show live with its id and breakdown, then survive its fold in the total,
/// exactly once. Controls FOLD-LOST and FOLD-TWICE (item 8: "a closed connection's reads lost from
/// the total, or counted twice (live and folded)").
#[test]
fn a_folded_connections_reads_stay_in_the_total_exactly_once() {
    let ledger = ReadLedger::default();
    let a = counted_conn(&["get_job", "get_job", "list_jobs"]);
    let b = counted_conn(&["wait_for_jobs"]);
    let (ia, ib) = (ledger.enter(a.clone()), ledger.enter(b.clone()));
    assert_eq!((ia, ib), (1, 2), "ids are the accept ordinals");

    let live = ledger.snapshot();
    assert_eq!(live.total, 4);
    assert_eq!(live.connections.len(), 2);
    assert_eq!(live.connections[0].id, 1);
    assert_eq!(live.connections[0].state, ConnectionState::Open);
    assert_eq!(live.connections[0].reads, 3);
    assert_eq!(live.connections[0].by_tool, tools(&[("get_job", 2), ("list_jobs", 1)]));

    a.closed.store(true, Ordering::SeqCst);
    assert_eq!(ledger.snapshot().connections[0].state, ConnectionState::Closing, "closed but not folded: closing");

    ledger.fold(ia);
    let after = ledger.snapshot();
    assert_eq!(after.total, 4, "FOLD-LOST/FOLD-TWICE: the total after a fold");
    assert_eq!(after.connections.iter().map(|c| c.id).collect::<Vec<_>>(), [2], "FOLD-TWICE: the folded entry is gone");

    ledger.fold(ia);
    assert_eq!(ledger.snapshot().total, 4, "FOLD-TWICE: a second fold of the same id");
    ledger.fold(ib);
    let all = ledger.snapshot();
    assert_eq!((all.total, all.connections.len()), (4, 0), "FOLD-LOST: every fold kept");
    assert_eq!(ledger.enter(Arc::new(ConnState::new())), 3, "ids are never reused");
}

/// The fold and the snapshot are one mutex: while connections whose counts are final fold on
/// several threads, every snapshot shows the same total. Control FOLD-STRESS.
#[test]
fn every_snapshot_during_concurrent_folds_sees_each_count_once() {
    const CONNS: usize = 64;
    let ledger = Arc::new(ReadLedger::default());
    let mut ids = Vec::new();
    for i in 0..CONNS {
        let conn = counted_conn(&vec!["get_job"; i % 5 + 1]);
        ids.push(ledger.enter(conn));
    }
    let expected = ledger.snapshot().total;
    let done = Arc::new(AtomicBool::new(false));
    let start = Arc::new(Barrier::new(5));
    let watcher = {
        let (ledger, done, start) = (ledger.clone(), done.clone(), start.clone());
        thread::spawn(move || {
            start.wait();
            let mut seen = 0usize;
            while !done.load(Ordering::SeqCst) {
                let s = ledger.snapshot();
                assert_eq!(s.total, expected, "FOLD-STRESS: a snapshot saw a count lost or twice");
                seen += 1;
            }
            seen
        })
    };
    let folders: Vec<_> = ids
        .chunks(CONNS / 4)
        .map(|chunk| {
            let (ledger, chunk, start) = (ledger.clone(), chunk.to_vec(), start.clone());
            thread::spawn(move || {
                start.wait();
                for id in chunk {
                    ledger.fold(id);
                    thread::yield_now();
                }
            })
        })
        .collect();
    for f in folders {
        f.join().unwrap();
    }
    done.store(true, Ordering::SeqCst);
    assert!(watcher.join().unwrap() > 0);
    let end = ledger.snapshot();
    assert_eq!((end.total, end.connections.len()), (expected, 0), "FOLD-STRESS");
}

/// The order the ledger's correctness rests on, pinned in the source (the accept loop and the serve
/// task need a socket and a runtime, and the interleavings are not reachable from a test): the
/// connection enters the ledger after `Registry::register` succeeds and before its service is
/// spawned, and is folded only after its drain. Control ORDER.
#[test]
fn a_connection_enters_at_admission_and_is_folded_after_its_drain() {
    let src = include_str!("listener.rs");
    let body = |sig: &str| {
        let start = src.find(sig).unwrap_or_else(|| panic!("{sig}"));
        &src[start..start + src[start..].find("\n}\n").unwrap()]
    };
    let admit = body(concat!("fn ", "admit("));
    let at = |s: &str, needle: &str| s.find(needle).unwrap_or_else(|| panic!("ORDER: `{needle}` missing"));
    let (register, enter, service, spawn) = (
        at(admit, "ctx.registry.register()"),
        at(admit, "ctx.reads.enter(conn.clone())"),
        at(admit, "McpService::new("),
        at(admit, "tauri::async_runtime::spawn(task)"),
    );
    assert!(register < enter && enter < service && service < spawn, "ORDER: register, enter, service, spawn");

    let serve = body(concat!("async fn ", "serve_connection("));
    let (drain, fold) = (at(serve, "conn.drain().await"), at(serve, "ctx.reads.fold(id)"));
    assert!(drain < fold, "ORDER: folded only after the drain");
    assert_eq!(serve.matches(".fold(").count(), 1, "ORDER: folded once");
}

// ----- end to end -----

struct Serving {
    scratch: Scratch,
    db: Arc<DbState>,
    lc: Arc<McpLifecycle>,
}

fn serving(tag: &str, hooks: Hooks) -> Serving {
    let scratch = Scratch::new(tag);
    let db = scratch.db();
    let (lc, _) = lifecycle(&scratch.paths(), db.clone(), hooks);
    assert!(lc.set_enabled(true).unwrap().serving);
    Serving { scratch, db, lc }
}

impl Serving {
    fn client(&self) -> Client {
        let mut c = Client::connect(&self.scratch.paths().socket);
        c.initialize(1);
        c
    }

    /// The status's one connection entry, once there is exactly one.
    fn only_entry(&self) -> ConnectionReads {
        assert!(eventually(SOON, || self.lc.status().reads.connections.len() == 1), "{:?}", self.lc.status().reads);
        self.lc.status().reads.connections.remove(0)
    }
}

/// The count over the socket. Counted: an admitted read, and one that then fails argument parsing.
/// Not counted: an unknown tool, a write tool, a busy refusal. Controls E2E-READ (item 8: "a read
/// call not counted") and E2E-NOT (item 8: "a write tool, a busy refusal or an unknown tool
/// counted").
#[test]
fn the_count_takes_admitted_reads_only() {
    let s = serving("reads-e2e", Hooks::default());
    let mut c = s.client();

    let (text, is_error) = c.call_and_wait(2, "get_grant", json!({}));
    assert!(!is_error, "{text}");
    let (text, is_error) = c.call_and_wait(3, "list_jobs", json!({ "no_such_argument": 1 }));
    assert!(is_error && text.contains("failed to deserialize"), "an argument error: {text}");
    assert_eq!(s.only_entry().by_tool, tools(&[("get_grant", 1), ("list_jobs", 1)]), "E2E-READ");

    c.call(4, "no_such_tool", json!({}));
    assert!(c.response(4).expect("an answer to an unknown tool").get("error").is_some());
    let (text, is_error) = c.call_and_wait(5, "create_group", json!({ "name": "reads" }));
    assert!(!is_error, "{text}");
    assert_eq!(s.only_entry().reads, 2, "E2E-NOT: an unknown or write tool counted");

    let conn = {
        let conns = s.lc.connections();
        assert_eq!(conns.len(), 1);
        conns[0].clone()
    };
    let held = s.db.lock().unwrap();
    for id in 10..10 + REQUESTS_PER_CONNECTION as u64 {
        c.call_get_grant(id);
    }
    assert!(eventually(SOON, || conn.in_flight() == REQUESTS_PER_CONNECTION), "four permits taken");
    c.set_read_timeout(Duration::from_secs(2));
    c.call_get_grant(99);
    assert_eq!(tool_text(&c.response(99).expect("busy at once")), (BUSY.to_string(), true));
    assert_eq!(conn.reads.total(), 6, "E2E-NOT: the busy refusal counted (or an admitted read not)");
    drop(held);
    c.set_read_timeout(SOON);
    for id in 10..10 + REQUESTS_PER_CONNECTION as u64 {
        assert!(!tool_text(&c.response(id).unwrap()).1);
    }
    let entry = s.only_entry();
    assert_eq!((entry.reads, entry.state), (6, ConnectionState::Open), "E2E-READ");
    assert_eq!(entry.by_tool, tools(&[("get_grant", 5), ("list_jobs", 1)]), "E2E-READ");
    assert_eq!(s.lc.status().reads.total, 6, "E2E-READ: the total includes the live connection");
}

/// A closed connection's reads stay in the total once it is folded, across a switch-off and a
/// switch-on, and a later connection gets a fresh id. Control E2E-FOLD (item 8: "a closed
/// connection's reads lost from the total").
#[test]
fn the_total_since_app_start_survives_a_disconnect_and_a_switch_off_and_on() {
    let s = serving("reads-survive", Hooks::default());
    let mut c = s.client();
    for id in 2..5 {
        assert!(!c.call_and_wait(id, "list_groups", json!({})).1);
    }
    let first = s.only_entry();
    assert_eq!(first.reads, 3);
    drop(c);
    // rmcp's response drain can keep the serve loop up to 5 s after the hang-up.
    assert!(eventually(Duration::from_secs(8), || s.lc.status().reads.connections.is_empty()), "the connection folded");
    assert_eq!(s.lc.status().reads.total, 3, "E2E-FOLD: the closed connection's reads");

    assert!(!s.lc.set_enabled(false).unwrap().serving);
    assert_eq!(s.lc.status().reads.total, 3, "E2E-FOLD: the total after a switch-off");
    assert!(eventually(SOON, || s.lc.set_enabled(true).unwrap().serving), "serving again");
    let mut c = s.client();
    assert!(!c.call_and_wait(2, "get_grant", json!({})).1);
    let second = s.only_entry();
    assert!(second.id > first.id, "a fresh id: {second:?} after {first:?}");
    assert_eq!(s.lc.status().reads.total, 4, "E2E-FOLD: across the switch-off and on");
}

/// A switched-off connection that still drains shows as "closing", and still counts.
#[test]
fn a_draining_connection_shows_as_closing() {
    let s = serving("reads-closing", Hooks::default());
    let mut c = s.client();
    assert!(!c.call_and_wait(2, "get_grant", json!({})).1);
    let conn = s.lc.connections().remove(0);
    let permit = conn.try_permit().expect("a free permit"); // holds the drain
    assert!(!s.lc.set_enabled(false).unwrap().serving);
    let status = s.lc.status();
    assert_eq!(status.reads.connections.len(), 1, "{status:?}");
    assert_eq!(status.reads.connections[0].state, ConnectionState::Closing);
    assert_eq!(status.reads.total, 1);
    drop(permit);
    assert!(eventually(Duration::from_secs(8), || s.lc.status().reads.connections.is_empty()));
    assert_eq!(s.lc.status().reads.total, 1);
}

/// Run `status` on another thread; whether it answered within 1 s.
fn status_answers(lc: &Arc<McpLifecycle>) -> Option<super::lifecycle::McpStatus> {
    let (tx, rx) = mpsc::channel();
    let reader = lc.clone();
    thread::spawn(move || {
        let _ = tx.send(reader.status());
    });
    rx.recv_timeout(Duration::from_secs(1)).ok()
}

/// Control STATUS-PERMITS (item 8: "`mcp_status` failing to return while another thread holds
/// every permit of a connection"): the status answers, with the connection's entry.
#[test]
fn the_status_answers_while_every_permit_of_a_connection_is_held() {
    let s = serving("reads-permits", Hooks::default());
    let mut c = s.client();
    assert!(!c.call_and_wait(2, "get_grant", json!({})).1);
    let conn = s.lc.connections().remove(0);
    let permits: Vec<_> = (0..REQUESTS_PER_CONNECTION).map(|_| conn.try_permit().expect("a permit")).collect();
    assert!(conn.try_permit().is_none(), "every permit held");
    let status = status_answers(&s.lc).expect("STATUS-PERMITS: the status waited on a connection's permits");
    assert_eq!(status.reads.connections.len(), 1);
    assert_eq!(status.reads.connections[0].reads, 1);
    drop(permits);
}

/// Control STATUS-GATE (item 8: "`mcp_status` failing to return while another thread holds the
/// gate's mutex"): a spawn stopped between the gate's check and its spawn holds the gate's mutex,
/// and the status still answers.
#[test]
fn the_status_answers_while_the_gates_mutex_is_held() {
    let (inside_tx, inside) = mpsc::channel();
    let (release_tx, release) = mpsc::channel::<()>();
    let release = Arc::new(Mutex::new(release));
    let hooks = Hooks {
        gate: Some(Arc::new(move || {
            let _ = inside_tx.send(());
            let _ = release.lock().unwrap().recv_timeout(SOON);
        })),
        ..Hooks::default()
    };
    let s = serving("reads-gate", hooks);
    let gate = s.lc.gate().clone();
    let spawner = thread::spawn(move || gate.spawn(async {}).is_ok());
    inside.recv_timeout(SOON).expect("the spawn holds the gate's mutex");
    let status = status_answers(&s.lc).expect("STATUS-GATE: the status waited for the gate's mutex");
    assert!(status.serving);
    drop(release_tx);
    assert!(spawner.join().unwrap());
}
