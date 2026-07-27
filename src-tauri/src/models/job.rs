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
}

impl Job {
    /// Column list used by every `SELECT` that hydrates a [`Job`]. The order
    /// here is the contract [`Job::from_row`] relies on.
    pub const COLUMNS: &'static str = "id, title, input_content, status, job_dir, \
         energy, wall_time, error_message, created_at, started_at, completed_at";

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
        })
    }
}
