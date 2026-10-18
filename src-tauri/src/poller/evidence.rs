//! **The evidence fetch** of a cancelled job that ran (ADR-024 o22.6, o22.13.5): row 4
//! `Cancelled { started: true }` and row 1 `CorruptStarted { cancelled: true }`, whichever step saw
//! them — keyed on the snapshot, never on the path that delivered the cancel.
//!
//! 1. the **fetching flag** is set (the live log stops, o16.6);
//! 2. **`fetch_remote`**: rsync down with the shared filter, then the server's listing must equal
//!    the local copy, hash for hash;
//! 3. only then the **terminal write**: row 4 `cancelled` (`mark_cancelled_conn`), row 1 `failed`
//!    with "processes of the job may survive on the server" through `finalise` (its live-row
//!    re-check). Both NULL the pending-cancel columns (o22.2);
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

use super::plan::MAX_FETCH_STRIKES;
use super::{finalise, Poller, StatusStep, Written, LIVE};
use crate::error::AppError;
use crate::models::job::{Job, JobStatus};
use crate::remote::classify::{FailReason, Outcome};
use crate::ssh_backend::{fetch_remote, RemoteCoordinates};

/// Which evidence verdict a step saw.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Evidence {
    /// Row 4 with `.started`: it ran, and nothing of it runs now → `cancelled`.
    Cancelled,
    /// Row 1 with `.cancelled`: without `sid` the job cannot be swept → `failed`.
    CorruptStarted { detail: String },
}

impl Evidence {
    /// The evidence verdict `outcome` is, if any. Row 4 without `.started` is not one: nothing ran,
    /// so nothing is downloaded.
    pub(super) fn of(outcome: &Outcome) -> Option<Evidence> {
        match outcome {
            Outcome::Cancelled { started: true } => Some(Evidence::Cancelled),
            Outcome::Failed { reason: FailReason::CorruptStarted { detail, cancelled: true } } => {
                Some(Evidence::CorruptStarted { detail: detail.clone() })
            }
            _ => None,
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
            Evidence::CorruptStarted { detail } => format!(
                "Failed after the cancel: corrupt .started marker ({detail}); without its sid the job cannot be swept, so \
                 processes of the job may survive on the server; {files}."
            ),
        }
    }
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
            status => StatusStep::Finalised { outcome, status, drain_error },
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
            Evidence::CorruptStarted { .. } => {
                let note = self.ended_first_note(&job.id);
                finalise(self.db, &job.id, coords, JobStatus::Failed, Some(&message), None, None, note)
            }
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
                status => StatusStep::Finalised { outcome, status, drain_error: None },
            });
        }
        let message = format!(
            "Downloading the cancelled job's files as evidence failed (attempt {strikes} of {MAX_FETCH_STRIKES}); the \
             next check retries, and the last attempt ends the job without them: {reason}"
        );
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
