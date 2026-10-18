//! The evidence fetch (ADR-024 o22.6, o22.13.5) over the delivery tests' fake server: what is
//! downloaded, when the terminal status is written, and the strikes.

use std::cell::Cell;

use crate::models::job::JobStatus;
use crate::poller::cancel::tests::{deliver, request, server, waiting, Call::*, Fake};
use crate::poller::plan::MAX_FETCH_STRIKES;
use crate::poller::remote_state::Known;
use crate::poller::tests::{pending_cancel, record, Remote, STARTED_EARLIER_BOOT};
use crate::poller::{Event, StatusStep};
use crate::remote::classify::{FailReason, Outcome};
use crate::ssh_backend::tests::{exited, snapshot, JOB};

/// Row 4 with `.started`: `.cancelled`, a `.started` of an earlier boot, nothing runs. `exit_code`
/// and `tail` as the server has them.
fn cancelled_after_start(exit_code: Option<&str>, tail: Option<&str>) -> Vec<u8> {
    snapshot(true, exit_code, tail, (Some(STARTED_EARLIER_BOOT), Some(STARTED_EARLIER_BOOT)))
}

/// Row 1 with `.cancelled`: a `.started` that does not parse.
fn corrupt_and_cancelled() -> Vec<u8> {
    snapshot(true, None, None, (Some("garbage\n"), Some("garbage\n")))
}

/// A `running` job that ran and was cancelled: its partial output, stderr and markers on the server.
fn ran_and_cancelled() -> Remote {
    let r = Remote::new();
    r.w.sql("UPDATE jobs SET status = 'running'");
    let s = r.server();
    std::fs::write(s.join("output.out"), "line 1\nSCF ITERATIONS\n").unwrap();
    std::fs::write(s.join("stderr.log"), "").unwrap();
    std::fs::write(s.join(".started"), STARTED_EARLIER_BOOT).unwrap();
    std::fs::write(s.join(".cancelled"), "").unwrap();
    r
}

/// o22.6 / o22.13.5 / control: row 4 with `.started` is downloaded and listed, **then** written
/// `cancelled` (the row is still live while the LIST check runs), then the watched view is drained
/// from the copy before the terminal `job:status`. NEGATIVE CONTROL: write the terminal status
/// before `fetch_remote` → red (the row is `cancelled` at the LIST call).
#[test]
fn a_cancelled_job_that_ran_is_downloaded_listed_then_cancelled_then_drained() {
    let r = ran_and_cancelled();
    r.live.open(JOB, &r.sink);
    let status_at_list = Cell::new(None);
    let base = server(&r, vec![cancelled_after_start(None, None)]);
    let fake = Fake::new(&r, |call, values| {
        if call == List {
            status_at_list.set(Some(r.w.job().status));
        }
        base(call, values)
    });
    assert_eq!(r.poller(&fake).status_step(JOB).unwrap(), StatusStep::Cancelled);
    assert_eq!(fake.calls(), [Collect, Download, List]);
    assert_eq!(status_at_list.get(), Some(JobStatus::Running), "the terminal write waits for the LIST check");
    let job = r.w.job();
    assert_eq!(job.status, JobStatus::Cancelled);
    assert!(job.error_message.unwrap().contains("its files were downloaded as evidence"));
    assert!(r.local().join("output.out").exists(), "the evidence came down");
    let events = r.sink.take();
    let lines: Vec<String> = events.iter().flat_map(|e| match e { Event::Log(_, l) => l.clone(), _ => vec![] }).collect();
    assert_eq!(lines, ["line 1", "SCF ITERATIONS"], "drained from the copy");
    assert_eq!(events.last(), Some(&Event::Status(JOB.into(), JobStatus::Cancelled)), "the status last");
    assert_eq!(r.memory.get(JOB), Default::default());
    assert_eq!(record(&r), None);
}

