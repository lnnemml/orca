//! **The evidence fetch** before a terminal write the server's files must back (ADR-024 o22.6,
//! o22.13.5, o25.2, o25.5, o25.8), keyed on the snapshot — never on the path that delivered a
//! cancel:
//! - row 4 `Cancelled { started: true }` → `cancelled`;
//! - row 1 `CorruptStarted` → `failed`, with `.cancelled` or without it (o25.8: the wrapper's own
//!   check makes it unlikely ORCA ran, and without `sid` nothing can be swept);
//! - row 8 `Lost` once its orphans are swept (or the boot is stale) → `lost` (o25.2);
//! - row 11's second never-start `Failed { WrapperNeverStarted }` → `failed`, its `.tsp-out/` and
//!   markers fetched (o25.5).
//!
//! 1. the **fetching flag** is set (the live log stops, o16.6);
//! 2. **`fetch_remote`**: rsync down with the shared filter, then the server's listing must equal
//!    the local copy, hash for hash;
//! 3. only then the **terminal write**: row 4 `cancelled` (`mark_cancelled_conn`), row 1 with
//!    `.cancelled` `failed` with "processes of the job may survive on the server" through `finalise`
//!    (its live-row re-check); the 5.4b kinds through [`finalise_evidence`], which also says so when a
//!    cancel recorded during the fetch is dropped by the write (o25.2). All NULL the pending-cancel
//!    columns (o22.2);
//! 4. the live view is **drained** from the local copy, then `job:status`.
//!
//! **`detect_completion` decides nothing and nothing is parsed**: the files are for the log and file
//! views, and a copy that holds `.exit_code` 0 still ends `cancelled` (rule #6: the evidence path
//! never claims a completion it did not verify). The guarantee is weaker than o18's and stated as
//! such: the copy equals the server's listing at the moment of the LIST check; a process the sweep
//! could not kill (row 1) may still write afterwards.
//!
//! **A failed evidence fetch** counts a strike with its own text (`FetchFailed`). The step that takes
//! the [`MAX_FETCH_STRIKES`]-th strike writes the terminal status itself — "evidence not downloaded:
//! <why>; it stays on the server" — and forgets the job, also when that write finds the row changed,
//! so `FetchStopped` is never reached for an evidence job and no job is left non-terminal waiting for
//! a manual retry that o22.8 may disable.

use std::path::PathBuf;

use rusqlite::params;

use super::plan::MAX_FETCH_STRIKES;
use super::{finalise, live_row, Poller, StatusStep, Written, LIVE};
use crate::commands::jobs::finalize_job_conn;
use crate::commands::settings::DbState;
use crate::error::AppError;
use crate::models::job::{Job, JobStatus};
use crate::remote::classify::{FailReason, Outcome};
use crate::ssh_backend::{fetch_remote, RemoteCoordinates};

/// Which evidence verdict a step saw.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Evidence {
    /// Row 4 with `.started`: it ran, and nothing of it runs now → `cancelled`.
    Cancelled,
    /// Row 1: without `sid` the job cannot be swept → `failed`. `cancelled`: a cancel was delivered
    /// (o22.4.5); without it, the wrapper's own check of `.started` makes it unlikely ORCA ran (o25.8).
    CorruptStarted { detail: String, cancelled: bool },
    /// Row 8, its orphans swept or none (o25.2) → `lost`. `survivors`: what 3 sweeps without progress
    /// left (the evidence is weaker then).
    Lost { stale_boot: bool, survivors: Vec<u32> },
    /// Row 11 after the one re-enqueue (o25.5) → `failed`.
    NeverStarted,
}

/// The note a terminal write adds when a cancel was recorded while the job was being finalised: the
/// job had already stopped, so the cancel is moot and the write NULLs its columns (o25.2).
pub const DROPPED_CANCEL_NOTE: &str = "A pending cancel was dropped: the job had already stopped.";

impl Evidence {
    /// The evidence verdict `outcome` is, if any — every outcome listed, no wildcard (o25.1). Row 4
    /// without `.started` is not one: nothing ran, so nothing is downloaded. A `Lost` with orphans is
    /// not one yet: they are swept first (`lost.rs`).
    pub(super) fn of(outcome: &Outcome) -> Option<Evidence> {
        match outcome {
            Outcome::Cancelled { started: true } => Some(Evidence::Cancelled),
            Outcome::Cancelled { started: false } => None,
            Outcome::Failed { reason: FailReason::CorruptStarted { detail, cancelled } } => {
                Some(Evidence::CorruptStarted { detail: detail.clone(), cancelled: *cancelled })
            }
            Outcome::Failed { reason: FailReason::WrapperNeverStarted } => Some(Evidence::NeverStarted),
            Outcome::Failed { reason: FailReason::NonZeroExit { .. } | FailReason::BadExitCode { .. } | FailReason::NoNormalTermination } => None,
            Outcome::Lost { orphans, stale_boot } if orphans.is_empty() => {
                Some(Evidence::Lost { stale_boot: *stale_boot, survivors: Vec::new() })
            }
            Outcome::Lost { .. } => None,
            Outcome::Queued
            | Outcome::Running
            | Outcome::Completed { .. }
            | Outcome::Cancelling { .. }
            | Outcome::Indeterminate { .. }
            | Outcome::ReEnqueue => None,
        }
    }

