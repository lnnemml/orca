//! LocalBackend: run ORCA on this machine.
//!
//! Honours the hard-won ORCA domain rules (see CLAUDE.md / `wiki/orca`):
//!   1. ORCA is invoked by its **full absolute path** (from `settings.orca_path`)
//!      so its MPI self-re-invocation for `%pal` works.
//!   3. Every calculation gets **one isolated directory** (`<data>/jobs/<id>/`).
//!   5. Output is **never slurped into memory** — stdout is streamed line-by-line
//!      to `output.out` and to the UI; completion detection reads only a ~5 KB tail.
//!   6. Completion = `.exit_code` written AND `ORCA TERMINATED NORMALLY` in output.
//!   4. Default concurrency = 1 — a single-slot gate rejects a second run.

use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use rusqlite::OptionalExtension;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

use crate::commands::jobs::{
    finalize_job_conn, get_job_conn, set_job_dir_conn, set_job_results_conn,
    update_job_status_conn,
};
use crate::commands::settings::DbState;
use crate::error::AppError;
use crate::models::job::JobStatus;

/// Flush the log batch to the UI when it reaches this many lines...
const LOG_BATCH_LINES: usize = 50;
/// ...or when this many milliseconds have elapsed since the last flush.
const LOG_BATCH_MILLIS: u64 = 100;
/// How many bytes from the end of a file to inspect for markers / errors.
const TAIL_BYTES: u64 = 5 * 1024;
/// Larger tail for result extraction: a Freq/Opt run prints the final energy
/// well before the end (normal modes + thermochemistry follow), so 5 KB isn't
/// enough — 64 KB comfortably reaches back to the last `FINAL SINGLE POINT ENERGY`.
const RESULT_TAIL_BYTES: u64 = 64 * 1024;

/// App-wide LocalBackend state: the job-directory root plus the single
/// execution slot (`Some(job_id)` while a job is running — concurrency = 1).
pub struct JobRunner {
    data_dir: PathBuf,
    running: Mutex<Option<String>>,
}

impl JobRunner {
    pub fn new(data_dir: PathBuf) -> Self {
        Self {
            data_dir,
            running: Mutex::new(None),
        }
    }

    fn running_lock(&self) -> Result<std::sync::MutexGuard<'_, Option<String>>, AppError> {
        self.running
            .lock()
            .map_err(|_| AppError::Internal("job runner mutex poisoned".into()))
    }
}

/// Payload of the `job:log` event: a batch of freshly produced output lines.
#[derive(Clone, Serialize)]
struct LogPayload {
    job_id: String,
    lines: Vec<String>,
}

/// Payload of the `job:status` event: a job's new lifecycle state.
#[derive(Clone, Serialize)]
struct StatusPayload {
    job_id: String,
    status: String,
}

/// Create `<data_dir>/jobs/<job_id>/` and write `input.inp` into it.
/// Returns the absolute job directory.
pub fn prepare_job_dir(
    data_dir: &Path,
    job_id: &str,
    input_content: &str,
) -> Result<PathBuf, AppError> {
    let dir = data_dir.join("jobs").join(job_id);
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join("input.inp"), input_content)?;
    Ok(dir)
}

/// Spawn ORCA (full absolute `orca_path`) on `input.inp` inside `job_dir`.
/// stdout is piped (for tailing); stderr goes to `stderr.log`.
pub fn run_orca(orca_path: &str, job_dir: &Path) -> Result<Child, AppError> {
    let stderr = File::create(job_dir.join("stderr.log"))?;
    Command::new(orca_path)
        .arg("input.inp")
        .current_dir(job_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::from(stderr))
        .spawn()
        .map_err(|e| AppError::Backend(format!("failed to spawn ORCA at '{orca_path}': {e}")))
}

