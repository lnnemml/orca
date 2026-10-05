//! Server-profile commands: the CRUD surface over the `server_profiles` table (schema
//! v19, Phase 5 unit 5.1, ADR-023, ADR-024 n) and the `jobs.backend_id` nullable FK.
//!
//! Same shape as `commands::reactions`: each Tauri command is a thin wrapper that locks
//! the shared connection and delegates to a `*_conn` helper taking a `&Connection`, so the
//! logic is unit-testable without a running Tauri app.
//!
//! Load-bearing safety property (mirrors reactions' invariant 1): **jobs are the work;
//! server profiles are runtime config metadata.** Deleting a profile NEVER deletes a job —
//! it nulls the `backend_id` of any jobs that ran on that profile (they revert to `NULL =
//! local`, ADR-023) and then removes the profile row. The v18 FK is declared `ON DELETE
//! SET NULL`, but `delete_server_profile_conn` nulls the children **explicitly first**
//! anyway — the jobs-survive invariant must hold even if FK enforcement were off (the same
//! defensive ordering `delete_reaction` uses).
//!
//! The verified-spec columns (`orca_version`, `openmpi_version`, `core_count`, `verified_at`)
//! are NOT user-editable. They are written only by [`set_profile_verified_conn`] after a **full
//! pass** of the connection test, and set back to NULL together (ADR-024 n items 5–6) when
//! - an update changes the **value** of a target field (`host`, `remote_orca_path`,
//!   `remote_scratch_dir`, `core_mask`, `slot_count` — [`ProfileTarget`]); a rename, a window
//!   edit or a save that rewrites a field with its unchanged value keeps the stamp;
//! - a re-test is not a full pass ([`clear_profile_verified_conn`]).
//!
//! A stamp never outlives the facts it certified, and never certifies a target it did not test:
//! [`set_profile_verified_conn`] takes the target the test ran against and refuses
//! ([`AppError::Conflict`]) if the profile no longer has it. Every write of user fields is
//! validated first ([`validate_profile`]); an invalid field is [`AppError::Invalid`] and nothing
//! is written.
//!
//! **The connection test** ([`test_server_profile`], ADR-024 n) is the only writer of the stamp. It
//! runs the static script over `ssh … -- <host> bash -s` ([`ssh_bash_argv`]) with a
//! [`CONNTEST_TIMEOUT`], and Rust's own verdict ([`connection_test::run`]) decides:
//! - a **full pass** stamps the profile through [`set_profile_verified_conn`], bound to the target
//!   that was tested (an edit during the test is a conflict, reported, not stamped);
//! - **anything else** — a failed check, an ssh failure, a timeout, unreadable output, a non-zero
//!   ssh exit after a complete output — clears the stamp ([`clear_profile_verified_conn`]).
//!
//! `set_profile_verified` is deliberately **not** an IPC command: a stamp from the frontend would be
//! facts the webview asserts, not facts Rust measured (rule #9). A test pins its absence.

use std::time::{Duration, Instant};

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use tauri::{AppHandle, Manager, State};
use uuid::Uuid;

use crate::commands::settings::DbState;
use crate::connection_test::{
    self, conntest_stdin, Check, CheckFailure, ConnTestArgs, Verdict, VerifiedFacts, Warning,
};
use crate::error::AppError;
use crate::models::server_profile::{
    is_run_target, validate_profile, ProfileTarget, ServerProfile,
};
use crate::remote::ssh::{ssh_bash_argv, CommandRunner, SystemRunner, SSH_PROGRAM};

// --- Connection-level helpers (testable) ------------------------------------

fn profile_exists(conn: &Connection, id: &str) -> Result<bool, AppError> {
    Ok(conn
        .query_row(
            "SELECT 1 FROM server_profiles WHERE id = ?1",
            params![id],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

/// A single profile by id, or [`AppError::NotFound`].
pub(crate) fn get_profile_conn(conn: &Connection, id: &str) -> Result<ServerProfile, AppError> {
    let sql = format!(
        "SELECT {} FROM server_profiles WHERE id = ?1",
        ServerProfile::COLUMNS
    );
    conn.query_row(&sql, params![id], ServerProfile::from_row)
        .optional()?
        .ok_or_else(|| AppError::NotFound(format!("server profile {id}")))
}

/// The user-owned fields of a create or an update, as the Tauri commands receive them.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ProfileFields<'a> {
    pub name: &'a str,
    pub host: &'a str,
    pub remote_orca_path: &'a str,
    pub remote_scratch_dir: &'a str,
    pub core_mask: Option<&'a str>,
    pub availability_window: Option<&'a str>,
}

impl ProfileFields<'_> {
    fn target(&self, slot_count: u32) -> ProfileTarget {
        ProfileTarget {
            host: self.host.to_string(),
            remote_orca_path: self.remote_orca_path.to_string(),
            remote_scratch_dir: self.remote_scratch_dir.to_string(),
            core_mask: self.core_mask.map(str::to_string),
            slot_count,
        }
    }

    fn validate(&self, slot_count: u32) -> Result<(), AppError> {
        validate_profile(&self.target(slot_count), self.availability_window)
            .map_err(|e| AppError::Invalid(e.to_string()))
    }
}

/// Create a profile from validated user fields. `slot_count` takes the column default (1); the
/// verified_* columns stay NULL — a new profile is unverified until a connection test passes.
fn create_server_profile_conn(
    conn: &Connection,
    fields: ProfileFields<'_>,
) -> Result<ServerProfile, AppError> {
    fields.validate(1)?;
    let id = Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO server_profiles
             (id, name, host, remote_orca_path, remote_scratch_dir, core_mask, availability_window)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            id,
            fields.name,
            fields.host,
            fields.remote_orca_path,
            fields.remote_scratch_dir,
            fields.core_mask,
            fields.availability_window
        ],
    )?;
    get_profile_conn(conn, &id)
}

