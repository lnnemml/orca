//! The `ServerProfile` model (schema v18, Phase 5 unit 5.1, ADR-023).
//!
//! A **server profile** is the runtime configuration of one remote execution target:
//! a `~/.ssh/config` host alias (ADR-005 — the app stores NO credentials, auth stays
//! with the user's SSH setup), the remote absolute ORCA path (rule #1), the remote
//! scratch dir where isolated job dirs live (rule #3), and the core-pinning mask
//! (rule #8). It is **data, not code** — "add a server" is a settings action, not a
//! build (ADR-023): one `SshBackend` is parameterized by any number of these rows.
//!
//! The four `verified_*` fields (`orca_version`, `openmpi_version`, `core_count`,
//! `verified_at`) are the connection-test's rule-#10 measurement, stored as DISCRETE
//! typed columns (never a JSON blob). `verified_at` is the usability gate: a profile
//! with `verified_at IS NULL` has NOT passed the connection-test and is **not offered
//! as a run target** (ADR-023). Part B's real SSH connection-test stamps these via
//! `set_profile_verified`; until then they are `None` (honest-or-absent, never a
//! guessed stand-in).

use rusqlite::Row;
use serde::Serialize;

/// A remote execution target. Mirrors the `server_profiles` table one-to-one.
#[derive(Debug, Clone, Serialize)]
pub struct ServerProfile {
    pub id: String,
    /// User-facing display name (e.g. "uni cluster").
    pub name: String,
    /// `~/.ssh/config` host alias — the transport handle (ADR-005). NOT a hostname the
    /// app resolves; the user's SSH config owns connection + auth details.
    pub host: String,
    /// Absolute path to the remote `orca` binary (rule #1 — ORCA is always invoked by
    /// its full absolute path, or OpenMPI parallelization silently fails).
    pub remote_orca_path: String,
    /// Remote directory under which each calculation gets its own isolated job dir
    /// (rule #3).
    pub remote_scratch_dir: String,
    /// `taskset` core mask for CPU pinning (rule #8). `None` until measured by the
    /// performance probe — the connection-test only establishes the `core_count`
    /// ceiling; the mask itself is a separate rule-#8 measurement.
    pub core_mask: Option<String>,
    /// Verified remote ORCA version (connection-test, rule #10). `None` until verified.
    pub orca_version: Option<String>,
    /// Verified remote OpenMPI version (rule #2 — must match the ORCA build). `None`
    /// until verified.
    pub openmpi_version: Option<String>,
    /// Verified logical CPU count (`nproc` ceiling, rule #8). `None` until verified.
    pub core_count: Option<u32>,
    /// Timestamp of the last successful connection-test. **`None` = not verified = not a
    /// run target** (ADR-023). This is the profile's usability gate.
    pub verified_at: Option<String>,
    pub created_at: String,
}

impl ServerProfile {
    /// Column list used by every `SELECT` that hydrates a [`ServerProfile`]. The order
    /// here is the contract [`ServerProfile::from_row`] relies on.
    pub const COLUMNS: &'static str = "id, name, host, remote_orca_path, remote_scratch_dir, \
         core_mask, orca_version, openmpi_version, core_count, verified_at, created_at";

    /// Build a [`ServerProfile`] from a row selected in [`ServerProfile::COLUMNS`] order.
    pub fn from_row(row: &Row) -> rusqlite::Result<ServerProfile> {
        Ok(ServerProfile {
            id: row.get(0)?,
            name: row.get(1)?,
            host: row.get(2)?,
            remote_orca_path: row.get(3)?,
            remote_scratch_dir: row.get(4)?,
            core_mask: row.get(5)?,
            orca_version: row.get(6)?,
            openmpi_version: row.get(7)?,
            core_count: row.get(8)?,
            verified_at: row.get(9)?,
            created_at: row.get(10)?,
        })
    }
}
