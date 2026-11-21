//! Shared test support for the transport: a scratch directory with a migrated database, a counting
//! lifecycle environment, a raw NDJSON MCP client over the socket, and a poll helper.
//!
//! No test here creates a runtime of its own: every async piece runs on `tauri::async_runtime`, as
//! in the app (M2 amendment, items 1 and 5).

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rusqlite::Connection;
use serde_json::{json, Value};

use super::create_job_task::CreateJobSeams;
use super::lifecycle::{Hooks, LifecycleEnv, McpLifecycle};
use super::listener::{kernel_peer_uid, own_uid};
use super::paths::{resolve, McpPaths};
use super::service::HostPaths;
use crate::commands::settings::DbState;
use crate::mcp::classify::Target;
use crate::mcp::create_job::{StatusEmit, StepB, StepBOutcome};
use crate::mcp::create_job_test_support::NoSidecar;
use crate::mcp::steps::reconcile_submitting_conn;
use crate::models::job::JobStatus;

/// A scratch directory, removed on drop. Short, so the socket path stays far below `sun_path`.
pub struct Scratch {
    pub dir: PathBuf,
}

impl Scratch {
    pub fn new(tag: &str) -> Scratch {
        static N: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "osmcp-{tag}-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        Scratch { dir }
    }

    /// The MCP paths under this scratch dir, as `XDG_RUNTIME_DIR`.
    pub fn paths(&self) -> McpPaths {
        resolve(Some(&self.dir), None).unwrap()
    }

    /// A migrated database in this scratch dir, as the app's `DbState`.
    pub fn db(&self) -> Arc<DbState> {
        Arc::new(DbState(Mutex::new(crate::db::init_db(&self.dir.join("db")).unwrap())))
    }

    /// The host facts `memory_estimate` reads, under this scratch dir: `meminfo` (absent until a
    /// test writes it) and the dir itself as the data dir.
    pub fn host(&self) -> HostPaths {
        HostPaths { meminfo: self.dir.join("meminfo"), data_dir: self.dir.clone() }
    }

    /// Another connection on the same database file (another instance's view).
    pub fn another_connection(&self) -> Connection {
        Connection::open(self.dir.join("db").join("orcastudio.db")).unwrap()
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.dir).ok();
    }
}

/// Counts what a lifecycle did.
#[derive(Default)]
pub struct Counts {
    pub reconciles: AtomicUsize,
    pub binds: AtomicUsize,
}

impl Counts {
    pub fn reconciles(&self) -> usize {
        self.reconciles.load(Ordering::SeqCst)
    }
    pub fn binds(&self) -> usize {
        self.binds.load(Ordering::SeqCst)
    }
}

/// A lifecycle over `paths` and `db` with the real reconciliation, counted, and `hooks` (whose
/// `after_bind`, if any, runs after the bind is counted).
pub fn lifecycle(paths: &McpPaths, db: Arc<DbState>, hooks: Hooks) -> (Arc<McpLifecycle>, Arc<Counts>) {
    let (env, counts) = counted_env(paths, db, hooks);
    (McpLifecycle::new(env), counts)
}

/// The environment [`lifecycle`] builds, for a test that overrides a field first.
pub fn counted_env(paths: &McpPaths, db: Arc<DbState>, hooks: Hooks) -> (LifecycleEnv, Arc<Counts>) {
    let counts = Arc::new(Counts::default());
    let (c1, c2) = (counts.clone(), counts.clone());
    let user_after_bind = hooks.after_bind.clone();
    let hooks = Hooks {
        after_bind: Some(Arc::new(move |p: &McpPaths| {
            c2.binds.fetch_add(1, Ordering::SeqCst);
            if let Some(h) = &user_after_bind {
                h(p);
            }
        })),
        ..hooks
    };
    let env = LifecycleEnv {
        paths: Ok(paths.clone()),
        db,
        host: Arc::new(HostPaths { meminfo: paths.dir.join("meminfo"), data_dir: paths.dir.clone() }),
        reconcile: Box::new(move |conn| {
            c1.reconciles.fetch_add(1, Ordering::SeqCst);
            reconcile_submitting_conn(conn)
        }),
        peer_uid: kernel_peer_uid(),
        own_uid: own_uid(),
        hooks,
        create_job: no_create_job(),
    };
    (env, counts)
}

/// A step B for a test whose `create_job` never reaches one: it refuses, so step C refunds.
pub struct NoStepB;

