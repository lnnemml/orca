//! `%maxcore` and the memory verdict for an MCP job (ADR-027 decision 4; C8), computed **before step
//! A**, outside any lock: [`crate::commands::memory::memory_estimate_with`] and
//! `local_fitted_maxcore_with` lock `DbState` themselves, so nothing here takes a lock of its own
//! (a second lock on the same mutex would deadlock).
//!
//! **`%maxcore`** ([`mcp_maxcore`]):
//! - Local: the builder's local default, `local_fitted_maxcore_with` (P; 300 when P is below it;
//!   2000 when this machine's RAM is unknown);
//! - a server: the 5.5 proposal P for its stored facts and core mask, when there is one; else
//!   [`SERVER_DEFAULT_MAXCORE_MIB`] (unknown memory, no mask, or P below 300). The verdict then
//!   says `Unknown` or `Warns`, so such a job is P4.
//!
//! **The verdict** ([`memory_verdict_for`], on the assembled input with the app's `%maxcore`):
//! `Warns` when the RAM or the disk warns; `Unknown` when any fact or the estimate itself is missing
//! (fail closed: what cannot be shown to fit is not `Fits`); else `Fits`.

use std::path::Path;

use serde::Serialize;

use super::assemble::SERVER_DEFAULT_MAXCORE_MIB;
use super::classify::{MemoryVerdict, Target};
use crate::commands::memory::{local_fitted_maxcore_with, memory_estimate_with};
use crate::commands::settings::DbState;
use crate::error::AppError;
use crate::memory::{DiskCheck, EstimateState, MemoryCheck, RamCheck};

/// Where the app's `%maxcore` came from, for the response ("set by the app").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MaxcoreSource {
    /// This machine: `local_fitted_maxcore_with`'s default.
    LocalFitted,
    /// A server: the 5.5 proposal P from its stored facts.
    ServerProposal,
    /// A server without a proposal: the builder's server default.
    ServerDefault,
}

/// The app's `%maxcore` for one job, in MiB.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct McpMaxcore {
    pub mib: u64,
    pub source: MaxcoreSource,
}

/// The one check that judges `target` in a memory-estimate reply.
fn check_for(
    db: &DbState,
    meminfo: &Path,
    data_dir: &Path,
    input: &str,
    target: &Target,
) -> Result<MemoryCheck, AppError> {
    match target {
        Target::Local => Ok(memory_estimate_with(db, meminfo, data_dir, 0, input, None)?.local),
        Target::Server { profile_id } => memory_estimate_with(db, meminfo, data_dir, 0, input, Some(profile_id))?
            .server
            .map(|s| s.check)
            .ok_or_else(|| AppError::Internal(format!("no memory check came back for server {profile_id}"))),
    }
}

/// The app's `%maxcore` for `header` on `target` (decision 4). Takes no lock of its own.
pub fn mcp_maxcore(db: &DbState, meminfo: &Path, data_dir: &Path, header: &str, target: &Target) -> Result<McpMaxcore, AppError> {
    match target {
        Target::Local => Ok(McpMaxcore {
            mib: local_fitted_maxcore_with(db, meminfo, data_dir)?.default_maxcore_mib,
            source: MaxcoreSource::LocalFitted,
        }),
        Target::Server { .. } => Ok(match check_for(db, meminfo, data_dir, header, target)?.proposal_mib {
            Some(p) => McpMaxcore { mib: p, source: MaxcoreSource::ServerProposal },
            None => McpMaxcore { mib: SERVER_DEFAULT_MAXCORE_MIB, source: MaxcoreSource::ServerDefault },
        }),
    }
}

/// The three-state verdict of one check (C8). Pure.
pub fn verdict_of(check: &MemoryCheck) -> MemoryVerdict {
    if check.warns {
        return MemoryVerdict::Warns;
    }
    let ram_known = matches!(check.ram, RamCheck::Known { warns: Some(_), .. });
    let disk_known = matches!(check.disk, DiskCheck::Known { .. });
    let estimated = matches!(check.estimate, EstimateState::Estimated(_));
    if ram_known && disk_known && estimated {
        MemoryVerdict::Fits
    } else {
        MemoryVerdict::Unknown
    }
}

/// The memory verdict for the assembled `input` on `target` (decision 4; C8). Takes no lock of its
/// own: `memory_estimate_with` locks `DbState` itself.
pub fn memory_verdict_for(
    db: &DbState,
    meminfo: &Path,
    data_dir: &Path,
    input: &str,
    target: &Target,
) -> Result<MemoryVerdict, AppError> {
    Ok(verdict_of(&check_for(db, meminfo, data_dir, input, target)?))
}
