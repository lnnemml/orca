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
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rusqlite::{Connection, OptionalExtension};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

use crate::commands::jobs::{
    finalize_job_conn, get_job_conn, set_job_dir_conn, set_job_energy_conn,
    set_job_parse_error_conn, set_job_results_conn, update_job_status_conn,
};
use crate::commands::settings::DbState;
use crate::convergence::{ConvergenceEvent, ConvergenceParser};
use crate::cpu_presets::{CpuPreset, DEFAULT_PRESET_ID};
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

/// App-wide LocalBackend state: the job-directory root, the single execution
/// slot (`Some` while a job is running — concurrency = 1, domain rule #4), and a
/// pause flag for the queue.
///
/// The queue itself is NOT held in memory — `queued` jobs live in SQLite and are
/// picked up by [`try_start_next`]. That survives an app restart and needs no
/// separate worker thread or channel.
pub struct JobRunner {
    data_dir: PathBuf,
    running: Mutex<Option<RunningJob>>,
    /// When true, finished jobs don't pull the next `queued` one. The currently
    /// running job (if any) always runs to completion — pause is queue-only.
    paused: AtomicBool,
}

/// The one job currently executing, plus what cancellation needs.
struct RunningJob {
    job_id: String,
    /// Process group id of the ORCA MPI tree (== the child pid, because we spawn
    /// with `process_group(0)`). `0` during the brief startup window before the
    /// child is spawned.
    pgid: i32,
    /// Set by [`cancel`] so [`drive_job`] records `cancelled` rather than `failed`.
    cancelled: Arc<AtomicBool>,
}

impl JobRunner {
    pub fn new(data_dir: PathBuf) -> Self {
        Self {
            data_dir,
            running: Mutex::new(None),
            paused: AtomicBool::new(false),
        }
    }

    fn running_lock(&self) -> Result<std::sync::MutexGuard<'_, Option<RunningJob>>, AppError> {
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

/// Payload of the `job:convergence` event: a batch of freshly parsed SCF /
/// optimization datapoints for the live dashboard. Batched on the same cadence
/// as logs so a fast SCF (dozens of iterations/sec) doesn't flood the UI.
#[derive(Clone, Serialize)]
struct ConvergencePayload {
    job_id: String,
    events: Vec<ConvergenceEvent>,
}

/// Create `<data_dir>/jobs/<job_id>/` and write `input.inp` into it, plus any
/// `aux_files` (name, content) — e.g. a NEB job's `product.xyz` end image (Stage
/// E3a-1). Aux files are written at RUN time here, not create time, so the "one dir,
/// created at run" invariant (rule #3) holds. Returns the absolute job directory.
pub fn prepare_job_dir(
    data_dir: &Path,
    job_id: &str,
    input_content: &str,
    aux_files: &[(String, String)],
) -> Result<PathBuf, AppError> {
    let dir = data_dir.join("jobs").join(job_id);
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join("input.inp"), input_content)?;
    for (name, content) in aux_files {
        // Guard: an aux name is a plain basename inside THIS job dir — never a path that
        // could escape it (rule #3 isolation). Refuse anything with a parent component.
        let file_name = std::path::Path::new(name).file_name().ok_or_else(|| {
            AppError::Internal(format!("invalid aux filename: {name:?}"))
        })?;
        if std::path::Path::new(name) != std::path::Path::new(file_name) {
            return Err(AppError::Internal(format!(
                "aux filename must be a plain basename, got {name:?}"
            )));
        }
        std::fs::write(dir.join(file_name), content)?;
    }
    Ok(dir)
}

/// Read a single settings value, or `None` if absent / unreadable.
fn read_setting(conn: &Connection, key: &str) -> Option<String> {
    conn.query_row(
        "SELECT value FROM settings WHERE key = ?1",
        [key],
        |r| r.get::<_, String>(0),
    )
    .optional()
    .ok()
    .flatten()
}

/// Resolve the effective `(taskset mask, nprocs)` for a run from settings
/// (domain rule #8). Falls back to the `interactive` preset when settings are
/// missing or malformed. A `None` mask means "no pinning" (direct invocation).
///
/// - `cpu_preset = custom` → use `cpu_mask` / `cpu_nprocs` verbatim.
/// - `cpu_preset = <known id>` → that preset's measured mask + nprocs.
/// - anything else (missing / unknown) → the default preset.
pub(crate) fn resolve_cpu_config(conn: &Connection) -> (Option<String>, u32) {
    let preset = read_setting(conn, "cpu_preset").unwrap_or_else(|| DEFAULT_PRESET_ID.to_string());

    if preset == "custom" {
        let mask = read_setting(conn, "cpu_mask").unwrap_or_default();
        let mask = if mask.trim().is_empty() {
            None
        } else {
            Some(mask.trim().to_string())
        };
        let nprocs = read_setting(conn, "cpu_nprocs")
            .and_then(|s| s.trim().parse::<u32>().ok())
            .filter(|n| *n >= 1)
            .unwrap_or(1);
        return (mask, nprocs);
    }

    let p = CpuPreset::by_id(&preset)
        .or_else(|| CpuPreset::by_id(DEFAULT_PRESET_ID))
        .expect("the default preset always exists");
    let mask = if p.mask.trim().is_empty() {
        None
    } else {
        Some(p.mask.to_string())
    };
    (mask, p.nprocs)
}

/// A human-readable label for the resolved preset, for the alignment log line.
fn resolve_cpu_label(conn: &Connection) -> String {
    let preset = read_setting(conn, "cpu_preset").unwrap_or_else(|| DEFAULT_PRESET_ID.to_string());
    if preset == "custom" {
        return "Custom".to_string();
    }
    CpuPreset::by_id(&preset)
        .or_else(|| CpuPreset::by_id(DEFAULT_PRESET_ID))
        .map(|p| p.label.to_string())
        .unwrap_or_else(|| "Interactive".to_string())
}

