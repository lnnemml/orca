//! The MCP read tools' cores (ADR-027 decision 7; M2 amendment, items 1, 7 and 8; unit M2b).
//! Each core takes a `&Connection` (or `&DbState`, for the memory estimate, which locks itself) or
//! plain inputs, and returns a serialisable result. `server::read_tools` registers each in the
//! `rmcp` service, with its description, its arguments and its lock.
//!
//! | tool | core | reuses |
//! |---|---|---|
//! | `list_jobs` | [`jobs::list_jobs`] | `Job::COLUMNS` / `Job::from_row`, filtered and paged |
//! | `get_job` | [`jobs::get_job`] | `get_job_conn`, projected; the header built from `parse_header` + the allowlist ([`header_view`]), never raw text |
//! | `get_results` | [`results::get_results`] | `read_job_results`, summarised |
//! | `get_geometry` | [`geometry::get_geometry`] | `resolve_db_conn`; refused above 600 atoms |
//! | `measure` | [`geometry::measure`] | `resolve_db_conn`, `xtb::distance`/`angle`; the dihedral is ASE's formula (`xtb::dihedral` is 360° − ASE) |
//! | `read_output` | [`output::read_output_at`] | `read_tail_window`, `search_output` |
//! | `search_manual` | [`manual::search_manual`] | `manual::index::search_manual` |
//! | `list_run_targets` | [`targets::list_run_targets`] | `list_server_profiles_conn`, `RunTargetStatus::of` |
//! | `memory_estimate` | [`memory::memory_estimate`] | `mcp_maxcore`, `memory_estimate_with` |
//! | `list_groups` | [`groups::list_groups`] | `list_groups_conn`, paged |
//!
//! **Units (rule #11).** Every number a tool returns names its unit in its field name: `_eh`,
//! `_ang`, `_deg`, `_cm1`, `_km_mol`, `_k`, `_s`, `_mib`, `_kib`, `_bytes`. A count ends in
//! `_count`; an index is named so. Times stored by SQLite's `datetime('now')` are UTC on the
//! laptop's clock and end in `_utc`.
//!
//! **Size (M2 amendment, item 7).** The client spills a result longer than
//! [`CLIENT_SPILL_CHARS`] characters to a file (M0, measured on ASCII text only). Every core keeps
//! its result at or under [`RESULT_CAP_BYTES`] by its own caps (pages, cut fields, the 600-atom cap,
//! the bounded output reads), states any paging or cut in the result, and then checks the
//! serialised result as a post-condition ([`within_cap`]): a result over the cap is an error, never
//! served.
//!
//! **The unit is the UTF-8 byte.** What the client counts for non-ASCII text (Unicode scalar values,
//! UTF-16 units, or something else) is not measured. A UTF-8 byte count is at least each of those,
//! so it is the conservative bound: every size here ([`size_of_text`], every `_BYTES` budget, and
//! [`clip`]) counts the bytes of the **serialised** text, JSON escapes included.
//!
//! **Refusal text** echoes caller input cut to [`ECHO_MAX_BYTES`] ([`echo`]), and every refusal
//! message is cut to [`REFUSAL_MAX_BYTES`] ([`ToolError::refused`]).

use serde::Serialize;
use thiserror::Error;

use crate::error::AppError;

pub mod geometry;
pub mod groups;
pub mod header_view;
pub mod jobs;
pub mod manual;
pub mod memory;
pub mod output;
pub mod results;
pub mod targets;

#[cfg(test)]
mod budget_tests;
#[cfg(test)]
mod geometry_tests;
#[cfg(test)]
mod groups_tests;
#[cfg(test)]
mod header_view_tests;
#[cfg(test)]
mod jobs_tests;
#[cfg(test)]
mod manual_tests;
#[cfg(test)]
mod memory_tests;
#[cfg(test)]
mod output_tests;
#[cfg(test)]
mod results_tests;
#[cfg(test)]
mod targets_tests;
#[cfg(test)]
pub(crate) mod test_support;

/// The client writes a result longer than this to a file and hands the model a pointer instead
/// (M0, measured: 49,000 characters inline, 51,000 spilled; `mcp-transport-probe.md`, item 3).
pub const CLIENT_SPILL_CHARS: usize = 50_000;

