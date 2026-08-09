//! Reaction/Pathway commands: the CRUD + grouping surface over the `reactions` and
//! `pathways` tables and the nullable `jobs.pathway_id` (schema v13, Phase 4.5 C1),
//! plus the **summed reactant reference** — `reaction_reference_jobs` (schema v14,
//! Phase 4.5 C2b-2a, ADR-018), the reference for ABSOLUTE barriers.
//!
//! Same shape as `commands::molecules`: each Tauri command is a thin wrapper that
//! locks the shared connection and delegates to a `*_conn` helper taking a
//! `&Connection`, so the logic is unit-testable without a running Tauri app.
//!
//! Load-bearing safety property (invariant 1): **jobs are the work; reactions and
//! pathways are grouping metadata.** Deleting a reaction or a pathway NEVER deletes
//! a job — it removes the grouping rows and nulls the `pathway_id` of any jobs that
//! pointed at them. A job un-grouped this way is exactly the standalone job it was
//! before. `reaction_reference_jobs` rows follow the same rule: `delete_reaction`
//! removes a reaction's reference rows, and neither that nor `remove_reference_job`
//! ever deletes the underlying job. Referential integrity (a pathway's reaction must
//! exist; an attached/referenced job and pathway must exist) is enforced HERE, in app
//! code, so the caller gets a clean [`AppError::NotFound`] rather than a raw SQLite
//! constraint error.
//!
//! NOTE (measured, rule #10 — contradicts the C1 comment that this DB "leaves FK
//! enforcement off"): the `bundled` SQLite is compiled with `SQLITE_DEFAULT_FOREIGN_KEYS=1`
//! (`PRAGMA foreign_keys` reads 1 on the init_db connection), so the `REFERENCES`
//! clauses are **actively enforced**, not just documentation. This makes deletion
//! ordering load-bearing: `delete_reaction` must drop the child `pathways` and
//! `reaction_reference_jobs` rows BEFORE the parent `reactions` row, or SQLite raises
//! error 787 (`SQLITE_CONSTRAINT_FOREIGNKEY`). The commands already order it that way.
//!
//! Honest-or-absent (ADR-018): a reaction's `E(ref)` is the SUM of its reference jobs'
//! parsed final energies, computed on demand from the authoritative `results` tier —
//! never cached on `reactions` (no two-sources-of-truth drift). If ANY reference job
//! is unparsed, the reference is incomplete and `reaction_reference_energy` returns
//! `energy_eh: None` (with the jobs listed so the caller sees which is missing) — a
//! partial sum is never returned, because a wrong `E(ref)` silently poisons every
//! absolute barrier built on it.

use rusqlite::{params, Connection, OptionalExtension};
use tauri::State;
use uuid::Uuid;

use crate::commands::settings::DbState;
use crate::error::AppError;
use crate::models::reaction::{Pathway, Reaction, ReferenceEnergy, ReferenceJob};

// --- Existence probes (app-level referential integrity) ---------------------

fn reaction_exists(conn: &Connection, id: &str) -> Result<bool, AppError> {
    Ok(conn
        .query_row("SELECT 1 FROM reactions WHERE id = ?1", params![id], |_| Ok(()))
        .optional()?
        .is_some())
}

fn pathway_exists(conn: &Connection, id: &str) -> Result<bool, AppError> {
    Ok(conn
        .query_row("SELECT 1 FROM pathways WHERE id = ?1", params![id], |_| Ok(()))
        .optional()?
        .is_some())
}

fn job_exists(conn: &Connection, id: &str) -> Result<bool, AppError> {
    Ok(conn
        .query_row("SELECT 1 FROM jobs WHERE id = ?1", params![id], |_| Ok(()))
        .optional()?
        .is_some())
}

// --- Connection-level helpers (testable) ------------------------------------

/// A single reaction by id, or [`AppError::NotFound`].
fn get_reaction_conn(conn: &Connection, id: &str) -> Result<Reaction, AppError> {
    let sql = format!("SELECT {} FROM reactions WHERE id = ?1", Reaction::COLUMNS);
    conn.query_row(&sql, params![id], Reaction::from_row)
        .optional()?
        .ok_or_else(|| AppError::NotFound(format!("reaction {id}")))
}