/// Rewrite `%pal nprocs N end` to match the pinned core count. ORCA would
/// otherwise oversubscribe the mask — 12 ranks on 4 cores is 3x slower, not
/// faster. If no `%pal` line exists, insert one after the `!` keyword line
/// (or at the top if there isn't one). Handles both the single-line
/// (`%pal nprocs 4 end`) and block (`%pal\n nprocs 4\nend`) forms.
///
/// Returns the (possibly unchanged) input and whether it was rewritten.
pub(crate) fn align_pal_nprocs(input: &str, nprocs: u32) -> (String, bool) {
    let canonical = format!("%pal nprocs {nprocs} end");
    let trailing_newline = input.ends_with('\n');
    let mut lines: Vec<String> = input.lines().map(str::to_string).collect();

    let pal_idx = lines
        .iter()
        .position(|l| l.trim_start().to_ascii_lowercase().starts_with("%pal"));

    if let Some(idx) = pal_idx {
        // Find the extent of the directive. Single-line form carries its own
        // `end`; block form spans until a line that is just `end`.
        let opener = lines[idx].trim().to_ascii_lowercase();
        let mut last = idx;
        if !opener.contains("end") {
            let mut j = idx + 1;
            while j < lines.len() {
                last = j;
                if lines[j].trim().eq_ignore_ascii_case("end") {
                    break;
                }
                j += 1;
            }
        }
        if idx == last && lines[idx] == canonical {
            return (input.to_string(), false); // already aligned
        }
        lines.splice(idx..=last, std::iter::once(canonical));
    } else if let Some(idx) = lines.iter().position(|l| l.trim_start().starts_with('!')) {
        lines.insert(idx + 1, canonical);
    } else {
        lines.insert(0, canonical);
    }

    let mut out = lines.join("\n");
    if trailing_newline {
        out.push('\n');
    }
    (out, true)
}

/// Spawn ORCA on `input.inp` inside `job_dir`. When `cpu_mask` is `Some`, ORCA
/// is launched under `taskset -c <mask>` with OpenMPI's own binding disabled so
/// the two don't fight (domain rule #8); otherwise it's invoked directly.
/// stdout is piped (for tailing); stderr goes to `stderr.log`.
///
/// The child (and every MPI rank it forks) is placed in its own process group so
/// a cancel can signal the whole tree with `killpg` — killing only the parent
/// would leave orphaned ranks burning CPU.
pub fn run_orca(orca_path: &str, job_dir: &Path, cpu_mask: Option<&str>) -> Result<Child, AppError> {
    let stderr = File::create(job_dir.join("stderr.log"))?;

    let mut cmd = match cpu_mask {
        Some(mask) => {
            let mut c = Command::new("taskset");
            c.arg("-c").arg(mask).arg(orca_path).arg("input.inp");
            c.env("OMPI_MCA_hwloc_base_binding_policy", "none");
            c
        }
        None => {
            let mut c = Command::new(orca_path);
            c.arg("input.inp");
            c
        }
    };

    cmd.current_dir(job_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::from(stderr));

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }

    cmd.spawn().map_err(|e| {
        if cpu_mask.is_some() && e.kind() == std::io::ErrorKind::NotFound {
            AppError::Backend(
                "taskset not found — install util-linux, or set cpu_preset to disable pinning"
                    .into(),
            )
        } else {
            AppError::Backend(format!("failed to spawn ORCA at '{orca_path}': {e}"))
        }
    })
}

/// Submit a draft job: move it to `queued` and try to start it. With the
/// single-slot queue this never fails with "another job is running" — a busy
/// slot just means the job waits its turn (oldest `created_at` first).
pub fn submit(app: &AppHandle, job_id: &str) -> Result<(), AppError> {
    let db = app.state::<DbState>();
    {
        let conn = db.lock()?;
        let job = get_job_conn(&conn, job_id)?;
        if job.status != JobStatus::Draft {
            return Err(AppError::Backend(format!(
                "job {job_id} is '{}'; only draft jobs can be queued",
                job.status.as_str()
            )));
        }
        update_job_status_conn(&conn, job_id, "queued")?;
    }
    emit_status(app, job_id, JobStatus::Queued);
    try_start_next(app);
    Ok(())
}

/// Start the oldest `queued` job if the slot is free and the queue isn't paused.
/// Called after every enqueue, after each job finishes, and on resume. A no-op
/// when the slot is busy, the queue is paused, or nothing is queued.
pub fn try_start_next(app: &AppHandle) {
    let runner = app.state::<JobRunner>();
    let db = app.state::<DbState>();

    // Claim the slot atomically: under the running lock, bail if paused or busy,
    // otherwise pick the oldest queued job and reserve the slot (pgid 0 = still
    // starting). Lock order is always running -> db (see cancel/try_start_next).
    let (job_id, cancelled) = {
        let mut running = match runner.running.lock() {
            Ok(g) => g,
            Err(_) => return,
        };
        if runner.paused.load(Ordering::SeqCst) || running.is_some() {
            return;
        }
        let next: Option<String> = {
            let conn = match db.lock() {
                Ok(c) => c,
                Err(_) => return,
            };
            conn.query_row(
                "SELECT id FROM jobs WHERE status = 'queued' ORDER BY created_at ASC LIMIT 1",
                [],
                |r| r.get::<_, String>(0),
            )
            .optional()
            .ok()
            .flatten()
        };
        let Some(job_id) = next else {
            return;
        };
        let cancelled = Arc::new(AtomicBool::new(false));
        *running = Some(RunningJob {
            job_id: job_id.clone(),
            pgid: 0,
            cancelled: cancelled.clone(),
        });
        (job_id, cancelled)
    };

    // Spawn is slow and needs the db lock, so it runs with no lock held. On
    // failure, record it, free the slot, and try the next queued job.
    if let Err(e) = start_run(app, &job_id, cancelled) {
        if let Ok(conn) = db.lock() {
            let _ = finalize_job_conn(&conn, &job_id, JobStatus::Failed, Some(&e.to_string()));
        }
        release_slot(&runner, &job_id);
        emit_status(app, &job_id, JobStatus::Failed);
        try_start_next(app);
    }
}

