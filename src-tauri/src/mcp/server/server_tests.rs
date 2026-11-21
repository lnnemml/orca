//! The server end to end: the lifecycle binds and serves on `tauri::async_runtime` (no test runtime
//! exists in these tests), and a raw NDJSON client talks MCP over the socket.

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use serde_json::json;

use super::lifecycle::{Hooks, LifecycleEnv, McpLifecycle};
use super::listener::{own_uid, PeerUid};
use super::service::{ConnState, McpService, BUSY, RECOVERY_SENTENCE};
use super::test_support::{eventually, lifecycle, no_write_seams, tool_text, Client, Scratch};
use super::{CONNECTIONS, REQUESTS_PER_CONNECTION};
use crate::commands::mcp::create_grant_core;
use crate::commands::settings::DbState;
use crate::mcp::steps::reconcile_submitting_conn;

const SOON: Duration = Duration::from_secs(3);

/// The ADR's numbers (decision 1; M0 amendment, item 5).
#[test]
fn the_caps_are_four_connections_of_four_requests() {
    assert_eq!((CONNECTIONS, REQUESTS_PER_CONNECTION), (4, 4));
}

struct Serving {
    scratch: Scratch,
    db: Arc<DbState>,
    lc: Arc<McpLifecycle>,
}

fn serving(tag: &str) -> Serving {
    let scratch = Scratch::new(tag);
    let db = scratch.db();
    let (lc, _) = lifecycle(&scratch.paths(), db.clone(), Hooks::default());
    let status = lc.set_enabled(true).unwrap();
    assert!(status.serving, "{status:?}");
    Serving { scratch, db, lc }
}

impl Serving {
    fn client(&self) -> Client {
        Client::connect(&self.scratch.paths().socket)
    }

    /// The one connection whose transport is still open.
    fn live_connection(&self) -> Arc<ConnState> {
        let live = || self.lc.connections().into_iter().filter(|c| !c.is_closed()).collect::<Vec<_>>();
        assert!(eventually(SOON, || live().len() == 1));
        live().remove(0)
    }
}

/// The runtime requirement (M2 amendment items 1 and 5, not a control): the first bind and serve
/// runs on Tauri's runtime, the one the app has. `get_grant` reads the grant through
/// `current_grant_core`.
#[test]
fn the_first_bind_and_serve_runs_on_tauris_runtime_and_get_grant_answers() {
    let s = serving("first-serve");
    let mut c = s.client();
    let init = c.initialize(1);
    assert_eq!(init["serverInfo"]["name"], "orcastudio");

    c.call_get_grant(2);
    let (text, is_error) = tool_text(&c.response(2).unwrap());
    assert!(!is_error, "{text}");
    assert_eq!(serde_json::from_str::<serde_json::Value>(&text).unwrap(), json!({ "grant": null }));

    {
        let conn = s.db.lock().unwrap();
        create_grant_core(&conn, vec!["local".into()], vec!["! wB97X-3c\n".into()], 3, 1, "2099-01-01 00:00:00".into())
            .unwrap();
    }
    c.call_get_grant(3);
    let (text, is_error) = tool_text(&c.response(3).unwrap());
    assert!(!is_error, "{text}");
    let grant: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(grant["grant"]["state"], "active");
    assert_eq!(grant["grant"]["max_jobs"], 3);
}