/// A single pathway by id, or [`AppError::NotFound`].
fn get_pathway_conn(conn: &Connection, id: &str) -> Result<Pathway, AppError> {
    let sql = format!("SELECT {} FROM pathways WHERE id = ?1", Pathway::COLUMNS);
    conn.query_row(&sql, params![id], Pathway::from_row)
        .optional()?
        .ok_or_else(|| AppError::NotFound(format!("pathway {id}")))
}

fn create_reaction_conn(
    conn: &Connection,
    name: &str,
    description: Option<&str>,
) -> Result<Reaction, AppError> {
    let id = Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO reactions (id, name, description) VALUES (?1, ?2, ?3)",
        params![id, name, description],
    )?;
    get_reaction_conn(conn, &id)
}

/// All reactions, newest first.
fn list_reactions_conn(conn: &Connection) -> Result<Vec<Reaction>, AppError> {
    let sql = format!(
        "SELECT {} FROM reactions ORDER BY created_at DESC, id",
        Reaction::COLUMNS
    );
    let mut stmt = conn.prepare(&sql)?;
    let reactions = stmt
        .query_map([], Reaction::from_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(reactions)
}

/// Rename a reaction; [`AppError::NotFound`] if the id is absent. Returns the
/// updated reaction.
fn rename_reaction_conn(conn: &Connection, id: &str, name: &str) -> Result<Reaction, AppError> {
    let affected = conn.execute(
        "UPDATE reactions SET name = ?1 WHERE id = ?2",
        params![name, id],
    )?;
    if affected == 0 {
        return Err(AppError::NotFound(format!("reaction {id}")));
    }
    get_reaction_conn(conn, id)
}

/// Delete a reaction. Removes its pathway rows and **nulls the `pathway_id` of every
/// job that pointed at them** — the jobs survive as standalone jobs (invariant 1).
/// [`AppError::NotFound`] if the reaction is absent (nothing is touched in that case).
fn delete_reaction_conn(conn: &Connection, id: &str) -> Result<(), AppError> {
    if !reaction_exists(conn, id)? {
        return Err(AppError::NotFound(format!("reaction {id}")));
    }
    // Null the grouping FK on jobs attached to any of this reaction's pathways FIRST,
    // while the pathways still exist to name them — never DELETE a job.
    conn.execute(
        "UPDATE jobs SET pathway_id = NULL
         WHERE pathway_id IN (SELECT id FROM pathways WHERE reaction_id = ?1)",
        params![id],
    )?;
    conn.execute("DELETE FROM pathways WHERE reaction_id = ?1", params![id])?;
    // Drop this reaction's reference-job rows too (v14). These are grouping metadata,
    // never jobs — the referenced jobs stay standalone in the Jobs list (invariant 2).
    conn.execute(
        "DELETE FROM reaction_reference_jobs WHERE reaction_id = ?1",
        params![id],
    )?;
    conn.execute("DELETE FROM reactions WHERE id = ?1", params![id])?;
    Ok(())
}

/// Create a pathway under an existing reaction. [`AppError::NotFound`] if the
/// reaction does not exist — no orphan pathway row is written.
fn create_pathway_conn(
    conn: &Connection,
    reaction_id: &str,
    label: &str,
) -> Result<Pathway, AppError> {
    if !reaction_exists(conn, reaction_id)? {
        return Err(AppError::NotFound(format!("reaction {reaction_id}")));
    }
    let id = Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO pathways (id, reaction_id, label) VALUES (?1, ?2, ?3)",
        params![id, reaction_id, label],
    )?;
    get_pathway_conn(conn, &id)
}

