//! The read count (ADR-027 M4 amendment, item 2): which tool calls are reads, each connection's
//! counters, and the lifecycle-level ledger that keeps a total since app start.
//!
//! - **What counts.** A call `call_tool` admits — after its permit is taken — to one of the twelve
//!   read tools of [`TOOL_KINDS`]. A call that then fails argument parsing still counts. A busy
//!   refusal (no permit), an unknown tool and a write tool never count. One table classifies every
//!   tool; `reads_tests` pins it equal to the service's tool list, so a new tool cannot go
//!   uncounted silently.
//! - **Per connection** ([`ReadCounts`], in `ConnState`): a total and one counter per read tool, all
//!   atomics. Counting takes **no lock** on the call path.
//! - **The ledger** ([`ReadLedger`]) lives in the lifecycle, outside the per-start `Registry`, so the
//!   total survives a switch-off and a switch-on; it is lost when the process ends (decision 9). It
//!   is one leaf mutex over the live connections' handles and the folded total:
//!   - a connection **enters** it at admission, right after `Registry::register` succeeds and before
//!     its service is spawned ([`ReadLedger::enter`]), and gets its stable id, the accept ordinal;
//!   - it is **folded** after its `drain()`, once no permit can be taken, so its counters are final
//!     ([`ReadLedger::fold`]): adding it to the folded total and removing its live entry happen under
//!     the one mutex;
//!   - the status snapshot ([`ReadLedger::snapshot`]) takes the same mutex. So each count is seen
//!     exactly once: in its live entry before the fold, in the folded total after it.
//! - **Lock nesting.** Nothing else is locked while the ledger mutex is held: its holders read
//!   atomics only, and no holder awaits.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use serde::Serialize;

use super::service::ConnState;

/// Whether a tool reads or writes (decision 7's "kind" column).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolKind {
    /// Counted per connection (this module).
    Read,
    /// Audited instead (`mcp::audit`), never counted.
    Write,
}

/// Every tool the service lists, classified (decision 7). The read tools come first, in the order
/// of [`ReadCounts`]'s per-tool counters.
pub const TOOL_KINDS: [(&str, ToolKind); 15] = [
    ("get_grant", ToolKind::Read),
    ("list_jobs", ToolKind::Read),
    ("get_job", ToolKind::Read),
    ("get_results", ToolKind::Read),
    ("get_geometry", ToolKind::Read),
    ("measure", ToolKind::Read),
    ("read_output", ToolKind::Read),
    ("search_manual", ToolKind::Read),
    ("list_run_targets", ToolKind::Read),
    ("memory_estimate", ToolKind::Read),
    ("list_groups", ToolKind::Read),
    ("wait_for_jobs", ToolKind::Read),
    ("create_job", ToolKind::Write),
    ("cancel_job", ToolKind::Write),
    ("create_group", ToolKind::Write),
];

/// How many read tools [`TOOL_KINDS`] lists (its first entries).
pub const READ_TOOL_COUNT: usize = 12;

/// The kind of `tool`, or `None` for a tool the table does not know.
pub fn kind_of(tool: &str) -> Option<ToolKind> {
    TOOL_KINDS.iter().find(|(name, _)| *name == tool).map(|(_, kind)| *kind)
}

/// The per-tool counter index of a read tool; `None` for a write tool or an unknown one.
fn read_index(tool: &str) -> Option<usize> {
    TOOL_KINDS[..READ_TOOL_COUNT].iter().position(|(name, kind)| *name == tool && *kind == ToolKind::Read)
}

/// One connection's read counters: lock-free.
#[derive(Debug, Default)]
pub struct ReadCounts {
    total: AtomicU64,
    per_tool: [AtomicU64; READ_TOOL_COUNT],
}

impl ReadCounts {
    /// Count one admitted call to `tool` if it is a read; a write or unknown tool is not counted.
    /// Returns whether it counted. Call it only once the call holds its permit.
    pub fn count(&self, tool: &str) -> bool {
        let Some(i) = read_index(tool) else { return false };
        self.per_tool[i].fetch_add(1, Ordering::SeqCst);
        self.total.fetch_add(1, Ordering::SeqCst);
        true
    }

    pub fn total(&self) -> u64 {
        self.total.load(Ordering::SeqCst)
    }

    /// The non-zero per-tool counts, by tool name.
    pub fn by_tool(&self) -> BTreeMap<String, u64> {
        TOOL_KINDS[..READ_TOOL_COUNT]
            .iter()
            .zip(&self.per_tool)
            .map(|((name, _), n)| (name.to_string(), n.load(Ordering::SeqCst)))
            .filter(|(_, n)| *n > 0)
            .collect()
    }
}

/// A connection entry's state in the status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ConnectionState {
    /// The transport is open.
    Open,
    /// The transport has closed (or MCP was switched off) and the connection is still draining.
    Closing,
}

/// One live or closing connection's reads, in the status.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ConnectionReads {
    /// The accept ordinal: stable across status polls, unique for the process.
    pub id: u64,
    pub state: ConnectionState,
    pub reads: u64,
    /// The non-zero counts, by tool name.
    pub by_tool: BTreeMap<String, u64>,
}

/// The status's `reads` (M4 amendment, item 2). It covers this instance only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct McpReads {
    /// Every read call this instance admitted since app start: the folded connections plus the
    /// live and closing ones.
    pub total: u64,
    /// One entry per live or closing connection, by id.
    pub connections: Vec<ConnectionReads>,
}

/// The lifecycle-level ledger: one leaf mutex.
#[derive(Default)]
pub struct ReadLedger {
    inner: Mutex<LedgerInner>,
}

#[derive(Default)]
struct LedgerInner {
    /// The last id handed out (ids start at 1).
    last_id: u64,
    /// The reads of every connection already folded.
    folded: u64,
    live: Vec<(u64, Arc<ConnState>)>,
}

impl ReadLedger {
    fn inner(&self) -> MutexGuard<'_, LedgerInner> {
        // Each field stays valid on its own across a panic; nothing under this mutex panics anyway.
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// A newly admitted connection enters the ledger; returns its id (the accept ordinal).
    pub fn enter(&self, conn: Arc<ConnState>) -> u64 {
        let mut inner = self.inner();
        inner.last_id += 1;
        let id = inner.last_id;
        inner.live.push((id, conn));
        id
    }

    /// Fold connection `id` after its drain: add its (final) reads to the folded total and remove its
    /// live entry, under one hold of the mutex. Folding an id twice, or an unknown one, does nothing.
    pub fn fold(&self, id: u64) {
        let mut inner = self.inner();
        if let Some(pos) = inner.live.iter().position(|(i, _)| *i == id) {
            let (_, conn) = inner.live.remove(pos);
            inner.folded += conn.reads.total();
        }
    }

    /// The status's `reads`, under the same mutex as the fold.
    pub fn snapshot(&self) -> McpReads {
        let inner = self.inner();
        let connections: Vec<ConnectionReads> = inner
            .live
            .iter()
            .map(|(id, conn)| ConnectionReads {
                id: *id,
                state: if conn.is_closed() { ConnectionState::Closing } else { ConnectionState::Open },
                reads: conn.reads.total(),
                by_tool: conn.reads.by_tool(),
            })
            .collect();
        let live: u64 = connections.iter().map(|c| c.reads).sum();
        McpReads { total: inner.folded + live, connections }
    }
}