/// Control (M0 amendment item 9 / M2 amendment item 1): an unknown method closing the connection
/// instead of returning an error. Measured here on `rmcp` 3.11.0:
/// - `server/discover` before `initialize`, without the 2026-07-28 `_meta` (as Claude Code sent it
///   in M0): a JSON-RPC error, and the connection stays open;
/// - `server/discover` with that `_meta`: answered, and the connection stays open;
/// - an unknown method after `initialize`: −32601, and the connection stays open.
#[test]
fn an_unknown_method_gets_an_error_and_the_connection_stays_open() {
    let s = serving("unknown-method");
    let mut c = s.client();

    c.request(1, "server/discover", json!({}));
    let bare = c.response(1).expect("a reply to a bare server/discover, not a closed connection");
    assert!(bare["error"]["code"].is_i64(), "{bare}");

    let meta = json!({ "_meta": {
        "io.modelcontextprotocol/protocolVersion": "2026-07-28",
        "io.modelcontextprotocol/clientCapabilities": {}
    }});
    c.request(2, "server/discover", meta);
    let full = c.response(2).expect("a reply to server/discover with _meta, not a closed connection");
    assert!(full.get("result").is_some() || full.get("error").is_some(), "{full}");

    c.initialize(3);
    c.request(4, "orcastudio/no-such-method", json!({}));
    let unknown = c.response(4).expect("a reply to an unknown method, not a closed connection");
    assert_eq!(unknown["error"]["code"], -32601, "{unknown}");

    c.request(5, "tools/list", json!({}));
    assert!(c.response(5).expect("the connection is still open")["result"]["tools"].is_array());
}

/// Control (M0 amendment item 9 / M2 amendment item 1): a tool description without the recovery
/// sentence. Checked on the wire (`tools/list` and the instructions) and on every routed tool.
#[test]
fn every_tool_description_and_the_instructions_carry_the_recovery_sentence() {
    let s = serving("recovery");
    let mut c = s.client();
    let init = c.initialize(1);
    assert!(init["instructions"].as_str().unwrap().contains(RECOVERY_SENTENCE));

    c.request(2, "tools/list", json!({}));
    let tools = c.response(2).unwrap()["result"]["tools"].as_array().unwrap().clone();
    assert!(!tools.is_empty());
    for tool in &tools {
        assert!(tool["description"].as_str().unwrap_or_default().contains(RECOVERY_SENTENCE), "{tool}");
    }

    let service = McpService::new(Arc::new(ConnState::new()), s.db.clone(), Arc::new(s.scratch.host()), s.lc.write_wiring());
    assert_eq!(service.tools().len(), tools.len());
    for tool in service.tools() {
        assert!(tool.description.as_deref().unwrap_or_default().contains(RECOVERY_SENTENCE), "{}", tool.name);
    }
}

/// Control (M0 amendment item 9 / M2 amendment item 1): a fifth concurrent request queued instead
/// of refused "busy". Four `get_grant` calls hold their permits while the test holds `DbState`; the
/// fifth gets the busy tool error at once, before the database is released.
#[test]
fn a_fifth_concurrent_request_is_refused_busy_at_once() {
    let s = serving("busy");
    let mut c = s.client();
    c.initialize(1);
    let conn = s.live_connection();

    let held = s.db.lock().unwrap();
    for id in 10..10 + REQUESTS_PER_CONNECTION as u64 {
        c.call_get_grant(id);
    }
    assert!(eventually(SOON, || conn.in_flight() == REQUESTS_PER_CONNECTION), "four permits taken");

    c.set_read_timeout(Duration::from_secs(2));
    c.call_get_grant(99);
    let fifth = c.response(99).expect("the fifth request answered at once, not queued");
    assert_eq!(tool_text(&fifth), (BUSY.to_string(), true));

    drop(held);
    c.set_read_timeout(SOON);
    for id in 10..10 + REQUESTS_PER_CONNECTION as u64 {
        let (text, is_error) = tool_text(&c.response(id).unwrap());
        assert!(!is_error, "{text}");
    }
    assert!(eventually(SOON, || conn.in_flight() == 0), "every permit returned");
}