/// Prepare the dir, spawn ORCA (pinned per settings), mark running, and hand off
/// to the tailing thread. The slot is already reserved by [`try_start_next`];
/// `cancelled` is that slot's flag, shared with [`drive_job`].
fn start_run(app: &AppHandle, job_id: &str, cancelled: Arc<AtomicBool>) -> Result<(), AppError> {
    let db = app.state::<DbState>();
    let runner = app.state::<JobRunner>();

    // Read input + ORCA path (full absolute path, domain rule #1) + CPU config + any
    // aux files (a NEB job's product.xyz — Stage E3a-1).
    let (input_content, aux_files, orca_path, cpu_mask, nprocs, preset_label) = {
        let conn = db.lock()?;
        let job = get_job_conn(&conn, job_id)?;
        let aux_files: Vec<(String, String)> = conn
            .query_row("SELECT aux_files_json FROM jobs WHERE id = ?1", [job_id], |r| {
                r.get::<_, Option<String>>(0)
            })
            .optional()?
            .flatten()
            .and_then(|j| {
                serde_json::from_str::<std::collections::BTreeMap<String, String>>(&j).ok()
            })
            .map(|m| m.into_iter().collect())
            .unwrap_or_default();
        let orca_path = conn
            .query_row(
                "SELECT value FROM settings WHERE key = 'orca_path'",
                [],
                |r| r.get::<_, String>(0),
            )
            .optional()?
            .filter(|p| !p.trim().is_empty())
            .ok_or_else(|| {
                AppError::Backend("ORCA path is not configured (see Settings)".into())
            })?;
        let (mask, nprocs) = resolve_cpu_config(&conn);
        let label = resolve_cpu_label(&conn);
        (job.input_content, aux_files, orca_path, mask, nprocs, label)
    };

    // Align %pal to the pinned rank count (avoid oversubscribing the mask).
    let (aligned_input, rewritten) = align_pal_nprocs(&input_content, nprocs);

    // Isolated job dir + aligned input.inp + any aux files (product.xyz); persist path.
    let job_dir = prepare_job_dir(&runner.data_dir, job_id, &aligned_input, &aux_files)?;
    {
        let conn = db.lock()?;
        set_job_dir_conn(&conn, job_id, &job_dir.to_string_lossy())?;
    }

    // Spawn ORCA pinned to the resolved core mask.
    let mut child = run_orca(&orca_path, &job_dir, cpu_mask.as_deref())?;
    let pgid = child.id() as i32; // process_group(0) → pgid == child pid

    // Record the pgid on the reserved slot so cancel can signal the whole tree.
    {
        let mut running = runner.running_lock()?;
        match running.as_mut() {
            Some(rj) if rj.job_id == job_id => rj.pgid = pgid,
            _ => {
                // Slot vanished (e.g. cancelled during startup) — kill the child.
                let _ = child.kill();
                return Err(AppError::Backend("execution slot lost during startup".into()));
            }
        }
    }

    // If a cancel landed during the startup window, terminate the tree now
    // (off-thread — see `cancel`); drive_job finalizes it as cancelled on exit.
    if cancelled.load(Ordering::SeqCst) {
        let dir = job_dir.clone();
        std::thread::spawn(move || terminate_job(pgid, &dir));
    }

    // Mark running + notify. If the DB update fails, kill the child.
    {
        let conn = db.lock()?;
        if let Err(e) = update_job_status_conn(&conn, job_id, "running") {
            let _ = child.kill();
            return Err(e);
        }
    }
    emit_status(app, job_id, JobStatus::Running);

    // Surface the %pal alignment so it isn't silent magic (learning instrument).
    if rewritten {
        emit_log_line(
            app,
            job_id,
            &format!("[OrcaStudio] %pal nprocs aligned to {nprocs} (cpu preset: {preset_label})"),
        );
    }

    // Drive stdout tailing + completion detection off-thread.
    let app = app.clone();
    let job_id = job_id.to_string();
    std::thread::spawn(move || drive_job(app, job_id, job_dir, child, cancelled));
    Ok(())
}

/// Cancel a `queued` or `running` job. A queued job is dropped in place (nothing
/// to kill). A running job's tree is terminated **off-thread** — [`drive_job`]
/// records the terminal `cancelled` state once the process exits. Any other
/// status is an error.
pub fn cancel(app: &AppHandle, job_id: &str) -> Result<(), AppError> {
    let db = app.state::<DbState>();
    let runner = app.state::<JobRunner>();

    // Is this the currently running job? Grab its pgid + cancel flag.
    let running_info = {
        let running = runner.running_lock()?;
        running
            .as_ref()
            .filter(|r| r.job_id == job_id)
            .map(|r| (r.pgid, r.cancelled.clone()))
    };

    if let Some((pgid, flag)) = running_info {
        // Mark first so drive_job finalizes as `cancelled`, not `failed`.
        flag.store(true, Ordering::SeqCst);
        // pgid 0 = still in the startup window; start_run will terminate once it
        // has the real pgid (it checks the same flag).
        if pgid > 0 {
            // terminate_job blocks up to ~12 s (SIGTERM grace + sweep + SIGKILL).
            // The `cancelled` flag already guarantees drive_job finalizes as
            // `cancelled`, so don't make the UI wait for the kill to finish.
            let job_dir = {
                let conn = db.lock()?;
                get_job_conn(&conn, job_id)?.job_dir
            }
            .map(PathBuf::from)
            .unwrap_or_else(|| runner.data_dir.join("jobs").join(job_id));
            std::thread::spawn(move || terminate_job(pgid, &job_dir));
        }
        return Ok(());
    }

    // Not running — a queued job can be cancelled in place.
    let status = {
        let conn = db.lock()?;
        get_job_conn(&conn, job_id)?.status
    };
    match status {
        JobStatus::Queued => {
            {
                let conn = db.lock()?;
                finalize_job_conn(
                    &conn,
                    job_id,
                    JobStatus::Cancelled,
                    Some("Cancelled before it started."),
                )?;
            }
            emit_status(app, job_id, JobStatus::Cancelled);
            Ok(())
        }
        other => Err(AppError::Backend(format!(
            "job is not running or queued (status: {})",
            other.as_str()
        ))),
    }
}

