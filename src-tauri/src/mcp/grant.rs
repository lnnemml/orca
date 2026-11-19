//! The grant core (ADR-027 decision 5, unit M1b): create, revoke and read the grant that lets the
//! MCP server spend compute without Anton, and the snapshot the classifier's P5 rows read.
//!
//! **The invariants are the database's, not this file's.** At most one grant is unrevoked (the v24
//! expression index `mcp_grants_one_unrevoked`); `charged_jobs` stays within `0..=max_jobs` (a
//! CHECK). Every write here runs in a `BEGIN IMMEDIATE` transaction ([`begin_immediate`]), so two
//! app instances on one database file are ordered by SQLite's write lock, not by luck.
//!
//! **The counters (decided by Anton).**
//! - `charged_jobs` is **stored**: step A increments it and only a refund decrements it, so
//!   deleting a job never re-opens the budget.
//! - `in_flight` is **derived**: every job that is not terminal and has a non-null `grant_id`,
//!   under **any** grant, a `draft` still in step B included ([`in_flight_conn`]).
//!
//! **Time.** Every time here is SQLite `datetime('now')` text, `YYYY-MM-DD HH:MM:SS`, **UTC on the
//! laptop's clock** (no server clock is involved, so ADR-024 (j) does not apply). `expires_at` is
//! compared as text, which orders correctly only in that exact form, so every writer checks it
//! first ([`check_utc_text`]). The caller reads `now` with [`now_utc_conn`].

use rusqlite::{params, Connection, OptionalExtension, Transaction, TransactionBehavior};
use serde::Serialize;
use uuid::Uuid;

use super::classify::{GrantSnapshot, GrantStatus, Target};
use super::signature::MethodSignature;
use crate::error::AppError;
use crate::models::job::JobStatus;

/// The JSON spelling of [`Target::Local`] in `mcp_grants.targets`; a server is its profile id.
pub const LOCAL_TARGET: &str = "local";

/// A grant as stored.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Grant {
    pub id: String,
    pub created_at: String,
    pub expires_at: String,
    pub revoked_at: Option<String>,
    pub targets: Vec<Target>,
    pub levels: Vec<MethodSignature>,
    pub max_jobs: u32,
    pub max_in_flight: u32,
    pub charged_jobs: u32,
}

/// What Anton asks for in Settings → MCP (M4 builds it).
#[derive(Debug, Clone, PartialEq)]
pub struct NewGrant {
    pub targets: Vec<Target>,
    /// Each level's text: a `!` line and, optionally, a `%cpcm` block. Read strictly
    /// ([`MethodSignature::of_level_text`]).
    pub levels: Vec<String>,
    pub max_jobs: u32,
    pub max_in_flight: u32,
    /// UTC, laptop clock, `YYYY-MM-DD HH:MM:SS`; must lie after `now`.
    pub expires_at: String,
}

/// The newest grant and its counters, for step A and the classifier.
#[derive(Debug, Clone, PartialEq)]
pub struct CurrentGrant {
    pub id: String,
    pub snapshot: GrantSnapshot,
}

/// Open a `BEGIN IMMEDIATE` transaction on the shared connection: the write lock is taken at
/// `BEGIN`, so every read inside it sees what the write will be judged against. Rolled back on
/// drop unless committed.
pub(crate) fn begin_immediate(conn: &Connection) -> Result<Transaction<'_>, AppError> {
    Ok(Transaction::new_unchecked(conn, TransactionBehavior::Immediate)?)
}

/// SQLite's `datetime('now')`: UTC, laptop clock (`db::tests::sqlite_datetime_now_is_utc_under_a_non_utc_tz`).
pub(crate) fn now_utc_conn(conn: &Connection) -> Result<String, AppError> {
    Ok(conn.query_row("SELECT datetime('now')", [], |r| r.get(0))?)
}

/// Check that `s` is exactly `YYYY-MM-DD HH:MM:SS` with plausible fields, the form `datetime('now')`
/// writes and the only one whose text order is time order.
pub fn check_utc_text(s: &str) -> Result<(), String> {
    let b = s.as_bytes();
    let shape_ok = b.len() == 19
        && b.iter().enumerate().all(|(i, c)| match i {
            4 | 7 => *c == b'-',
            10 => *c == b' ',
            13 | 16 => *c == b':',
            _ => c.is_ascii_digit(),
        });
    if !shape_ok {
        return Err(format!("`{s}` is not a UTC time `YYYY-MM-DD HH:MM:SS`"));
    }
    let num = |r: std::ops::Range<usize>| s[r].parse::<u32>().unwrap_or(u32::MAX);
    let (mo, d, h, mi, se) = (num(5..7), num(8..10), num(11..13), num(14..16), num(17..19));
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || h > 23 || mi > 59 || se > 59 {
        return Err(format!("`{s}` has a field out of range"));
    }
    Ok(())
}

