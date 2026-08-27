//! The `Job` model and its status state machine.
//!
//! A job is one ORCA calculation: an `.inp` payload plus everything we learn
//! about running it. The local lifecycle is
//! `draft -> queued -> running -> completed | failed | cancelled` (the queue and
//! cancellation arrived in Phase 2). Remote-execution states (uploading, syncing)
//! and the post-run `parsed` state arrive with later phases — see
//! `wiki/modules/tauri-core.md`.

use rusqlite::Row;
use serde::Serialize;

use crate::error::AppError;

/// The lifecycle state of a job.
///
/// Serialized to lowercase strings (`draft`, `queued`, `running`, `completed`,
/// `failed`, `cancelled`) both on the wire (to the frontend) and in the
/// `jobs.status` column.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum JobStatus {
    Draft,
    /// Submitted, waiting for the single execution slot (sequential queue).
    Queued,
    Running,
    Completed,
    /// Completed AND its `.property.txt` results were parsed, verified, and stored
    /// (Phase 3). A post-`completed` state: the calculation already succeeded; this
    /// only says our own parse of it did too. A `completed` job with a parse error
    /// is a calculation that ran fine but whose results we could not read.
    Parsed,
    Failed,
    /// Stopped by the user (queued job dropped, or running process killed).
    Cancelled,
}

impl JobStatus {
    /// The canonical lowercase spelling stored in SQLite.
    pub fn as_str(&self) -> &'static str {
        match self {
            JobStatus::Draft => "draft",
            JobStatus::Queued => "queued",
            JobStatus::Running => "running",
            JobStatus::Completed => "completed",
            JobStatus::Parsed => "parsed",
            JobStatus::Failed => "failed",
            JobStatus::Cancelled => "cancelled",
        }
    }

    /// Parse a status string coming from the database or an IPC caller.
    /// Rejects anything outside the known state set.
    pub fn from_db(s: &str) -> Result<JobStatus, AppError> {
        match s {
            "draft" => Ok(JobStatus::Draft),
            "queued" => Ok(JobStatus::Queued),
            "running" => Ok(JobStatus::Running),
            "completed" => Ok(JobStatus::Completed),
            "parsed" => Ok(JobStatus::Parsed),
            "failed" => Ok(JobStatus::Failed),
            "cancelled" => Ok(JobStatus::Cancelled),
            other => Err(AppError::Internal(format!("unknown job status: {other}"))),
        }
    }
}

/// A single ORCA calculation and everything persisted about it.
///
/// Mirrors the `jobs` table one-to-one. `Option` fields are `NULL` until the
/// relevant lifecycle step fills them (e.g. `energy`/`wall_time` after parsing).
#[derive(Debug, Clone, Serialize)]
pub struct Job {
    pub id: String,
    pub title: String,
    pub input_content: String,
    pub status: JobStatus,
    pub job_dir: Option<String>,
    pub energy: Option<f64>,
    pub wall_time: Option<f64>,
    pub error_message: Option<String>,
    pub created_at: String,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
    /// Versioned SceneFragment snapshot (ADR-008 #5), written once at create
    /// time. `None` for jobs created before schema v4, or with no scene. It
    /// *annotates* `input_content`; the input text stays authoritative for
    /// geometry (restore reconciles them — see `restoreScene` in the frontend).
    pub scene_json: Option<String>,
    /// Serialized operation log (ADR-017 unit 2b), co-written with `scene_json`.
    /// `None` for jobs created before schema v11, or with no scene. "New
    /// iteration" restores it, cross-checked against the snapshot (`restoreSceneLog`).
    pub scene_log_json: Option<String>,
    /// Grouping FK into `pathways` (schema v13, Phase 4.5 C1). `None` is the normal
    /// state for a standalone job — every job today. Set by `attach_job_to_pathway`
    /// and nulled by `detach_job_from_pathway` / a reaction-or-pathway deletion; the
    /// job itself is never deleted by any of those (the jobs-survive invariant). The
    /// reaction is derived by joining `pathways` — no `reaction_id` on jobs. Exposed
    /// here so the reaction UI (C2a) can map a pathway to its attached job.
    pub pathway_id: Option<String>,
    /// Grouping FK into `groups` (schema v16, Phase 4.7.2, ADR-019). `None` = ungrouped
    /// (root / "All jobs"). Set by `move_job`; nulled when its group is deleted (the
    /// command re-parents to the deleted group's parent — [`Group`](crate::models::group::Group);
    /// `ON DELETE SET NULL` is only the fallback). Orthogonal to `pathway_id` and the
    /// re-opt/reference links: a job can sit in a group AND be a re-opt child AND be a
    /// reaction reference at once (ADR-019 Decision 5).
    pub group_id: Option<String>,
    /// Execution-backend FK into `server_profiles` (schema v18, Phase 5 unit 5.1, ADR-023).
    /// **`None` = the local backend** — the normal state for every job today. There is NO
    /// `'local'` profile row (the v13 `pathway_id` precedent): a `NULL` here *is* "run
    /// locally". A non-null value names the remote `ServerProfile` a job runs on; the
    /// per-job "Run on:" selector (a later Phase 5 unit) is a `match` on this. Nulled if
    /// its profile is deleted (`delete_server_profile` nulls children explicitly AND
    /// `ON DELETE SET NULL` is enforced) — the job survives, dropping back to local.
    /// Orthogonal to `pathway_id` / `group_id` / the re-opt links.
    pub backend_id: Option<String>,
}

