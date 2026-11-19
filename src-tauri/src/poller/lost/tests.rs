//! A `Lost` job (ADR-024 o25.1–2) and the other 5.4b evidence verdicts (o25.5's second never-start
//! is in `reenqueue/tests.rs`; row 1 without `.cancelled`, o25.8, is here) over the 5.4b fake server
//! (`poller::fake_tests`). Each test names the o25.11 / o25.14.7 gate it is; its negative control is
//! a source mutation in `run_54b_controls.sh`, named at the test.

use std::cell::Cell;

use crate::models::job::JobStatus;
use crate::poller::evidence::DROPPED_CANCEL_NOTE;
use crate::poller::fake_tests::{lost_in_this_boot, orphans, server, Call::*, Fake, ID};
use crate::poller::lost::lost_sweep_message;
use crate::poller::plan::{candidates, MAX_FETCH_STRIKES, MAX_SWEEPS_WITHOUT_PROGRESS};
use crate::poller::remote_state::{CheckError, Known};
use crate::poller::tests::{lost, pending_cancel, record, Remote, STARTED_EARLIER_BOOT};
use crate::poller::{Event, StatusStep};
use crate::remote::classify::{FailReason, Outcome};
use crate::ssh_backend::tests::{exited, ran, snapshot};

/// The server's job dir of a job that started and stopped without an `.exit_code`: a partial
/// output, `.started`, the submit's markers.
fn started_on_server(r: &Remote) {
    let s = r.server();
    std::fs::write(s.join("output.out"), "SCF ITERATIONS\npartial\n").unwrap();
    std::fs::write(s.join(".started"), STARTED_EARLIER_BOOT).unwrap();
    std::fs::write(s.join(".enqueued"), format!("socket={}\nid=0\n", crate::ssh_backend::tests::expected_coords().socket)).unwrap();
    std::os::unix::fs::symlink("x", s.join(".submitting")).unwrap();
}

fn job(r: &Remote) -> crate::models::job::Job {
    r.w.job()
}

/// o25.1–2, o25.11 (Lost, stale boot) and o25.14.7 ("`Lost` reached from a `queued` row"): a job the
/// server restarted under is fetched as evidence (download + listing, no sweep, no ready) and
/// written terminal `lost` with the restart's text, `completed_at` stamped, every pending column
/// NULL; it is no candidate any more, and the next step finds it not live. NEGATIVE CONTROLS
/// (`run_54b_controls.sh`): `lost-no-terminal-write` (the evidence write writes nothing → the row
/// stays queued, red); `lost-text-swapped` (the two causes swapped → red).
#[test]
fn a_stale_boot_lost_is_fetched_as_evidence_and_written_lost() {
    let r = Remote::new();
    started_on_server(&r);
    let fake = Fake::new(&r, server(&r, vec![lost()]));
    let poller = r.poller(&fake);
    let step = poller.status_step(ID).unwrap();
    assert_eq!(
        step,
        StatusStep::Finalised { outcome: Outcome::Lost { orphans: vec![], stale_boot: true }, status: JobStatus::Lost, drain_error: None }
    );
    assert_eq!(fake.calls(), [Label, Collect, Download, List], "label (a queued row), collect, evidence; no sweep");
    let j = job(&r);
    assert_eq!(j.status, JobStatus::Lost);
    assert!(j.completed_at.is_some(), "a terminal write stamps completed_at");
    assert_eq!(
        j.error_message.as_deref(),
        Some("Lost: the server restarted while the job ran; its files were downloaded as evidence.")
    );
    assert_eq!(pending_cancel(&r), (None, None));
    assert_eq!(j.submit_pending_since, None);
    assert_eq!(std::fs::read_to_string(r.local().join("output.out")).unwrap(), "SCF ITERATIONS\npartial\n", "the evidence came down");
    assert_eq!(r.sink.events(), [Event::Status(ID.into(), JobStatus::Lost)]);
    assert_eq!(r.memory.get(ID), Default::default(), "forgotten once terminal");
    assert!(candidates(&r.w.db.lock().unwrap()).unwrap().jobs.is_empty(), "lost is live for no predicate");
    assert_eq!(poller.status_step(ID).unwrap(), StatusStep::NotLive, "not polled again");
    assert_eq!(fake.calls().len(), 4, "no call for a terminal row");
}