/// Control (M0 amendment item 9 / M2 amendment item 1): a connection slot freed while a permit is
/// still held. Four clients each leave a blocked `get_grant` and hang up; their transports end, but
/// their slots stay taken until the handlers return, so a fifth client is closed at once.
#[test]
fn a_slot_stays_taken_until_its_last_permit_returns() {
    let s = serving("slot-permit");
    let held = s.db.lock().unwrap();
    let mut clients: Vec<Client> = (0..CONNECTIONS).map(|_| s.client()).collect();
    for (i, c) in clients.iter_mut().enumerate() {
        c.initialize(1);
        c.call_get_grant(100 + i as u64);
    }
    let lc = s.lc.clone();
    assert!(eventually(SOON, || {
        let conns = lc.connections();
        conns.len() == CONNECTIONS && conns.iter().all(|c| c.in_flight() == 1)
    }));
    let conns = s.lc.connections();
    drop(clients); // every client hangs up
    // At the read EOF, not after rmcp's 5 s response drain (the listener's `Transport`).
    assert!(eventually(Duration::from_secs(2), || conns.iter().all(|c| c.is_closed())), "every transport ended");

    // rmcp keeps each slot's serve loop alive for its response drain (up to 5 s, "M2a facts" 4); wait
    // until every serve loop has ended, so only the permits still hold the slots.
    // (`status()` would read the row through the `DbState` this test holds.)
    assert!(eventually(Duration::from_secs(8), || s.lc.open_transports() == 0), "the serve loops ended");
    let mut fifth = s.client();
    assert!(fifth.is_closed_by_server(Duration::from_secs(1)), "a slot was freed while its permit was held");

    drop(held);
    assert!(eventually(SOON, || conns.iter().all(|c| c.in_flight() == 0)));
    assert!(eventually(SOON, || {
        let mut c = s.client();
        c.set_read_timeout(Duration::from_millis(500));
        c.request(1, "initialize", json!({ "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": { "name": "t", "version": "0" } }));
        c.response(1).is_some_and(|r| r.get("result").is_some())
    }), "the slots free once the permits return");
}

/// Control (M2 amendment item 1): a fifth connection served. Four connected clients fill the
/// slots; a fifth is closed at once, and the four still work.
#[test]
fn a_fifth_connection_is_closed_at_once() {
    let s = serving("fifth-conn");
    let mut clients: Vec<Client> = (0..CONNECTIONS).map(|_| s.client()).collect();
    for c in clients.iter_mut() {
        c.initialize(1);
    }
    let mut fifth = s.client();
    assert!(fifth.is_closed_by_server(Duration::from_secs(1)), "a fifth connection was served");
    for c in clients.iter_mut() {
        c.request(2, "tools/list", json!({}));
        assert!(c.response(2).is_some());
    }
}

fn serving_with_peer_uid(tag: &str, peer_uid: PeerUid) -> (Scratch, Arc<McpLifecycle>) {
    let scratch = Scratch::new(tag);
    let env = LifecycleEnv {
        paths: Ok(scratch.paths()),
        db: scratch.db(),
        host: Arc::new(scratch.host()),
        reconcile: Box::new(reconcile_submitting_conn),
        peer_uid,
        own_uid: own_uid(),
        hooks: Hooks::default(),
        write_seams: no_write_seams(),
    };
    let lc = McpLifecycle::new(env);
    assert!(lc.set_enabled(true).unwrap().serving);
    (scratch, lc)
}

/// Control (M2 amendment item 1): a foreign-uid peer accepted. The uid source is injected (no
/// second account exists: Open question c′); a peer reported as another uid is dropped at accept.
#[test]
fn a_peer_with_another_uid_is_dropped() {
    let foreign: PeerUid = Arc::new(|_| Ok(own_uid() + 1));
    let (scratch, lc) = serving_with_peer_uid("foreign", foreign);
    let mut c = Client::connect(&scratch.paths().socket);
    assert!(c.is_closed_by_server(Duration::from_secs(1)), "a foreign uid was served");
    assert!(lc.connections().is_empty());

    let unreadable: PeerUid = Arc::new(|_| Err(std::io::Error::other("no SO_PEERCRED")));
    let (scratch, _lc) = serving_with_peer_uid("no-cred", unreadable);
    let mut c = Client::connect(&scratch.paths().socket);
    assert!(c.is_closed_by_server(Duration::from_secs(1)), "an unknown uid was served");
}