fn target_text(t: &Target) -> &str {
    match t {
        Target::Local => LOCAL_TARGET,
        Target::Server { profile_id } => profile_id,
    }
}

fn target_of(text: String) -> Target {
    if text == LOCAL_TARGET {
        Target::Local
    } else {
        Target::Server { profile_id: text }
    }
}

/// Decode a stored JSON column; a malformed one is an error, never an empty list (rule #9).
fn decode<T: serde::de::DeserializeOwned>(grant: &str, col: &str, json: &str) -> Result<T, AppError> {
    serde_json::from_str(json).map_err(|e| AppError::Internal(format!("grant {grant}: {col} is not readable: {e}")))
}

/// A stored count as `u32`; the CHECKs keep it ≥ 0, and anything else is an error.
fn count(grant: &str, col: &str, v: i64) -> Result<u32, AppError> {
    u32::try_from(v).map_err(|_| AppError::Internal(format!("grant {grant}: {col} = {v} is out of range")))
}

const GRANT_COLUMNS: &str =
    "id, created_at, expires_at, revoked_at, targets, levels, max_jobs, max_in_flight, charged_jobs";

type GrantRow = (String, String, String, Option<String>, String, String, i64, i64, i64);

fn grant_row(r: &rusqlite::Row) -> rusqlite::Result<GrantRow> {
    Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?, r.get(7)?, r.get(8)?))
}

fn grant_of(row: GrantRow) -> Result<Grant, AppError> {
    let (id, created_at, expires_at, revoked_at, targets, levels, max_jobs, max_in_flight, charged) = row;
    let targets: Vec<String> = decode(&id, "targets", &targets)?;
    let levels: Vec<MethodSignature> = decode(&id, "levels", &levels)?;
    Ok(Grant {
        max_jobs: count(&id, "max_jobs", max_jobs)?,
        max_in_flight: count(&id, "max_in_flight", max_in_flight)?,
        charged_jobs: count(&id, "charged_jobs", charged)?,
        targets: targets.into_iter().map(target_of).collect(),
        levels,
        id,
        created_at,
        expires_at,
        revoked_at,
    })
}

fn query_grant(conn: &Connection, where_order: &str, p: &[&dyn rusqlite::ToSql]) -> Result<Option<Grant>, AppError> {
    let sql = format!("SELECT {GRANT_COLUMNS} FROM mcp_grants {where_order} LIMIT 1");
    conn.query_row(&sql, p, grant_row).optional()?.map(grant_of).transpose()
}

/// Grant `id`, or [`AppError::NotFound`].
pub fn get_grant_conn(conn: &Connection, id: &str) -> Result<Grant, AppError> {
    query_grant(conn, "WHERE id = ?1", &[&id])?.ok_or_else(|| AppError::NotFound(format!("grant {id}")))
}

/// The active grant: unrevoked and `expires_at > now`. `None` when there is none.
pub fn active_grant_conn(conn: &Connection, now: &str) -> Result<Option<Grant>, AppError> {
    check_utc_text(now).map_err(AppError::Internal)?;
    query_grant(conn, "WHERE revoked_at IS NULL AND expires_at > ?1", &[&now])
}

/// The SQL list of terminal statuses, built from [`JobStatus::is_terminal`] so the two cannot drift.
fn terminal_status_list() -> String {
    JobStatus::ALL
        .iter()
        .filter(|s| s.is_terminal())
        .map(|s| format!("'{}'", s.as_str()))
        .collect::<Vec<_>>()
        .join(", ")
}

/// `max_in_flight`'s count (decided by Anton): every job that is not terminal and has a non-null
/// `grant_id`, under **any** grant. A `draft` in step B counts (its `grant_id` is set at step A);
/// a refunded draft does not (its `grant_id` is NULL).
pub fn in_flight_conn(conn: &Connection) -> Result<u32, AppError> {
    let sql = format!(
        "SELECT COUNT(*) FROM jobs WHERE grant_id IS NOT NULL AND status NOT IN ({})",
        terminal_status_list()
    );
    let n: i64 = conn.query_row(&sql, [], |r| r.get(0))?;
    count("(all)", "in_flight", n)
}

/// The newest grant and its counters: the unrevoked one if any (`Active`, or `Expired` once
/// `expires_at <= now`), else the most recently created revoked one (`Revoked`). `None` when no
/// grant was ever made.
pub fn grant_snapshot_conn(conn: &Connection, now: &str) -> Result<Option<CurrentGrant>, AppError> {
    check_utc_text(now).map_err(AppError::Internal)?;
    let grant = match query_grant(conn, "WHERE revoked_at IS NULL", &[])? {
        Some(g) => Some(g),
        None => query_grant(conn, "ORDER BY created_at DESC, rowid DESC", &[])?,
    };
    let Some(g) = grant else { return Ok(None) };
    let status = if g.revoked_at.is_some() {
        GrantStatus::Revoked
    } else if g.expires_at.as_str() <= now {
        GrantStatus::Expired
    } else {
        GrantStatus::Active
    };
    Ok(Some(CurrentGrant {
        snapshot: GrantSnapshot {
            status,
            targets: g.targets,
            levels: g.levels,
            charged_jobs: g.charged_jobs,
            max_jobs: g.max_jobs,
            in_flight: in_flight_conn(conn)?,
            max_in_flight: g.max_in_flight,
        },
        id: g.id,
    }))
}