/// o25.2, o25.5, o25.8 (5.4b F-B): the terminal write of an evidence kind finds the row changed while
/// the evidence downloaded — moved to a new remote dir (still live), or turned terminal by another
/// writer — and writes **nothing**: `RowChanged`, the row's status and text as the other writer left
/// them, nothing emitted. NEGATIVE CONTROL (`run_p5fu_controls.sh`, `finalise-evidence-no-live-row`):
/// the live-row re-check removed from `finalise_evidence` → the `Lost` overwrites the row, red.
#[test]
fn an_evidence_write_on_a_row_that_changed_during_the_download_writes_nothing() {
    for (change, status, message) in [
        ("UPDATE jobs SET remote_job_dir = remote_job_dir || '-moved'", JobStatus::Queued, None),
        ("UPDATE jobs SET status = 'cancelled', error_message = 'someone else'", JobStatus::Cancelled, Some("someone else")),
    ] {
        let r = Remote::new();
        started_on_server(&r);
        let base = server(&r, vec![lost()]);
        let fake = Fake::new(&r, |call, values| {
            if call == Download {
                r.w.sql(change);
            }
            base(call, values)
        });
        assert_eq!(r.poller(&fake).status_step(ID).unwrap(), StatusStep::RowChanged, "{change}");
        let j = job(&r);
        assert_eq!(j.status, status, "{change}");
        assert_eq!(j.error_message.as_deref(), message, "{change}");
        assert_eq!(r.sink.events(), [], "{change}: no status event");
    }
}

/// o25.1, o25.11 (Status: "`lost` is not live in any listed predicate"): a remote `lost` row is no
/// poller candidate, not `is_live` for the poller's writes, not remote-live (the delete and the
/// cancel core's routing), not a live job of its profile (edits and deletes of the profile), and a
/// cancel records nothing on it — every one a positive list. NEGATIVE CONTROL
/// (`candidates-lost`): `lost` added to the candidates' SQL → red.
#[test]
fn a_lost_row_is_live_for_no_predicate() {
    let r = Remote::new();
    r.w.sql("UPDATE jobs SET status = 'lost', completed_at = datetime('now')");
    let j = job(&r);
    assert!(j.is_remote());
    let conn = r.w.db.lock().unwrap();
    assert!(candidates(&conn).unwrap().jobs.is_empty() && candidates(&conn).unwrap().unreadable.is_empty(), "no candidate");
    assert!(crate::commands::server_profiles::live_remote_jobs(&conn, "p1").unwrap().is_empty(), "no live job of its profile");
    drop(conn);
    assert!(!crate::poller::is_live(JobStatus::Lost), "the poller's own writes never touch it");
    assert!(!crate::ssh_backend::is_remote_live(&j), "not remote-live");
    assert!(crate::ssh_backend::refuse_if_remote_live(&j).is_ok(), "deletable like any terminal row");
    let intent = crate::commands::jobs::record_intent(&r.w.db, ID).unwrap();
    assert_eq!(intent, crate::commands::jobs::Intent::MovedOn(JobStatus::Lost), "a cancel records nothing");
    assert_eq!(pending_cancel(&r), (None, None));
}

/// o25.2, o25.11 (Lost, same boot): orphans in this boot are swept first — ready, `cancel.sh
/// sweep`, collect — and only an empty session is finalised, `lost` with the wrapper's text.
/// NEGATIVE CONTROL (`lost-finalise-before-sweep`): every `Lost` treated as evidence at once → red
/// (no sweep before the download).
#[test]
fn same_boot_orphans_are_swept_before_the_job_is_written_lost() {
    let r = Remote::new();
    started_on_server(&r);
    let fake = Fake::new(&r, server(&r, vec![lost_in_this_boot(2), lost_in_this_boot(0)]));
    let step = r.poller(&fake).status_step(ID).unwrap();
    assert_eq!(
        step,
        StatusStep::Finalised { outcome: Outcome::Lost { orphans: vec![], stale_boot: false }, status: JobStatus::Lost, drain_error: None }
    );
    assert_eq!(fake.calls(), [Label, Collect, Prepare, Sweep, Collect, Download, List], "ready, sweep, re-classify, then evidence");
    let j = job(&r);
    assert_eq!(j.status, JobStatus::Lost);
    assert_eq!(
        j.error_message.as_deref(),
        Some("Lost: the job's wrapper died without an exit code; its files were downloaded as evidence.")
    );
}