#[test]
fn our_own_uid_from_so_peercred_is_served() {
    let s = serving("own-uid");
    let mut c = s.client();
    assert!(c.initialize(1)["serverInfo"].is_object());
}

/// A `DbSource` over the scratch `DbState` that, once armed, holds every database touch made off
/// the arming thread until released: a tool call then stays in flight on its blocking task.
struct HeldDb {
    db: Arc<DbState>,
    armed_on: std::sync::Mutex<Option<std::thread::ThreadId>>,
    release: (std::sync::Mutex<bool>, std::sync::Condvar),
}

impl HeldDb {
    fn arm(&self) {
        *self.armed_on.lock().unwrap() = Some(std::thread::current().id());
    }
    fn release(&self) {
        *self.release.0.lock().unwrap() = true;
        self.release.1.notify_all();
    }
}

impl super::DbSource for HeldDb {
    fn db(&self) -> &DbState {
        let armed_on = *self.armed_on.lock().unwrap();
        if armed_on.is_some_and(|t| t != std::thread::current().id()) {
            let released = self.release.0.lock().unwrap();
            let _released = self.release.1.wait_while(released, |r| !*r).unwrap();
        }
        &self.db
    }
}

/// The `closed` flag (M0 amendment items 6 and 10): set when the transport ends, and on switch-off —
/// on **every** connection (M3 amendment, item 1: the remaining item-9 obligation, "switch-off or exit
/// not setting every connection's `closed` flag", extended to two connections).
///
/// Each of the two connections has a `get_grant` call held in flight (its database touch waits), so
/// neither service can end, and set `closed` on its own way out, before the check: the flags read at
/// switch-off's return are switch-off's own. NEGATIVE CONTROL: delete the `closed` store in the
/// listener's `Registry::shut` → both read open → red.
#[test]
fn closed_is_set_on_hang_up_and_on_switch_off() {
    let s = serving("closed-flag");
    let mut first = s.client();
    first.initialize(1);
    let conn = s.live_connection();
    assert!(!conn.is_closed());
    drop(first);
    assert!(eventually(SOON, || conn.is_closed()), "hang-up sets closed");

    // Switch-off, two connections, each with a call in flight.
    let scratch = Scratch::new("closed-flag-2");
    let held = Arc::new(HeldDb {
        db: scratch.db(),
        armed_on: std::sync::Mutex::new(None),
        release: (std::sync::Mutex::new(false), std::sync::Condvar::new()),
    });
    let (mut env, _) = super::test_support::counted_env(&scratch.paths(), scratch.db(), Hooks::default());
    env.db = held.clone();
    let lc = McpLifecycle::new(env);
    assert!(lc.set_enabled(true).unwrap().serving);
    let mut clients: Vec<Client> = (0..2).map(|_| Client::connect(&scratch.paths().socket)).collect();
    for c in &mut clients {
        c.initialize(1);
    }
    assert!(eventually(SOON, || lc.connections().iter().filter(|c| !c.is_closed()).count() == 2));
    let conns = lc.connections();
    held.arm();
    for c in &mut clients {
        c.call_get_grant(2);
    }
    assert!(eventually(SOON, || conns.iter().all(|c| c.in_flight() == 1)), "each connection has a call in flight");

    lc.set_enabled(false).unwrap();
    let closed: Vec<bool> = conns.iter().map(|c| c.closed.load(Ordering::SeqCst)).collect();
    held.release();
    assert_eq!(closed, vec![true, true], "CLOSED-EVERY: switch-off did not set every connection's `closed` before it returned");
    for c in &mut clients {
        let _ = c.response(2); // the released call may still answer during rmcp's drain
        assert!(c.is_closed_by_server(SOON), "switch-off cancels the service");
    }
}