/// All pathways under one reaction, newest first.
fn list_pathways_conn(conn: &Connection, reaction_id: &str) -> Result<Vec<Pathway>, AppError> {
    let sql = format!(
        "SELECT {} FROM pathways WHERE reaction_id = ?1 ORDER BY created_at DESC, id",
        Pathway::COLUMNS
    );
    let mut stmt = conn.prepare(&sql)?;
    let pathways = stmt
        .query_map(params![reaction_id], Pathway::from_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(pathways)
}

/// Delete a pathway. Nulls the `pathway_id` of every job attached to it (never
/// deletes a job), then removes the row. [`AppError::NotFound`] if absent.
fn delete_pathway_conn(conn: &Connection, id: &str) -> Result<(), AppError> {
    if !pathway_exists(conn, id)? {
        return Err(AppError::NotFound(format!("pathway {id}")));
    }
    conn.execute(
        "UPDATE jobs SET pathway_id = NULL WHERE pathway_id = ?1",
        params![id],
    )?;
    conn.execute("DELETE FROM pathways WHERE id = ?1", params![id])?;
    Ok(())
}

/// Attach a job to a pathway — the bottom-up **promote** action. Permissive about
/// job kind (any job may be attached; comparability guards live in C2). Errors if
/// either the job or the pathway does not exist — no partial write.
///
/// `pub(crate)` so the OptTS create-side (`commands::jobs::create_optts_job`) can attach a refined
/// TS to its SOURCE job's pathway through this one mechanism (ADR-020) — no second attach path.
pub(crate) fn attach_job_to_pathway_conn(
    conn: &Connection,
    job_id: &str,
    pathway_id: &str,
) -> Result<(), AppError> {
    if !job_exists(conn, job_id)? {
        return Err(AppError::NotFound(format!("job {job_id}")));
    }
    if !pathway_exists(conn, pathway_id)? {
        return Err(AppError::NotFound(format!("pathway {pathway_id}")));
    }
    conn.execute(
        "UPDATE jobs SET pathway_id = ?1 WHERE id = ?2",
        params![pathway_id, job_id],
    )?;
    Ok(())
}

/// Detach a job from its pathway (nulls `pathway_id`); the job becomes standalone
/// again. [`AppError::NotFound`] if the job does not exist. Idempotent for a job
/// that is already standalone.
fn detach_job_from_pathway_conn(conn: &Connection, job_id: &str) -> Result<(), AppError> {
    if !job_exists(conn, job_id)? {
        return Err(AppError::NotFound(format!("job {job_id}")));
    }
    conn.execute(
        "UPDATE jobs SET pathway_id = NULL WHERE id = ?1",
        params![job_id],
    )?;
    Ok(())
}

// --- Reference jobs (summed reactant reference, v14, ADR-018) ---------------

/// Add a job to a reaction's reactant reference. Errors ([`AppError::NotFound`]) if
/// either the reaction or the job does not exist — no partial write. Idempotent on the
/// `(reaction_id, job_id)` PK (`INSERT OR IGNORE`): adding the same job twice is a
/// no-op, not an error. A reference row is grouping metadata — it never touches the job.
fn add_reference_job_conn(
    conn: &Connection,
    reaction_id: &str,
    job_id: &str,
) -> Result<(), AppError> {
    if !reaction_exists(conn, reaction_id)? {
        return Err(AppError::NotFound(format!("reaction {reaction_id}")));
    }
    if !job_exists(conn, job_id)? {
        return Err(AppError::NotFound(format!("job {job_id}")));
    }
    conn.execute(
        "INSERT OR IGNORE INTO reaction_reference_jobs (reaction_id, job_id) VALUES (?1, ?2)",
        params![reaction_id, job_id],
    )?;
    Ok(())
}

/// Remove a job from a reaction's reactant reference. Removes the grouping row only —
/// the job survives as a standalone job (invariant 2). Idempotent: removing a pair that
/// is not referenced is a no-op.
fn remove_reference_job_conn(
    conn: &Connection,
    reaction_id: &str,
    job_id: &str,
) -> Result<(), AppError> {
    conn.execute(
        "DELETE FROM reaction_reference_jobs WHERE reaction_id = ?1 AND job_id = ?2",
        params![reaction_id, job_id],
    )?;
    Ok(())
}

/// The reference jobs of a reaction, each with its title and its **per-job** parsed
/// final energy read from the authoritative `results` tier (ADR-012) — `None` when that
/// job is unparsed / still running / failed. Ordered by attachment (`created_at`, then
/// `job_id` as a stable tiebreak). The energy is read on demand, never cached.
fn list_reference_jobs_conn(
    conn: &Connection,
    reaction_id: &str,
) -> Result<Vec<ReferenceJob>, AppError> {
    // LEFT JOIN on `results`: a reference job with no parsed result yields a NULL
    // final_energy_eh (the honest-or-absent signal), not a dropped row.
    let mut stmt = conn.prepare(
        "SELECT rrj.job_id, j.title, r.final_energy_eh
         FROM reaction_reference_jobs rrj
         JOIN jobs j ON j.id = rrj.job_id
         LEFT JOIN results r ON r.job_id = rrj.job_id
         WHERE rrj.reaction_id = ?1
         ORDER BY rrj.created_at, rrj.job_id",
    )?;
    let jobs = stmt
        .query_map(params![reaction_id], |row| {
            Ok(ReferenceJob {
                job_id: row.get(0)?,
                title: row.get(1)?,
                final_energy_eh: row.get(2)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(jobs)
}

/// A reaction's summed reactant reference: the provenance AND the honest total.
/// `energy_eh` is `Some(Σ final_energy_eh)` **only if the list is non-empty and every
/// reference job has a parsed final energy**; otherwise `None` (incomplete or empty).
/// A partial reference is NEVER summed — the honest-or-absent property (ADR-018): a
/// wrong `E(ref)` would silently poison every absolute barrier built on it.
fn reaction_reference_energy_conn(
    conn: &Connection,
    reaction_id: &str,
) -> Result<ReferenceEnergy, AppError> {
    let jobs = list_reference_jobs_conn(conn, reaction_id)?;
    // Honest-or-absent, expressed totally: `Option<f64>: Sum<Option<f64>>` yields `None`
    // the moment ANY element is `None`, else `Some(Σ)` — exactly the incomplete-vs-complete
    // semantics, no `unwrap`. The empty list would sum to `Some(0.0)`, so an explicit guard
    // keeps an empty reference `None` (not "a reference of zero").
    let energy_eh = if jobs.is_empty() {
        None
    } else {
        jobs.iter().map(|j| j.final_energy_eh).sum::<Option<f64>>()
    };
    Ok(ReferenceEnergy { jobs, energy_eh })
}

// --- Tauri commands ---------------------------------------------------------

#[tauri::command]
pub fn create_reaction(
    db: State<'_, DbState>,
    name: String,
    description: Option<String>,
) -> Result<Reaction, AppError> {
    let conn = db.lock()?;
    create_reaction_conn(&conn, &name, description.as_deref())
}

#[tauri::command]
pub fn list_reactions(db: State<'_, DbState>) -> Result<Vec<Reaction>, AppError> {
    let conn = db.lock()?;
    list_reactions_conn(&conn)
}

#[tauri::command]
pub fn rename_reaction(
    db: State<'_, DbState>,
    id: String,
    name: String,
) -> Result<Reaction, AppError> {
    let conn = db.lock()?;
    rename_reaction_conn(&conn, &id, &name)
}

#[tauri::command]
pub fn delete_reaction(db: State<'_, DbState>, id: String) -> Result<(), AppError> {
    let conn = db.lock()?;
    delete_reaction_conn(&conn, &id)
}

#[tauri::command]
pub fn create_pathway(
    db: State<'_, DbState>,
    reaction_id: String,
    label: String,
) -> Result<Pathway, AppError> {
    let conn = db.lock()?;
    create_pathway_conn(&conn, &reaction_id, &label)
}

#[tauri::command]
pub fn list_pathways(
    db: State<'_, DbState>,
    reaction_id: String,
) -> Result<Vec<Pathway>, AppError> {
    let conn = db.lock()?;
    list_pathways_conn(&conn, &reaction_id)
}

#[tauri::command]
pub fn delete_pathway(db: State<'_, DbState>, id: String) -> Result<(), AppError> {
    let conn = db.lock()?;
    delete_pathway_conn(&conn, &id)
}

#[tauri::command]
pub fn attach_job_to_pathway(
    db: State<'_, DbState>,
    job_id: String,
    pathway_id: String,
) -> Result<(), AppError> {
    let conn = db.lock()?;
    attach_job_to_pathway_conn(&conn, &job_id, &pathway_id)
}

#[tauri::command]
pub fn detach_job_from_pathway(db: State<'_, DbState>, job_id: String) -> Result<(), AppError> {
    let conn = db.lock()?;
    detach_job_from_pathway_conn(&conn, &job_id)
}

#[tauri::command]
pub fn add_reference_job(
    db: State<'_, DbState>,
    reaction_id: String,
    job_id: String,
) -> Result<(), AppError> {
    let conn = db.lock()?;
    add_reference_job_conn(&conn, &reaction_id, &job_id)
}

#[tauri::command]
pub fn remove_reference_job(
    db: State<'_, DbState>,
    reaction_id: String,
    job_id: String,
) -> Result<(), AppError> {
    let conn = db.lock()?;
    remove_reference_job_conn(&conn, &reaction_id, &job_id)
}

#[tauri::command]
pub fn list_reference_jobs(
    db: State<'_, DbState>,
    reaction_id: String,
) -> Result<Vec<ReferenceJob>, AppError> {
    let conn = db.lock()?;
    list_reference_jobs_conn(&conn, &reaction_id)
}

#[tauri::command]
pub fn reaction_reference_energy(
    db: State<'_, DbState>,
    reaction_id: String,
) -> Result<ReferenceEnergy, AppError> {
    let conn = db.lock()?;
    reaction_reference_energy_conn(&conn, &reaction_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::init_db;

    /// A migrated database in a throwaway temp dir. A process-wide atomic counter
    /// keeps each test's directory unique even under parallel runs.
    fn test_db() -> (Connection, std::path::PathBuf) {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "orcastudio-reactions-test-{}-{}",
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

    fn job_pathway(conn: &Connection, id: &str) -> Option<String> {
        conn.query_row("SELECT pathway_id FROM jobs WHERE id = ?1", params![id], |r| {
            r.get::<_, Option<String>>(0)
        })
        .expect("job should exist")
    }

    /// Insert a parsed `results` row for a job with the given final energy — makes the
    /// job "parsed" for the reference-energy sum. A job with no such row reads back with
    /// `final_energy_eh = None` (unparsed / still running / failed).
    fn insert_result(conn: &Connection, job_id: &str, energy: f64) {
        conn.execute(
            "INSERT INTO results (job_id, final_energy_eh, data_json, parser_version)
             VALUES (?1, ?2, '{}', 3)",
            params![job_id, energy],
        )
        .expect("insert result");
    }

    #[test]
    fn create_and_list_reactions() {
        let (conn, dir) = test_db();

        let r = create_reaction_conn(&conn, "NaBH4 reduction", Some("stereoselectivity")).unwrap();
        assert_eq!(r.name, "NaBH4 reduction");
        assert_eq!(r.description.as_deref(), Some("stereoselectivity"));

        // description is optional.
        let r2 = create_reaction_conn(&conn, "SN2", None).unwrap();
        assert_eq!(r2.description, None);

        let all = list_reactions_conn(&conn).unwrap();
        assert_eq!(all.len(), 2);

        // rename updates in place.
        let renamed = rename_reaction_conn(&conn, &r.id, "NaBH4 reduction of ketone").unwrap();
        assert_eq!(renamed.name, "NaBH4 reduction of ketone");
        assert_eq!(get_reaction_conn(&conn, &r.id).unwrap().name, "NaBH4 reduction of ketone");

        // rename of a missing id is NotFound.
        assert!(matches!(
            rename_reaction_conn(&conn, "no-such", "x").unwrap_err(),
            AppError::NotFound(_)
        ));

        std::fs::remove_dir_all(&dir).ok();
    }

    // C-referential-integrity: create_pathway under a missing reaction → Err, and no
    // orphan pathway row is written. attach with a missing pathway or job → Err.
    #[test]
    fn referential_integrity_is_enforced() {
        let (conn, dir) = test_db();

        // A pathway cannot hang off a reaction that does not exist.
        let err = create_pathway_conn(&conn, "no-such-reaction", "si-face").unwrap_err();
        assert!(matches!(err, AppError::NotFound(_)));
        let orphans: i64 = conn
            .query_row("SELECT COUNT(*) FROM pathways", [], |r| r.get(0))
            .unwrap();
        assert_eq!(orphans, 0, "no orphan pathway row is written");

        let r = create_reaction_conn(&conn, "R", None).unwrap();
        let p = create_pathway_conn(&conn, &r.id, "si-face").unwrap();
        insert_job(&conn, "j1");

        // attach with a missing pathway → Err, job untouched.
        assert!(matches!(
            attach_job_to_pathway_conn(&conn, "j1", "no-such-pathway").unwrap_err(),
            AppError::NotFound(_)
        ));
        assert_eq!(job_pathway(&conn, "j1"), None);

        // attach with a missing job → Err.
        assert!(matches!(
            attach_job_to_pathway_conn(&conn, "no-such-job", &p.id).unwrap_err(),
            AppError::NotFound(_)
        ));

        std::fs::remove_dir_all(&dir).ok();
    }

    // C-delete-reaction-keeps-jobs (the load-bearing invariant). A reaction with a
    // pathway that has an attached job → delete_reaction → the job STILL EXISTS, its
    // pathway_id is NULL, the pathway row is gone, the reaction is gone.
    //
    // The bite: an implementation that `DELETE`s the job (a naive cascade) fails the
    // "job still exists" assert; one that leaves a dangling pathway_id fails the NULL
    // assert. This test distinguishes the safe implementation from either bug.
    #[test]
    fn delete_reaction_keeps_jobs() {
        let (conn, dir) = test_db();

        let r = create_reaction_conn(&conn, "R", None).unwrap();
        let p = create_pathway_conn(&conn, &r.id, "si-face").unwrap();
        insert_job(&conn, "j1");
        attach_job_to_pathway_conn(&conn, "j1", &p.id).unwrap();
        assert_eq!(job_pathway(&conn, "j1").as_deref(), Some(p.id.as_str()));

        delete_reaction_conn(&conn, &r.id).unwrap();

        // The job survives as a standalone job — pathway_id nulled, everything else
        // intact.
        assert!(job_exists(&conn, "j1").unwrap(), "the job MUST survive");
        assert_eq!(job_pathway(&conn, "j1"), None, "un-grouped: pathway_id is NULL");
        // The grouping rows are gone.
        assert!(!pathway_exists(&conn, &p.id).unwrap());
        assert!(!reaction_exists(&conn, &r.id).unwrap());

        // deleting a missing reaction is NotFound.
        assert!(matches!(
            delete_reaction_conn(&conn, "no-such").unwrap_err(),
            AppError::NotFound(_)
        ));

        std::fs::remove_dir_all(&dir).ok();
    }

    // delete_pathway has the same job-preserving contract as delete_reaction.
    #[test]
    fn delete_pathway_keeps_jobs() {
        let (conn, dir) = test_db();

        let r = create_reaction_conn(&conn, "R", None).unwrap();
        let p = create_pathway_conn(&conn, &r.id, "re-face").unwrap();
        insert_job(&conn, "j1");
        attach_job_to_pathway_conn(&conn, "j1", &p.id).unwrap();

        delete_pathway_conn(&conn, &p.id).unwrap();

        assert!(job_exists(&conn, "j1").unwrap(), "the job MUST survive");
        assert_eq!(job_pathway(&conn, "j1"), None);
        assert!(!pathway_exists(&conn, &p.id).unwrap());
        // The reaction itself is untouched by a pathway deletion.
        assert!(reaction_exists(&conn, &r.id).unwrap());

        assert!(matches!(
            delete_pathway_conn(&conn, "no-such").unwrap_err(),
            AppError::NotFound(_)
        ));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn attach_detach_roundtrip_and_list_pathways() {
        let (conn, dir) = test_db();

        let r = create_reaction_conn(&conn, "R", None).unwrap();
        let si = create_pathway_conn(&conn, &r.id, "si-face").unwrap();
        let re = create_pathway_conn(&conn, &r.id, "re-face").unwrap();

        let pathways = list_pathways_conn(&conn, &r.id).unwrap();
        assert_eq!(pathways.len(), 2);
        assert!(pathways.iter().all(|p| p.reaction_id == r.id));

        insert_job(&conn, "j1");
        attach_job_to_pathway_conn(&conn, "j1", &si.id).unwrap();
        assert_eq!(job_pathway(&conn, "j1").as_deref(), Some(si.id.as_str()));

        // Re-attaching to another pathway moves it (one source of truth).
        attach_job_to_pathway_conn(&conn, "j1", &re.id).unwrap();
        assert_eq!(job_pathway(&conn, "j1").as_deref(), Some(re.id.as_str()));

        // Detach → standalone again.
        detach_job_from_pathway_conn(&conn, "j1").unwrap();
        assert_eq!(job_pathway(&conn, "j1"), None);

        // Detaching a missing job is NotFound.
        assert!(matches!(
            detach_job_from_pathway_conn(&conn, "no-such-job").unwrap_err(),
            AppError::NotFound(_)
        ));

        std::fs::remove_dir_all(&dir).ok();
    }

    // --- Reference jobs (v14, ADR-018) --------------------------------------

    // C-referential-integrity (reference variant): add_reference_job with a missing
    // reaction or a missing job → Err, no partial write. Idempotent on the PK.
    #[test]
    fn add_reference_job_integrity_and_idempotent() {
        let (conn, dir) = test_db();

        let r = create_reaction_conn(&conn, "R", None).unwrap();
        insert_job(&conn, "j1");

        // Missing reaction → Err, nothing written.
        assert!(matches!(
            add_reference_job_conn(&conn, "no-such-reaction", "j1").unwrap_err(),
            AppError::NotFound(_)
        ));
        // Missing job → Err.
        assert!(matches!(
            add_reference_job_conn(&conn, &r.id, "no-such-job").unwrap_err(),
            AppError::NotFound(_)
        ));
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM reaction_reference_jobs", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 0, "no partial write on a failed add");

        // Valid add, then a second identical add is a no-op (PK idempotent), not an error.
        add_reference_job_conn(&conn, &r.id, "j1").unwrap();
        add_reference_job_conn(&conn, &r.id, "j1").unwrap();
        assert_eq!(list_reference_jobs_conn(&conn, &r.id).unwrap().len(), 1);

        // remove_reference_job drops the row (job survives); idempotent when absent.
        remove_reference_job_conn(&conn, &r.id, "j1").unwrap();
        assert_eq!(list_reference_jobs_conn(&conn, &r.id).unwrap().len(), 0);
        assert!(job_exists(&conn, "j1").unwrap(), "remove never deletes the job");
        remove_reference_job_conn(&conn, &r.id, "j1").unwrap(); // no-op, no error

        std::fs::remove_dir_all(&dir).ok();
    }

    // C-sum: two parsed reference jobs → energy_eh == Some(e1 + e2), exact.
    #[test]
    fn reference_energy_sums_when_all_parsed() {
        let (conn, dir) = test_db();

        let r = create_reaction_conn(&conn, "R", None).unwrap();
        insert_job(&conn, "sub");
        insert_job(&conn, "bh4");
        insert_result(&conn, "sub", -270.123456);
        insert_result(&conn, "bh4", -27.654321);
        add_reference_job_conn(&conn, &r.id, "sub").unwrap();
        add_reference_job_conn(&conn, &r.id, "bh4").unwrap();

        let ref_e = reaction_reference_energy_conn(&conn, &r.id).unwrap();
        assert_eq!(ref_e.jobs.len(), 2);
        let expected = -270.123456 + -27.654321;
        let got = ref_e.energy_eh.expect("complete reference sums");
        assert!((got - expected).abs() < 1e-12, "sum {got} != {expected}");

        std::fs::remove_dir_all(&dir).ok();
    }

    // C-incomplete-not-summed (THE load-bearing property): one parsed, one unparsed →
    // energy_eh is None (incomplete), and jobs lists BOTH with the missing one's energy
    // None. The bite: an implementation that sums the present energy and returns a total
    // fails the `energy_eh == None` assert — honest-or-absent, not a silent partial.
    #[test]
    fn reference_energy_incomplete_is_none_not_partial() {
        let (conn, dir) = test_db();

        let r = create_reaction_conn(&conn, "R", None).unwrap();
        insert_job(&conn, "parsed");
        insert_job(&conn, "unparsed"); // no results row → final_energy_eh None
        insert_result(&conn, "parsed", -76.4);
        add_reference_job_conn(&conn, &r.id, "parsed").unwrap();
        add_reference_job_conn(&conn, &r.id, "unparsed").unwrap();

        let ref_e = reaction_reference_energy_conn(&conn, &r.id).unwrap();

        // Honest-or-absent: the incomplete reference has NO total.
        assert_eq!(
            ref_e.energy_eh, None,
            "an incomplete reference must not present a partial sum"
        );
        // ...but the provenance is fully listed so the UI can name the missing job.
        assert_eq!(ref_e.jobs.len(), 2);
        let by_id = |id: &str| ref_e.jobs.iter().find(|j| j.job_id == id).unwrap();
        assert_eq!(by_id("parsed").final_energy_eh, Some(-76.4));
        assert_eq!(by_id("unparsed").final_energy_eh, None, "the missing one is flagged");

        // Empty reference → also None, empty jobs (all() is vacuously true; the emptiness
        // guard keeps it honest).
        let empty = create_reaction_conn(&conn, "empty", None).unwrap();
        let ref_empty = reaction_reference_energy_conn(&conn, &empty.id).unwrap();
        assert_eq!(ref_empty.energy_eh, None);
        assert!(ref_empty.jobs.is_empty());

        std::fs::remove_dir_all(&dir).ok();
    }

    // C-delete-reaction-keeps-jobs (reference variant): a reaction with reference jobs →
    // delete_reaction → the jobs STILL EXIST, the reaction_reference_jobs rows are gone,
    // no job deleted. The bite: a naive cascade that DELETEs the jobs fails the "job
    // survives" assert.
    #[test]
    fn delete_reaction_keeps_reference_jobs() {
        let (conn, dir) = test_db();

        let r = create_reaction_conn(&conn, "R", None).unwrap();
        insert_job(&conn, "sub");
        insert_job(&conn, "bh4");
        add_reference_job_conn(&conn, &r.id, "sub").unwrap();
        add_reference_job_conn(&conn, &r.id, "bh4").unwrap();
        assert_eq!(list_reference_jobs_conn(&conn, &r.id).unwrap().len(), 2);

        delete_reaction_conn(&conn, &r.id).unwrap();

        // The reference rows are gone with the reaction...
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM reaction_reference_jobs", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 0, "reference rows removed with the reaction");
        assert!(!reaction_exists(&conn, &r.id).unwrap());
        // ...but BOTH referenced jobs survive as standalone jobs.
        assert!(job_exists(&conn, "sub").unwrap(), "the reference job MUST survive");
        assert!(job_exists(&conn, "bh4").unwrap(), "the reference job MUST survive");

        std::fs::remove_dir_all(&dir).ok();
    }

    // C-standalone-unaffected: a job with pathway_id = NULL is the normal state and
    // is untouched by the reaction model. A job created and never attached reads back
    // with a NULL pathway_id; creating/deleting reactions around it changes nothing.
    #[test]
    fn standalone_job_unaffected_by_reaction_model() {
        let (conn, dir) = test_db();

        insert_job(&conn, "standalone");
        assert_eq!(job_pathway(&conn, "standalone"), None);

        // Build and tear down a whole reaction+pathway around it.
        let r = create_reaction_conn(&conn, "R", None).unwrap();
        let p = create_pathway_conn(&conn, &r.id, "si-face").unwrap();
        delete_pathway_conn(&conn, &p.id).unwrap();
        delete_reaction_conn(&conn, &r.id).unwrap();

        // The standalone job never had a pathway_id and still does not — untouched.
        assert!(job_exists(&conn, "standalone").unwrap());
        assert_eq!(job_pathway(&conn, "standalone"), None);

        std::fs::remove_dir_all(&dir).ok();
    }
}
