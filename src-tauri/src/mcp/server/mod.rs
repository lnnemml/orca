//! The MCP transport (ADR-027 unit M2a): the lock, the socket, the server and its caps, the
//! admission gate, the lifecycle that switches it all on and off, and the `orcastudio --mcp`
//! relay. [`wiring`] connects them to the app: `lib.rs` setup (the lifecycle and the tick thread)
//! and `ExitRequested`, and `main.rs`'s relay branch; `commands::mcp` has `mcp_status` and
//! `mcp_set_enabled`, which Settings → MCP calls.
//!
//! - [`paths`] resolves `$XDG_RUNTIME_DIR/orcastudio/{mcp.lock,mcp.sock}` (or the data-dir
//!   fallback) and refuses a socket path over `sun_path`.
//! - [`lock`] is the `flock`, opened `O_CLOEXEC`, with `FD_CLOEXEC` asserted.
//! - [`socket`] creates the `0700` directory, binds the `0600` socket, `stat`s both, and unlinks
//!   only the inode it bound.
//! - [`listener`] accepts: the `SO_PEERCRED` check, the 4 connection slots, the per-start registry.
//! - [`service`] is the `rmcp` service of one connection: the 4-permit request cap, the `closed`
//!   flag, `get_grant`, and the recovery sentence.
//! - [`gate`] is the admission gate over `TaskTracker` generations.
//! - [`lifecycle`] owns every transition under one mutex, the status, and the exit flow.
//! - [`tick`] is the 30 s tick driver.
//! - [`relay`] is the stdio ↔ socket byte relay.
//! - [`wiring`] is the app's side: the managed `DbSource`, the tick thread, the exit request and its
//!   backstop, and the relay's entry point.

use crate::commands::settings::DbState;

pub mod gate;
pub mod lifecycle;
pub mod listener;
pub mod lock;
pub mod paths;
pub mod relay;
pub mod service;
pub mod socket;
pub mod tick;
pub mod wiring;

#[cfg(test)]
mod gate_tests;
#[cfg(test)]
mod lifecycle_tests;
#[cfg(test)]
mod lock_tests;
#[cfg(test)]
mod paths_tests;
#[cfg(test)]
mod relay_tests;
#[cfg(test)]
mod server_tests;
#[cfg(test)]
mod socket_tests;
#[cfg(test)]
pub(crate) mod test_support;
#[cfg(test)]
mod wiring_tests;

/// At most this many connections are served at once (decision 1). A further one is closed at once.
pub const CONNECTIONS: usize = 4;
/// Each connection has at most this many requests in flight (M0 amendment, item 5).
pub const REQUESTS_PER_CONNECTION: usize = 4;

/// Where the server reaches the database: the app's managed `DbState` in production (Part B), a
/// scratch one in tests. Handlers lock it for one query or transaction, off the async workers.
pub trait DbSource: Send + Sync + 'static {
    fn db(&self) -> &DbState;
}

impl DbSource for DbState {
    fn db(&self) -> &DbState {
        self
    }
}