impl Job {
    /// Column list used by every `SELECT` that hydrates a [`Job`]. The order
    /// here is the contract [`Job::from_row`] relies on.
    pub const COLUMNS: &'static str = "id, title, input_content, status, job_dir, \
         energy, wall_time, error_message, created_at, started_at, completed_at, \
         scene_json, scene_log_json, pathway_id, group_id, backend_id";

    /// Build a [`Job`] from a row selected in [`Job::COLUMNS`] order.
    pub fn from_row(row: &Row) -> rusqlite::Result<Job> {
        let status_str: String = row.get(3)?;
        let status = JobStatus::from_db(&status_str).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(
                3,
                rusqlite::types::Type::Text,
                Box::new(std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string())),
            )
        })?;

        Ok(Job {
            id: row.get(0)?,
            title: row.get(1)?,
            input_content: row.get(2)?,
            status,
            job_dir: row.get(4)?,
            energy: row.get(5)?,
            wall_time: row.get(6)?,
            error_message: row.get(7)?,
            created_at: row.get(8)?,
            started_at: row.get(9)?,
            completed_at: row.get(10)?,
            scene_json: row.get(11)?,
            scene_log_json: row.get(12)?,
            pathway_id: row.get(13)?,
            group_id: row.get(14)?,
            backend_id: row.get(15)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    /// A minimal `jobs` table carrying the two columns that changed in v18-adjacent
    /// work, so `Job::COLUMNS` + `Job::from_row` round-trip `backend_id` in BOTH states:
    /// NULL (local) and a set value (a remote profile). Guards the COLUMNS-order contract
    /// against a drift where `backend_id`'s SELECT position and `row.get(15)` disagree.
    fn setup(conn: &Connection) {
        conn.execute_batch(
            "CREATE TABLE jobs (
                id TEXT PRIMARY KEY, title TEXT NOT NULL, input_content TEXT NOT NULL,
                status TEXT NOT NULL DEFAULT 'draft', job_dir TEXT, energy REAL,
                wall_time REAL, error_message TEXT,
                created_at TEXT NOT NULL DEFAULT (datetime('now')), started_at TEXT,
                completed_at TEXT, scene_json TEXT, scene_log_json TEXT,
                pathway_id TEXT, group_id TEXT, backend_id TEXT
            );",
        )
        .unwrap();
    }

    fn get(conn: &Connection, id: &str) -> Job {
        let sql = format!("SELECT {} FROM jobs WHERE id = ?1", Job::COLUMNS);
        conn.query_row(&sql, rusqlite::params![id], Job::from_row).unwrap()
    }

    #[test]
    fn from_row_round_trips_backend_id_null_and_set() {
        let conn = Connection::open_in_memory().unwrap();
        setup(&conn);
        conn.execute_batch(
            "INSERT INTO jobs (id, title, input_content, status) \
                VALUES ('local', 'runs local', '! SP', 'draft');
             INSERT INTO jobs (id, title, input_content, status, backend_id) \
                VALUES ('remote', 'runs remote', '! SP', 'draft', 'profile-1');",
        )
        .unwrap();

        assert_eq!(
            get(&conn, "local").backend_id,
            None,
            "NULL backend_id hydrates to None (= local backend, ADR-023)"
        );
        assert_eq!(
            get(&conn, "remote").backend_id.as_deref(),
            Some("profile-1"),
            "a set backend_id hydrates to the named profile id"
        );
        // Sanity that COLUMNS order is still coherent: an adjacent field survives too.
        assert_eq!(get(&conn, "local").title, "runs local");
    }
}