/// All server profiles, newest first.
fn list_server_profiles_conn(conn: &Connection) -> Result<Vec<ServerProfile>, AppError> {
    let sql = format!(
        "SELECT {} FROM server_profiles ORDER BY created_at DESC, id",
        ServerProfile::COLUMNS
    );
    let mut stmt = conn.prepare(&sql)?;
    let profiles = stmt
        .query_map([], ServerProfile::from_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(profiles)
}

/// Edit the user-owned fields of a profile. Validates first ([`AppError::Invalid`], nothing
/// written). A change of `host` or `remote_scratch_dir` is refused ([`AppError::Conflict`]) while
/// the profile has remote jobs that are not terminal (ADR-024 n 6b): their coordinates name that
/// host and root. If the **value** of any target field changes, the verification stamp and the facts
/// it certified are cleared in the same transaction (ADR-024 n item 5); otherwise they are kept.
/// `slot_count` is not editable (the CHECK pins it to 1), so it is carried over. [`AppError::NotFound`]
/// if the id is absent. Returns the updated profile.
fn update_server_profile_conn(
    conn: &Connection,
    id: &str,
    fields: ProfileFields<'_>,
) -> Result<ServerProfile, AppError> {
    let tx = conn.unchecked_transaction()?;
    let old = get_profile_conn(&tx, id)?;
    fields.validate(old.slot_count)?;
    if old.host != fields.host || old.remote_scratch_dir != fields.remote_scratch_dir {
        refuse_while_live(&tx, id, "change the host or the remote root of")?;
    }
    let target_changed = old.target() != fields.target(old.slot_count);
    tx.execute(
        "UPDATE server_profiles
         SET name = ?1, host = ?2, remote_orca_path = ?3, remote_scratch_dir = ?4,
             core_mask = ?5, availability_window = ?6
         WHERE id = ?7",
        params![
            fields.name,
            fields.host,
            fields.remote_orca_path,
            fields.remote_scratch_dir,
            fields.core_mask,
            fields.availability_window,
            id
        ],
    )?;
    if target_changed {
        clear_verified(&tx, id)?;
    }
    tx.commit()?;
    get_profile_conn(conn, id)
}

/// Set `verified_at` and the facts it certified back to NULL, together. Also the remote submit's
/// one writer of a clear: only a `SubmitReply::RefusedKup` reaches it (ADR-024 n item 7, o 13.3).
pub(crate) fn clear_verified(conn: &Connection, id: &str) -> Result<usize, AppError> {
    Ok(conn.execute(
        "UPDATE server_profiles
         SET verified_at = NULL, orca_version = NULL, openmpi_version = NULL, core_count = NULL
         WHERE id = ?1",
        params![id],
    )?)
}

/// The profile's remote jobs that are not terminal (`queued`/`running` with coordinates), oldest
/// first. While any exists, the profile's host and root cannot change and the profile cannot be
/// deleted (ADR-024 n 6b, o item 2): those jobs live on that server.
pub(crate) fn live_remote_jobs(conn: &Connection, profile_id: &str) -> Result<Vec<String>, AppError> {
    let mut stmt = conn.prepare(
        "SELECT id FROM jobs WHERE backend_id = ?1 AND remote_host IS NOT NULL \
         AND status IN ('queued', 'running') ORDER BY created_at, id",
    )?;
    let ids = stmt
        .query_map(params![profile_id], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(ids)
}

/// [`AppError::Conflict`] naming the live jobs, if the profile has any.
fn refuse_while_live(conn: &Connection, profile_id: &str, action: &str) -> Result<(), AppError> {
    let live = live_remote_jobs(conn, profile_id)?;
    if live.is_empty() {
        return Ok(());
    }
    Err(AppError::Conflict(format!(
        "cannot {action} server profile {profile_id}: it has {} job(s) on the server that are not finished ({}); \
         withdraw them, or wait until they finish (remote cancel arrives in unit 5.4)",
        live.len(),
        live.join(", ")
    )))
}

/// Delete a profile. Refused while the profile has remote jobs that are not terminal
/// ([`live_remote_jobs`], ADR-024 o item 2): nulling their `backend_id` would orphan jobs that
/// live on that server. Otherwise **nulls the `backend_id` of every job that ran on it FIRST**
/// (the jobs revert to `NULL = local`, ADR-023), then removes the profile row — the jobs survive
/// as standalone jobs, exactly like `delete_reaction` (the load-bearing invariant). The
/// explicit null holds even if the FK's `ON DELETE SET NULL` were not enforced. A finished remote
/// job keeps its coordinates, so it stays remote (`Job::is_remote`) without a profile.
/// [`AppError::NotFound`] if the profile is absent (nothing is touched in that case).
fn delete_server_profile_conn(conn: &Connection, id: &str) -> Result<(), AppError> {
    if !profile_exists(conn, id)? {
        return Err(AppError::NotFound(format!("server profile {id}")));
    }
    refuse_while_live(conn, id, "delete")?;
    // Null the run-target FK on jobs that used this profile FIRST — never DELETE a job.
    conn.execute(
        "UPDATE jobs SET backend_id = NULL WHERE backend_id = ?1",
        params![id],
    )?;
    conn.execute("DELETE FROM server_profiles WHERE id = ?1", params![id])?;
    Ok(())
}

/// Stamp a **full pass** of the connection test: `orca_version`, `openmpi_version` (recorded, NULL
/// when the host reported none — ADR-024 n item 9), `core_count`, and `verified_at =
/// datetime('now')`. `tested` is the target the test ran against; the stamp is written only if the
/// profile still has exactly that target, so an edit made while the test ran can never be
/// certified by it ([`AppError::Conflict`]). [`AppError::NotFound`] if the profile is absent.
fn set_profile_verified_conn(
    conn: &Connection,
    id: &str,
    tested: &ProfileTarget,
    orca_version: &str,
    openmpi_version: Option<&str>,
    core_count: u32,
) -> Result<ServerProfile, AppError> {
    let affected = conn.execute(
        "UPDATE server_profiles
         SET orca_version = ?1, openmpi_version = ?2, core_count = ?3,
             verified_at = datetime('now')
         WHERE id = ?4 AND host = ?5 AND remote_orca_path = ?6 AND remote_scratch_dir = ?7
           AND core_mask IS ?8 AND slot_count = ?9",
        params![
            orca_version,
            openmpi_version,
            core_count,
            id,
            tested.host,
            tested.remote_orca_path,
            tested.remote_scratch_dir,
            tested.core_mask,
            tested.slot_count
        ],
    )?;
    if affected == 0 {
        if profile_exists(conn, id)? {
            return Err(AppError::Conflict(format!(
                "server profile {id} changed while the connection test ran; not stamping it — test again"
            )));
        }
        return Err(AppError::NotFound(format!("server profile {id}")));
    }
    get_profile_conn(conn, id)
}

/// A re-test that is not a full pass: set `verified_at` and the verified facts to NULL (ADR-024 n
/// item 6). [`AppError::NotFound`] if the profile is absent. Returns the cleared profile.
fn clear_profile_verified_conn(conn: &Connection, id: &str) -> Result<ServerProfile, AppError> {
    if clear_verified(conn, id)? == 0 {
        return Err(AppError::NotFound(format!("server profile {id}")));
    }
    get_profile_conn(conn, id)
}

// --- Views for the UI -------------------------------------------------------

/// Whether a profile is a run target, and if not, why ([`is_run_target`], ADR-024 n item 2). Computed
/// in Rust so the UI never re-derives the rule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RunTargetStatus {
    pub is_run_target: bool,
    /// `None` exactly when `is_run_target` is true.
    pub reason: Option<String>,
}

impl RunTargetStatus {
    fn of(profile: &ServerProfile) -> Self {
        match is_run_target(profile) {
            Ok(()) => RunTargetStatus { is_run_target: true, reason: None },
            Err(why) => RunTargetStatus { is_run_target: false, reason: Some(why.to_string()) },
        }
    }
}

/// A profile as the UI shows it: the row plus its run-target status.
#[derive(Debug, Clone, Serialize)]
pub struct ServerProfileView {
    #[serde(flatten)]
    pub profile: ServerProfile,
    pub run_target: RunTargetStatus,
}

impl From<ServerProfile> for ServerProfileView {
    fn from(profile: ServerProfile) -> Self {
        let run_target = RunTargetStatus::of(&profile);
        ServerProfileView { profile, run_target }
    }
}

// --- The connection test --------------------------------------------------------

/// The overall bound on one connection test, connect included. Warm, the test takes ≈1.4 s over an
/// existing ControlMaster (probe 5.1c); ssh gives up connecting after its own `ConnectTimeout` (10 s).
/// 30 s leaves room for a slow login on top of both; past it, ssh is killed and the stamp cleared.
pub const CONNTEST_TIMEOUT: Duration = Duration::from_secs(30);

/// One mandatory check (ADR-024 n item 8) as the UI lists it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CheckResult {
    pub check: Check,
    pub passed: bool,
    /// Why it did not pass; `None` when it passed.
    pub reason: Option<String>,
}

/// What the connection test concluded, and what was written.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case", tag = "outcome")]
pub enum ConnTestOutcome {
    /// A full pass; the profile is stamped with these facts.
    Verified { checks: Vec<CheckResult>, facts: VerifiedFacts, warnings: Vec<Warning> },
    /// A full pass, but the profile was edited while the test ran: nothing was stamped (n 6c).
    Conflict {
        reason: String,
        checks: Vec<CheckResult>,
        facts: VerifiedFacts,
        warnings: Vec<Warning>,
    },
    /// At least one mandatory check did not pass; the stamp was cleared.
    NotPassed { checks: Vec<CheckResult>, warnings: Vec<Warning> },
    /// No verdict: the profile is invalid, ssh failed or timed out, or the output was unreadable.
    /// The stamp was cleared.
    Failed { reason: String },
}