/// o22.13.5 / control (rule #6): the evidence path never decides from the files — a copy holding
/// `.exit_code` 0 without `ORCA TERMINATED NORMALLY` still ends `cancelled`, with no energy and no
/// results. NEGATIVE CONTROL: route the evidence through `fetch_and_finalise` (`detect_completion`)
/// → red (`failed`).
#[test]
fn evidence_is_never_decided_by_detect_completion() {
    let r = ran_and_cancelled();
    std::fs::write(r.server().join(".exit_code"), "0\n").unwrap();
    let fake = Fake::new(&r, server(&r, vec![cancelled_after_start(Some("0\n"), Some("SCF NOT CONVERGED\n"))]));
    assert_eq!(r.poller(&fake).status_step(JOB).unwrap(), StatusStep::Cancelled);
    let job = r.w.job();
    assert_eq!((job.status, job.energy), (JobStatus::Cancelled, None));
}

/// o22.6 / o22.9 control: row 1 with `.cancelled` takes the evidence path and ends `failed` with
/// "processes of the job may survive on the server" — decided by the snapshot, so a click's
/// delivery that leaves it is finished by the poller's next ordinary step. NEGATIVE CONTROL: key the
/// evidence on the path that delivered instead of the snapshot (send `CorruptStarted` back to the
/// shown arm) → red (the row stays `running`).
#[test]
fn row_1_with_cancelled_is_fetched_and_ends_failed_at_the_next_ordinary_step() {
    let r = ran_and_cancelled();
    request(&r);
    let fake = Fake::new(&r, server(&r, vec![waiting(), corrupt_and_cancelled()]));
    assert!(matches!(deliver(&r, &fake), StatusStep::CancelDelivered(Outcome::Failed { .. })), "the click's delivery");
    assert_eq!(r.w.job().status, JobStatus::Running, "delivery writes no terminal status");
    let step = r.poller(&fake).status_step(JOB).unwrap();
    let StatusStep::Finalised { outcome, status, drain_error: None } = &step else { panic!("{step:?}") };
    assert!(matches!(outcome, Outcome::Failed { reason: FailReason::CorruptStarted { cancelled: true, .. } }), "{outcome:?}");
    assert_eq!(*status, JobStatus::Failed);
    assert!(fake.calls().ends_with(&[Collect, Download, List]), "{:?}", fake.calls());
    let message = r.row().1.unwrap();
    assert!(message.contains("processes of the job may survive on the server"), "{message}");
    assert!(message.contains("downloaded as evidence"), "{message}");
}

/// A listing that disagrees with the copy writes nothing terminal: one strike with the evidence
/// text, the row still `running`, the record `Fetching`.
#[test]
fn a_failed_evidence_listing_counts_a_strike_and_writes_nothing_terminal() {
    let r = ran_and_cancelled();
    let base = server(&r, vec![cancelled_after_start(None, None)]);
    let fake = Fake::new(&r, |call, values| {
        let reply = base(call, values);
        if call == Download {
            std::fs::write(r.local().join("output.out"), "tampered\n").unwrap();
        }
        reply
    });
    let step = r.poller(&fake).status_step(JOB).unwrap();
    assert!(matches!(&step, StatusStep::FetchFailed { strikes: 1, reason } if reason.contains("differing")), "{step:?}");
    let (status, message) = r.row();
    assert_eq!(status, JobStatus::Running);
    assert!(message.unwrap().contains("as evidence failed (attempt 1 of 3)"));
    assert!(r.memory.get(JOB).fetching, "the live log stops");
    assert_eq!(record(&r).unwrap().known, Some(Known::Fetching));
}