/// Check a new grant before any write; the levels come back as signatures, de-duplicated in order.
fn validate(new: &NewGrant, now: &str) -> Result<Vec<MethodSignature>, AppError> {
    let invalid = |m: String| Err(AppError::Invalid(m));
    check_utc_text(now).map_err(AppError::Internal)?;
    check_utc_text(&new.expires_at).map_err(AppError::Invalid)?;
    if new.expires_at.as_str() <= now {
        return invalid(format!("the grant would expire at {} UTC, which is not after now ({now} UTC)", new.expires_at));
    }
    if new.max_jobs < 1 {
        return invalid("max_jobs must be at least 1".into());
    }
    if new.max_in_flight < 1 {
        return invalid("max_in_flight must be at least 1".into());
    }
    if new.targets.is_empty() {
        return invalid("a grant names at least one target".into());
    }
    if new.levels.is_empty() {
        return invalid("a grant names at least one level".into());
    }
    let mut levels = Vec::new();
    for (i, text) in new.levels.iter().enumerate() {
        let sig = MethodSignature::of_level_text(text).map_err(|e| AppError::Invalid(format!("level {}: {e}", i + 1)))?;
        if !levels.contains(&sig) {
            levels.push(sig);
        }
    }
    Ok(levels)
}

/// Create a grant (ADR-027 decision 5): in one `BEGIN IMMEDIATE` transaction, revoke whatever grant
/// is unrevoked, then insert this one. Refused before any write: a level that does not parse
/// cleanly or names no method token, `max_jobs` or `max_in_flight` below 1, an `expires_at` not
/// after `now`, no target or level, a server target with no profile.
pub fn create_grant_conn(conn: &Connection, new: &NewGrant, now: &str) -> Result<Grant, AppError> {
    let levels = validate(new, now)?;
    let mut targets: Vec<&str> = Vec::new();
    for t in &new.targets {
        let text = target_text(t);
        if matches!(t, Target::Server { .. }) && text == LOCAL_TARGET {
            return Err(AppError::Invalid(format!("a server profile id cannot be `{LOCAL_TARGET}`")));
        }
        if !targets.contains(&text) {
            targets.push(text);
        }
    }
    let targets_json = serde_json::to_string(&targets).map_err(|e| AppError::Internal(format!("grant targets: {e}")))?;
    let levels_json = serde_json::to_string(&levels).map_err(|e| AppError::Internal(format!("grant levels: {e}")))?;
    let id = Uuid::new_v4().to_string();

    let tx = begin_immediate(conn)?;
    for t in &new.targets {
        if let Target::Server { profile_id } = t {
            let exists: bool =
                tx.query_row("SELECT EXISTS (SELECT 1 FROM server_profiles WHERE id = ?1)", [profile_id], |r| r.get(0))?;
            if !exists {
                return Err(AppError::NotFound(format!("server profile {profile_id}")));
            }
        }
    }
    tx.execute("UPDATE mcp_grants SET revoked_at = ?1 WHERE revoked_at IS NULL", [now])?;
    tx.execute(
        "INSERT INTO mcp_grants (id, created_at, expires_at, targets, levels, max_jobs, max_in_flight) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![id, now, new.expires_at, targets_json, levels_json, new.max_jobs, new.max_in_flight],
    )?;
    let grant = get_grant_conn(&tx, &id)?;
    tx.commit()?;
    Ok(grant)
}

/// Revoke grant `id` (ADR-027 decision 5), in a `BEGIN IMMEDIATE` transaction: whichever of a step A
/// and this revoke commits first wins. Revoking an already revoked grant changes nothing (its first
/// `revoked_at` is kept). Running jobs are not cancelled.
pub fn revoke_grant_conn(conn: &Connection, id: &str, now: &str) -> Result<Grant, AppError> {
    check_utc_text(now).map_err(AppError::Internal)?;
    let tx = begin_immediate(conn)?;
    get_grant_conn(&tx, id)?;
    tx.execute("UPDATE mcp_grants SET revoked_at = ?1 WHERE id = ?2 AND revoked_at IS NULL", params![now, id])?;
    let grant = get_grant_conn(&tx, id)?;
    tx.commit()?;
    Ok(grant)
}