/// Every read tool's serialised result stays at or under this many UTF-8 bytes: at least 10,000
/// below the client's threshold, whatever unit it counts in.
pub const RESULT_CAP_BYTES: usize = 40_000;

/// Caller input echoed in a refusal is cut to this many bytes.
pub const ECHO_MAX_BYTES: usize = 200;
/// A refusal message is cut to this many bytes.
pub const REFUSAL_MAX_BYTES: usize = 600;

/// Why a read tool returned no result.
#[derive(Debug, Error)]
pub enum ToolError {
    /// The request cannot be served as asked, and the message says why: an unknown id, a filter
    /// outside its values, a source with no geometry, a geometry over the atom cap.
    #[error("{0}")]
    Refused(String),
    /// A measurement that is undefined or out of range (`measure`).
    #[error(transparent)]
    Measure(#[from] geometry::MeasureRefusal),
    /// The post-condition of item 7: the serialised result is over the cap. Never served.
    #[error("the result is {bytes} bytes, over this tool's cap of {cap}")]
    TooLarge { bytes: usize, cap: usize },
    #[error(transparent)]
    App(#[from] AppError),
}

impl ToolError {
    /// A refusal, its message cut to [`REFUSAL_MAX_BYTES`]: a message built from caller input (or
    /// from a reused reader's error that quotes it) never grows with that input.
    pub fn refused(message: impl AsRef<str>) -> ToolError {
        ToolError::Refused(clip(message.as_ref(), REFUSAL_MAX_BYTES).0)
    }

    /// An app error, with a missing row read as the caller's mistake ("no job x"), not an app fault.
    pub fn from_app(e: AppError) -> ToolError {
        match e {
            AppError::NotFound(what) => ToolError::refused(format!("{} does not exist", echo(&what))),
            other => ToolError::App(other),
        }
    }
}

/// Caller input as a refusal quotes it: cut to [`ECHO_MAX_BYTES`].
pub fn echo(input: &str) -> String {
    clip(input, ECHO_MAX_BYTES).0
}

/// The size of a serialised text as the caps count it: UTF-8 bytes (see the module docs).
pub fn size_of_text(text: &str) -> usize {
    text.len()
}

/// The result as the tool's text content: compact JSON.
pub fn result_text<T: Serialize>(value: &T) -> Result<String, ToolError> {
    serde_json::to_string(value).map_err(|e| ToolError::App(AppError::Internal(format!("serialise a tool result: {e}"))))
}

/// The size post-condition: `value` serialises to at most `cap` bytes, or the call fails.
pub fn within_cap<T: Serialize>(value: T, cap: usize) -> Result<T, ToolError> {
    let bytes = size_of_text(&result_text(&value)?);
    if bytes > cap {
        return Err(ToolError::TooLarge { bytes, cap });
    }
    Ok(value)
}

/// The bytes `c` takes inside a serialised JSON string (`serde_json`'s escapes: `\"`, `\\`, the
/// short escapes of `\n` `\r` `\t` `\b` `\f`, `\u00XX` for every other control character).
fn escaped_len(c: char) -> usize {
    match c {
        '"' | '\\' | '\n' | '\r' | '\t' | '\u{8}' | '\u{c}' => 2,
        c if (c as u32) < 0x20 => 6,
        c => c.len_utf8(),
    }
}

/// `text` cut so that its serialised form (escapes included, quotes excluded) takes at most `max`
/// bytes, with `…` as the last character when cut. The flag says whether it was cut. So a field
/// full of control characters is cut, not refused `TooLarge`.
pub fn clip(text: &str, max: usize) -> (String, bool) {
    if text.chars().map(escaped_len).sum::<usize>() <= max {
        return (text.to_string(), false);
    }
    let budget = max.saturating_sub('…'.len_utf8());
    let mut used = 0;
    let kept: String = text
        .chars()
        .take_while(|c| {
            used += escaped_len(*c);
            used <= budget
        })
        .collect();
    (kept + "…", true)
}

/// A page request's limit: the default when absent, never above the maximum, never 0.
pub fn page_limit(asked: Option<u32>, default: u32, max: u32) -> u32 {
    asked.unwrap_or(default).clamp(1, max)
}