/// o22.6 / o22.13.5 / control: the `MAX_FETCH_STRIKES`-th failed evidence fetch writes the terminal
/// status itself — "evidence not downloaded: <why>; it stays on the server" — and forgets the job,
/// so `FetchStopped` is never reached; row 4 → `cancelled`, row 1 → `failed`. NEGATIVE CONTROL: take
/// the last strike like any other (no terminal write) → red (the row stays `running`, then
/// `FetchStopped`).
#[test]
fn the_last_evidence_strike_writes_the_terminal_status_itself() {
    for (snap, want) in [(cancelled_after_start(None, None), JobStatus::Cancelled), (corrupt_and_cancelled(), JobStatus::Failed)] {
        let r = ran_and_cancelled();
        let base = server(&r, vec![snap]);
        let fake = Fake::new(&r, |call, values| if call == Download { exited(23, vec![]) } else { base(call, values) });
        let poller = r.poller(&fake);
        for n in 1..MAX_FETCH_STRIKES {
            assert!(matches!(poller.status_step(JOB).unwrap(), StatusStep::FetchFailed { strikes, .. } if strikes == n));
        }
        let last = poller.status_step(JOB).unwrap();
        match want {
            JobStatus::Cancelled => assert_eq!(last, StatusStep::Cancelled),
            _ => assert!(
                matches!(&last, StatusStep::Finalised { status: JobStatus::Failed, outcome: Outcome::Failed { reason: FailReason::CorruptStarted { cancelled: true, .. } }, .. }),
                "{last:?}"
            ),
        }
        let (status, message) = r.row();
        assert_eq!(status, want);
        let message = message.unwrap();
        assert!(message.contains("evidence not downloaded: download: rsync exited with 23") && message.contains("it stays on the server"), "{message}");
        assert_eq!(r.memory.get(JOB), Default::default(), "forgotten");
        assert_eq!(poller.status_step(JOB).unwrap(), StatusStep::NotLive, "never FetchStopped");
    }
}

/// o22.13.5 / control: the last evidence strike whose terminal write finds the row changed — here
/// another writer moved the job to a new remote dir, so the row is still live — forgets the job
/// anyway, so its next step is not stuck at `FetchStopped` on stale memory. NEGATIVE CONTROL: count
/// the strike and forget only after a successful write → red (memory at the maximum with the
/// fetching flag).
#[test]
fn the_last_evidence_strike_on_a_changed_row_forgets_the_job() {
    let r = ran_and_cancelled();
    r.memory.set_fetching(JOB);
    for _ in 1..MAX_FETCH_STRIKES {
        r.memory.strike(JOB);
    }
    let base = server(&r, vec![cancelled_after_start(None, None)]);
    let fake = Fake::new(&r, |call, values| {
        if call == Download {
            r.w.sql("UPDATE jobs SET remote_job_dir = remote_job_dir || '-moved'");
            return exited(23, vec![]);
        }
        base(call, values)
    });
    assert_eq!(r.poller(&fake).status_step(JOB).unwrap(), StatusStep::RowChanged);
    assert_eq!(r.w.job().status, JobStatus::Running, "the changed row is not written");
    assert_eq!(r.memory.get(JOB), Default::default(), "forgotten although the write found the row changed");
}

/// o22.2, each new terminal write site with the pending-cancel columns set: a request (and its mark)
/// recorded while the evidence downloads is cleared by the terminal write — row 4's
/// (`mark_cancelled_conn`), row 1's (`finalize_job_conn`), and the last strike's of each. NEGATIVE
/// CONTROLS: drop the NULLs from `mark_cancelled_conn` / `finalize_job_conn` → red (the v21 CHECK
/// fails the write: an `Err`).
#[test]
fn every_evidence_terminal_write_clears_a_pending_cancel() {
    let snaps = [cancelled_after_start(None, None), corrupt_and_cancelled()];
    for (i, snap) in snaps.into_iter().enumerate() {
        for failing in [false, true] {
            let r = ran_and_cancelled();
            if failing {
                r.memory.set_fetching(JOB);
                for _ in 1..MAX_FETCH_STRIKES {
                    r.memory.strike(JOB);
                }
            }
            let base = server(&r, vec![snap.clone()]);
            let fake = Fake::new(&r, |call, values| {
                if call == Download {
                    r.w.sql("UPDATE jobs SET cancel_requested_at = '2026-10-18 10:00:00', cancel_sent_at = '2026-10-18 10:00:01'");
                    if failing {
                        return exited(23, vec![]);
                    }
                }
                base(call, values)
            });
            let step = r.poller(&fake).status_step(JOB);
            assert!(step.is_ok(), "snapshot {i}, failing {failing}: {step:?}");
            assert!(matches!(r.w.job().status, JobStatus::Cancelled | JobStatus::Failed), "snapshot {i}, failing {failing}");
            assert_eq!(pending_cancel(&r), (None, None), "snapshot {i}, failing {failing}");
        }
    }
}
