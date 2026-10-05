//! Remote-job reconciliation: the pure classifier of ADR-024 Decision l (Phase 5 unit 5.2).
//!
//! A remote ORCA job runs under a static wrapper script inside a per-slot `tsp` queue on the
//! server. The server filesystem is the source of truth (ADR-024 c), so whenever the app
//! reconnects it collects a **snapshot of raw facts** about each non-terminal job (marker files,
//! `/proc` lines, `tsp -l` rows) and decides the job's state here, in Rust (rule #9: never a
//! remote verdict such as `alive=yes`).
//!
//! The decision does **no** I/O: no ssh, no files, no processes. The facts come from three
//! static scripts that run on the server ([`scripts`]): the wrapper that starts ORCA, the cancel
//! script, and the collector, whose length-prefixed output [`wire`] parses into a
//! [`snapshot::Snapshot`]. The wiring into `SshBackend` and the DB is units 5.3/5.4.
//!
//! - [`markers`] — strict parsers for the job-dir markers `.started` and `.exit_code`, and for a
//!   `boot_id`.
//! - [`procfs`] — strict parsers for `/proc/<pid>/stat`, `/proc/<pid>/cmdline` and
//!   `/proc/net/unix`.
//! - [`tsp`] — matching a `tsp -l` row to a job by its job dir (whole token) and reading its
//!   state.
//! - [`snapshot`] — the raw-fact snapshot and the job's identity.
//! - [`classify`] — the predicates ("alive", "ours", job session, SID-reuse guard) and
//!   [`classify::classify`], the 11-row precedence table.
//! - [`scripts`] — the embedded `wrapper.sh`, `cancel.sh` and `collect.sh`, and their sha256.
//! - [`wire`] — the strict parser of the collector's output into a snapshot.
//! - [`ssh`] — the one `ssh … -- <host> bash -s` argv and a process runner with a hard timeout.

pub mod classify;
pub mod markers;
pub mod procfs;
pub mod scripts;
pub mod snapshot;
pub mod ssh;
pub mod tsp;
pub mod wire;

#[cfg(test)]
mod race_model;
#[cfg(test)]
mod script_tests;

/// Upper bound on a `tsp` socket path, in bytes. The kernel's `sun_path` is 108 bytes including
/// the NUL (`man 7 unix`, `UNIX_PATH_MAX`); submit and profile save keep a margin below it
/// (ADR-024 l, round 4 N-3).
pub const MAX_SOCKET_PATH_BYTES: usize = 100;

/// The socket path of slot `slot` under the remote root: `<root>/tsp/slot<slot>.sock` (ADR-024 n
/// item 4: the `tsp/` sockets live under the root). The one place this layout is spelled out;
/// profile save bounds it by [`MAX_SOCKET_PATH_BYTES`], and 5.3's submit uses it.
pub fn slot_socket_path(root: &str, slot: u32) -> String {
    format!("{root}/tsp/slot{slot}.sock")
}

/// A raw fact from the server that does not parse. Every parser here is strict: a malformed
/// input is this error, never a default value (rule #9). Where the precedence table assigns a
/// meaning to a parse failure (a corrupt `.started` is row 1, a bad `.exit_code` is row 6), the
/// classifier turns the error into that outcome; anywhere else it surfaces as a
/// [`classify::SnapshotError`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FactError {
    #[error(".started: {0}")]
    Started(String),

    #[error(".exit_code: {0}")]
    ExitCode(String),

    #[error("boot_id: {0}")]
    BootId(String),

    #[error("/proc/<pid>/stat: {0}")]
    Stat(String),

    #[error("/proc/net/unix: {0}")]
    NetUnix(String),

    #[error("tsp -l row: {0}")]
    TspRow(String),
}
