//! The MCP orchestration server's core (ADR-027): the guard that decides whether an AI model may
//! spend compute (unit M1a, pure), and the DB-level grant core, steps and resolver around it (unit
//! M1b Part A). No Tauri command, no socket and no HTTP yet: those are M1b Part B, M2 (the server)
//! and M3 (the write tools).
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
//! - [`preflight`] computes the app's `%maxcore` and the memory verdict before step A.
//!
//! The module page is `wiki/modules/mcp-server.md`.

pub mod allowlist;
pub mod assemble;
pub mod charge_mult;
pub mod classify;
pub mod grant;
pub mod header;
pub mod preflight;
pub mod resolve;
pub mod signature;
pub mod steps;

#[cfg(test)]
mod allowlist_tests;
#[cfg(test)]
mod assemble_tests;
#[cfg(test)]
mod charge_mult_tests;
#[cfg(test)]
mod classify_tests;
#[cfg(test)]
mod grant_tests;
#[cfg(test)]
mod header_tests;
#[cfg(test)]
mod preflight_tests;
#[cfg(test)]
mod resolve_tests;
#[cfg(test)]
mod signature_tests;
#[cfg(test)]
mod steps_tests;
#[cfg(test)]
mod test_db;