/// Submit a draft job: validate, reserve the slot, prepare the dir, spawn ORCA,
/// mark it running, and hand off to a background tailing thread.
pub fn submit(app: &AppHandle, job_id: &str) -> Result<(), AppError> {
    let db = app.state::<DbState>();
    let runner = app.state::<JobRunner>();

    // 1. Validate: the job exists and is a draft.
    let input_content = {
        let conn = db.lock()?;
        let job = get_job_conn(&conn, job_id)?;
        if job.status != JobStatus::Draft {
            return Err(AppError::Backend(format!(
                "job {job_id} is '{}'; only draft jobs can be run",
                job.status.as_str()
            )));
        }
        job.input_content
    };

    // 2. Resolve the ORCA path (full absolute path — domain rule #1).
    let orca_path = {
        let conn = db.lock()?;
        conn.query_row(
            "SELECT value FROM settings WHERE key = 'orca_path'",
            [],
            |r| r.get::<_, String>(0),
        )
        .optional()?
        .filter(|p| !p.trim().is_empty())
        .ok_or_else(|| AppError::Backend("ORCA path is not configured (see Settings)".into()))?
    };

    // 3. Reserve the single execution slot (domain rule #4).
    {
        let mut running = runner.running_lock()?;
        if let Some(other) = running.as_ref() {
            return Err(AppError::Backend(format!(
                "another job is already running ({other})"
            )));
        }
        *running = Some(job_id.to_string());
    }

    // Any failure past this point must free the slot.
    match start_run(app, &db, &runner, job_id, &input_content, &orca_path) {
        Ok(()) => Ok(()),
        Err(e) => {
            release_slot(&runner, job_id);
            Err(e)
        }
    }
}

/// The fallible middle of [`submit`], factored out so the slot is released on
/// any error (see the caller).
fn start_run(
    app: &AppHandle,
    db: &DbState,
    runner: &JobRunner,
    job_id: &str,
    input_content: &str,
    orca_path: &str,
) -> Result<(), AppError> {
    // 4. Isolated job dir + input.inp; persist the path.
    let job_dir = prepare_job_dir(&runner.data_dir, job_id, input_content)?;
    {
        let conn = db.lock()?;
        set_job_dir_conn(&conn, job_id, &job_dir.to_string_lossy())?;
    }

    // 5. Spawn ORCA.
    let mut child = run_orca(orca_path, &job_dir)?;

    // 6. Mark running + notify the UI. If the DB update fails, kill the child.
    {
        let conn = db.lock()?;
        if let Err(e) = update_job_status_conn(&conn, job_id, "running") {
            let _ = child.kill();
            return Err(e);
        }
    }
    emit_status(app, job_id, JobStatus::Running);

    // 7. Drive stdout tailing + completion detection off-thread.
    let app = app.clone();
    let job_id = job_id.to_string();
    std::thread::spawn(move || drive_job(app, job_id, job_dir, child));
    Ok(())
}