/// Remove a **deleted** job's isolated directory — but ONLY if it is canonically
/// a descendant of the managed jobs root (`data_dir/jobs/`). rule #9: check the
/// destructive fs op in OUR terms before doing it; never `rm` an arbitrary path
/// read from the DB.
///
/// **Best-effort.** The DB row is already gone by the time this runs (Phase 4.7.1
/// deletes the row first, then removes the dir), so a leftover directory is a
/// warning, never a user-facing "delete failed". On any error — path escapes the
/// root, or `remove_dir_all` fails — it `eprintln`s a warning (the killpg-sweep
/// idiom) and returns.
pub(crate) fn remove_job_dir(app: &AppHandle, job_dir: &str) {
    let runner = app.state::<JobRunner>();
    let root = runner.data_dir.join("jobs");
    let candidate = Path::new(job_dir);
    if !path_is_within(&root, candidate) {
        eprintln!(
            "delete_job: refusing to remove {job_dir:?} — not within the managed jobs root {root:?}"
        );
        return;
    }
    if let Err(e) = std::fs::remove_dir_all(candidate) {
        eprintln!("delete_job: could not remove job dir {job_dir:?}: {e}");
    }
}

/// True iff `candidate`, canonicalized, is `root` or a descendant of `root` (also
/// canonicalized). The rule #9 guard behind the single `remove_dir_all` in the
/// delete path: we never remove a path read from the DB unless it is provably
/// inside the managed jobs root.
///
/// A path that cannot be canonicalized (a non-existent `candidate` or `root`, or a
/// symlink whose target is gone) yields `false` — refuse. `canonicalize` resolves
/// symlinks, so a symlink escaping the root also yields `false`.
pub(crate) fn path_is_within(root: &Path, candidate: &Path) -> bool {
    match (root.canonicalize(), candidate.canonicalize()) {
        (Ok(root), Ok(candidate)) => candidate.starts_with(&root),
        _ => false,
    }
}

/// Set the queue pause flag. Pausing leaves the running job alone; resuming
/// immediately pulls the next queued job.
pub fn set_paused(app: &AppHandle, paused: bool) {
    if let Some(runner) = app.try_state::<JobRunner>() {
        runner.paused.store(paused, Ordering::SeqCst);
    }
    if !paused {
        try_start_next(app);
    }
}

/// Whether the queue is currently paused.
pub fn is_paused(app: &AppHandle) -> bool {
    app.try_state::<JobRunner>()
        .map(|r| r.paused.load(Ordering::SeqCst))
        .unwrap_or(false)
}

/// Reconcile job state on startup. After a restart nothing is actually running
/// (the process tree died with the app), so advance every job the DB still
/// thinks is `running`: if its dir shows a completed ORCA run, finalize it (with
/// results); otherwise mark it failed. `queued` jobs are left untouched — the
/// startup `try_start_next` picks them back up. Best-effort: DB errors are
/// swallowed so a bad row can't block launch.
/// Parse + store a completed job's structured `.property.txt` results and return
/// the resulting status: `parsed` on success, `completed` on a parse failure (with
/// the reason recorded — the calculation ran fine, only our parse did not) or when
/// there is nothing to parse. Shared by the live finish path and reconciliation.
fn parse_results_after_completion(conn: &Connection, job_id: &str) -> JobStatus {
    let job = match get_job_conn(conn, job_id) {
        Ok(j) => j,
        Err(_) => return JobStatus::Completed,
    };
    let Some(dir) = job.job_dir.as_deref() else {
        return JobStatus::Completed;
    };
    match crate::results::parse_and_store(conn, job_id, dir, &job.input_content) {
        crate::results::ParseOutcome::Parsed => {
            let _ = update_job_status_conn(conn, job_id, "parsed");
            // The header/list energy now comes from the AUTHORITATIVE results tier
            // (ADR-012), overwriting the output.out tail estimate: a large molecule
            // pushes the last FINAL SINGLE POINT ENERGY past the 64 KB estimate
            // window (measured 164 KB on a 33-atom Freq — unit 3.9 defect 2). The
            // regex stays a live estimate DURING the run; once `.property.txt` is
            // parsed, this is the real value.
            if let Some(e) = crate::results::stored_final_energy(conn, job_id) {
                let _ = set_job_energy_conn(conn, job_id, e);
            }
            JobStatus::Parsed
        }
        crate::results::ParseOutcome::ParseFailed(msg) => {
            let _ = set_job_parse_error_conn(conn, job_id, &format!("results parse failed: {msg}"));
            JobStatus::Completed
        }
        crate::results::ParseOutcome::NoArtifact => JobStatus::Completed,
    }
}

pub fn reconcile_on_startup(conn: &Connection) {
    let stale: Vec<(String, Option<String>)> = {
        let mut stmt = match conn.prepare("SELECT id, job_dir FROM jobs WHERE status = 'running'") {
            Ok(s) => s,
            Err(_) => return,
        };
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?)));
        match rows {
            Ok(iter) => iter.filter_map(Result::ok).collect(),
            Err(_) => return,
        }
    };

    for (id, job_dir) in stale {
        let Some(dir) = job_dir else {
            let _ = finalize_job_conn(
                conn,
                &id,
                JobStatus::Failed,
                Some("app was closed while this job was running"),
            );
            continue;
        };
        let dir = Path::new(&dir);
        let out_path = dir.join("output.out");
        let exit_path = dir.join(".exit_code");

        // If the job actually finished before the app closed, honour the marker.
        let (status, msg) = if exit_path.exists() {
            let exit_code = std::fs::read_to_string(&exit_path)
                .ok()
                .and_then(|s| s.trim().parse::<i32>().ok());
            detect_completion(&out_path, &dir.join("stderr.log"), exit_code)
        } else {
            (
                JobStatus::Failed,
                Some("app was closed while this job was running".to_string()),
            )
        };

        let _ = finalize_job_conn(conn, &id, status, msg.as_deref());
        if status == JobStatus::Completed {
            if let Ok(tail) = read_tail(&out_path, RESULT_TAIL_BYTES) {
                let energy = crate::result_extraction::extract_final_energy(&tail);
                let wall_time = crate::result_extraction::extract_wall_time(&tail);
                if energy.is_some() || wall_time.is_some() {
                    let _ = set_job_results_conn(conn, &id, energy, wall_time);
                }
            }
            // Same parse+store as the live finish path (idempotent on job id).
            let _ = parse_results_after_completion(conn, &id);
        }
    }
}

