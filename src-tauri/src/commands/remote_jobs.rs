//! Remote job commands (ADR-024 o, unit 5.3 B1): retry, withdraw and label a job on a server
//! profile, and the remote arm of `submit_job` (`commands::jobs`). Thin shells over
//! [`SshBackend`], which wraps the Tauri-free core in `ssh_backend`.
//!
//! Every remote operation here:
//! - holds the job's [`InFlightGuard`](crate::in_flight::InFlightGuard) for its whole run (o item
//!   4): a second operation on the same job is **refused** while one is in flight;
//! - runs in `spawn_blocking`, never on the IPC thread: each ssh call can take up to 60 s, and the
//!   core locks the database only around its own reads and writes, never across a call;
//! - emits `job:status` (the local run's payload, `local_backend::emit_status`) when it changed the
//!   row's status or error message, so the job list and detail reload as they do for a local job.

use serde::Serialize;
use tauri::{AppHandle, Manager};

use crate::commands::jobs::get_job_conn;
use crate::commands::settings::DbState;
use crate::error::AppError;
use crate::execution_backend::{Backend, SshBackend};
use crate::in_flight::InFlight;
use crate::models::job::{Job, JobStatus};
use crate::ssh_backend::{LabelReport, PalAlignment, SubmitAttempt, SubmitOutcome, WithdrawReport};

/// What `submit_job` (to a server) and `retry_remote_submit` return: the attempt's outcome, the
/// `%pal` alignment, and both in words for the UI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SubmitResponse {
    pub outcome: SubmitOutcome,
    pub pal: PalAlignment,
    /// The `%pal` change, worded like the local run's log line; `None` when the input was uploaded
    /// as it is (`PalAlignment::notice`).
    pub notice: Option<String>,
    /// The attempt's failure for people; `None` for `Enqueued` (`SubmitOutcome::failure`).
    pub failure: Option<String>,
}

impl From<SubmitAttempt> for SubmitResponse {
    fn from(attempt: SubmitAttempt) -> Self {
        SubmitResponse {
            notice: attempt.pal.notice(),
            failure: attempt.outcome.failure(),
            outcome: attempt.outcome,
            pal: attempt.pal,
        }
    }
}

/// The `job:status` an operation owes the UI: the row's status after it, when the status or the
/// error message changed (a failed attempt changes only `error_message`, and the UI must still
/// reload to show it); `None` when the row is as it was (a refusal, the read-only label call).
pub(crate) fn status_to_emit(before: &Job, after: &Job) -> Option<JobStatus> {
    (before.status != after.status || before.error_message != after.error_message).then_some(after.status)
}

fn load_job(app: &AppHandle, id: &str) -> Result<Job, AppError> {
    let db = app.state::<DbState>();
    let conn = db.lock()?;
    get_job_conn(&conn, id)
}

/// The backend of existing job `id`, which must be remote (by its coordinates, o item 1).
fn remote_backend(app: &AppHandle, job: &Job) -> Result<SshBackend, AppError> {
    match Backend::for_job(app.clone(), job)? {
        Backend::Ssh(ssh) => Ok(ssh),
        Backend::Local(_) => Err(AppError::Backend(format!("job {} is not a remote job", job.id))),
    }
}