/// Stream stdout to `output.out` + the UI, then wait, write `.exit_code`, detect
/// completion, persist the final state, and free the execution slot.
fn drive_job(app: AppHandle, job_id: String, job_dir: PathBuf, mut child: Child) {
    let out_path = job_dir.join("output.out");

    if let Some(stdout) = child.stdout.take() {
        let mut reader = BufReader::new(stdout);
        let mut writer = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&out_path)
            .ok()
            .map(BufWriter::new);
        let mut batch: Vec<String> = Vec::new();
        let mut last_flush = Instant::now();
        let mut line = String::new();

        loop {
            line.clear();
            match reader.read_line(&mut line) {
                Ok(0) => break, // EOF: process closed stdout
                Ok(_) => {
                    if let Some(w) = writer.as_mut() {
                        let _ = w.write_all(line.as_bytes());
                    }
                    batch.push(line.trim_end_matches(['\n', '\r']).to_string());
                    if batch.len() >= LOG_BATCH_LINES
                        || last_flush.elapsed() >= Duration::from_millis(LOG_BATCH_MILLIS)
                    {
                        if let Some(w) = writer.as_mut() {
                            let _ = w.flush();
                        }
                        emit_log(&app, &job_id, &mut batch);
                        last_flush = Instant::now();
                    }
                }
                Err(_) => break,
            }
        }
        if let Some(w) = writer.as_mut() {
            let _ = w.flush();
        }
        emit_log(&app, &job_id, &mut batch);
    }

    // Wait for exit and write the completion marker (domain rule #6).
    let exit_code = child.wait().ok().and_then(|s| s.code());
    let exit_str = exit_code
        .map(|c| c.to_string())
        .unwrap_or_else(|| "unknown".to_string());
    let _ = std::fs::write(job_dir.join(".exit_code"), format!("{exit_str}\n"));

    // Completion = marker written above AND normal-termination banner in output.
    let (status, error_message) =
        detect_completion(&out_path, &job_dir.join("stderr.log"), exit_code);

    if let Some(db) = app.try_state::<DbState>() {
        if let Ok(conn) = db.lock() {
            let _ = finalize_job_conn(&conn, &job_id, status, error_message.as_deref());
        }
    }

    // On success, pull the final energy + wall time from the output tail and
    // persist them BEFORE the terminal event (so the UI's reload sees them).
    if status == JobStatus::Completed {
        let tail = read_tail(&out_path, RESULT_TAIL_BYTES).unwrap_or_default();
        let energy = crate::result_extraction::extract_final_energy(&tail);
        let wall_time = crate::result_extraction::extract_wall_time(&tail);
        if energy.is_some() || wall_time.is_some() {
            if let Some(db) = app.try_state::<DbState>() {
                if let Ok(conn) = db.lock() {
                    let _ = set_job_results_conn(&conn, &job_id, energy, wall_time);
                }
            }
        }
    }

    if let Some(runner) = app.try_state::<JobRunner>() {
        release_slot(&runner, &job_id);
    }
    emit_status(&app, &job_id, status);
}

/// Free the execution slot iff it is still held by `job_id`.
fn release_slot(runner: &JobRunner, job_id: &str) {
    if let Ok(mut running) = runner.running_lock() {
        if running.as_deref() == Some(job_id) {
            *running = None;
        }
    }
}

/// Emit a batch of log lines (no-op if empty); drains `batch`.
fn emit_log(app: &AppHandle, job_id: &str, batch: &mut Vec<String>) {
    if batch.is_empty() {
        return;
    }
    let _ = app.emit(
        "job:log",
        LogPayload {
            job_id: job_id.to_string(),
            lines: std::mem::take(batch),
        },
    );
}

fn emit_status(app: &AppHandle, job_id: &str, status: JobStatus) {
    let _ = app.emit(
        "job:status",
        StatusPayload {
            job_id: job_id.to_string(),
            status: status.as_str().to_string(),
        },
    );
}

/// Decide a job's terminal state from its output tail + exit code (domain rule
/// #6): `completed` iff the output ends with `ORCA TERMINATED NORMALLY` AND the
/// exit code is 0; otherwise `failed` with a message pulled from stderr (or the
/// output tail if stderr is empty). Reads only file tails — never the whole file.
fn detect_completion(
    out_path: &Path,
    stderr_path: &Path,
    exit_code: Option<i32>,
) -> (JobStatus, Option<String>) {
    let tail = read_tail(out_path, TAIL_BYTES).unwrap_or_default();
    if tail.contains("ORCA TERMINATED NORMALLY") && exit_code == Some(0) {
        return (JobStatus::Completed, None);
    }
    let stderr_tail = read_tail(stderr_path, TAIL_BYTES).unwrap_or_default();
    let detail = if stderr_tail.trim().is_empty() {
        last_lines(&tail, 20)
    } else {
        stderr_tail
    };
    let exit_str = exit_code
        .map(|c| c.to_string())
        .unwrap_or_else(|| "unknown".to_string());
    let msg = format!("ORCA did not terminate normally (exit code {exit_str}).\n{}", detail.trim());
    (JobStatus::Failed, Some(msg))
}

