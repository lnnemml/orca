//! `memory_estimate` (ADR-027 decisions 4 and 7): the memory and disk check a `create_job` with this
//! header on this target would get. The model never writes `%maxcore` (R3), so the estimate is of
//! the header **with the app's `%maxcore`** ([`mcp_maxcore`], added by `apply_maxcore`, the route the
//! assembly takes), checked by `memory_estimate_with` (ADR-024 o26). The verdict is the classifier's
//! P4 input ([`verdict_of`]).
//!
//! **Locking.** `mcp_maxcore` and `memory_estimate_with` lock `DbState` themselves, so this core
//! takes `&DbState` and holds no lock of its own; it must not be called under one.

use std::path::Path;

use serde::Serialize;

use super::{within_cap, ToolError, RESULT_CAP_BYTES};
use crate::commands::memory::memory_estimate_with;
use crate::commands::settings::DbState;
use crate::mcp::classify::{MemoryVerdict, Target};
use crate::mcp::grant::{target_of, target_text};
use crate::mcp::preflight::{mcp_maxcore, verdict_of, McpMaxcore};
use crate::memory::{apply_maxcore, read_maxcore, MemoryCheck};

/// A header longer than this is refused (the estimate reads only `%pal`, `PALn` and `%maxcore`).
pub const HEADER_MAX_CHARS: usize = 20_000;

/// `memory_estimate`'s answer. Every size names its unit (`_mib`, `_kib`, `_bytes`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MemoryEstimateView {
    /// `"local"` or the server profile id.
    pub target: String,
    /// The `%maxcore` the app would set, MiB, and where it comes from.
    pub app_maxcore: McpMaxcore,
    /// The check of the header with that `%maxcore`, on the target's facts.
    pub check: MemoryCheck,
    /// `Fits` only when every fact is known and nothing warns; `create_job` proposes otherwise (P4).
    pub verdict: MemoryVerdict,
}

/// `memory_estimate` for `header` on `target` (`"local"` or a profile id). Takes no lock of its own.
pub fn memory_estimate(db: &DbState, meminfo: &Path, data_dir: &Path, header: &str, target: &str) -> Result<MemoryEstimateView, ToolError> {
    if header.chars().count() > HEADER_MAX_CHARS {
        return Err(ToolError::refused(format!("the header is longer than {HEADER_MAX_CHARS} characters")));
    }
    match read_maxcore(header) {
        Ok(None) => {}
        Ok(Some(_)) => return Err(ToolError::refused("the app sets %maxcore; leave it out of the header")),
        Err(why) => return Err(ToolError::refused(format!("the header's %maxcore line does not read: {why}"))),
    }
    let target = target_of(target.to_string());
    let app_maxcore = mcp_maxcore(db, meminfo, data_dir, header, &target).map_err(ToolError::from_app)?;
    let input = apply_maxcore(header, app_maxcore.mib).map_err(ToolError::refused)?;
    let profile = match &target {
        Target::Local => None,
        Target::Server { profile_id } => Some(profile_id.as_str()),
    };
    let reply = memory_estimate_with(db, meminfo, data_dir, 0, &input, profile).map_err(ToolError::from_app)?;
    let check = match (&target, reply.server) {
        (Target::Local, _) => reply.local,
        (Target::Server { .. }, Some(server)) => server.check,
        (Target::Server { profile_id }, None) => {
            return Err(ToolError::App(crate::error::AppError::Internal(format!("no memory check came back for server {profile_id}"))))
        }
    };
    let view = MemoryEstimateView { target: target_text(&target).to_string(), app_maxcore, verdict: verdict_of(&check), check };
    within_cap(view, RESULT_CAP_BYTES)
}
