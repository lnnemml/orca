//! Job commands: the CRUD + state-machine surface over the `jobs` table.
//!
//! The Tauri commands are thin wrappers that lock the shared connection and
//! delegate to the `*_conn` helpers, which take a `&Connection` directly so the
//! state-machine logic is unit-testable without a running Tauri app.

use rusqlite::{params, Connection, OptionalExtension};
use tauri::State;
use uuid::Uuid;

use crate::commands::settings::DbState;
use crate::error::AppError;
use crate::models::job::{Job, JobStatus};

// --- Connection-level helpers (testable) ------------------------------------

/// Insert a fresh `draft` job and return it fully hydrated.
fn create_job_conn(conn: &Connection, title: &str, input_content: &str) -> Result<Job, AppError> {
    let id = Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO jobs (id, title, input_content, status) VALUES (?1, ?2, ?3, ?4)",
        params![id, title, input_content, JobStatus::Draft.as_str()],
    )?;
    get_job_conn(conn, &id)
}

/// All jobs, newest first.
fn list_jobs_conn(conn: &Connection) -> Result<Vec<Job>, AppError> {
    let sql = format!("SELECT {} FROM jobs ORDER BY created_at DESC", Job::COLUMNS);
    let mut stmt = conn.prepare(&sql)?;
    let jobs = stmt
        .query_map([], Job::from_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(jobs)
}

/// A single job by id, or [`AppError::NotFound`].
fn get_job_conn(conn: &Connection, id: &str) -> Result<Job, AppError> {
    let sql = format!("SELECT {} FROM jobs WHERE id = ?1", Job::COLUMNS);
    conn.query_row(&sql, params![id], Job::from_row)
        .optional()?
        .ok_or_else(|| AppError::NotFound(format!("job {id}")))
}

/// Transition a job to `status`, stamping the matching timestamp:
/// `started_at` on entering `running`, `completed_at` on `completed`/`failed`.
fn update_job_status_conn(conn: &Connection, id: &str, status: &str) -> Result<(), AppError> {
    let status = JobStatus::from_db(status)?;
    let affected = match status {
        JobStatus::Running => conn.execute(
            "UPDATE jobs SET status = ?1, started_at = datetime('now') WHERE id = ?2",
            params![status.as_str(), id],
        )?,
        JobStatus::Completed | JobStatus::Failed => conn.execute(
            "UPDATE jobs SET status = ?1, completed_at = datetime('now') WHERE id = ?2",
            params![status.as_str(), id],
        )?,
        JobStatus::Draft => conn.execute(
            "UPDATE jobs SET status = ?1 WHERE id = ?2",
            params![status.as_str(), id],
        )?,
    };
    if affected == 0 {
        return Err(AppError::NotFound(format!("job {id}")));
    }
    Ok(())
}

// --- Tauri commands ---------------------------------------------------------

#[tauri::command]
pub fn create_job(
    db: State<'_, DbState>,
    title: String,
    input_content: String,
) -> Result<Job, AppError> {
    let conn = db.lock()?;
    create_job_conn(&conn, &title, &input_content)
}

#[tauri::command]
pub fn list_jobs(db: State<'_, DbState>) -> Result<Vec<Job>, AppError> {
    let conn = db.lock()?;
    list_jobs_conn(&conn)
}

#[tauri::command]
pub fn get_job(db: State<'_, DbState>, id: String) -> Result<Job, AppError> {
    let conn = db.lock()?;
    get_job_conn(&conn, &id)
}

#[tauri::command]
pub fn update_job_status(db: State<'_, DbState>, id: String, status: String) -> Result<(), AppError> {
    let conn = db.lock()?;
    update_job_status_conn(&conn, &id, &status)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::init_db;

    /// A migrated (v2) database in a throwaway temp dir. A process-wide atomic
    /// counter keeps each test's directory unique even under parallel runs.
    fn test_db() -> (Connection, std::path::PathBuf) {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "orcastudio-jobs-test-{}-{}",
            std::process::id(),
            n
        ));
        std::fs::remove_dir_all(&dir).ok();
        let conn = init_db(&dir).expect("init_db should succeed");
        (conn, dir)
    }

    #[test]
    fn create_lists_job_as_draft() {
        let (conn, dir) = test_db();

        let job = create_job_conn(&conn, "water opt", "! r2SCAN-3c Opt").unwrap();
        assert_eq!(job.status, JobStatus::Draft);
        assert!(job.started_at.is_none());
        assert!(job.completed_at.is_none());

        let all = list_jobs_conn(&conn).unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].id, job.id);
        assert_eq!(all[0].status, JobStatus::Draft);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn running_sets_started_at() {
        let (conn, dir) = test_db();

        let job = create_job_conn(&conn, "j", "! HF").unwrap();
        update_job_status_conn(&conn, &job.id, "running").unwrap();

        let reloaded = get_job_conn(&conn, &job.id).unwrap();
        assert_eq!(reloaded.status, JobStatus::Running);
        assert!(reloaded.started_at.is_some());
        assert!(reloaded.completed_at.is_none());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn completed_sets_completed_at() {
        let (conn, dir) = test_db();

        let job = create_job_conn(&conn, "j", "! HF").unwrap();
        update_job_status_conn(&conn, &job.id, "completed").unwrap();

        let reloaded = get_job_conn(&conn, &job.id).unwrap();
        assert_eq!(reloaded.status, JobStatus::Completed);
        assert!(reloaded.completed_at.is_some());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn get_missing_job_is_not_found() {
        let (conn, dir) = test_db();

        let err = get_job_conn(&conn, "no-such-id").unwrap_err();
        assert!(matches!(err, AppError::NotFound(_)));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn update_missing_job_is_not_found() {
        let (conn, dir) = test_db();

        let err = update_job_status_conn(&conn, "no-such-id", "running").unwrap_err();
        assert!(matches!(err, AppError::NotFound(_)));

        std::fs::remove_dir_all(&dir).ok();
    }
}