    /// The row's terminal text: downloaded as evidence, or `not_downloaded` with why.
    fn message(&self, not_downloaded: Option<&str>) -> String {
        let files = match not_downloaded {
            None => "its files were downloaded as evidence".to_string(),
            Some(why) => format!("evidence not downloaded: {why}; it stays on the server"),
        };
        match self {
            Evidence::Cancelled => format!("Cancelled: the server confirms .cancelled and nothing of the job runs; {files}."),
            Evidence::CorruptStarted { detail, cancelled: true } => format!(
                "Failed after the cancel: corrupt .started marker ({detail}); without its sid the job cannot be swept, so \
                 processes of the job may survive on the server; {files}."
            ),
            Evidence::CorruptStarted { detail, cancelled: false } => format!(
                "Failed: corrupt .started marker ({detail}); the wrapper checks its own .started before it starts ORCA, so \
                 ORCA is unlikely to have run, and without its sid nothing of the job can be swept; {files}."
            ),
            Evidence::Lost { stale_boot, survivors } => {
                let cause = if *stale_boot {
                    "the server restarted while the job ran"
                } else {
                    "the job's wrapper died without an exit code"
                };
                let survived = if survivors.is_empty() {
                    String::new()
                } else {
                    let pids: Vec<String> = survivors.iter().map(u32::to_string).collect();
                    format!(
                        " Processes of the job still ran after 3 sweeps (pid {}); they hold its cores and may have written \
                         after its files were listed.",
                        pids.join(", ")
                    )
                };
                format!("Lost: {cause}; {files}.{survived}")
            }
            Evidence::NeverStarted => format!(
                "Failed: the job never started on the server after one re-enqueue (or the re-enqueue did not reach the \
                 queue); see .tsp-out; {files}."
            ),
        }
    }

    /// The text of a failed evidence download (a strike below the bound).
    fn strike_message(&self, strikes: u32, reason: &str) -> String {
        let whose = match self {
            Evidence::Cancelled | Evidence::CorruptStarted { cancelled: true, .. } => "the cancelled job's files",
            Evidence::CorruptStarted { cancelled: false, .. } | Evidence::Lost { .. } | Evidence::NeverStarted => "the job's files",
        };
        format!(
            "Downloading {whose} as evidence failed (attempt {strikes} of {MAX_FETCH_STRIKES}); the next check retries, and \
             the last attempt ends the job without them: {reason}"
        )
    }
}

/// The terminal write of a 5.4b evidence kind (o25.2, o25.5, o25.8) in **one** locked transaction:
/// written only if the row is still this live remote job; a cancel recorded since the step's read
/// (the job had already stopped) is noted in the message — [`DROPPED_CANCEL_NOTE`] — and its columns
/// are NULLed by `finalize_job_conn` with the pending submit's. `None` when the row changed.
fn finalise_evidence(db: &DbState, job_id: &str, coords: &RemoteCoordinates, status: JobStatus, message: &str) -> Result<Option<JobStatus>, AppError> {
    let conn = db.lock()?;
    let tx = conn.unchecked_transaction()?;
    if live_row(&tx, job_id, coords)?.is_none() {
        return Ok(None);
    }
    let requested: Option<String> =
        tx.query_row("SELECT cancel_requested_at FROM jobs WHERE id = ?1", params![job_id], |row| row.get(0))?;
    let message = match requested {
        Some(_) => format!("{message} {DROPPED_CANCEL_NOTE}"),
        None => message.to_string(),
    };
    finalize_job_conn(&tx, job_id, status, Some(&message))?;
    tx.commit()?;
    Ok(Some(status))
}

