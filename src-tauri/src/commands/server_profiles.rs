//! Server-profile commands: the CRUD surface over the `server_profiles` table (schema
//! v18, Phase 5 unit 5.1, ADR-023) and the `jobs.backend_id` nullable FK.
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
//! The verified-spec columns (`orca_version`, `openmpi_version`, `core_count`,
//! `verified_at`) are NOT user-editable via `update_server_profile` — they are stamped only
//! by [`set_profile_verified_conn`], the pure DB write that Part B's real SSH
//! connection-test calls after it measures the server (rule #10). Until that runs they are
//! `NULL` (honest-or-absent), and `verified_at IS NULL` is the profile's usability gate: a
//! profile that has not passed the connection-test is not a run target (ADR-023).

use rusqlite::{params, Connection, OptionalExtension};
use tauri::State;
use uuid::Uuid;

use crate::commands::settings::DbState;
use crate::error::AppError;
use crate::models::server_profile::ServerProfile;

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
fn get_profile_conn(conn: &Connection, id: &str) -> Result<ServerProfile, AppError> {
    let sql = format!(
        "SELECT {} FROM server_profiles WHERE id = ?1",
        ServerProfile::COLUMNS
    );
    conn.query_row(&sql, params![id], ServerProfile::from_row)
        .optional()?
        .ok_or_else(|| AppError::NotFound(format!("server profile {id}")))
}

