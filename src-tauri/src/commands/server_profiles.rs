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

use rusqlite::{params, Connection, OptionalExtension};
use tauri::State;
use uuid::Uuid;

use crate::commands::settings::DbState;
use crate::error::AppError;
use crate::models::server_profile::{validate_profile, ProfileTarget, ServerProfile};

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
/// written). If the **value** of any target field changes, the verification stamp and the facts
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

/// Set `verified_at` and the facts it certified back to NULL, together.
fn clear_verified(conn: &Connection, id: &str) -> Result<usize, AppError> {
    Ok(conn.execute(
        "UPDATE server_profiles
         SET verified_at = NULL, orca_version = NULL, openmpi_version = NULL, core_count = NULL
         WHERE id = ?1",
        params![id],
    )?)
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
) -> Result<ServerProfile, AppError> {
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
}

#[tauri::command]
pub fn list_server_profiles(db: State<'_, DbState>) -> Result<Vec<ServerProfile>, AppError> {
    let conn = db.lock()?;
    list_server_profiles_conn(&conn)
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
) -> Result<ServerProfile, AppError> {
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
    tested: ProfileTarget,
    orca_version: String,
    openmpi_version: Option<String>,
    core_count: u32,
) -> Result<ServerProfile, AppError> {
    let conn = db.lock()?;
    set_profile_verified_conn(
        &conn,
        &id,
        &tested,
        &orca_version,
        openmpi_version.as_deref(),
        core_count,
    )
}

#[tauri::command]
pub fn clear_profile_verified(db: State<'_, DbState>, id: String) -> Result<ServerProfile, AppError> {
    let conn = db.lock()?;
    clear_profile_verified_conn(&conn, &id)
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
}