/// Read the last `max_bytes` of a file (lossy UTF-8). Seeks from the end so a
/// multi-MB output is never fully loaded (domain rule #5).
fn read_tail(path: &Path, max_bytes: u64) -> std::io::Result<String> {
    let mut f = File::open(path)?;
    let len = f.metadata()?.len();
    f.seek(SeekFrom::Start(len.saturating_sub(max_bytes)))?;
    let mut buf = Vec::new();
    f.read_to_end(&mut buf)?;
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

/// The last `n` non-empty-trimmed lines of `text`, joined with newlines.
fn last_lines(text: &str, n: usize) -> String {
    let all: Vec<&str> = text.lines().collect();
    let start = all.len().saturating_sub(n);
    all[start..].join("\n")
}

/// Bytes to read from the end when returning the last N lines of a file. Bounds
/// memory even for tens-of-MB outputs (domain rule #5).
const TAIL_LINES_MAX_BYTES: u64 = 8 * 1024 * 1024;

/// Return the last `max_lines` lines of a file, reading at most
/// [`TAIL_LINES_MAX_BYTES`] from the end. If the file is larger than that, the
/// first (possibly partial) line of the window is dropped. Used by
/// `read_job_output` to backfill the log console without loading whole files.
pub(crate) fn read_tail_lines(path: &Path, max_lines: usize) -> std::io::Result<Vec<String>> {
    let mut f = File::open(path)?;
    let len = f.metadata()?.len();
    let start = len.saturating_sub(TAIL_LINES_MAX_BYTES);
    f.seek(SeekFrom::Start(start))?;
    let mut buf = Vec::new();
    f.read_to_end(&mut buf)?;
    let text = String::from_utf8_lossy(&buf);

    let mut lines: Vec<&str> = text.lines().collect();
    // Truncated head → first line is partial; drop it.
    if start > 0 && !lines.is_empty() {
        lines.remove(0);
    }
    let begin = lines.len().saturating_sub(max_lines);
    Ok(lines[begin..].iter().map(|s| s.to_string()).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "orcastudio-lb-{tag}-{}-{n}",
            std::process::id()
        ));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn prepare_job_dir_writes_input() {
        let data = scratch("prep");
        let dir = prepare_job_dir(&data, "job-123", "! r2SCAN-3c\n").unwrap();
        assert!(dir.ends_with("jobs/job-123"));
        let inp = std::fs::read_to_string(dir.join("input.inp")).unwrap();
        assert_eq!(inp, "! r2SCAN-3c\n");
        std::fs::remove_dir_all(&data).ok();
    }

    #[test]
    fn read_tail_returns_only_the_end() {
        let data = scratch("tail");
        let path = data.join("out.txt");
        // 3000 'a' lines then the marker; a small tail must still catch the marker.
        let mut body = "a\n".repeat(3000);
        body.push_str("ORCA TERMINATED NORMALLY\n");
        std::fs::write(&path, &body).unwrap();

        let tail = read_tail(&path, 5 * 1024).unwrap();
        assert!(tail.len() < body.len());
        assert!(tail.contains("ORCA TERMINATED NORMALLY"));
        std::fs::remove_dir_all(&data).ok();
    }

    #[test]
    fn last_lines_takes_the_tail() {
        let text = "1\n2\n3\n4\n5";
        assert_eq!(last_lines(text, 2), "4\n5");
        assert_eq!(last_lines(text, 99), text);
    }

    #[test]
    fn read_tail_lines_caps_and_reads_all() {
        let data = scratch("taillines");
        let path = data.join("out.txt");
        let body: String = (1..=200).map(|i| format!("line {i}\n")).collect();
        std::fs::write(&path, &body).unwrap();

        // Whole file (fits well under the byte cap): all 200 lines.
        let all = read_tail_lines(&path, 10_000).unwrap();
        assert_eq!(all.len(), 200);
        assert_eq!(all.first().unwrap(), "line 1");
        assert_eq!(all.last().unwrap(), "line 200");

        // Capped to the last 5 lines.
        let tail = read_tail_lines(&path, 5).unwrap();
        assert_eq!(tail, vec!["line 196", "line 197", "line 198", "line 199", "line 200"]);

        std::fs::remove_dir_all(&data).ok();
    }

    #[test]
    fn detect_completion_needs_marker_and_zero_exit() {
        let data = scratch("detect");
        let out = data.join("output.out");
        let stderr = data.join("stderr.log");
        std::fs::write(&stderr, "").unwrap();

        // Marker + exit 0 → completed.
        std::fs::write(&out, "SCF done\nORCA TERMINATED NORMALLY\n").unwrap();
        assert_eq!(detect_completion(&out, &stderr, Some(0)).0, JobStatus::Completed);

        // Marker but non-zero exit → failed.
        assert_eq!(detect_completion(&out, &stderr, Some(1)).0, JobStatus::Failed);

        // No marker → failed, message carries the output tail.
        std::fs::write(&out, "aborting: SCF not converged\n").unwrap();
        let (status, msg) = detect_completion(&out, &stderr, Some(0));
        assert_eq!(status, JobStatus::Failed);
        assert!(msg.unwrap().contains("SCF not converged"));

        std::fs::remove_dir_all(&data).ok();
    }

    /// End-to-end against the real ORCA binary. Ignored by default (slow, needs
    /// ORCA installed); run with `cargo test -- --ignored`.
    #[test]
    #[ignore = "requires ORCA at /opt/orca/orca"]
    fn real_orca_water_single_point_completes() {
        let orca = "/opt/orca/orca";
        if !Path::new(orca).exists() {
            eprintln!("skipping: ORCA not found at {orca}");
            return;
        }
        let data = scratch("orca");
        let input = "! r2SCAN-3c TightSCF\n\n%pal nprocs 1 end\n%maxcore 2000\n\n\
                     * xyz 0 1\n  O   0.0000   0.0000   0.1173\n  \
                     H   0.0000   0.7572  -0.4692\n  H   0.0000  -0.7572  -0.4692\n*\n";
        let dir = prepare_job_dir(&data, "water-sp", input).unwrap();

        let mut child = run_orca(orca, &dir).unwrap();

        // Drain stdout to output.out — the same streaming drive_job does, minus
        // the Tauri emit (which needs an AppHandle).
        let out_path = dir.join("output.out");
        {
            let stdout = child.stdout.take().unwrap();
            let mut reader = BufReader::new(stdout);
            let mut writer = BufWriter::new(File::create(&out_path).unwrap());
            let mut line = String::new();
            while reader.read_line(&mut line).unwrap() != 0 {
                writer.write_all(line.as_bytes()).unwrap();
                line.clear();
            }
            writer.flush().unwrap();
        }
        let exit_code = child.wait().unwrap().code();
        std::fs::write(dir.join(".exit_code"), format!("{}\n", exit_code.unwrap())).unwrap();

        let (status, err) = detect_completion(&out_path, &dir.join("stderr.log"), exit_code);
        assert_eq!(status, JobStatus::Completed, "unexpected failure: {err:?}");
        assert!(dir.join(".exit_code").exists());
        let output = std::fs::read_to_string(&out_path).unwrap();
        assert!(output.contains("ORCA TERMINATED NORMALLY"));

        // Result extraction against genuine ORCA output.
        let tail = read_tail(&out_path, RESULT_TAIL_BYTES).unwrap();
        let energy = crate::result_extraction::extract_final_energy(&tail)
            .expect("should extract a final energy");
        // Verified reference value for this geometry is ≈ -76.419 Eh.
        assert!(
            (energy - (-76.419)).abs() < 0.05,
            "energy {energy} far from expected ~-76.419"
        );
        assert!(
            crate::result_extraction::extract_wall_time(&tail).is_some(),
            "should extract a wall time"
        );

        // read_tail_lines (backing read_job_output) returns the full log.
        let all = read_tail_lines(&out_path, 10_000).unwrap();
        assert!(all.iter().any(|l| l.contains("ORCA TERMINATED NORMALLY")));

        std::fs::remove_dir_all(&data).ok();
    }
}