/// The connection test's report: the outcome, the profile after the write, and the wall time.
#[derive(Debug, Clone, Serialize)]
pub struct ConnTestReport {
    #[serde(flatten)]
    pub outcome: ConnTestOutcome,
    pub profile: ServerProfileView,
    pub elapsed_ms: u64,
}

/// Every mandatory check with its result, in a fixed order. The core-mask check exists only when a
/// mask was sent. Derived from the verdict only: a check passed iff the verdict has no failure for it.
fn check_results(failures: &[CheckFailure], sent: &ConnTestArgs) -> Vec<CheckResult> {
    let mut checks = vec![Check::Orca, Check::Cores];
    if sent.core_mask.is_some() {
        checks.push(Check::CoreMask);
    }
    checks.extend([Check::KillUserProcesses, Check::Root]);
    checks
        .into_iter()
        .map(|check| {
            let reasons: Vec<&str> = failures
                .iter()
                .filter(|f| f.check == check)
                .map(|f| f.reason.as_str())
                .collect();
            CheckResult {
                check,
                passed: reasons.is_empty(),
                reason: (!reasons.is_empty()).then(|| reasons.join("; ")),
            }
        })
        .collect()
}

/// The last 500 characters of a stream, trimmed, for an error message.
fn tail(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    let text = text.trim();
    let skip = text.chars().count().saturating_sub(500);
    text.chars().skip(skip).collect()
}

/// Run the script on the target's host and return Rust's verdict, or why there is none. Does not
/// touch the database.
fn probe_target(
    target: &ProfileTarget,
    availability_window: Option<&str>,
    runner: &dyn CommandRunner,
    timeout: Duration,
) -> Result<Verdict, String> {
    validate_profile(target, availability_window)
        .map_err(|e| format!("the saved profile is invalid, so it was not tested: {e}"))?;
    let argv = ssh_bash_argv(&target.host).map_err(|e| e.to_string())?;
    let sent = ConnTestArgs::for_target(target);
    let stdin = conntest_stdin(&sent).map_err(|e| e.to_string())?;
    let out = runner.run(SSH_PROGRAM, &argv, &stdin, timeout).map_err(|e| e.to_string())?;
    let code = out.code.map_or_else(|| "a signal".to_string(), |c| c.to_string());
    if out.code == Some(255) {
        // ssh's own failure status: connect, auth or host-key failure (BatchMode never prompts).
        return Err(format!("ssh could not run the test (exit 255): {}", tail(&out.stderr)));
    }
    let verdict = connection_test::run(&out.stdout, &sent)
        .map_err(|e| format!("{e} (ssh exit {code}; stderr: {:?})", tail(&out.stderr)))?;
    // The script ends with `printf 'end\n'` and exits 0. A complete output with any other exit
    // status is not trusted (fail closed).
    if out.code != Some(0) {
        return Err(format!("ssh exited with {code} after a complete output; not trusting it"));
    }
    Ok(verdict)
}

/// The connection test of profile `id` (ADR-024 n items 6–8). The database lock is **not** held
/// while ssh runs: the profile is read, the lock released, the test run, and the lock taken again to
/// write. A full pass stamps through [`set_profile_verified_conn`] with the target that was tested,
/// so an edit made meanwhile is a [`ConnTestOutcome::Conflict`]; everything else clears the stamp.
/// A clear is unconditional: if a newer test stamped the profile meanwhile, this older failure
/// still clears it (fail closed; the UI runs one test per profile at a time).
pub(crate) fn test_server_profile_with(
    db: &DbState,
    id: &str,
    runner: &dyn CommandRunner,
    timeout: Duration,
) -> Result<ConnTestReport, AppError> {
    let start = Instant::now();
    let profile = {
        let conn = db.lock()?;
        get_profile_conn(&conn, id)?
    };
    let target = profile.target();
    let sent = ConnTestArgs::for_target(&target);
    let verdict =
        probe_target(&target, profile.availability_window.as_deref(), runner, timeout);

    let conn = db.lock()?;
    let outcome = match verdict {
        Ok(Verdict::FullPass { facts, warnings }) => {
            let checks = check_results(&[], &sent);
            match set_profile_verified_conn(
                &conn,
                id,
                &target,
                &facts.orca_version,
                facts.openmpi_version.as_deref(),
                facts.core_count,
            ) {
                Ok(_) => ConnTestOutcome::Verified { checks, facts, warnings },
                Err(AppError::Conflict(reason)) => {
                    ConnTestOutcome::Conflict { reason, checks, facts, warnings }
                }
                Err(e) => return Err(e),
            }
        }
        Ok(Verdict::NotPassed { failures, warnings }) => {
            clear_profile_verified_conn(&conn, id)?;
            ConnTestOutcome::NotPassed { checks: check_results(&failures, &sent), warnings }
        }
        Err(reason) => {
            clear_profile_verified_conn(&conn, id)?;
            ConnTestOutcome::Failed { reason }
        }
    };
    let profile = get_profile_conn(&conn, id)?.into();
    Ok(ConnTestReport { outcome, profile, elapsed_ms: start.elapsed().as_millis() as u64 })
}

// --- Tauri commands ---------------------------------------------------------

#[tauri::command]
pub fn create_server_profile(
    db: State<'_, DbState>,
    name: String,
    host: String,
    remote_orca_path: String,
    remote_scratch_dir: String,
    core_mask: Option<String>,
    availability_window: Option<String>,
) -> Result<ServerProfileView, AppError> {
    let conn = db.lock()?;
    create_server_profile_conn(
        &conn,
        ProfileFields {
            name: &name,
            host: &host,
            remote_orca_path: &remote_orca_path,
            remote_scratch_dir: &remote_scratch_dir,
            core_mask: core_mask.as_deref(),
            availability_window: availability_window.as_deref(),
        },
    )
    .map(Into::into)
}

#[tauri::command]
pub fn list_server_profiles(db: State<'_, DbState>) -> Result<Vec<ServerProfileView>, AppError> {
    let conn = db.lock()?;
    Ok(list_server_profiles_conn(&conn)?.into_iter().map(Into::into).collect())
}

#[tauri::command]
#[allow(clippy::too_many_arguments)] // one argument per user-owned column, as the IPC sends them
pub fn update_server_profile(
    db: State<'_, DbState>,
    id: String,
    name: String,
    host: String,
    remote_orca_path: String,
    remote_scratch_dir: String,
    core_mask: Option<String>,
    availability_window: Option<String>,
) -> Result<ServerProfileView, AppError> {
    let conn = db.lock()?;
    update_server_profile_conn(
        &conn,
        &id,
        ProfileFields {
            name: &name,
            host: &host,
            remote_orca_path: &remote_orca_path,
            remote_scratch_dir: &remote_scratch_dir,
            core_mask: core_mask.as_deref(),
            availability_window: availability_window.as_deref(),
        },
    )
    .map(Into::into)
}

#[tauri::command]
pub fn delete_server_profile(db: State<'_, DbState>, id: String) -> Result<(), AppError> {
    let conn = db.lock()?;
    delete_server_profile_conn(&conn, &id)
}

#[tauri::command]
pub fn clear_profile_verified(
    db: State<'_, DbState>,
    id: String,
) -> Result<ServerProfileView, AppError> {
    let conn = db.lock()?;
    clear_profile_verified_conn(&conn, &id).map(Into::into)
}