/// Stream stdout to `output.out` + the UI, then wait, write `.exit_code`, detect
/// completion (or cancellation), persist the final state, free the slot, and pull
/// the next queued job.
fn drive_job(
    app: AppHandle,
    job_id: String,
    job_dir: PathBuf,
    mut child: Child,
    cancelled: Arc<AtomicBool>,
) {
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
        let mut conv_batch: Vec<ConvergenceEvent> = Vec::new();
        let mut parser = ConvergenceParser::new();
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
                    let trimmed = line.trim_end_matches(['\n', '\r']);
                    // Same stream feeds the convergence parser — never a re-read
                    // of output.out (domain rule #5).
                    if let Some(ev) = parser.feed(trimmed) {
                        conv_batch.push(ev);
                    }
                    batch.push(trimmed.to_string());
                    if batch.len() >= LOG_BATCH_LINES
                        || last_flush.elapsed() >= Duration::from_millis(LOG_BATCH_MILLIS)
                    {
                        if let Some(w) = writer.as_mut() {
                            let _ = w.flush();
                        }
                        emit_log(&app, &job_id, &mut batch);
                        emit_convergence(&app, &job_id, &mut conv_batch);
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
        emit_convergence(&app, &job_id, &mut conv_batch);
    }

    // Wait for exit and write the completion marker (domain rule #6).
    let exit_code = child.wait().ok().and_then(|s| s.code());
    let was_cancelled = cancelled.load(Ordering::SeqCst);
    let exit_str = if was_cancelled {
        "cancelled".to_string()
    } else {
        exit_code
            .map(|c| c.to_string())
            .unwrap_or_else(|| "unknown".to_string())
    };
    let _ = std::fs::write(job_dir.join(".exit_code"), format!("{exit_str}\n"));

    // A user cancel takes precedence over the (aborted) exit code: don't dump a
    // stderr sheet as if it failed. Otherwise: completion = marker + banner.
    let (mut status, error_message) = if was_cancelled {
        (JobStatus::Cancelled, Some("Cancelled by user.".to_string()))
    } else {
        detect_completion(&out_path, &job_dir.join("stderr.log"), exit_code)
    };

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

    // Parse + store the structured `.property.txt` results (ADR-012), then advance
    // to `parsed`. A parse failure is OUR problem, not the calculation's: the job
    // stays `completed` and the reason is recorded (distinct from a `failed` run);
    // a missing artifact is "nothing to parse", not a failure.
    if status == JobStatus::Completed {
        if let Some(db) = app.try_state::<DbState>() {
            if let Ok(conn) = db.lock() {
                status = parse_results_after_completion(&conn, &job_id);
            }
        }
    }

    if let Some(runner) = app.try_state::<JobRunner>() {
        release_slot(&runner, &job_id);
    }
    emit_status(&app, &job_id, status);

    // Slot is free — pull the next queued job (unless the queue is paused).
    try_start_next(&app);
}

/// Free the execution slot iff it is still held by `job_id`.
fn release_slot(runner: &JobRunner, job_id: &str) {
    if let Ok(mut running) = runner.running_lock() {
        if running.as_ref().map(|r| r.job_id.as_str()) == Some(job_id) {
            *running = None;
        }
    }
}

/// Send `sig` to an entire process group (no-op for a non-positive pgid).
#[cfg(unix)]
pub(crate) fn signal_pgid(pgid: i32, sig: i32) {
    if pgid > 0 {
        // SAFETY: killpg is a thin syscall wrapper; passing a signal number and
        // a pgid has no memory-safety implications.
        unsafe {
            libc::killpg(pgid, sig);
        }
    }
}
#[cfg(not(unix))]
pub(crate) fn signal_pgid(_pgid: i32, _sig: i32) {}

/// Signal every live process whose working directory is `job_dir`.
///
/// This is the safety net behind `killpg`. `process_group(0)` puts the ORCA
/// parent, its `sh`, and `mpirun` in one group, but **not the MPI ranks** —
/// mpirun gives each rank its own process group so terminal signals can't reach
/// them (verified: ranks show PGID == their own PID). killpg therefore only
/// reaches mpirun, which forwards SIGTERM to its ranks on the graceful path.
/// After a SIGKILL mpirun cannot forward anything, which would strand N ranks
/// burning N cores. Every process of a job runs with its cwd set to the job
/// directory, so that is what we match on.
///
/// Best-effort: unreadable `/proc` entries (races, other users) are skipped.
#[cfg(unix)]
pub(crate) fn sweep_job_processes(job_dir: &Path, sig: i32) -> usize {
    let target = match job_dir.canonicalize() {
        Ok(p) => p,
        Err(_) => return 0,
    };
    let self_pid = std::process::id();
    let entries = match std::fs::read_dir("/proc") {
        Ok(e) => e,
        Err(_) => return 0,
    };

    let mut signalled = 0;
    for entry in entries.flatten() {
        // /proc/<pid>: numeric names only.
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        let Ok(pid) = name.parse::<i32>() else {
            continue;
        };
        if pid as u32 == self_pid {
            continue; // never signal ourselves
        }
        // The cwd symlink is unreadable if the process has exited or belongs to
        // another user — skip it rather than fail the whole sweep.
        let Ok(cwd) = std::fs::read_link(format!("/proc/{pid}/cwd")) else {
            continue;
        };
        if cwd == target {
            // SAFETY: kill is a thin syscall wrapper; a pid + signal number has
            // no memory-safety implications.
            unsafe {
                libc::kill(pid, sig);
            }
            signalled += 1;
        }
    }
    signalled
}
#[cfg(not(unix))]
pub(crate) fn sweep_job_processes(_job_dir: &Path, _sig: i32) -> usize {
    0
}

