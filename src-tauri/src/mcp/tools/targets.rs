//! `list_run_targets` (ADR-027 decision 7; M2 amendment, item 8): this machine and each server
//! profile, with whether it is a run target (`is_run_target`, ADR-024 n item 2), its availability
//! window and whether now is outside it.
//!
//! **The window** (ADR-024 g and n item 3) is `HH:MM-HH:MM` in the laptop's local time, possibly
//! wrapping midnight (`22:00-08:00`). [`window_state`] is a pure function of the window and `now` as
//! local minutes since midnight: inside is from the start minute **included** to the end minute
//! **excluded**. Production takes `now` from SQLite's `datetime('now','localtime')`, the one measured
//! route to the laptop's local time (`db.rs`, the rule-#10 record under ADR-024 o22.14.6). The
//! state is a label, never an error (ADR-024 g).
//!
//! **Reachability** is `"unknown"` for every profile: the poller's last contact is per job and has
//! no timestamp (`poller/remote_state.rs`, `remote/contact.rs`), so no per-profile reachability
//! exists in Stage 1. This machine's is `"this_machine"`.

use rusqlite::Connection;
use serde::Serialize;

use super::{clip, within_cap, ToolError, RESULT_CAP_BYTES};
use crate::commands::server_profiles::{list_server_profiles_conn, RunTargetStatus};
use crate::models::server_profile::validate_window;

/// At most this many server profiles are listed (newest first), beside this machine.
pub const PROFILES_MAX: usize = 50;
/// A profile's name and host alias are each cut to this many serialised bytes.
pub const PROFILE_NAME_MAX_BYTES: usize = 120;

/// Where now lies against a target's availability window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WindowState {
    /// The target has no window.
    NoWindow,
    Inside,
    /// "Unreachable — outside expected window" (ADR-024 g): a label, not an error.
    Outside,
    /// The stored window does not read as `HH:MM-HH:MM` (writes are validated, so this is a damaged row).
    Unreadable,
}

/// Minutes since midnight of `HH:MM`, digits already checked by `validate_window`.
fn minutes(hhmm: &str) -> Option<u32> {
    let (h, m) = hhmm.split_once(':')?;
    Some(h.parse::<u32>().ok()? * 60 + m.parse::<u32>().ok()?)
}

/// Whether `now_min` (local minutes since midnight, 0..1440) lies inside `window`: from the start
/// minute included to the end minute excluded, wrapping midnight when the end is before the start.
/// Pure.
pub fn window_state(window: Option<&str>, now_min: u32) -> WindowState {
    let Some(window) = window else { return WindowState::NoWindow };
    if validate_window(window).is_err() {
        return WindowState::Unreadable;
    }
    let Some((start, end)) = window.split_once('-').and_then(|(a, b)| Some((minutes(a)?, minutes(b)?))) else {
        return WindowState::Unreadable;
    };
    let inside = if start < end { start <= now_min && now_min < end } else { now_min >= start || now_min < end };
    if inside {
        WindowState::Inside
    } else {
        WindowState::Outside
    }
}

/// The laptop's local time from SQLite (`datetime('now','localtime')`, `YYYY-MM-DD HH:MM:SS`) and
/// its minutes since midnight.
pub fn local_now(conn: &Connection) -> Result<(String, u32), ToolError> {
    let text: String = conn
        .query_row("SELECT datetime('now', 'localtime')", [], |r| r.get(0))
        .map_err(|e| ToolError::App(e.into()))?;
    let min = text
        .get(11..16)
        .and_then(minutes)
        .filter(|m| *m < 24 * 60)
        .ok_or_else(|| ToolError::App(crate::error::AppError::Internal(format!("SQLite's local time `{text}` is not readable"))))?;
    Ok((text, min))
}

/// One run target.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RunTargetView {
    /// `"local"` for this machine, else the server profile id (what `create_job`'s target names).
    pub target: String,
    pub name: String,
    /// The `~/.ssh/config` host alias; `None` for this machine.
    pub host: Option<String>,
    pub is_run_target: bool,
    /// Why a profile is not a run target; `None` when it is.
    pub not_run_target_reason: Option<String>,
    /// `HH:MM-HH:MM`, laptop local time; `None` for no window.
    pub availability_window_local: Option<String>,
    pub window_state: WindowState,
    /// `"unknown"` for a server (no per-profile reachability in Stage 1), `"this_machine"` locally.
    pub reachability: &'static str,
}

/// `list_run_targets`' answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RunTargets {
    /// The laptop's local time the windows were judged at (`YYYY-MM-DD HH:MM:SS`).
    pub local_time: String,
    pub targets: Vec<RunTargetView>,
    pub profile_count: usize,
    /// More than [`PROFILES_MAX`] profiles exist; the newest are listed.
    pub profiles_truncated: bool,
}

/// `list_run_targets` at `now` (`local_time`, `now_min`), for the tests' clock.
pub fn list_run_targets_at(conn: &Connection, local_time: String, now_min: u32) -> Result<RunTargets, ToolError> {
    let profiles = list_server_profiles_conn(conn).map_err(ToolError::App)?;
    let profile_count = profiles.len();
    let mut targets = vec![RunTargetView {
        target: crate::mcp::grant::LOCAL_TARGET.to_string(),
        name: "this machine".into(),
        host: None,
        is_run_target: true,
        not_run_target_reason: None,
        availability_window_local: None,
        window_state: WindowState::NoWindow,
        reachability: "this_machine",
    }];
    for p in profiles.into_iter().take(PROFILES_MAX) {
        let status = RunTargetStatus::of(&p);
        targets.push(RunTargetView {
            target: p.id,
            name: clip(&p.name, PROFILE_NAME_MAX_BYTES).0,
            host: Some(clip(&p.host, PROFILE_NAME_MAX_BYTES).0),
            is_run_target: status.is_run_target,
            not_run_target_reason: status.reason,
            window_state: window_state(p.availability_window.as_deref(), now_min),
            availability_window_local: p.availability_window.map(|w| clip(&w, 16).0),
            reachability: "unknown",
        });
    }
    let result = RunTargets { local_time, targets, profile_count, profiles_truncated: profile_count > PROFILES_MAX };
    within_cap(result, RESULT_CAP_BYTES)
}

/// `list_run_targets` now, by the laptop's local clock.
pub fn list_run_targets(conn: &Connection) -> Result<RunTargets, ToolError> {
    let (local_time, now_min) = local_now(conn)?;
    list_run_targets_at(conn, local_time, now_min)
}