/// "Test connection": run the connection test of profile `id` over ssh and stamp or clear it
/// ([`test_server_profile_with`]). Async, so the up-to-[`CONNTEST_TIMEOUT`] ssh run stays off the
/// GTK/WebKit main thread.
#[tauri::command]
pub async fn test_server_profile(app: AppHandle, id: String) -> Result<ConnTestReport, AppError> {
    tauri::async_runtime::spawn_blocking(move || {
        let db = app.state::<DbState>();
        test_server_profile_with(&db, &id, &SystemRunner, CONNTEST_TIMEOUT)
    })
    .await
    .map_err(|e| AppError::Backend(format!("connection-test task failed: {e}")))?
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::init_db;

    /// A migrated database in a throwaway temp dir. A process-wide atomic counter keeps
    /// each test's directory unique even under parallel runs.
    fn test_db() -> (Connection, std::path::PathBuf) {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "orcastudio-serverprofiles-test-{}-{}",
            std::process::id(),
            n
        ));
        std::fs::remove_dir_all(&dir).ok();
        let conn = init_db(&dir).expect("init_db should succeed");
        (conn, dir)
    }

    fn fields<'a>(
        name: &'a str,
        host: &'a str,
        remote_orca_path: &'a str,
        remote_scratch_dir: &'a str,
        core_mask: Option<&'a str>,
    ) -> ProfileFields<'a> {
        ProfileFields { name, host, remote_orca_path, remote_scratch_dir, core_mask, availability_window: None }
    }

    /// The uni profile as created by most tests.
    fn uni() -> ProfileFields<'static> {
        fields("uni", "uni", "/opt/orca/orca", "/home/anton/.orcastudio", Some("0-23"))
    }

    fn stamp(conn: &Connection, p: &ServerProfile) -> ServerProfile {
        set_profile_verified_conn(conn, &p.id, &p.target(), "6.1.1", Some("4.1.6"), 48).unwrap()
    }

    /// Insert a standalone job directly (title + input_content are the only NOT NULL
    /// columns without a default). Returns nothing — callers use the id they passed.
    fn insert_job(conn: &Connection, id: &str) {
        conn.execute(
            "INSERT INTO jobs (id, title, input_content) VALUES (?1, ?2, ?3)",
            params![id, format!("job {id}"), "! r2SCAN-3c Opt"],
        )
        .expect("insert job");
    }

    fn job_backend(conn: &Connection, id: &str) -> Option<String> {
        conn.query_row(
            "SELECT backend_id FROM jobs WHERE id = ?1",
            params![id],
            |r| r.get::<_, Option<String>>(0),
        )
        .expect("job should exist")
    }

    fn job_exists(conn: &Connection, id: &str) -> bool {
        conn.query_row("SELECT 1 FROM jobs WHERE id = ?1", params![id], |_| Ok(()))
            .optional()
            .unwrap()
            .is_some()
    }

    /// The usability gate expressed as a query: how many profiles have passed the
    /// connection-test (`verified_at IS NOT NULL`).
    fn usable_count(conn: &Connection) -> i64 {
        conn.query_row(
            "SELECT COUNT(*) FROM server_profiles WHERE verified_at IS NOT NULL",
            [],
            |r| r.get(0),
        )
        .unwrap()
    }

    /// Stamp and facts are all set or all NULL — never a stamp without its facts.
    fn assert_cleared(p: &ServerProfile, why: &str) {
        assert_eq!(p.verified_at, None, "{why}: verified_at must be NULL");
        assert_eq!(p.orca_version, None, "{why}: orca_version must be NULL");
        assert_eq!(p.openmpi_version, None, "{why}: openmpi_version must be NULL");
        assert_eq!(p.core_count, None, "{why}: core_count must be NULL");
    }

    fn assert_stamped(p: &ServerProfile, why: &str) {
        assert!(p.verified_at.is_some(), "{why}: verified_at must survive");
        assert_eq!(p.orca_version.as_deref(), Some("6.1.1"), "{why}");
        assert_eq!(p.openmpi_version.as_deref(), Some("4.1.6"), "{why}");
        assert_eq!(p.core_count, Some(48), "{why}");
    }

    // C-create-list-roundtrip: every user field survives the create→list round-trip, and a
    // freshly created profile is honestly unverified (all four verified_* columns NULL).
    #[test]
    fn create_and_list_roundtrips_all_fields() {
        let (conn, dir) = test_db();

        let p = create_server_profile_conn(
            &conn,
            ProfileFields {
                availability_window: Some("22:00-08:00"),
                ..fields("uni cluster", "uni", "/opt/orca/orca", "/scratch/anton", Some("0-7"))
            },
        )
        .unwrap();
        assert_eq!(p.name, "uni cluster");
        assert_eq!(p.host, "uni");
        assert_eq!(p.remote_orca_path, "/opt/orca/orca");
        assert_eq!(p.remote_scratch_dir, "/scratch/anton");
        assert_eq!(p.core_mask.as_deref(), Some("0-7"));
        assert_eq!(p.slot_count, 1);
        assert_eq!(p.availability_window.as_deref(), Some("22:00-08:00"));
        // Honest-or-absent: unverified profile carries no forged specs.
        assert_cleared(&p, "a new profile is not yet a run target");

        // core_mask and the window are optional.
        let p2 = create_server_profile_conn(
            &conn,
            fields("lab box", "lab", "/usr/local/orca/orca", "/tmp/orca", None),
        )
        .unwrap();
        assert_eq!(p2.core_mask, None);
        assert_eq!(p2.availability_window, None);

        // list round-trips both, and re-hydrates every field via COLUMNS/from_row.
        let all = list_server_profiles_conn(&conn).unwrap();
        assert_eq!(all.len(), 2);
        let by_id = |id: &str| all.iter().find(|x| x.id == id).unwrap();
        assert_eq!(by_id(&p.id).remote_scratch_dir, "/scratch/anton");
        assert_eq!(by_id(&p2.id).host, "lab");

        std::fs::remove_dir_all(&dir).ok();
    }

    // Save-time validation (ADR-024 n items 2–4): an invalid field is AppError::Invalid and
    // NOTHING is written — neither by create nor by update.
    #[test]
    fn invalid_fields_are_refused_and_nothing_is_written() {
        let (conn, dir) = test_db();
        // 86 bytes: its slot-0 socket `<root>/tsp/slot0.sock` is 101 bytes, over the bound.
        let long_root = format!("/{}", "a".repeat(85));
        let bad: Vec<ProfileFields<'_>> = vec![
            fields("x", "uni", "orca", "/home/anton/.orcastudio", None),
            fields("x", "uni", "/opt/orca/orca", "relative", None),
            fields("x", "uni", "/opt/orca/orca", "/home/anton/../root", None),
            fields("x", "uni", "/opt/orca/orca", "/home/anton/.orcastudio/", None),
            fields("x", "uni", "/opt/orca/orca", &long_root, None),
            fields("x", "uni", "/opt/orca/orca", "/home/anton/.orcastudio", Some("-1")),
            fields("x", "uni", "/opt/orca/orca", "/home/anton/.orcastudio", Some("")),
            ProfileFields { availability_window: Some("08:00-08:00"), ..uni() },
            ProfileFields { availability_window: Some("25:00-08:00"), ..uni() },
            fields("x", "", "/opt/orca/orca", "/home/anton/.orcastudio", None),
        ];
        for f in &bad {
            assert!(matches!(create_server_profile_conn(&conn, *f), Err(AppError::Invalid(_))), "{f:?}");
        }
        assert!(list_server_profiles_conn(&conn).unwrap().is_empty(), "create wrote nothing");

        let p = create_server_profile_conn(&conn, uni()).unwrap();
        let p = stamp(&conn, &p);
        for f in &bad {
            assert!(matches!(update_server_profile_conn(&conn, &p.id, *f), Err(AppError::Invalid(_))), "{f:?}");
        }
        let after = get_profile_conn(&conn, &p.id).unwrap();
        assert_eq!(after.target(), p.target(), "update wrote nothing");
        assert_eq!(after.name, "uni");
        assert_stamped(&after, "a refused update clears nothing");

        std::fs::remove_dir_all(&dir).ok();
    }

    // C-update-user-fields: update mutates the user fields, and NotFound on a missing id.
    #[test]
    fn update_mutates_user_fields_and_notfound() {
        let (conn, dir) = test_db();

        let p = create_server_profile_conn(
            &conn,
            fields("old", "old-host", "/opt/orca/orca", "/scratch", None),
        )
        .unwrap();

        let updated = update_server_profile_conn(
            &conn,
            &p.id,
            ProfileFields {
                availability_window: Some("18:00-07:30"),
                ..fields("new name", "new-host", "/opt/orca6/orca", "/scratch2", Some("0-3"))
            },
        )
        .unwrap();
        assert_eq!(updated.name, "new name");
        assert_eq!(updated.host, "new-host");
        assert_eq!(updated.remote_orca_path, "/opt/orca6/orca");
        assert_eq!(updated.remote_scratch_dir, "/scratch2");
        assert_eq!(updated.core_mask.as_deref(), Some("0-3"));
        assert_eq!(updated.availability_window.as_deref(), Some("18:00-07:30"));
        assert_eq!(updated.slot_count, 1);

        // update of a missing id → NotFound.
        assert!(matches!(
            update_server_profile_conn(&conn, "no-such", uni()).unwrap_err(),
            AppError::NotFound(_)
        ));

        std::fs::remove_dir_all(&dir).ok();
    }

    // C-set-verified-preserved-by-non-target-update: set_profile_verified flips verified_at from
    // NULL to set and the usability gate `verified_at IS NOT NULL` now holds; an update that
    // changes NO target field (a rename, a window edit, a same-value save) keeps the stamp
    // (ADR-024 n item 5).
    //
    // NEGATIVE CONTROL (bites, control f): an update that clears the stamp on every save (or on
    // a rename) fails the "stamp survives" asserts — the gate would drop back to unverified.
    #[test]
    fn set_profile_verified_stamps_and_a_non_target_update_preserves_it() {
        let (conn, dir) = test_db();

        let p = create_server_profile_conn(&conn, uni()).unwrap();

        // Before: the usability gate is closed (verified_at NULL).
        assert_eq!(p.verified_at, None);
        assert_eq!(usable_count(&conn), 0, "no profile passes the gate yet");

        let stamped = stamp(&conn, &p);
        assert_stamped(&stamped, "after the stamp");
        assert_eq!(usable_count(&conn), 1, "the gate now admits the profile");

        // A rename keeps the stamp.
        let after = update_server_profile_conn(&conn, &p.id, ProfileFields { name: "uni renamed", ..uni() }).unwrap();
        assert_eq!(after.name, "uni renamed");
        assert_stamped(&after, "a rename");

        // A window edit keeps the stamp.
        let after = update_server_profile_conn(
            &conn,
            &p.id,
            ProfileFields { availability_window: Some("22:00-08:00"), ..uni() },
        )
        .unwrap();
        assert_eq!(after.availability_window.as_deref(), Some("22:00-08:00"));
        assert_stamped(&after, "a window edit");

        // A same-value save keeps the stamp.
        let after = update_server_profile_conn(&conn, &p.id, uni()).unwrap();
        assert_stamped(&after, "a same-value save");
        assert_eq!(usable_count(&conn), 1, "still a run target after the edits");

        // set_profile_verified of a missing id → NotFound.
        assert!(matches!(
            set_profile_verified_conn(&conn, "no-such", &p.target(), "6.1.0", Some("4.1.6"), 8).unwrap_err(),
            AppError::NotFound(_)
        ));

        std::fs::remove_dir_all(&dir).ok();
    }

    // The INVERTED half of the old "update preserves the stamp" test (ADR-024 n item 5): changing
    // the VALUE of any target field clears verified_at AND the facts it certified, together.
    //
    // NEGATIVE CONTROL (bites, control e): an update that clears nothing (the pre-v19 behaviour)
    // leaves a stamp certifying a host it never tested, and every case here goes red.
    #[test]
    fn changing_a_target_field_clears_the_stamp_and_its_facts() {
        let (conn, dir) = test_db();
        let changes: Vec<(&str, ProfileFields<'static>)> = vec![
            ("host", ProfileFields { host: "uni2", ..uni() }),
            ("remote_orca_path", ProfileFields { remote_orca_path: "/opt/orca-6.1.0/orca", ..uni() }),
            ("remote_scratch_dir", ProfileFields { remote_scratch_dir: "/home/anton/.orcastudio2", ..uni() }),
            ("core_mask value", ProfileFields { core_mask: Some("0-11"), ..uni() }),
            ("core_mask removed", ProfileFields { core_mask: None, ..uni() }),
        ];
        for (what, change) in changes {
            let p = create_server_profile_conn(&conn, uni()).unwrap();
            stamp(&conn, &p);
            let after = update_server_profile_conn(&conn, &p.id, change).unwrap();
            assert_cleared(&after, what);
        }
        // A mask set where there was none is a change too.
        let p = create_server_profile_conn(&conn, ProfileFields { core_mask: None, ..uni() }).unwrap();
        stamp(&conn, &p);
        assert_cleared(&update_server_profile_conn(&conn, &p.id, uni()).unwrap(), "core_mask added");

        std::fs::remove_dir_all(&dir).ok();
    }

    // slot_count is part of the target: a stamp taken for a different slot_count is a stamp for a
    // different target (the column is pinned to 1, so the mismatch is exercised via the stamp).
    #[test]
    fn a_stamp_for_a_target_the_profile_no_longer_has_is_refused() {
        let (conn, dir) = test_db();
        let p = create_server_profile_conn(&conn, uni()).unwrap();
        let tested = p.target();

        // The user edits the host while the test runs: the stamp must not certify the new host.
        update_server_profile_conn(&conn, &p.id, ProfileFields { host: "other", ..uni() }).unwrap();
        assert!(matches!(
            set_profile_verified_conn(&conn, &p.id, &tested, "6.1.1", Some("4.1.6"), 48),
            Err(AppError::Conflict(_))
        ));
        assert_cleared(&get_profile_conn(&conn, &p.id).unwrap(), "no stamp for a changed target");

        let mut two_slots = get_profile_conn(&conn, &p.id).unwrap().target();
        two_slots.slot_count = 2;
        assert!(matches!(
            set_profile_verified_conn(&conn, &p.id, &two_slots, "6.1.1", None, 48),
            Err(AppError::Conflict(_))
        ));

        // The current target stamps, with an absent OpenMPI version recorded as NULL.
        let current = get_profile_conn(&conn, &p.id).unwrap().target();
        let ok = set_profile_verified_conn(&conn, &p.id, &current, "6.1.1", None, 48).unwrap();
        assert!(ok.verified_at.is_some());
        assert_eq!(ok.openmpi_version, None, "an absent OpenMPI version is NULL, never guessed");

        std::fs::remove_dir_all(&dir).ok();
    }

    // A re-test that is not a full pass clears the stamp and every fact (ADR-024 n item 6).
    #[test]
    fn clear_profile_verified_clears_stamp_and_facts() {
        let (conn, dir) = test_db();
        let p = create_server_profile_conn(&conn, uni()).unwrap();
        stamp(&conn, &p);
        let cleared = clear_profile_verified_conn(&conn, &p.id).unwrap();
        assert_cleared(&cleared, "after a failed re-test");
        assert_eq!(cleared.target(), p.target(), "clearing touches no user field");
        assert_eq!(usable_count(&conn), 0);
        assert!(matches!(clear_profile_verified_conn(&conn, "no-such"), Err(AppError::NotFound(_))));
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A remote job of profile `p` in `status`, with coordinates.
    fn insert_remote_job(conn: &Connection, id: &str, p: &str, status: &str) {
        conn.execute(
            "INSERT INTO jobs (id, title, input_content, status, backend_id, remote_host, remote_job_dir, remote_socket) \
             VALUES (?1, 'remote', '! HF', ?2, ?3, 'uni', '/home/anton/.orcastudio/jobs/' || ?1, '/home/anton/.orcastudio/tsp/slot0.sock')",
            params![id, status, p],
        )
        .unwrap();
    }

    /// NEGATIVE CONTROL target (e): a profile with a remote job that is not terminal cannot be
    /// deleted (ADR-024 o item 2) — nulling the job's `backend_id` would orphan a job that lives on
    /// that server. Drop the `refuse_while_live` call from `delete_server_profile_conn` and the
    /// `queued`/`running` rows go red.
    #[test]
    fn a_profile_with_live_remote_jobs_cannot_be_deleted() {
        for status in ["queued", "running"] {
            let (conn, dir) = test_db();
            let p = create_server_profile_conn(&conn, uni()).unwrap();
            insert_remote_job(&conn, "r1", &p.id, status);
            let err = delete_server_profile_conn(&conn, &p.id).unwrap_err();
            assert!(matches!(&err, AppError::Conflict(m) if m.contains("r1")), "{status}: {err}");
            assert!(profile_exists(&conn, &p.id).unwrap(), "{status}: the profile stays");
            assert_eq!(job_backend(&conn, "r1").as_deref(), Some(p.id.as_str()), "{status}: the job keeps its profile");
            std::fs::remove_dir_all(&dir).ok();
        }
        // Finished remote jobs, and local jobs of any status, do not hold the profile.
        for status in ["completed", "parsed", "failed", "cancelled"] {
            let (conn, dir) = test_db();
            let p = create_server_profile_conn(&conn, uni()).unwrap();
            insert_remote_job(&conn, "r1", &p.id, status);
            insert_job(&conn, "local");
            conn.execute("UPDATE jobs SET status = 'queued', backend_id = ?1 WHERE id = 'local'", params![p.id]).unwrap();
            delete_server_profile_conn(&conn, &p.id).unwrap_or_else(|e| panic!("{status}: {e}"));
            assert_eq!(job_backend(&conn, "r1"), None, "{status}: nulled, as before");
            std::fs::remove_dir_all(&dir).ok();
        }
    }

    /// n 6b: while a remote job is live, the profile's host and root cannot change; any other edit
    /// (name, ORCA path, mask, window) still can.
    #[test]
    fn a_profile_with_live_remote_jobs_keeps_its_host_and_root() {
        let (conn, dir) = test_db();
        let p = create_server_profile_conn(&conn, uni()).unwrap();
        insert_remote_job(&conn, "r1", &p.id, "running");
        let base = uni();
        for (case, fields) in [
            ("host", ProfileFields { host: "other", ..base }),
            ("root", ProfileFields { remote_scratch_dir: "/srv/os", ..base }),
        ] {
            let err = update_server_profile_conn(&conn, &p.id, fields).unwrap_err();
            assert!(matches!(&err, AppError::Conflict(m) if m.contains("r1")), "{case}: {err}");
            assert_eq!(get_profile_conn(&conn, &p.id).unwrap().target(), p.target(), "{case}: nothing written");
        }
        for (case, fields) in [
            ("name", ProfileFields { name: "renamed", ..base }),
            ("ORCA path", ProfileFields { remote_orca_path: "/opt/orca-6.1.0/orca", ..base }),
            ("mask", ProfileFields { core_mask: Some("0-11"), ..base }),
        ] {
            update_server_profile_conn(&conn, &p.id, fields).unwrap_or_else(|e| panic!("{case}: {e}"));
        }
        conn.execute("UPDATE jobs SET status = 'completed' WHERE id = 'r1'", []).unwrap();
        update_server_profile_conn(&conn, &p.id, ProfileFields { host: "other", ..base }).expect("a finished job holds nothing");
        std::fs::remove_dir_all(&dir).ok();
    }

    // C-delete-nulls-children-jobs-survive (THE load-bearing invariant): a job that ran on
    // a profile → delete_server_profile → the job STILL EXISTS with backend_id NULL, the
    // profile row is gone.
    //
    // The bite: an implementation that DELETEs the job (a naive cascade) fails the "job
    // survives" assert; one that leaves a dangling backend_id fails the NULL assert. This
    // test distinguishes the safe implementation from either bug.
    #[test]
    fn delete_profile_nulls_children_and_jobs_survive() {
        let (conn, dir) = test_db();

        let p = create_server_profile_conn(&conn, fields("uni", "uni", "/opt/orca/orca", "/scratch", None)).unwrap();
        insert_job(&conn, "j1");
        // Point the job at this profile (Part B does this at job-creation; here direct).
        conn.execute(
            "UPDATE jobs SET backend_id = ?1 WHERE id = ?2",
            params![p.id, "j1"],
        )
        .unwrap();
        assert_eq!(job_backend(&conn, "j1").as_deref(), Some(p.id.as_str()));

        delete_server_profile_conn(&conn, &p.id).unwrap();

        // The job survives as a standalone (local) job — backend_id nulled.
        assert!(job_exists(&conn, "j1"), "the job MUST survive the profile deletion");
        assert_eq!(
            job_backend(&conn, "j1"),
            None,
            "the job reverts to NULL = local, not a dangling id"
        );
        // The profile row is gone.
        assert!(!profile_exists(&conn, &p.id).unwrap());

        // deleting a missing profile is NotFound.
        assert!(matches!(
            delete_server_profile_conn(&conn, "no-such").unwrap_err(),
            AppError::NotFound(_)
        ));

        std::fs::remove_dir_all(&dir).ok();
    }

    // ---- The connection-test command (ADR-024 n items 6–8, verifier F3) -------------------

    use std::cell::RefCell;
    use std::sync::Mutex;

    use crate::connection_test::conntest_tests::{encode, uni_facts, with};
    use crate::connection_test::CheckRun;
    use crate::remote::ssh::{ProcessOutput, TransportError};

    type Reply = Result<ProcessOutput, TransportError>;

    /// A runner that records each call, runs `during` (while the command holds no lock), and
    /// returns `reply`.
    struct FakeRunner<'a> {
        reply: Box<dyn Fn() -> Reply + 'a>,
        during: Box<dyn Fn() + 'a>,
        calls: RefCell<Vec<(String, Vec<String>, Vec<u8>)>>,
    }

    impl<'a> FakeRunner<'a> {
        fn replying(reply: impl Fn() -> Reply + 'a) -> Self {
            FakeRunner { reply: Box::new(reply), during: Box::new(|| {}), calls: RefCell::default() }
        }
    }

    impl CommandRunner for FakeRunner<'_> {
        fn run(&self, program: &str, args: &[String], stdin: &[u8], _: Duration) -> Reply {
            self.calls.borrow_mut().push((program.into(), args.to_vec(), stdin.to_vec()));
            (self.during)();
            (self.reply)()
        }
    }

    fn exited(code: i32, stdout: Vec<u8>, stderr: &[u8]) -> Reply {
        Ok(ProcessOutput { code: Some(code), stdout, stderr: stderr.to_vec() })
    }

    /// The uni profile's values, as the script echoes them.
    const UNI_VALUES: [&str; 3] = ["/opt/orca/orca", "/home/anton/.orcastudio", "0-23"];

    fn full_pass() -> Vec<u8> {
        encode(&UNI_VALUES, &uni_facts())
    }

    /// A database holding the uni profile, stamped.
    fn stamped_uni() -> (DbState, ServerProfile, std::path::PathBuf) {
        let (conn, dir) = test_db();
        let p = create_server_profile_conn(&conn, uni()).unwrap();
        let p = stamp(&conn, &p);
        assert_stamped(&p, "precondition");
        (DbState(Mutex::new(conn)), p, dir)
    }

    fn reload(db: &DbState, id: &str) -> ServerProfile {
        get_profile_conn(&db.lock().unwrap(), id).unwrap()
    }

    #[test]
    fn a_full_pass_stamps_the_tested_target_with_exactly_the_measured_transport() {
        let (conn, dir) = test_db();
        let p = create_server_profile_conn(&conn, uni()).unwrap();
        let db = DbState(Mutex::new(conn));
        let runner = FakeRunner::replying(|| exited(0, full_pass(), b""));

        let report = test_server_profile_with(&db, &p.id, &runner, CONNTEST_TIMEOUT).unwrap();

        let ConnTestOutcome::Verified { checks, facts, warnings } = &report.outcome else {
            panic!("expected Verified, got {:?}", report.outcome);
        };
        assert_eq!(
            (facts.orca_version.as_str(), facts.openmpi_version.as_deref(), facts.core_count),
            ("6.1.1", Some("4.1.6"), 48)
        );
        assert!(warnings.is_empty());
        let names: Vec<Check> = checks.iter().map(|c| c.check).collect();
        assert_eq!(
            names,
            [Check::Orca, Check::Cores, Check::CoreMask, Check::KillUserProcesses, Check::Root]
        );
        assert!(checks.iter().all(|c| c.passed && c.reason.is_none()));

        let stored = reload(&db, &p.id);
        assert_stamped(&stored, "a full pass stamps");
        assert_eq!(stored.core_count, Some(48));
        assert_eq!(report.profile.run_target, RunTargetStatus { is_run_target: true, reason: None });

        // The transport is exactly the measured shape: `ssh <argv>` with the script, then the
        // three values as a NUL list, and nothing after it.
        let calls = runner.calls.borrow();
        assert_eq!(calls.len(), 1);
        let (program, args, stdin) = &calls[0];
        assert_eq!(program, "ssh");
        assert_eq!(args, &ssh_bash_argv("uni").unwrap());
        let sent = ConnTestArgs::for_target(&p.target());
        assert_eq!(stdin, &conntest_stdin(&sent).unwrap());
        assert!(stdin.ends_with(b"/opt/orca/orca\0/home/anton/.orcastudio\x000-23\0"));
        std::fs::remove_dir_all(&dir).ok();
    }

    // NEGATIVE CONTROL: stamp on `NotPassed` (call set_profile_verified_conn there) and this goes red.
    #[test]
    fn a_check_that_does_not_pass_clears_the_stamp_and_names_the_reason() {
        let (db, p, dir) = stamped_uni();
        let facts = with(|f| {
            f.busctl = CheckRun::Ran { rc: 0, out: b"b true\n".to_vec(), err: Vec::new() }
        });
        let runner = FakeRunner::replying(|| exited(0, encode(&UNI_VALUES, &facts), b""));

        let report = test_server_profile_with(&db, &p.id, &runner, CONNTEST_TIMEOUT).unwrap();

        let ConnTestOutcome::NotPassed { checks, .. } = &report.outcome else {
            panic!("expected NotPassed, got {:?}", report.outcome);
        };
        let failed: Vec<&CheckResult> = checks.iter().filter(|c| !c.passed).collect();
        assert_eq!(failed.len(), 1);
        assert_eq!(failed[0].check, Check::KillUserProcesses);
        assert!(failed[0].reason.as_deref().unwrap().contains("KillUserProcesses is true"));
        assert_cleared(&reload(&db, &p.id), "a check that did not pass");
        assert_eq!(
            report.profile.run_target,
            RunTargetStatus {
                is_run_target: false,
                reason: Some("the profile has not passed the connection test".into())
            }
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    // NEGATIVE CONTROL: skip the clear on the no-verdict path (`Err(reason)` arm) and every case
    // here goes red — a stale stamp would survive a timeout or an ssh failure.
    #[test]
    fn no_verdict_clears_the_stamp() {
        let cases: Vec<(&str, Box<dyn Fn() -> Reply>)> = vec![
            ("timeout", Box::new(|| Err(TransportError::Timeout { program: "ssh".into(), secs: 30 }))),
            ("spawn", Box::new(|| Err(TransportError::Spawn { program: "ssh".into(), message: "not found".into() }))),
            ("ssh 255", Box::new(|| exited(255, Vec::new(), b"ssh: Could not resolve hostname uni"))),
            ("empty output", Box::new(|| exited(0, Vec::new(), b""))),
            ("truncated output", Box::new(|| {
                let mut out = full_pass();
                out.truncate(out.len() - 4);
                exited(0, out, b"")
            })),
            ("complete output, exit 1", Box::new(|| exited(1, full_pass(), b""))),
            ("killed by a signal", Box::new(|| Ok(ProcessOutput { code: None, stdout: full_pass(), stderr: Vec::new() }))),
        ];
        for (name, reply) in cases {
            let (db, p, dir) = stamped_uni();
            let runner = FakeRunner::replying(reply);
            let report = test_server_profile_with(&db, &p.id, &runner, CONNTEST_TIMEOUT).unwrap();
            assert!(
                matches!(&report.outcome, ConnTestOutcome::Failed { reason } if !reason.is_empty()),
                "{name}: {:?}",
                report.outcome
            );
            assert_cleared(&reload(&db, &p.id), name);
            assert!(!report.profile.run_target.is_run_target, "{name}");
            std::fs::remove_dir_all(&dir).ok();
        }
    }

    #[test]
    fn the_ssh_failure_reason_carries_ssh_stderr() {
        let (db, p, dir) = stamped_uni();
        let runner = FakeRunner::replying(|| exited(255, Vec::new(), b"Permission denied (publickey).\n"));
        let report = test_server_profile_with(&db, &p.id, &runner, CONNTEST_TIMEOUT).unwrap();
        assert_eq!(
            report.outcome,
            ConnTestOutcome::Failed {
                reason: "ssh could not run the test (exit 255): Permission denied (publickey).".into()
            }
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    // n 6c: the stamp is bound to the tested target. The edit happens while ssh "runs", which also
    // shows the database lock is not held then (`try_lock` succeeds).
    #[test]
    fn an_edit_during_the_test_is_a_conflict_not_a_stamp() {
        let (conn, dir) = test_db();
        let p = create_server_profile_conn(&conn, uni()).unwrap();
        let db = DbState(Mutex::new(conn));
        let id = p.id.clone();
        let runner = FakeRunner {
            reply: Box::new(|| exited(0, full_pass(), b"")),
            during: Box::new(|| {
                let conn = db.0.try_lock().expect("the lock must not be held while ssh runs");
                let mut edited = uni();
                edited.host = "uni2";
                update_server_profile_conn(&conn, &id, edited).unwrap();
            }),
            calls: RefCell::default(),
        };

        let report = test_server_profile_with(&db, &p.id, &runner, CONNTEST_TIMEOUT).unwrap();

        assert!(matches!(report.outcome, ConnTestOutcome::Conflict { .. }), "{:?}", report.outcome);
        let stored = reload(&db, &p.id);
        assert_eq!(stored.host, "uni2");
        assert_cleared(&stored, "a target that was not tested is never stamped");
        std::fs::remove_dir_all(&dir).ok();
    }

    // F2 defence in depth: a host stored before the rule existed (written here behind validation's
    // back) never reaches ssh, and the stamp is cleared.
    #[test]
    fn an_invalid_stored_host_never_reaches_ssh() {
        let (db, p, dir) = stamped_uni();
        db.lock()
            .unwrap()
            .execute(
                "UPDATE server_profiles SET host = '-oProxyCommand=sh' WHERE id = ?1",
                params![p.id],
            )
            .unwrap();
        let runner = FakeRunner::replying(|| exited(0, full_pass(), b""));

        let report = test_server_profile_with(&db, &p.id, &runner, CONNTEST_TIMEOUT).unwrap();

        assert!(runner.calls.borrow().is_empty(), "ssh must not be run");
        assert!(
            matches!(&report.outcome, ConnTestOutcome::Failed { reason } if reason.contains("invalid")),
            "{:?}",
            report.outcome
        );
        assert_cleared(&reload(&db, &p.id), "an invalid stored host");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn testing_a_missing_profile_is_not_found_and_runs_nothing() {
        let (conn, dir) = test_db();
        let db = DbState(Mutex::new(conn));
        let runner = FakeRunner::replying(|| exited(0, full_pass(), b""));
        assert!(matches!(
            test_server_profile_with(&db, "no-such", &runner, CONNTEST_TIMEOUT),
            Err(AppError::NotFound(_))
        ));
        assert!(runner.calls.borrow().is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }

    // F3: only Rust's own verdict stamps. The stamp is not reachable over IPC: no
    // `set_profile_verified` Tauri command exists, and the handler list does not name it.
    // NEGATIVE CONTROL: re-add `commands::server_profiles::set_profile_verified` to
    // `generate_handler!` (and the wrapper) and this goes red.
    #[test]
    fn the_stamp_is_not_an_ipc_command() {
        let lib = include_str!("../lib.rs");
        let start = lib.find("generate_handler![").expect("the handler list exists");
        let handlers = &lib[start..start + lib[start..].find("])").expect("the list ends")];
        assert!(handlers.contains("commands::server_profiles::test_server_profile"));
        assert!(handlers.contains("commands::server_profiles::list_server_profiles"));
        assert!(!handlers.contains("set_profile_verified"), "the stamp must not be an IPC command");
        let this = include_str!("server_profiles.rs");
        let wrapper = concat!("pub fn set_profile_", "verified(");
        let async_wrapper = concat!("pub async fn set_profile_", "verified(");
        assert!(!this.contains(wrapper) && !this.contains(async_wrapper), "no stamp command exists");
    }

    #[test]
    fn the_run_target_status_carries_the_reason() {
        let (conn, dir) = test_db();
        let p = create_server_profile_conn(&conn, uni()).unwrap();
        assert_eq!(
            ServerProfileView::from(p.clone()).run_target,
            RunTargetStatus {
                is_run_target: false,
                reason: Some("the profile has not passed the connection test".into())
            }
        );
        let p = stamp(&conn, &p);
        assert_eq!(ServerProfileView::from(p).run_target, RunTargetStatus { is_run_target: true, reason: None });
        let no_mask = create_server_profile_conn(&conn, fields("m", "uni", "/opt/orca/orca", "/scratch", None)).unwrap();
        let no_mask = stamp(&conn, &no_mask);
        assert_eq!(
            ServerProfileView::from(no_mask).run_target.reason.as_deref(),
            Some("the profile has no core mask")
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_report_serializes_flat_for_the_ui() {
        let (conn, dir) = test_db();
        let p = create_server_profile_conn(&conn, uni()).unwrap();
        let db = DbState(Mutex::new(conn));
        let runner = FakeRunner::replying(|| exited(0, full_pass(), b""));
        let report = test_server_profile_with(&db, &p.id, &runner, CONNTEST_TIMEOUT).unwrap();
        let json = serde_json::to_value(&report).unwrap();
        assert_eq!(json["outcome"], "verified");
        assert_eq!(json["checks"][0]["check"], "orca");
        assert_eq!(json["facts"]["core_count"], 48);
        assert_eq!(json["profile"]["host"], "uni");
        assert_eq!(json["profile"]["run_target"]["is_run_target"], true);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// The JSON shapes `src/types.ts` mirrors (`ConnTestWarning`, `ConnTestOutcome`).
    #[test]
    fn warnings_and_outcomes_serialize_as_the_frontend_types_expect() {
        let json = |w: Warning| serde_json::to_value(w).unwrap();
        assert_eq!(json(Warning::SudoGroup), serde_json::json!({ "kind": "sudo_group" }));
        assert_eq!(
            json(Warning::GroupsUndetermined("rc 1".into())),
            serde_json::json!({ "kind": "groups_undetermined", "detail": "rc 1" })
        );
        assert_eq!(
            json(Warning::OpenMpiNotReported("rc 127".into())),
            serde_json::json!({ "kind": "open_mpi_not_reported", "detail": "rc 127" })
        );
        let failed = serde_json::to_value(ConnTestOutcome::Failed { reason: "x".into() }).unwrap();
        assert_eq!(failed, serde_json::json!({ "outcome": "failed", "reason": "x" }));
        let not_passed =
            serde_json::to_value(ConnTestOutcome::NotPassed { checks: Vec::new(), warnings: Vec::new() })
                .unwrap();
        assert_eq!(not_passed["outcome"], "not_passed");
        let conflict = ConnTestOutcome::Conflict {
            reason: "x".into(),
            checks: Vec::new(),
            facts: VerifiedFacts { orca_version: "6.1.1".into(), openmpi_version: None, core_count: 48 },
            warnings: Vec::new(),
        };
        assert_eq!(serde_json::to_value(conflict).unwrap()["outcome"], "conflict");
    }

    // ---- Live (rule #10): run by hand only ------------------------------------------------

    /// The real command path against the `uni` alias, in a throwaway database (never the app's
    /// DB). Run: `cargo test live_uni -- --ignored --nocapture`.
    #[test]
    #[ignore = "live: ssh to the `uni` alias; run by hand"]
    fn live_uni_connection_test() {
        use crate::remote::ssh::{CommandRunner as _, SystemRunner};

        let (conn, dir) = test_db();
        let p = create_server_profile_conn(
            &conn,
            fields("uni (live test)", "uni", "/opt/orca/orca", "/home/anton/.orcastudio", Some("0-3")),
        )
        .unwrap();
        let bogus = create_server_profile_conn(
            &conn,
            fields("uni bogus orca", "uni", "/opt/orca/no-such-orca", "/home/anton/.orcastudio", Some("0-3")),
        )
        .unwrap();
        let db = DbState(Mutex::new(conn));

        // 1. The raw stream, with exactly the command's argv and stdin.
        let sent = ConnTestArgs::for_target(&p.target());
        let argv = ssh_bash_argv("uni").unwrap();
        let t = Instant::now();
        let raw = SystemRunner
            .run(SSH_PROGRAM, &argv, &conntest_stdin(&sent).unwrap(), CONNTEST_TIMEOUT)
            .expect("ssh ran");
        eprintln!("=== run 1 (raw): {} ms, ssh exit {:?}", t.elapsed().as_millis(), raw.code);
        eprintln!("--- argv: ssh {argv:?}");
        eprintln!("--- stdout:\n{}", String::from_utf8_lossy(&raw.stdout));
        eprintln!("--- stderr:\n{}", String::from_utf8_lossy(&raw.stderr));
        eprintln!("--- verdict: {:?}", connection_test::run(&raw.stdout, &sent));

        // 2. Through the command path.
        let report = test_server_profile_with(&db, &p.id, &SystemRunner, CONNTEST_TIMEOUT).unwrap();
        eprintln!("=== run 2 (command): {} ms", report.elapsed_ms);
        eprintln!("{}", serde_json::to_string_pretty(&report).unwrap());
        assert!(matches!(report.outcome, ConnTestOutcome::Verified { .. }), "{:?}", report.outcome);
        assert_stamped(&reload(&db, &p.id), "live full pass");

        // 3. A bogus ORCA path: not passed, stamp cleared.
        let report = test_server_profile_with(&db, &bogus.id, &SystemRunner, CONNTEST_TIMEOUT).unwrap();
        eprintln!("=== run 3 (bogus ORCA): {} ms", report.elapsed_ms);
        eprintln!("{}", serde_json::to_string_pretty(&report).unwrap());
        let ConnTestOutcome::NotPassed { checks, .. } = &report.outcome else {
            panic!("expected NotPassed, got {:?}", report.outcome);
        };
        assert!(checks.iter().any(|c| c.check == Check::Orca && !c.passed));
        assert_cleared(&reload(&db, &bogus.id), "live bogus ORCA");
        std::fs::remove_dir_all(&dir).ok();
    }
}
