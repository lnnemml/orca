//! The MCP orchestration server's core (ADR-027): the guard that decides whether an AI model may
//! spend compute (unit M1a, pure), the DB-level grant core, steps and resolver around it (unit M1b
//! Part A), and the sidecar's SMILES call (M1b Part B). The grant's Tauri commands are
//! `commands::mcp`; the `submit_job` guard is wired into the commands that submit by hand. The
//! transport cores (M2a Part A, [`server`]) are built and tested but not yet wired into the app.
//!
//! - [`header`] parses the model's header under the closed grammar of ADR-027 decision 3: `!`
//!   lines, `%…end` blocks (depth-tracked, `#` comments and quoted strings masked), `#` comments
//!   and blank lines. Anything else is a grammar violation.
//! - [`signature`] builds the method signature (decision 6) and holds the Rust copy of the TS
//!   `NON_METHOD` list, pinned to it by a parity test.
//! - [`allowlist`] judges every block, key, sub-block and value against decision 6's table.
//! - [`charge_mult`] reads a source job's charge and multiplicity strictly from its `* xyz` line.
//! - [`classify`] is `classify_mcp_job`: REFUSED rows R1–R9 first, then PROPOSED rows P1–P5, else
//!   SUBMITTED. It **fails closed**: what it does not recognise is never SUBMITTED.
//! - [`assemble`] builds the input (header, the app's `%maxcore`, the coordinate block) and
//!   re-parses it as its post-condition (rule #9).
//! - [`grant`] creates, revokes and reads the grant (schema v24), and builds the classifier's
//!   `GrantSnapshot`; every write is a `BEGIN IMMEDIATE` transaction.
//! - [`steps`] is step A (charge and insert), step C (finish or refund), the crash reconciliation
//!   and the `submit_job` guard.
//! - [`resolve`] maps a geometry reference (`job:`, `molecule:`, `smiles:`) to a `ResolvedSource`.
//! - [`sidecar_smiles`] is the resolver's SMILES seam over HTTP, with its post-condition.
//! - [`preflight`] computes the app's `%maxcore` and the memory verdict before step A.
//! - [`switch`] is the `mcp_enabled` row, which the generic `set_setting` refuses.
//! - [`server`] is the transport (M2a): the lock, the socket, the `rmcp` server with `get_grant`,
//!   the admission gate, the lifecycle and the `orcastudio --mcp` relay.
//! - [`tools`] holds the read tools' cores (M2b Part A): `list_jobs`, `get_job`, `get_results`,
//!   `get_geometry`, `measure`, `read_output`, `search_manual`, `list_run_targets`,
//!   `memory_estimate` and `list_groups`, each with its character cap. Not yet registered.
//!
//! The module page is `wiki/modules/mcp-server.md`.

pub mod allowlist;
pub mod assemble;
pub mod audit;
pub mod charge_mult;
pub mod classify;
#[cfg(unix)]
pub mod create_job;
pub mod grant;
pub mod header;
pub mod preflight;
pub mod resolve;
#[cfg(unix)]
pub mod server;
pub mod sidecar_smiles;
pub mod signature;
pub mod steps;
pub mod switch;
pub mod tools;

#[cfg(test)]
mod allowlist_tests;
#[cfg(test)]
mod assemble_tests;
#[cfg(test)]
mod charge_mult_tests;
#[cfg(test)]
mod classify_tests;
#[cfg(all(test, unix))]
pub(crate) mod create_job_test_support;
#[cfg(all(test, unix))]
mod create_job_tests;
#[cfg(test)]
mod grant_tests;
#[cfg(test)]
mod header_tests;
#[cfg(test)]
mod preflight_tests;
#[cfg(test)]
mod resolve_tests;
#[cfg(test)]
mod sidecar_smiles_tests;
#[cfg(test)]
mod signature_tests;
#[cfg(test)]
mod step_a_tests;
#[cfg(test)]
mod steps_tests;
#[cfg(test)]
mod submit_guard_tests;
#[cfg(test)]
pub(crate) mod test_db;