/// Run `work` off the IPC thread while holding job `id`'s in-flight guard (o item 4). The guard is
/// claimed **before** the task is spawned, so a refused second operation never queues behind the
/// first; it moves into the task and is dropped when `work` ends, returns early or panics. The
/// `AppHandle`-free core of [`run_guarded`], tested on its own.
pub(crate) async fn guarded_blocking<T, W>(in_flight: &InFlight, id: &str, what: &'static str, work: W) -> Result<T, AppError>
where
    T: Send + 'static,
    W: FnOnce() -> Result<T, AppError> + Send + 'static,
{
    let guard = in_flight.acquire(id)?;
    tauri::async_runtime::spawn_blocking(move || {
        let _guard = guard;
        work()
    })
    .await
    .map_err(|e| AppError::Backend(format!("{what} of job: the background task failed: {e}")))?
}

/// Run `op` on job `id` through [`guarded_blocking`], then emit `job:status` if the row changed
/// ([`status_to_emit`]). Every remote command goes through here.
async fn run_guarded<T, F>(app: AppHandle, id: String, what: &'static str, op: F) -> Result<T, AppError>
where
    T: Send + 'static,
    F: FnOnce(&AppHandle, &Job) -> Result<T, AppError> + Send + 'static,
{
    let in_flight = app.state::<InFlight>().inner().clone();
    let job_id = id.clone();
    guarded_blocking(&in_flight, &job_id, what, move || {
        let before = load_job(&app, &id)?;
        let result = op(&app, &before);
        // The row after the operation, whatever its result: a failure after the persist step
        // still changed it. A row that cannot be read now has nothing to announce.
        if let Some(status) = load_job(&app, &id).ok().and_then(|after| status_to_emit(&before, &after)) {
            crate::local_backend::emit_status(&app, &id, status);
        }
        result
    })
    .await
}

/// The remote arm of `submit_job`: submit draft `id` through `ssh` (whose target is the chosen
/// profile).
pub(crate) async fn submit_to_server(app: AppHandle, id: String, ssh: SshBackend) -> Result<SubmitResponse, AppError> {
    run_guarded(app, id, "submit", move |_, job| ssh.submit_attempt(&job.id).map(SubmitResponse::from)).await
}

/// Retry a remote job the label call finds "not on the server" (o item 3.4), with the profile's
/// current core mask.
#[tauri::command]
pub async fn retry_remote_submit(app: AppHandle, id: String) -> Result<SubmitResponse, AppError> {
    run_guarded(app, id, "retry", |app, job| remote_backend(app, job)?.retry(&job.id).map(SubmitResponse::from)).await
}

/// Withdraw a remote job that is "not on the server" or "submit interrupted" (o items 2, 14.1).
/// The classifier decides the row's status; only `Cancelled` makes it `cancelled`.
#[tauri::command]
pub async fn withdraw_remote_job(app: AppHandle, id: String) -> Result<WithdrawReport, AppError> {
    run_guarded(app, id, "withdraw", |app, job| remote_backend(app, job)?.withdraw(&job.id)).await
}

/// The read-only label call for a remote job (o item 3.4): what the UI may offer next.
#[tauri::command]
pub async fn label_remote_job(app: AppHandle, id: String) -> Result<LabelReport, AppError> {
    run_guarded(app, id, "label", |app, job| remote_backend(app, job)?.label(job)).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::init_db;
    use crate::remote::submit::KupEvidence;

    fn job(status: &str, error: Option<&str>) -> Job {
        let dir = std::env::temp_dir().join(format!("orcastudio-remotejobs-test-{}-{status}-{}", std::process::id(), error.is_some()));
        std::fs::remove_dir_all(&dir).ok();
        let conn = init_db(&dir).unwrap();
        conn.execute(
            "INSERT INTO jobs (id, title, input_content, status, error_message) VALUES ('j1', 't', '! HF', ?1, ?2)",
            rusqlite::params![status, error],
        )
        .unwrap();
        let job = get_job_conn(&conn, "j1").unwrap();
        drop(conn);
        std::fs::remove_dir_all(&dir).ok();
        job
    }

    /// NEGATIVE CONTROL target: compare only the status in `status_to_emit` and the second case goes
    /// red — a failed attempt after the persist (row still `queued`, a new `error_message`) would
    /// never tell the UI to show the reason.
    #[test]
    fn a_changed_status_or_error_message_is_announced_and_nothing_else() {
        let draft = job("draft", None);
        let queued = job("queued", None);
        let queued_failed = job("queued", Some("not submitted (Upload step; …)"));
        for (case, before, after, want) in [
            ("persisted: draft → queued", &draft, &queued, Some(JobStatus::Queued)),
            ("a failure after the persist: only the error message", &queued, &queued_failed, Some(JobStatus::Queued)),
            ("a retry that enqueued: the error message cleared", &queued_failed, &queued, Some(JobStatus::Queued)),
            ("refused, or the read-only label call", &queued_failed, &queued_failed, None),
            ("refused before the persist", &draft, &draft, None),
        ] {
            assert_eq!(status_to_emit(before, after), want, "{case}");
        }
    }

    /// The response carries the notice and the failure in words, derived from the attempt.
    #[test]
    fn the_submit_response_words_the_pal_change_and_the_failure() {
        let pal = PalAlignment { input_nprocs: Some(48), nprocs: 24, mask_cpus: 24, rewritten: true };
        let enqueued = SubmitResponse::from(SubmitAttempt { outcome: SubmitOutcome::Enqueued { tsp_id: 3 }, pal });
        assert_eq!(enqueued.failure, None);
        assert_eq!(enqueued.notice, pal.notice());
        assert!(enqueued.notice.as_deref().is_some_and(|n| n.contains("aligned to 24")), "{:?}", enqueued.notice);

        let kup = SubmitOutcome::KillUserProcesses { evidence: KupEvidence { rc: 0, stdout: "b true".into(), stderr: String::new() } };
        let refused = SubmitResponse::from(SubmitAttempt { outcome: kup.clone(), pal: PalAlignment { rewritten: false, ..pal } });
        assert_eq!(refused.failure, kup.failure());
        assert_eq!(refused.notice, None, "an input uploaded as it is has no notice");

        let wire = serde_json::to_value(&enqueued).unwrap();
        assert_eq!(wire["outcome"]["outcome"], "enqueued");
        assert_eq!(wire["outcome"]["tsp_id"], 3);
        assert_eq!(wire["pal"]["nprocs"], 24);
    }

    /// `submit_job` to this machine still answers what it answered when it returned `()`: `null`.
    #[test]
    fn a_local_submit_answers_null_as_before() {
        assert_eq!(serde_json::to_string(&None::<SubmitResponse>).unwrap(), serde_json::to_string(&()).unwrap());
    }

    // --- The in-flight guard around the blocking work (o item 4) -----------------------------

    /// The job is claimed for the whole of the blocking work — a concurrent claim (the poller's
    /// `try_acquire`, another command's `acquire`) fails from inside it — and free once it ends,
    /// also after an error. NEGATIVE CONTROL: `drop(guard)` as the closure's first statement in
    /// `guarded_blocking` → red ("busy during the work").
    #[test]
    fn the_job_is_claimed_for_the_whole_blocking_work_and_freed_after() {
        let in_flight = InFlight::default();
        let probe = in_flight.clone();
        let seen = tauri::async_runtime::block_on(guarded_blocking(&in_flight, "j1", "test", move || {
            Ok((probe.is_busy("j1"), probe.try_acquire("j1").is_none(), probe.acquire("j1").is_err(), probe.is_busy("j2")))
        }))
        .unwrap();
        assert_eq!(seen, (true, true, true, false), "busy during the work (is_busy, try_acquire None, acquire refused, j2 free)");
        assert!(!in_flight.is_busy("j1"), "freed after the work");

        let failed: Result<(), AppError> = tauri::async_runtime::block_on(guarded_blocking(&in_flight, "j1", "test", || {
            Err(AppError::Backend("ssh failed".into()))
        }));
        assert!(failed.is_err());
        assert!(!in_flight.is_busy("j1"), "freed after a failed operation");
    }

    /// A job already claimed is refused before any work runs.
    #[test]
    fn a_busy_job_is_refused_and_its_work_never_runs() {
        let in_flight = InFlight::default();
        let _held = in_flight.acquire("j1").unwrap();
        let ran = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = ran.clone();
        let refused = tauri::async_runtime::block_on(guarded_blocking(&in_flight, "j1", "test", move || {
            flag.store(true, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        }));
        assert!(matches!(&refused, Err(AppError::Conflict(m)) if m.contains("already in progress")), "{refused:?}");
        assert!(!ran.load(std::sync::atomic::Ordering::SeqCst), "the work never ran");
    }

    /// The body of `fn <signature>` in `src`, up to its closing brace at column 0.
    fn fn_body<'a>(src: &'a str, signature: &str) -> &'a str {
        let start = src.find(signature).unwrap_or_else(|| panic!("{signature} exists"));
        &src[start..start + src[start..].find("\n}\n").expect("the fn ends")]
    }

    /// Every remote command reaches the core only through `run_guarded`, `run_guarded` only through
    /// `guarded_blocking`, and `guarded_blocking` claims the guard before it spawns the task (a
    /// claim inside the task would let a refused operation queue behind the first). These need an
    /// `AppHandle`, so — as `cancel_job`'s test — the bodies are pinned in the source.
    /// NEGATIVE CONTROLS: the claim moved into the task → red ("claimed before the task is
    /// spawned"); `retry_remote_submit` calling `SshBackend::retry` directly → red ("goes through
    /// run_guarded").
    #[test]
    fn every_remote_command_goes_through_the_guard_claimed_before_the_task() {
        let src = include_str!("remote_jobs.rs");
        let core = fn_body(src, concat!("pub(crate) async fn ", "guarded_blocking<"));
        let claim = core.find(".acquire(id)?").expect("guarded_blocking claims the guard");
        let spawn = core.find("spawn_blocking(").expect("guarded_blocking spawns the task");
        assert!(claim < spawn, "the guard must be claimed before the task is spawned");
        assert!(core[spawn..].contains("let _guard = guard;"), "the guard moves into the task and lives to its end");

        let shell = fn_body(src, concat!("async fn ", "run_guarded<"));
        assert!(shell.contains("guarded_blocking(&in_flight"), "run_guarded goes through guarded_blocking");
        for command in [
            concat!("pub(crate) async fn ", "submit_to_server("),
            concat!("pub async fn ", "retry_remote_submit("),
            concat!("pub async fn ", "withdraw_remote_job("),
            concat!("pub async fn ", "label_remote_job("),
        ] {
            let body = fn_body(src, command);
            assert!(body.contains(concat!("run_", "guarded(app, id,")), "{command} goes through run_guarded");
            assert_eq!(body.matches(".await").count(), 1, "{command}: one awaited call, the guarded one");
        }
    }

    // --- The wire shapes B3 binds to (pinned, not designed here) -------------------------------

    #[test]
    fn the_withdraw_report_wire_shape() {
        use crate::remote::classify::{FailReason, Outcome};
        use crate::remote::submit::Label;
        let report = WithdrawReport { label: Label::SubmitInterrupted, outcome: Outcome::Cancelled, status: JobStatus::Cancelled };
        assert_eq!(
            serde_json::to_value(&report).unwrap(),
            serde_json::json!({ "label": "submit_interrupted", "outcome": { "outcome": "cancelled" }, "status": "cancelled" })
        );
        for (outcome, want) in [
            (
                Outcome::Failed { reason: FailReason::NonZeroExit { code: 1 } },
                serde_json::json!({ "outcome": "failed", "reason": { "reason": "non_zero_exit", "code": 1 } }),
            ),
            (
                Outcome::Failed { reason: FailReason::NoNormalTermination },
                serde_json::json!({ "outcome": "failed", "reason": { "reason": "no_normal_termination" } }),
            ),
            (Outcome::Completed { late_cancel: true }, serde_json::json!({ "outcome": "completed", "late_cancel": true })),
            (Outcome::Lost { orphans: vec![41, 42] }, serde_json::json!({ "outcome": "lost", "orphans": [41, 42] })),
            (Outcome::Cancelling, serde_json::json!({ "outcome": "cancelling" })),
            (Outcome::ReEnqueue, serde_json::json!({ "outcome": "re_enqueue" })),
        ] {
            assert_eq!(serde_json::to_value(&outcome).unwrap(), want, "{outcome:?}");
        }
    }

    #[test]
    fn the_label_report_wire_shape() {
        use crate::remote::submit::{Label, LabelFacts, Markers};
        let report = LabelReport {
            facts: LabelFacts {
                dir_exists: true,
                markers: Markers { started: false, exit_code: false, cancelled: false, enqueued: false, submitting: true },
                row_holds_job: false,
                socket_error: false,
            },
            label: Label::SubmitInterrupted,
        };
        assert_eq!(
            serde_json::to_value(&report).unwrap(),
            serde_json::json!({
                "facts": {
                    "dir_exists": true,
                    "markers": { "started": false, "exit_code": false, "cancelled": false, "enqueued": false, "submitting": true },
                    "row_holds_job": false,
                    "socket_error": false
                },
                "label": "submit_interrupted"
            })
        );
        assert_eq!(serde_json::to_value(Label::NotOnServer).unwrap(), "not_on_server");
        assert_eq!(serde_json::to_value(Label::Classifier).unwrap(), "classifier");
    }
}