/// Terminate a whole ORCA job tree: its process group **and** the MPI ranks that
/// escaped it (see [`sweep_job_processes`]).
///
/// 1. `killpg(SIGTERM)` — the graceful path: mpirun reaps its own ranks.
/// 2. wait up to **10 s** for the group to drain (a heavy job may still be
///    flushing `.gbw`), polling liveness.
/// 3. regardless of that result, `sweep(SIGTERM)` — catches ranks mpirun never
///    reached. Done **before** the SIGKILL so ranks still get a clean-exit shot.
/// 4. 2 s grace.
/// 5. `killpg(SIGKILL)` **and** `sweep(SIGKILL)` for anything still alive.
///
/// If the SIGKILL sweep had to kill anything, the graceful path failed — log it,
/// because that means mpirun did not forward our signal to its ranks.
#[cfg(unix)]
pub(crate) fn terminate_job(pgid: i32, job_dir: &Path) {
    signal_pgid(pgid, libc::SIGTERM);

    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        // killpg(pgid, 0) probes liveness: 0 = the group still has members.
        if unsafe { libc::killpg(pgid, 0) } != 0 {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }

    // The ranks live outside the group — SIGTERM them by cwd before escalating.
    sweep_job_processes(job_dir, libc::SIGTERM);
    std::thread::sleep(Duration::from_secs(2));

    // Escalate: hard-kill the group and any surviving ranks.
    signal_pgid(pgid, libc::SIGKILL);
    let killed = sweep_job_processes(job_dir, libc::SIGKILL);
    if killed > 0 {
        eprintln!(
            "[OrcaStudio] cancel: SIGKILLed {killed} orphaned process(es) in {} — \
             mpirun did not reap its MPI ranks on SIGTERM",
            job_dir.display()
        );
    }
}
#[cfg(not(unix))]
pub(crate) fn terminate_job(_pgid: i32, _job_dir: &Path) {}

