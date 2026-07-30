//! Job commands: the CRUD + state-machine surface over the `jobs` table.
//!
//! The Tauri commands are thin wrappers that lock the shared connection and
//! delegate to the `*_conn` helpers, which take a `&Connection` directly so the
//! state-machine logic is unit-testable without a running Tauri app.

use std::collections::VecDeque;
use std::io::{BufRead, BufReader};

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use tauri::State;
use uuid::Uuid;

use crate::commands::settings::DbState;
use crate::error::AppError;
use crate::models::job::{Job, JobStatus};

// --- Connection-level helpers (testable) ------------------------------------

/// Insert a fresh `draft` job and return it fully hydrated. `scene_json` is the
/// optional SceneFragment snapshot (ADR-008 #5), written once here at create
/// time and never updated — the job's input is immutable, so its snapshot is too.
fn create_job_conn(
    conn: &Connection,
    title: &str,
    input_content: &str,
    scene_json: Option<&str>,
) -> Result<Job, AppError> {
    let id = Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO jobs (id, title, input_content, status, scene_json) \
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![id, title, input_content, JobStatus::Draft.as_str(), scene_json],
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
pub(crate) fn get_job_conn(conn: &Connection, id: &str) -> Result<Job, AppError> {
    let sql = format!("SELECT {} FROM jobs WHERE id = ?1", Job::COLUMNS);
    conn.query_row(&sql, params![id], Job::from_row)
        .optional()?
        .ok_or_else(|| AppError::NotFound(format!("job {id}")))
}

/// Record the isolated job directory for a job (set once at submit time).
pub(crate) fn set_job_dir_conn(conn: &Connection, id: &str, job_dir: &str) -> Result<(), AppError> {
    conn.execute(
        "UPDATE jobs SET job_dir = ?1 WHERE id = ?2",
        params![job_dir, id],
    )?;
    Ok(())
}

/// Store extracted results (final SCF energy in Hartree, wall time in seconds).
/// Either may be `None` if extraction didn't find it.
pub(crate) fn set_job_results_conn(
    conn: &Connection,
    id: &str,
    energy: Option<f64>,
    wall_time: Option<f64>,
) -> Result<(), AppError> {
    conn.execute(
        "UPDATE jobs SET energy = ?1, wall_time = ?2 WHERE id = ?3",
        params![energy, wall_time, id],
    )?;
    Ok(())
}

/// Overwrite a job's energy from the **authoritative** parsed result (ADR-012),
/// leaving `wall_time` untouched. Called after a successful parse: the output.out
/// tail regex in [`set_job_results_conn`] is only a **live estimate** during a
/// run and misses the final energy on a large molecule (the last
/// `FINAL SINGLE POINT ENERGY` sits past the tail window — measured 164 KB on a
/// 33-atom Freq, unit 3.9 defect 2). `results.final_energy_eh`, read from
/// `.property.txt`, is the value the header/list must show.
pub(crate) fn set_job_energy_conn(conn: &Connection, id: &str, energy: f64) -> Result<(), AppError> {
    conn.execute(
        "UPDATE jobs SET energy = ?1 WHERE id = ?2",
        params![energy, id],
    )?;
    Ok(())
}

/// Terminal transition (`completed`/`failed`): set status, stamp `completed_at`,
/// and store an optional `error_message`. Used by the LocalBackend when a run
/// finishes.
pub(crate) fn finalize_job_conn(
    conn: &Connection,
    id: &str,
    status: JobStatus,
    error_message: Option<&str>,
) -> Result<(), AppError> {
    conn.execute(
        "UPDATE jobs SET status = ?1, completed_at = datetime('now'), error_message = ?2 \
         WHERE id = ?3",
        params![status.as_str(), error_message, id],
    )?;
    Ok(())
}

/// Record a **results parse** failure on a job that otherwise completed fine. The
/// status stays `completed` (the calculation ran; only our parse of it did not) —
/// this only surfaces the reason in the UI. Distinct from a `failed` calculation.
pub(crate) fn set_job_parse_error_conn(
    conn: &Connection,
    id: &str,
    message: &str,
) -> Result<(), AppError> {
    conn.execute(
        "UPDATE jobs SET error_message = ?1 WHERE id = ?2",
        params![message, id],
    )?;
    Ok(())
}

/// Transition a job to `status`, stamping the matching timestamp:
/// `started_at` on entering `running`, `completed_at` on `completed`/`failed`.
pub(crate) fn update_job_status_conn(conn: &Connection, id: &str, status: &str) -> Result<(), AppError> {
    let status = JobStatus::from_db(status)?;
    let affected = match status {
        JobStatus::Running => conn.execute(
            "UPDATE jobs SET status = ?1, started_at = datetime('now') WHERE id = ?2",
            params![status.as_str(), id],
        )?,
        JobStatus::Completed | JobStatus::Failed | JobStatus::Cancelled => conn.execute(
            "UPDATE jobs SET status = ?1, completed_at = datetime('now') WHERE id = ?2",
            params![status.as_str(), id],
        )?,
        // `parsed` is post-`completed`; completed_at is already stamped, so only
        // the status advances.
        JobStatus::Parsed => conn.execute(
            "UPDATE jobs SET status = ?1 WHERE id = ?2",
            params![status.as_str(), id],
        )?,
        JobStatus::Draft | JobStatus::Queued => conn.execute(
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
    scene_json: Option<String>,
) -> Result<Job, AppError> {
    let conn = db.lock()?;
    create_job_conn(&conn, &title, &input_content, scene_json.as_deref())
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

/// The parsed `.property.txt` results for a job (the full structure, incl. per-atom
/// arrays with their element order), or `None` if the job has none yet.
#[tauri::command]
pub fn read_job_results(
    db: State<'_, DbState>,
    id: String,
) -> Result<Option<crate::results::ParsedResults>, AppError> {
    let conn = db.lock()?;
    crate::results::read_job_results(&conn, &id)
}

#[tauri::command]
pub fn update_job_status(db: State<'_, DbState>, id: String, status: String) -> Result<(), AppError> {
    let conn = db.lock()?;
    update_job_status_conn(&conn, &id, &status)
}

/// Submit a draft job to the LocalBackend: prepare its dir, spawn ORCA, and
/// stream the log. Returns immediately — the run proceeds on a background
/// thread. See [`crate::local_backend`].
#[tauri::command]
pub fn submit_job(app: tauri::AppHandle, id: String) -> Result<(), AppError> {
    crate::local_backend::submit(&app, &id)
}

/// Cancel a running or queued job (see [`crate::local_backend::cancel`]).
#[tauri::command]
pub fn cancel_job(app: tauri::AppHandle, id: String) -> Result<(), AppError> {
    crate::local_backend::cancel(&app, &id)
}

/// Pause the sequential queue: the running job finishes, but no queued job
/// starts until [`resume_queue`].
#[tauri::command]
pub fn pause_queue(app: tauri::AppHandle) -> Result<(), AppError> {
    crate::local_backend::set_paused(&app, true);
    Ok(())
}

/// Resume the queue and immediately pull the next queued job if the slot is free.
#[tauri::command]
pub fn resume_queue(app: tauri::AppHandle) -> Result<(), AppError> {
    crate::local_backend::set_paused(&app, false);
    Ok(())
}

/// Whether the queue is currently paused.
#[tauri::command]
pub fn is_queue_paused(app: tauri::AppHandle) -> Result<bool, AppError> {
    Ok(crate::local_backend::is_paused(&app))
}

/// Max lines returned by [`read_job_output`] (also the default when `tail_lines`
/// is omitted). Bounds both the read and the payload for Phase 1.
const OUTPUT_LINE_CAP: usize = 10_000;

/// Read a job's `output.out` for the log console. Returns the last `tail_lines`
/// lines (default/cap [`OUTPUT_LINE_CAP`]). Returns an empty vec — not an error —
/// when the job has no directory yet or hasn't produced output.
#[tauri::command]
pub fn read_job_output(
    db: State<'_, DbState>,
    id: String,
    tail_lines: Option<usize>,
) -> Result<Vec<String>, AppError> {
    let job_dir = {
        let conn = db.lock()?;
        get_job_conn(&conn, &id)?.job_dir
    };
    let Some(job_dir) = job_dir else {
        return Ok(Vec::new());
    };
    let out_path = std::path::Path::new(&job_dir).join("output.out");
    if !out_path.exists() {
        return Ok(Vec::new());
    }
    let max_lines = tail_lines.map_or(OUTPUT_LINE_CAP, |n| n.min(OUTPUT_LINE_CAP));
    Ok(crate::local_backend::read_tail_lines(&out_path, max_lines)?)
}

/// Max size of a GOAT ensemble file we'll read whole. The ensemble is a
/// multi-frame xyz of ONE (small) fragment — kilobytes in practice — so reading
/// it fully is fine (unlike `output.out`, domain rule #5). Cap defensively so a
/// pathological file can't blow up the IPC payload.
const MAX_ENSEMBLE_BYTES: u64 = 8 * 1024 * 1024;

/// Read a GOAT job's conformer ensemble (`input.finalensemble.xyz`, the file
/// name GOAT derives from `input.inp` — see `wiki/orca/goat.md`). Returns an
/// empty string (not an error) when the job has no dir, hasn't produced the file,
/// or it isn't a GOAT run. Read lazily by `JobDetailScreen` on a completed job.
#[tauri::command]
pub fn read_job_ensemble(db: State<'_, DbState>, id: String) -> Result<String, AppError> {
    let job_dir = {
        let conn = db.lock()?;
        get_job_conn(&conn, &id)?.job_dir
    };
    let Some(job_dir) = job_dir else {
        return Ok(String::new());
    };
    let path = std::path::Path::new(&job_dir).join("input.finalensemble.xyz");
    if !path.exists() {
        return Ok(String::new());
    }
    if std::fs::metadata(&path)?.len() > MAX_ENSEMBLE_BYTES {
        return Err(AppError::Internal(format!(
            "ensemble file for job {id} is unexpectedly large (> {MAX_ENSEMBLE_BYTES} bytes)"
        )));
    }
    Ok(std::fs::read_to_string(&path)?)
}

/// Max lines handed to the Monaco output viewer. An ORCA output can reach
/// hundreds of MB; neither the IPC payload nor the editor model should carry
/// that. ~300k lines ≈ 30 MB — a comfortable ceiling for the viewer.
const MAX_VIEWER_LINES: usize = 300_000;

/// Full output for the Monaco-based viewer. Capped by line count: when capped we
/// keep the **tail** (that's where the interesting end of a run is) and report
/// `first_line_no` so the viewer can display absolute file line numbers and map
/// search hits correctly.
#[derive(Serialize)]
pub struct OutputContent {
    pub content: String,
    /// 1-indexed file line number of the first line in `content`.
    /// `> 1` exactly when `truncated`.
    pub first_line_no: usize,
    pub total_lines: usize,
    pub truncated: bool,
}

/// Read a job's `output.out` for the Monaco viewer, capped to the last
/// [`MAX_VIEWER_LINES`] lines. Streams the file line by line (never loads a
/// hundreds-of-MB file whole — domain rule #5), keeping only the tail window in
/// memory. Empty content (not an error) when there's no dir or output yet.
#[tauri::command]
pub fn read_job_output_for_viewer(
    db: State<'_, DbState>,
    id: String,
) -> Result<OutputContent, AppError> {
    let empty = OutputContent {
        content: String::new(),
        first_line_no: 1,
        total_lines: 0,
        truncated: false,
    };

    let job_dir = {
        let conn = db.lock()?;
        get_job_conn(&conn, &id)?.job_dir
    };
    let Some(job_dir) = job_dir else {
        return Ok(empty);
    };
    let out_path = std::path::Path::new(&job_dir).join("output.out");
    if !out_path.exists() {
        return Ok(empty);
    }

    let reader = BufReader::new(std::fs::File::open(&out_path)?);
    let mut kept: VecDeque<String> = VecDeque::new();
    let mut total_lines = 0usize;
    for line in reader.lines() {
        let line = line?;
        total_lines += 1;
        if kept.len() == MAX_VIEWER_LINES {
            kept.pop_front();
        }
        kept.push_back(line);
    }

    let first_line_no = total_lines.saturating_sub(kept.len()) + 1;
    let truncated = total_lines > kept.len();
    let content = kept.into_iter().collect::<Vec<_>>().join("\n");
    Ok(OutputContent {
        content,
        first_line_no,
        total_lines,
        truncated,
    })
}

/// Backfill the convergence dashboard: replay a job's `output.out` through the
/// incremental parser and return every SCF / optimization datapoint. Returns an
/// empty vec — not an error — when the job has no directory or output yet.
/// Streams the file line by line (never loads it whole — domain rule #5).
#[tauri::command]
pub fn read_job_convergence(
    db: State<'_, DbState>,
    id: String,
) -> Result<Vec<crate::convergence::ConvergenceEvent>, AppError> {
    let job_dir = {
        let conn = db.lock()?;
        get_job_conn(&conn, &id)?.job_dir
    };
    let Some(job_dir) = job_dir else {
        return Ok(Vec::new());
    };
    let out_path = std::path::Path::new(&job_dir).join("output.out");
    if !out_path.exists() {
        return Ok(Vec::new());
    }
    Ok(crate::local_backend::read_convergence(&out_path)?)
}

/// Open a job's directory in the OS file manager.
#[tauri::command]
pub fn open_job_folder(db: State<'_, DbState>, id: String) -> Result<(), AppError> {
    let job_dir = {
        let conn = db.lock()?;
        get_job_conn(&conn, &id)?.job_dir
    };
    let job_dir = job_dir.ok_or_else(|| AppError::Backend("job has no directory yet".into()))?;
    open_in_file_manager(&job_dir)
}

/// Spawn the platform file manager on `path` (Linux-first; also handles macOS /
/// Windows). Detached — we don't wait on the viewer.
fn open_in_file_manager(path: &str) -> Result<(), AppError> {
    #[cfg(target_os = "linux")]
    let program = "xdg-open";
    #[cfg(target_os = "macos")]
    let program = "open";
    #[cfg(target_os = "windows")]
    let program = "explorer";

    std::process::Command::new(program)
        .arg(path)
        .spawn()
        .map_err(|e| AppError::Backend(format!("failed to open '{path}': {e}")))?;
    Ok(())
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

        let job = create_job_conn(&conn, "water opt", "! r2SCAN-3c Opt", None).unwrap();
        assert_eq!(job.status, JobStatus::Draft);
        assert!(job.started_at.is_none());
        assert!(job.completed_at.is_none());

        let all = list_jobs_conn(&conn).unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].id, job.id);
        assert_eq!(all[0].status, JobStatus::Draft);
        // No snapshot passed → NULL, not an empty string.
        assert_eq!(job.scene_json, None);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn create_persists_and_reloads_scene_json() {
        let (conn, dir) = test_db();

        let snap = r#"{"version":1,"fragments":[],"multiplicity":1}"#;
        let job = create_job_conn(&conn, "with scene", "! HF", Some(snap)).unwrap();
        assert_eq!(job.scene_json.as_deref(), Some(snap));

        // Survives a fresh hydration (get + list both read the new column).
        let reloaded = get_job_conn(&conn, &job.id).unwrap();
        assert_eq!(reloaded.scene_json.as_deref(), Some(snap));
        let listed = list_jobs_conn(&conn).unwrap();
        assert_eq!(listed[0].scene_json.as_deref(), Some(snap));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn running_sets_started_at() {
        let (conn, dir) = test_db();

        let job = create_job_conn(&conn, "j", "! HF", None).unwrap();
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

        let job = create_job_conn(&conn, "j", "! HF", None).unwrap();
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

    #[test]
    fn set_job_dir_persists() {
        let (conn, dir) = test_db();

        let job = create_job_conn(&conn, "j", "! HF", None).unwrap();
        set_job_dir_conn(&conn, &job.id, "/data/jobs/abc").unwrap();

        let reloaded = get_job_conn(&conn, &job.id).unwrap();
        assert_eq!(reloaded.job_dir.as_deref(), Some("/data/jobs/abc"));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn finalize_sets_status_error_and_timestamp() {
        let (conn, dir) = test_db();

        let job = create_job_conn(&conn, "j", "! HF", None).unwrap();
        finalize_job_conn(&conn, &job.id, JobStatus::Failed, Some("boom")).unwrap();

        let reloaded = get_job_conn(&conn, &job.id).unwrap();
        assert_eq!(reloaded.status, JobStatus::Failed);
        assert_eq!(reloaded.error_message.as_deref(), Some("boom"));
        assert!(reloaded.completed_at.is_some());

        std::fs::remove_dir_all(&dir).ok();
    }
}