/// Create a profile from the user-supplied fields. The verified_* columns are left NULL
/// (the DB defaults) — a freshly created profile is unverified until Part B's
/// connection-test stamps it. Returns the new [`ServerProfile`].
fn create_server_profile_conn(
    conn: &Connection,
    name: &str,
    host: &str,
    remote_orca_path: &str,
    remote_scratch_dir: &str,
    core_mask: Option<&str>,
) -> Result<ServerProfile, AppError> {
    let id = Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO server_profiles
             (id, name, host, remote_orca_path, remote_scratch_dir, core_mask)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![id, name, host, remote_orca_path, remote_scratch_dir, core_mask],
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

/// Edit the **user-owned** fields of a profile (name, host, paths, mask). Deliberately
/// does NOT touch the verified_* columns — those are the connection-test's measurement,
/// not user input (a manual edit would forge a rule-#10 fact). Editing the host or ORCA
/// path silently invalidating a prior verification is a Part B concern; this write leaves
/// the stamp as-is. [`AppError::NotFound`] if the id is absent. Returns the updated profile.
fn update_server_profile_conn(
    conn: &Connection,
    id: &str,
    name: &str,
    host: &str,
    remote_orca_path: &str,
    remote_scratch_dir: &str,
    core_mask: Option<&str>,
) -> Result<ServerProfile, AppError> {
    let affected = conn.execute(
        "UPDATE server_profiles
         SET name = ?1, host = ?2, remote_orca_path = ?3,
             remote_scratch_dir = ?4, core_mask = ?5
         WHERE id = ?6",
        params![name, host, remote_orca_path, remote_scratch_dir, core_mask, id],
    )?;
    if affected == 0 {
        return Err(AppError::NotFound(format!("server profile {id}")));
    }
    get_profile_conn(conn, id)
}

/// Delete a profile. **Nulls the `backend_id` of every job that ran on it FIRST** (the jobs
/// revert to `NULL = local`, ADR-023), then removes the profile row — the jobs survive as
/// standalone jobs, exactly like `delete_reaction` (the load-bearing invariant). The
/// explicit null holds even if the FK's `ON DELETE SET NULL` were not enforced.
/// [`AppError::NotFound`] if the profile is absent (nothing is touched in that case).
fn delete_server_profile_conn(conn: &Connection, id: &str) -> Result<(), AppError> {
    if !profile_exists(conn, id)? {
        return Err(AppError::NotFound(format!("server profile {id}")));
    }
    // Null the run-target FK on jobs that used this profile FIRST — never DELETE a job.
    conn.execute(
        "UPDATE jobs SET backend_id = NULL WHERE backend_id = ?1",
        params![id],
    )?;
    conn.execute("DELETE FROM server_profiles WHERE id = ?1", params![id])?;
    Ok(())
}

/// Stamp the four verified_* columns after a successful connection-test:
/// `orca_version`, `openmpi_version`, `core_count`, and `verified_at = datetime('now')`.
/// This is the pure DB write half of the connection-test — Part B's real SSH session
/// measures the specs (rule #10) and calls this to persist them. Flipping `verified_at`
/// from NULL to a timestamp is what makes the profile a usable run target (ADR-023).
/// [`AppError::NotFound`] if the profile is absent. Returns the stamped profile.
fn set_profile_verified_conn(
    conn: &Connection,
    id: &str,
    orca_version: &str,
    openmpi_version: &str,
    core_count: u32,
) -> Result<ServerProfile, AppError> {
    let affected = conn.execute(
        "UPDATE server_profiles
         SET orca_version = ?1, openmpi_version = ?2, core_count = ?3,
             verified_at = datetime('now')
         WHERE id = ?4",
        params![orca_version, openmpi_version, core_count, id],
    )?;
    if affected == 0 {
        return Err(AppError::NotFound(format!("server profile {id}")));
    }
    get_profile_conn(conn, id)
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
) -> Result<ServerProfile, AppError> {
    let conn = db.lock()?;
    create_server_profile_conn(
        &conn,
        &name,
        &host,
        &remote_orca_path,
        &remote_scratch_dir,
        core_mask.as_deref(),
    )
}

#[tauri::command]
pub fn list_server_profiles(db: State<'_, DbState>) -> Result<Vec<ServerProfile>, AppError> {
    let conn = db.lock()?;
    list_server_profiles_conn(&conn)
}

#[tauri::command]
pub fn update_server_profile(
    db: State<'_, DbState>,
    id: String,
    name: String,
    host: String,
    remote_orca_path: String,
    remote_scratch_dir: String,
    core_mask: Option<String>,
) -> Result<ServerProfile, AppError> {
    let conn = db.lock()?;
    update_server_profile_conn(
        &conn,
        &id,
        &name,
        &host,
        &remote_orca_path,
        &remote_scratch_dir,
        core_mask.as_deref(),
    )
}

#[tauri::command]
pub fn delete_server_profile(db: State<'_, DbState>, id: String) -> Result<(), AppError> {
    let conn = db.lock()?;
    delete_server_profile_conn(&conn, &id)
}

#[tauri::command]
pub fn set_profile_verified(
    db: State<'_, DbState>,
    id: String,
    orca_version: String,
    openmpi_version: String,
    core_count: u32,
) -> Result<ServerProfile, AppError> {
    let conn = db.lock()?;
    set_profile_verified_conn(&conn, &id, &orca_version, &openmpi_version, core_count)
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

    // C-create-list-roundtrip: every user field survives the create→list round-trip, and a
    // freshly created profile is honestly unverified (all four verified_* columns NULL).
    #[test]
    fn create_and_list_roundtrips_all_fields() {
        let (conn, dir) = test_db();

        let p = create_server_profile_conn(
            &conn,
            "uni cluster",
            "uni",
            "/opt/orca/orca",
            "/scratch/anton",
            Some("0-7"),
        )
        .unwrap();
        assert_eq!(p.name, "uni cluster");
        assert_eq!(p.host, "uni");
        assert_eq!(p.remote_orca_path, "/opt/orca/orca");
        assert_eq!(p.remote_scratch_dir, "/scratch/anton");
        assert_eq!(p.core_mask.as_deref(), Some("0-7"));
        // Honest-or-absent: unverified profile carries no forged specs.
        assert_eq!(p.orca_version, None);
        assert_eq!(p.openmpi_version, None);
        assert_eq!(p.core_count, None);
        assert_eq!(p.verified_at, None, "a new profile is not yet a run target");

        // core_mask is optional.
        let p2 = create_server_profile_conn(
            &conn, "lab box", "lab", "/usr/local/orca/orca", "/tmp/orca", None,
        )
        .unwrap();
        assert_eq!(p2.core_mask, None);

        // list round-trips both, and re-hydrates every field via COLUMNS/from_row.
        let all = list_server_profiles_conn(&conn).unwrap();
        assert_eq!(all.len(), 2);
        let by_id = |id: &str| all.iter().find(|x| x.id == id).unwrap();
        assert_eq!(by_id(&p.id).remote_scratch_dir, "/scratch/anton");
        assert_eq!(by_id(&p2.id).host, "lab");

        std::fs::remove_dir_all(&dir).ok();
    }

    // C-update-user-fields-only: update mutates the user fields, and NotFound on a missing
    // id. The bite is paired with set_profile_verified below (update must not clear a stamp).
    #[test]
    fn update_mutates_user_fields_and_notfound() {
        let (conn, dir) = test_db();

        let p = create_server_profile_conn(
            &conn, "old", "old-host", "/opt/orca/orca", "/scratch", None,
        )
        .unwrap();

        let updated = update_server_profile_conn(
            &conn,
            &p.id,
            "new name",
            "new-host",
            "/opt/orca6/orca",
            "/scratch2",
            Some("0-3"),
        )
        .unwrap();
        assert_eq!(updated.name, "new name");
        assert_eq!(updated.host, "new-host");
        assert_eq!(updated.remote_orca_path, "/opt/orca6/orca");
        assert_eq!(updated.remote_scratch_dir, "/scratch2");
        assert_eq!(updated.core_mask.as_deref(), Some("0-3"));

        // update of a missing id → NotFound.
        assert!(matches!(
            update_server_profile_conn(
                &conn, "no-such", "x", "y", "z", "w", None
            )
            .unwrap_err(),
            AppError::NotFound(_)
        ));

        std::fs::remove_dir_all(&dir).ok();
    }

    // C-set-verified-preserved-by-update: set_profile_verified flips verified_at from NULL
    // to set and the usability gate `verified_at IS NOT NULL` now holds; a subsequent
    // update_server_profile (user fields) must NOT clear the stamp.
    //
    // The bite: an implementation of update that touched the verified_* columns (e.g. a
    // blanket UPDATE resetting them) fails the "stamp survives the update" assert — the
    // gate would silently drop back to unverified.
    #[test]
    fn set_profile_verified_stamps_and_update_preserves_it() {
        let (conn, dir) = test_db();

        let p = create_server_profile_conn(
            &conn, "uni", "uni", "/opt/orca/orca", "/scratch", None,
        )
        .unwrap();

        // Before: the usability gate is closed (verified_at NULL).
        assert_eq!(p.verified_at, None);
        assert_eq!(usable_count(&conn), 0, "no profile passes the gate yet");

        let stamped = set_profile_verified_conn(&conn, &p.id, "6.1.0", "4.1.6", 16).unwrap();
        assert_eq!(stamped.orca_version.as_deref(), Some("6.1.0"));
        assert_eq!(stamped.openmpi_version.as_deref(), Some("4.1.6"));
        assert_eq!(stamped.core_count, Some(16));
        assert!(stamped.verified_at.is_some(), "verified_at is stamped");
        assert_eq!(usable_count(&conn), 1, "the gate now admits the profile");

        // A user-field update must PRESERVE the verification stamp.
        let after = update_server_profile_conn(
            &conn, &p.id, "uni renamed", "uni", "/opt/orca/orca", "/scratch", None,
        )
        .unwrap();
        assert_eq!(after.name, "uni renamed");
        assert!(
            after.verified_at.is_some(),
            "update of user fields must NOT clear the verification stamp"
        );
        assert_eq!(after.orca_version.as_deref(), Some("6.1.0"));
        assert_eq!(usable_count(&conn), 1, "still a run target after the edit");

        // set_profile_verified of a missing id → NotFound.
        assert!(matches!(
            set_profile_verified_conn(&conn, "no-such", "6.1.0", "4.1.6", 8).unwrap_err(),
            AppError::NotFound(_)
        ));

        std::fs::remove_dir_all(&dir).ok();
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

        let p = create_server_profile_conn(
            &conn, "uni", "uni", "/opt/orca/orca", "/scratch", None,
        )
        .unwrap();
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
}