/// Kill the currently running ORCA tree (if any) **synchronously** — used on app
/// exit. A spawned thread would die with the process before the ranks do, so the
/// blocking wait here is deliberate: it's the only place we must not return until
/// the tree is dead, otherwise the MPI ranks outlive the app burning CPU.
pub fn terminate_on_exit(app: &AppHandle) {
    let Some(runner) = app.try_state::<JobRunner>() else {
        return;
    };
    let (job_id, pgid) = {
        let Ok(running) = runner.running.lock() else {
            return;
        };
        match running.as_ref() {
            Some(r) if r.pgid > 0 => (r.job_id.clone(), r.pgid),
            _ => return,
        }
    };
    // The job dir is deterministic (prepare_job_dir), so no DB read is needed.
    let job_dir = runner.data_dir.join("jobs").join(&job_id);
    terminate_job(pgid, &job_dir);
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

/// Emit a batch of convergence datapoints (no-op if empty); drains `batch`.
fn emit_convergence(app: &AppHandle, job_id: &str, batch: &mut Vec<ConvergenceEvent>) {
    if batch.is_empty() {
        return;
    }
    let _ = app.emit(
        "job:convergence",
        ConvergencePayload {
            job_id: job_id.to_string(),
            events: std::mem::take(batch),
        },
    );
}

/// Emit a single app-generated log line (e.g. the %pal alignment notice).
fn emit_log_line(app: &AppHandle, job_id: &str, line: &str) {
    let _ = app.emit(
        "job:log",
        LogPayload {
            job_id: job_id.to_string(),
            lines: vec![line.to_string()],
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
/// multi-MB output is never fully loaded (domain rule #5). `pub(crate)` so the
/// results parser can read the same bounded tail for the convergence verdict.
pub(crate) fn read_tail(path: &Path, max_bytes: u64) -> std::io::Result<String> {
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

/// Replay a finished/running job's `output.out` through a fresh
/// [`ConvergenceParser`] and return every datapoint, for backfilling the
/// dashboard when the detail screen opens. Reads **line by line** via a
/// `BufReader` — the file (tens of MB for a long run) is never loaded whole
/// (domain rule #5); only the compact event list is held.
pub(crate) fn read_convergence(path: &Path) -> std::io::Result<Vec<ConvergenceEvent>> {
    let file = File::open(path)?;
    let reader = BufReader::new(file);
    let mut parser = ConvergenceParser::new();
    let mut events = Vec::new();
    for line in reader.lines() {
        let line = line?;
        if let Some(ev) = parser.feed(&line) {
            events.push(ev);
        }
    }
    Ok(events)
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

    // --- path_is_within: the rule #9 guard behind the delete path's remove_dir_all ---

    #[test]
    fn path_is_within_accepts_a_descendant() {
        let root = scratch("within-root");
        let child = root.join("some-job-id");
        std::fs::create_dir_all(&child).unwrap();
        assert!(path_is_within(&root, &child));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn path_is_within_accepts_the_root_itself() {
        let root = scratch("within-self");
        assert!(path_is_within(&root, &root));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn path_is_within_rejects_a_sibling_outside() {
        // `<parent>/jobs` is the root; `<parent>/jobs-evil` is a sibling that shares
        // a textual prefix but is NOT inside — component-wise starts_with rejects it.
        let parent = scratch("within-sibling");
        let root = parent.join("jobs");
        let sibling = parent.join("jobs-evil");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&sibling).unwrap();
        assert!(!path_is_within(&root, &sibling));
        std::fs::remove_dir_all(&parent).ok();
    }

    #[test]
    fn path_is_within_rejects_the_parent() {
        let parent = scratch("within-parent");
        let root = parent.join("jobs");
        std::fs::create_dir_all(&root).unwrap();
        assert!(!path_is_within(&root, &parent));
        std::fs::remove_dir_all(&parent).ok();
    }

    #[test]
    fn path_is_within_rejects_a_nonexistent_candidate() {
        let root = scratch("within-nonexistent");
        let ghost = root.join("never-created");
        // Can't canonicalize a non-existent path → refuse.
        assert!(!path_is_within(&root, &ghost));
        std::fs::remove_dir_all(&root).ok();
    }

    #[cfg(unix)]
    #[test]
    fn path_is_within_rejects_a_symlink_escape() {
        // A dir INSIDE the root that is actually a symlink pointing OUTSIDE it.
        // canonicalize resolves the link, so the guard sees the real (outside)
        // target and refuses.
        use std::os::unix::fs::symlink;
        let parent = scratch("within-symlink");
        let root = parent.join("jobs");
        let outside = parent.join("outside-target");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        let link = root.join("escape");
        symlink(&outside, &link).unwrap();
        assert!(!path_is_within(&root, &link));
        std::fs::remove_dir_all(&parent).ok();
    }

    /// A `sh -c 'sleep 30'` child with its cwd set to `dir`. `sh` execs `sleep`,
    /// so the returned `Child` *is* the process running in `dir`.
    #[cfg(unix)]
    fn spawn_sleeper_in(dir: &Path) -> std::process::Child {
        let child = Command::new("sh")
            .arg("-c")
            .arg("sleep 30")
            .current_dir(dir)
            .spawn()
            .expect("spawn sleeper");
        // Let it start and settle its cwd before we scan /proc.
        std::thread::sleep(Duration::from_millis(200));
        child
    }

    /// Reap `child`, hard-killing it first so a test can never leak a `sleep 30`.
    #[cfg(unix)]
    fn reap(mut child: std::process::Child) {
        let _ = child.kill();
        let _ = child.wait();
    }

    #[cfg(unix)]
    #[test]
    fn sweep_job_processes_matches_cwd() {
        let dir = scratch("sweep-match");
        let mut child = spawn_sleeper_in(&dir);

        let hit = sweep_job_processes(&dir, libc::SIGTERM);
        assert!(hit >= 1, "expected to signal ≥1 process in the job dir, got {hit}");

        // The SIGTERM should terminate it; wait for exit (also reaps the zombie).
        let mut exited = false;
        for _ in 0..60 {
            if matches!(child.try_wait(), Ok(Some(_))) {
                exited = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        if !exited {
            reap(child);
        }
        std::fs::remove_dir_all(&dir).ok();
        assert!(exited, "sweep SIGTERM should have killed the process in the job dir");
    }

    #[cfg(unix)]
    #[test]
    fn sweep_ignores_other_dirs() {
        let job_dir = scratch("sweep-jobdir");
        let other_dir = scratch("sweep-otherdir");
        let mut child = spawn_sleeper_in(&other_dir);

        // Sweeping the (empty) job dir must not touch a process living elsewhere.
        let hit = sweep_job_processes(&job_dir, libc::SIGTERM);
        assert_eq!(hit, 0, "a process in another dir must not be signalled");

        std::thread::sleep(Duration::from_millis(150));
        let still_alive = matches!(child.try_wait(), Ok(None));
        reap(child);
        std::fs::remove_dir_all(&job_dir).ok();
        std::fs::remove_dir_all(&other_dir).ok();
        assert!(still_alive, "the process in the other dir should be untouched");
    }

    #[cfg(unix)]
    #[test]
    fn sweep_never_kills_self() {
        // Point the sweep at a dir that is THIS process's cwd, with SIGKILL. The
        // self-PID guard must skip us — if it didn't, the test process would die
        // here. A unique temp dir guarantees no other process shares this cwd,
        // so nothing else is matched (no collateral); cwd is restored after.
        let original = std::env::current_dir().unwrap();
        let dir = scratch("sweep-self");
        std::env::set_current_dir(&dir).unwrap();
        let hit = sweep_job_processes(&dir, libc::SIGKILL);
        std::env::set_current_dir(&original).unwrap();
        std::fs::remove_dir_all(&dir).ok();

        // Reaching this line at all means we weren't killed; and self is excluded.
        assert_eq!(hit, 0, "sweep must skip its own pid");
        assert_eq!(
            unsafe { libc::kill(std::process::id() as i32, 0) },
            0,
            "the test process must still be alive"
        );
    }

    #[test]
    fn prepare_job_dir_writes_input() {
        let data = scratch("prep");
        let dir = prepare_job_dir(&data, "job-123", "! r2SCAN-3c\n", &[]).unwrap();
        assert!(dir.ends_with("jobs/job-123"));
        let inp = std::fs::read_to_string(dir.join("input.inp")).unwrap();
        assert_eq!(inp, "! r2SCAN-3c\n");
        std::fs::remove_dir_all(&data).ok();
    }

    #[test]
    fn prepare_job_dir_materializes_aux_files() {
        // A NEB job's product.xyz aux file lands beside input.inp in the isolated dir.
        let data = scratch("prep-aux");
        let aux = vec![("product.xyz".to_string(), "1\nproduct\nN 0 0 1.5\n".to_string())];
        let dir = prepare_job_dir(&data, "neb-1", "! NEB-TS\n", &aux).unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.join("product.xyz")).unwrap(),
            "1\nproduct\nN 0 0 1.5\n"
        );
        std::fs::remove_dir_all(&data).ok();
    }

    #[test]
    fn prepare_job_dir_refuses_a_path_traversal_aux_name() {
        // An aux name that escapes the isolated dir (rule #3) is refused, never written.
        let data = scratch("prep-escape");
        let aux = vec![("../escape.xyz".to_string(), "x".to_string())];
        assert!(prepare_job_dir(&data, "neb-2", "! NEB-TS\n", &aux).is_err());
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

    /// Defect 2 (unit 3.9), on a REAL 200 KB dexketoprofen output tail: the last
    /// `FINAL SINGLE POINT ENERGY` sits 164 KB from EOF — past `RESULT_TAIL_BYTES`
    /// (64 KB). So the tail regex (a live estimate) MISSES it, which is exactly
    /// why the header/list energy is taken from the authoritative `results` tier
    /// (ADR-012), not from here. This locks the measured window gap so a future
    /// "just bump the constant" regression is caught.
    #[test]
    fn final_energy_sits_past_the_estimate_window_on_a_real_big_molecule() {
        let fixture = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/dexketoprofen_output_tail.out"
        );
        let path = Path::new(fixture);
        // The 64 KB estimate window does NOT reach the final energy.
        let tail64 = read_tail(path, RESULT_TAIL_BYTES).unwrap();
        assert!(
            crate::result_extraction::extract_final_energy(&tail64).is_none(),
            "the 64 KB estimate window unexpectedly reached the final energy — \
             the defect-2 gap is not being exercised"
        );
        // A large enough tail DOES contain it → this is a window-size problem, not
        // a missing value. The value matches the parsed/authoritative one.
        let tail_all = read_tail(path, 1024 * 1024).unwrap();
        let e = crate::result_extraction::extract_final_energy(&tail_all)
            .expect("a full read must find the final energy");
        assert!((e - (-843.690395750533)).abs() < 1e-6, "got {e}");
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
    fn align_pal_rewrites_existing_single_line() {
        let input = "! B3LYP def2-SVP Opt\n%pal nprocs 4 end\n%maxcore 2000\n\n* xyz 0 1\n H 0 0 0\n*\n";
        let (out, changed) = align_pal_nprocs(input, 8);
        assert!(changed);
        assert!(out.contains("%pal nprocs 8 end"));
        assert!(!out.contains("nprocs 4"));
        // The coordinate block is untouched.
        assert!(out.contains("* xyz 0 1\n H 0 0 0\n*"));
        assert!(out.contains("%maxcore 2000"));
    }

    #[test]
    fn align_pal_is_case_insensitive() {
        let (out, changed) = align_pal_nprocs("! HF\n%PAL NPROCS 4 END\n", 8);
        assert!(changed);
        assert!(out.contains("%pal nprocs 8 end"));
        assert!(!out.to_ascii_uppercase().contains("NPROCS 4"));
    }

    #[test]
    fn align_pal_inserts_after_bang_line_when_absent() {
        let input = "! r2SCAN-3c Opt\n\n* xyz 0 1\n H 0 0 0\n*\n";
        let (out, changed) = align_pal_nprocs(input, 8);
        assert!(changed);
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines[0], "! r2SCAN-3c Opt");
        assert_eq!(lines[1], "%pal nprocs 8 end"); // right after the ! line
        assert!(out.contains("* xyz 0 1\n H 0 0 0\n*"));
    }

    #[test]
    fn align_pal_rewrites_block_form() {
        // Block form: %pal opening line, then nprocs, then a lone end.
        let input = "! HF\n%pal\n  nprocs 4\nend\n%maxcore 1000\n";
        let (out, changed) = align_pal_nprocs(input, 12);
        assert!(changed);
        assert!(out.contains("%pal nprocs 12 end"));
        assert!(!out.contains("nprocs 4"));
        // The stray block lines are gone; maxcore survives.
        assert!(out.contains("%maxcore 1000"));
        assert_eq!(out.matches("end").count(), 1);
    }

    #[test]
    fn align_pal_noop_when_already_aligned() {
        let input = "! HF\n%pal nprocs 8 end\n";
        let (out, changed) = align_pal_nprocs(input, 8);
        assert!(!changed);
        assert_eq!(out, input);
    }

    /// An in-memory settings table for resolver tests.
    fn settings_db(pairs: &[(&str, &str)]) -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);")
            .unwrap();
        for (k, v) in pairs {
            conn.execute(
                "INSERT INTO settings (key, value) VALUES (?1, ?2)",
                rusqlite::params![k, v],
            )
            .unwrap();
        }
        conn
    }

    #[test]
    fn resolve_cpu_config_uses_named_preset() {
        // Default (interactive) when nothing is set.
        let (mask, nprocs) = resolve_cpu_config(&settings_db(&[]));
        assert_eq!(mask.as_deref(), Some("8-15"));
        assert_eq!(nprocs, 8);

        // Explicit max_throughput preset.
        let (mask, nprocs) =
            resolve_cpu_config(&settings_db(&[("cpu_preset", "max_throughput")]));
        assert_eq!(mask.as_deref(), Some("0,2,4,6,8-15"));
        assert_eq!(nprocs, 12);

        // Unknown preset id → falls back to the default.
        let (mask, nprocs) = resolve_cpu_config(&settings_db(&[("cpu_preset", "bogus")]));
        assert_eq!(mask.as_deref(), Some("8-15"));
        assert_eq!(nprocs, 8);
    }

    #[test]
    fn resolve_cpu_config_custom() {
        let (mask, nprocs) = resolve_cpu_config(&settings_db(&[
            ("cpu_preset", "custom"),
            ("cpu_mask", "0-3"),
            ("cpu_nprocs", "4"),
        ]));
        assert_eq!(mask.as_deref(), Some("0-3"));
        assert_eq!(nprocs, 4);
    }

    #[test]
    fn resolve_cpu_config_custom_malformed_values() {
        // Empty mask → no pinning; unparseable nprocs → 1.
        let (mask, nprocs) = resolve_cpu_config(&settings_db(&[
            ("cpu_preset", "custom"),
            ("cpu_mask", "   "),
            ("cpu_nprocs", "not-a-number"),
        ]));
        assert_eq!(mask, None);
        assert_eq!(nprocs, 1);
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
        let dir = prepare_job_dir(&data, "water-sp", input, &[]).unwrap();

        let mut child = run_orca(orca, &dir, None).unwrap();

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