impl Poller<'_> {
    /// The evidence fetch of `job` (the module doc); `outcome` is the classify's verdict.
    pub(super) fn fetch_evidence(
        &self,
        job: &Job,
        coords: &RemoteCoordinates,
        evidence: Evidence,
        outcome: Outcome,
    ) -> Result<StatusStep, AppError> {
        self.memory.set_fetching(&job.id);
        let fetched = match &job.job_dir {
            None => Err("the job has no local job dir to download into".to_string()),
            Some(dir) => fetch_remote(self.runner, coords, std::path::Path::new(dir), self.policy).map(|()| PathBuf::from(dir)),
        };
        let local_dir = match fetched {
            Ok(dir) => dir,
            Err(reason) => return self.evidence_strike(job, coords, &evidence, outcome, reason),
        };

        let Some(status) = self.write_evidence(job, coords, &evidence, None)? else {
            return Ok(StatusStep::RowChanged);
        };
        self.memory.forget(&job.id);
        let drain_error = self.drain(&job.id, &local_dir.join("output.out")).err().map(|e| e.to_string());
        self.live.drop_state(&job.id);
        self.sink.status(&job.id, status);
        Ok(match status {
            JobStatus::Cancelled => {
                // `Cancelled` carries no drain error; the row is final either way, so say it here.
                if let Some(e) = drain_error {
                    eprintln!("[OrcaStudio] remote poller: job {}: the live view could not be drained from the evidence: {e}", job.id);
                }
                StatusStep::Cancelled
            }
            // Every other status listed (o25.1): `failed` and `lost` are what the evidence kinds write;
            // the rest are never written here, and would be reported as written.
            status @ (JobStatus::Failed
            | JobStatus::Lost
            | JobStatus::Draft
            | JobStatus::Queued
            | JobStatus::Running
            | JobStatus::Cancelling
            | JobStatus::Completed
            | JobStatus::Parsed) => StatusStep::Finalised { outcome, status, drain_error },
        })
    }

    /// The terminal write of an evidence verdict, guarded on the row still being this live remote
    /// job: the status written, or `None` when the row changed.
    fn write_evidence(
        &self,
        job: &Job,
        coords: &RemoteCoordinates,
        evidence: &Evidence,
        not_downloaded: Option<&str>,
    ) -> Result<Option<JobStatus>, AppError> {
        let message = evidence.message(not_downloaded);
        match evidence {
            Evidence::Cancelled => {
                Ok(self.cancel_row(&job.id, coords, &LIVE, &message)?.then_some(JobStatus::Cancelled))
            }
            Evidence::CorruptStarted { cancelled: true, .. } => {
                let note = self.ended_first_note(&job.id);
                finalise(self.db, &job.id, coords, JobStatus::Failed, Some(&message), None, None, note)
            }
            Evidence::CorruptStarted { cancelled: false, .. } | Evidence::NeverStarted => {
                finalise_evidence(self.db, &job.id, coords, JobStatus::Failed, &message)
            }
            Evidence::Lost { .. } => finalise_evidence(self.db, &job.id, coords, JobStatus::Lost, &message),
        }
    }

    /// A failed evidence fetch: a strike with its own text; the [`MAX_FETCH_STRIKES`]-th writes the
    /// terminal status itself and forgets the job whatever that write found.
    fn evidence_strike(
        &self,
        job: &Job,
        coords: &RemoteCoordinates,
        evidence: &Evidence,
        outcome: Outcome,
        reason: String,
    ) -> Result<StatusStep, AppError> {
        let strikes = self.memory.get(&job.id).strikes + 1;
        if strikes >= MAX_FETCH_STRIKES {
            let written = self.write_evidence(job, coords, evidence, Some(&reason))?;
            // Forgotten even when the row changed: an evidence job never reaches `FetchStopped`.
            self.memory.forget(&job.id);
            let Some(status) = written else {
                return Ok(StatusStep::RowChanged);
            };
            self.live.drop_state(&job.id);
            self.sink.status(&job.id, status);
            return Ok(match status {
                JobStatus::Cancelled => StatusStep::Cancelled,
                status @ (JobStatus::Failed
                | JobStatus::Lost
                | JobStatus::Draft
                | JobStatus::Queued
                | JobStatus::Running
                | JobStatus::Cancelling
                | JobStatus::Completed
                | JobStatus::Parsed) => StatusStep::Finalised { outcome, status, drain_error: None },
            });
        }
        let message = evidence.strike_message(strikes, &reason);
        match self.write_message(&job.id, coords, Some(&message))? {
            Written::No => return Ok(StatusStep::RowChanged),
            Written::Yes(emit) => {
                self.memory.strike(&job.id);
                if let Some(status) = emit {
                    self.sink.status(&job.id, status);
                }
            }
        }
        Ok(StatusStep::FetchFailed { strikes, reason })
    }
}

#[cfg(test)]
mod tests;