impl StepB for NoStepB {
    fn submit(&self, _: &str, _: &Target) -> StepBOutcome {
        StepBOutcome::Refused("no step B in this test".into())
    }
}

/// A `job:status` emit that goes nowhere.
pub struct NoEmit;

impl StatusEmit for NoEmit {
    fn job_status(&self, _: &str, _: JobStatus) {}
}

/// `create_job`'s seams for a test that does not exercise it: no sidecar, a refusing step B, no emit.
pub fn no_create_job() -> CreateJobSeams {
    CreateJobSeams { smiles: Arc::new(NoSidecar), step_b: Arc::new(NoStepB), emit: Arc::new(NoEmit) }
}

/// Write the switch row through another connection (another instance, or the test's hand).
pub fn set_row(conn: &Connection, on: bool) {
    crate::mcp::switch::write_enabled_conn(conn, on).unwrap();
}

/// Poll `cond` every 10 ms until it holds or `timeout` passes.
pub fn eventually(timeout: Duration, mut cond: impl FnMut() -> bool) -> bool {
    let start = Instant::now();
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

/// A raw MCP client: NDJSON JSON-RPC over the socket, as the relay would carry it.
pub struct Client {
    reader: BufReader<UnixStream>,
    writer: UnixStream,
    /// Messages read while waiting for another id.
    pending: Vec<Value>,
}

impl Client {
    pub fn connect(socket: &Path) -> Client {
        let stream = UnixStream::connect(socket).unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        Client { writer: stream.try_clone().unwrap(), reader: BufReader::new(stream), pending: Vec::new() }
    }

    pub fn set_read_timeout(&self, timeout: Duration) {
        self.writer.set_read_timeout(Some(timeout)).unwrap();
    }

    pub fn send(&mut self, message: Value) {
        let mut line = message.to_string();
        line.push('\n');
        self.writer.write_all(line.as_bytes()).unwrap();
    }

    pub fn request(&mut self, id: u64, method: &str, params: Value) {
        self.send(json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }));
    }

    /// The next line, or `None` on EOF or timeout.
    pub fn read_message(&mut self) -> Option<Value> {
        let mut line = String::new();
        match self.reader.read_line(&mut line) {
            Ok(0) | Err(_) => None,
            Ok(_) => Some(serde_json::from_str(&line).unwrap()),
        }
    }

    /// The response to `id`, skipping (and keeping) anything else; `None` on EOF or timeout.
    pub fn response(&mut self, id: u64) -> Option<Value> {
        if let Some(i) = self.pending.iter().position(|m| m["id"] == json!(id)) {
            return Some(self.pending.remove(i));
        }
        loop {
            let message = self.read_message()?;
            if message["id"] == json!(id) {
                return Some(message);
            }
            self.pending.push(message);
        }
    }

    /// `initialize` and `notifications/initialized`; returns the `initialize` result.
    pub fn initialize(&mut self, id: u64) -> Value {
        self.request(
            id,
            "initialize",
            json!({ "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": { "name": "m2a-test", "version": "0" } }),
        );
        let reply = self.response(id).expect("an initialize reply");
        self.send(json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }));
        reply["result"].clone()
    }

    /// Send `tools/call get_grant` with this id (no wait).
    pub fn call_get_grant(&mut self, id: u64) {
        self.request(id, "tools/call", json!({ "name": "get_grant", "arguments": {} }));
    }

    /// Send `tools/call` of `name` with `arguments` (no wait).
    pub fn call(&mut self, id: u64, name: &str, arguments: Value) {
        self.request(id, "tools/call", json!({ "name": name, "arguments": arguments }));
    }

    /// Call `name` and wait for its result: the text and `isError`, as the model would get them.
    pub fn call_and_wait(&mut self, id: u64, name: &str, arguments: Value) -> (String, bool) {
        self.call(id, name, arguments);
        tool_text(&self.response(id).unwrap_or_else(|| panic!("no reply to {name}")))
    }

    /// Whether the server has closed this connection: a read returns EOF within `within`.
    pub fn is_closed_by_server(&mut self, within: Duration) -> bool {
        self.set_read_timeout(within);
        let mut line = String::new();
        matches!(self.reader.read_line(&mut line), Ok(0))
    }
}

/// A tool result's text and its `isError`.
pub fn tool_text(response: &Value) -> (String, bool) {
    let result = &response["result"];
    let text = result["content"][0]["text"].as_str().unwrap_or_default().to_string();
    (text, result["isError"].as_bool().unwrap_or(false))
}