/// o25.2: a session that does not shrink is swept at most 3 times (o22.13.4's rule over the lost
/// counter); the 3rd writes `lost` anyway, naming the survivors and saying the evidence is weaker.
/// Each step before it shows the sweep in the row and the record. NEGATIVE CONTROL
/// (`lost-unbounded`): the bound never reached → red (a 4th sweep).
#[test]
fn three_sweeps_without_progress_write_lost_naming_the_survivors() {
    let r = Remote::new();
    started_on_server(&r);
    let fake = Fake::new(&r, server(&r, vec![lost_in_this_boot(1)]));
    let poller = r.poller(&fake);
    for n in 1..MAX_SWEEPS_WITHOUT_PROGRESS {
        let step = poller.status_step(ID).unwrap();
        assert_eq!(step, StatusStep::LostSwept { outcome: Some(Outcome::Lost { orphans: orphans(1), stale_boot: false }), error: None }, "sweep {n}");
        assert_eq!(r.memory.get(ID).lost_sweeps, n);
        assert_eq!((job(&r).status, job(&r).error_message), (JobStatus::Queued, Some(lost_sweep_message(1, n))));
        assert_eq!(record(&r).unwrap().known, Some(Known::Shown(Outcome::Lost { orphans: orphans(1), stale_boot: false })));
    }
    let step = poller.status_step(ID).unwrap();
    assert!(matches!(&step, StatusStep::Finalised { status: JobStatus::Lost, .. }), "{step:?}");
    assert_eq!(fake.count(Sweep), MAX_SWEEPS_WITHOUT_PROGRESS as usize, "no 4th sweep");
    let message = job(&r).error_message.unwrap();
    assert!(message.starts_with("Lost: the job's wrapper died without an exit code; its files were downloaded as evidence."), "{message}");
    assert!(message.contains("still ran after 3 sweeps (pid 70001)") && message.contains("may have written after its files were listed"), "{message}");
}

/// o25.2: a sweep in which the session shrinks is progress — the count restarts — so a session
/// that keeps shrinking is swept to the end, then written `lost` without survivors.
#[test]
fn a_shrinking_session_is_progress() {
    let r = Remote::new();
    started_on_server(&r);
    let collects = vec![lost_in_this_boot(4), lost_in_this_boot(3), lost_in_this_boot(3), lost_in_this_boot(2), lost_in_this_boot(1), lost_in_this_boot(0)];
    let fake = Fake::new(&r, server(&r, collects));
    let poller = r.poller(&fake);
    // Each step collects twice: the verdict, then after the sweep. Step 1: 4 (the baseline), swept,
    // 3 — progress. Step 2: 3, swept, 2 — progress. Step 3: 1, swept, 0 — the evidence path.
    assert!(matches!(poller.status_step(ID).unwrap(), StatusStep::LostSwept { .. }));
    assert_eq!(r.memory.get(ID).lost_sweeps, 0, "4 → 3 is progress");
    assert!(matches!(poller.status_step(ID).unwrap(), StatusStep::LostSwept { .. }));
    assert_eq!(r.memory.get(ID).lost_sweeps, 0, "3 → 2 is progress");
    let step = poller.status_step(ID).unwrap();
    assert!(matches!(&step, StatusStep::Finalised { status: JobStatus::Lost, outcome: Outcome::Lost { orphans, .. }, .. } if orphans.is_empty()), "{step:?}");
}

/// o25.2, o25.14.6: a sweep that finds row 8 no longer holding — "wrapper alive" (exit 4) — signals
/// nothing and **is not counted**: the job is re-classified at the next step, never finalised by
/// it. NEGATIVE CONTROL (`sweep-alive-counted`): count it like any other answered failure → red
/// (the 3rd such step writes `lost`).
#[test]
fn a_wrapper_alive_sweep_is_not_counted() {
    let r = Remote::new();
    let base = server(&r, vec![lost_in_this_boot(1)]);
    let fake = Fake::new(&r, |call, values| match call {
        Sweep => ran(values, 4, b"sweep: skip wrapper alive\n", b""),
        _ => base(call, values),
    });
    let poller = r.poller(&fake);
    for _ in 0..4 {
        let step = poller.status_step(ID).unwrap();
        let StatusStep::LostSwept { outcome: Some(Outcome::Lost { .. }), error: Some(error) } = &step else { panic!("{step:?}") };
        assert!(error.answered && error.text.contains("exit 4"), "{error:?}");
    }
    assert_eq!(r.memory.get(ID).lost_sweeps, 0, "nothing counted");
    assert_eq!(job(&r).status, JobStatus::Queued, "never finalised by a sweep that signalled nothing");
    assert_eq!(fake.count(Collect), 4, "no re-classify after a sweep that signalled nothing");
}

/// o25.2: the sweep's ready is o22.13.3's — a ready **without any answer** is a failed check,
/// counting nothing and sweeping nothing.
#[test]
fn a_lost_sweep_without_an_answer_counts_nothing() {
    let r = Remote::new();
    let away = Cell::new(true);
    let base = server(&r, vec![lost_in_this_boot(1)]);
    let fake = Fake::new(&r, |call, values| match call {
        Prepare if away.get() => crate::poller::fake_tests::timeout(),
        _ => base(call, values),
    });
    let poller = r.poller(&fake);
    for _ in 0..3 {
        assert!(matches!(poller.status_step(ID).unwrap(), StatusStep::CheckFailed(_)));
    }
    assert_eq!((r.memory.get(ID).lost_sweeps, fake.count(Sweep)), (0, 0));
    away.set(false);
    assert!(matches!(poller.status_step(ID).unwrap(), StatusStep::LostSwept { .. }));
    assert_eq!(fake.count(Sweep), 1);
}

/// o25.2 (evidence strikes): a failed evidence download counts the fetch strikes; the
/// `MAX_FETCH_STRIKES`-th writes `lost` without evidence and says so.
#[test]
fn a_lost_job_whose_evidence_cannot_be_fetched_is_written_lost_without_it() {
    let r = Remote::new();
    started_on_server(&r);
    let base = server(&r, vec![lost()]);
    let fake = Fake::new(&r, |call, values| match call {
        Download => exited(23, Vec::new()),
        _ => base(call, values),
    });
    let poller = r.poller(&fake);
    for n in 1..MAX_FETCH_STRIKES {
        let step = poller.status_step(ID).unwrap();
        assert!(matches!(&step, StatusStep::FetchFailed { strikes, .. } if *strikes == n), "{step:?}");
        let message = job(&r).error_message.unwrap();
        assert!(message.starts_with(&format!("Downloading the job's files as evidence failed (attempt {n} of 3)")), "{message}");
    }
    let step = poller.status_step(ID).unwrap();
    assert!(matches!(&step, StatusStep::Finalised { status: JobStatus::Lost, .. }), "{step:?}");
    let message = job(&r).error_message.unwrap();
    assert!(message.starts_with("Lost: the server restarted while the job ran; evidence not downloaded:"), "{message}");
    assert!(message.contains("it stays on the server"), "{message}");
}

/// o25.2 / o25.14.7 ("the 'pending cancel was dropped' text"): a cancel recorded while the job is
/// being finalised `lost` — after the step's read, during the evidence download — is moot: the
/// write NULLs its columns and the row says so. NEGATIVE CONTROL (`dropped-note-missing`): the note
/// left out → red.
#[test]
fn a_cancel_recorded_while_the_job_is_written_lost_is_dropped_with_a_note() {
    let r = Remote::new();
    started_on_server(&r);
    let base = server(&r, vec![lost()]);
    let fake = Fake::new(&r, |call, values| {
        if call == Download {
            r.w.sql("UPDATE jobs SET cancel_requested_at = '2026-11-18 10:00:00'");
        }
        base(call, values)
    });
    assert!(matches!(r.poller(&fake).status_step(ID).unwrap(), StatusStep::Finalised { status: JobStatus::Lost, .. }));
    let message = job(&r).error_message.unwrap();
    assert!(message.ends_with(DROPPED_CANCEL_NOTE), "{message}");
    assert_eq!(pending_cancel(&r), (None, None), "its columns are NULLed by that write");
}

/// o25.8, o25.11 ("Row 1 without `.cancelled` → `failed` with evidence"): a corrupt `.started` with
/// no cancel is fetched as evidence and written `failed`, the text saying why nothing was swept.
/// NEGATIVE CONTROL (`row1-left-live`): no evidence verdict for it → shown, the row stays queued →
/// red.
#[test]
fn a_corrupt_started_without_cancelled_is_failed_with_evidence() {
    let r = Remote::new();
    std::fs::write(r.server().join(".started"), "garbage\n").unwrap();
    let fake = Fake::new(&r, server(&r, vec![snapshot(false, None, None, (Some("garbage\n"), Some("garbage\n")))]));
    let step = r.poller(&fake).status_step(ID).unwrap();
    let StatusStep::Finalised { outcome: Outcome::Failed { reason: FailReason::CorruptStarted { cancelled: false, .. } }, status: JobStatus::Failed, .. } = &step
    else {
        panic!("{step:?}")
    };
    assert_eq!(fake.calls(), [Label, Collect, Download, List]);
    let message = job(&r).error_message.unwrap();
    assert!(message.starts_with("Failed: corrupt .started marker") && message.contains("nothing of the job can be swept"), "{message}");
    assert!(r.local().join(".started").exists(), "the evidence came down");
}

/// The record of a sweep step carries its error like a `cancelling` sweep's (o22.14.3): an answered
/// `cancel.sh sweep` failure, the step still classifying.
#[test]
fn a_failed_lost_sweep_is_answered_and_still_classifies() {
    let r = Remote::new();
    let base = server(&r, vec![lost_in_this_boot(1)]);
    let fake = Fake::new(&r, |call, values| match call {
        Sweep => ran(values, 3, b"", b"cancel: /proc/1/stat does not parse"),
        _ => base(call, values),
    });
    let step = r.poller(&fake).status_step(ID).unwrap();
    assert_eq!(
        step,
        StatusStep::LostSwept {
            outcome: Some(Outcome::Lost { orphans: orphans(1), stale_boot: false }),
            error: Some(CheckError::answered("cancel.sh sweep exited 3: cancel: /proc/1/stat does not parse")),
        }
    );
    assert_eq!(r.memory.get(ID).lost_sweeps, 1, "an answered failure counts");
}
