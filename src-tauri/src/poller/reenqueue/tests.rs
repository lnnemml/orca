//! The one automatic re-enqueue (ADR-024 o25.3–5, o25.14) over the 5.4b fake server
//! (`poller::fake_tests`), and the pure decision table. Each test names the o25.11 / o25.14.7 gate it
//! is; its negative control is a source mutation in `run_54b_controls.sh`, named at the test.

use std::cell::Cell;

use super::{decide, Decision, ReEnqueueWait};
use crate::models::job::JobStatus;
use crate::poller::fake_tests::{server, timeout, Call::*, Fake, ID};
use crate::poller::pending::Wait;
use std::time::Duration;

use crate::poller::plan::{MAX_REENQUEUE_REFUSALS, NEVER_STARTED_CONFIRM};
use crate::poller::remote_state::Known;
use crate::poller::tests::{record, Remote};
use crate::poller::StatusStep;
use crate::remote::classify::{FailReason, Outcome};
use crate::remote::contact::Contact;
use crate::remote::scripts::{sha256_hex, WRAPPER};
use crate::remote::submit::Refusal;
use crate::ssh_backend::tests::{
    blocked_record, expected_coords, snapshot, snapshot_claimed, snapshot_with_row, submit_reply, ROOT,
};
use crate::ssh_backend::{align_remote_pal, SubmitOutcome, SubmitStep};

/// The input the first submit uploaded under the profile's mask (`0-23`: `%pal nprocs 24`).
fn uploaded_input() -> String {
    align_remote_pal("! HF def2-SVP\n", "0-23").unwrap().0
}

/// A queued remote job submitted under the current mask, whose queue was then dropped: the local
/// dir and the server's hold the uploaded input, the server the submit's markers.
fn submitted() -> Remote {
    let r = Remote::new();
    std::fs::write(r.local().join("input.inp"), uploaded_input()).unwrap();
    let s = r.server();
    std::fs::write(s.join("input.inp"), uploaded_input()).unwrap();
    std::fs::write(s.join(".enqueued"), format!("socket={}\nid=0\n", expected_coords().socket)).unwrap();
    std::os::unix::fs::symlink("x", s.join(".submitting")).unwrap();
    r
}

/// Nothing ran, no queue holds the job, no claim: row 11's first never-start.
fn never_started() -> Vec<u8> {
    snapshot(false, None, None, (None, None))
}

/// Nothing ran, no queue holds it, and the claim `.reenqueue-1` is there: the second never-start.
fn never_started_again() -> Vec<u8> {
    snapshot_claimed(true, false, None, None, (None, None))
}

/// Every file of `dir` with its bytes, for a byte-identity check.
fn files(dir: &std::path::Path) -> Vec<(String, Vec<u8>)> {
    let mut out: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| {
            let e = e.unwrap();
            (e.file_name().to_string_lossy().into_owned(), std::fs::read(e.path()).unwrap())
        })
        .collect();
    out.sort();
    out
}

/// o25.4–5, o25.11 (Re-enqueue): a first never-start is re-enqueued **once** — ready, then one
/// `submit.sh reenqueue` carrying the recorded coordinates, the profile's current mask and ORCA path,
/// the wrapper's sha and the local dir's files — and the row stays `queued`. The next step's collect
/// reports the claim, so the second never-start — confirmed by a pass 45 s later (o25.15) — is fetched
/// as evidence and written `failed`, with no second re-enqueue. NEGATIVE CONTROLS: `claim-count-literal-0` (the collector's count not passed to
/// `classify` → the second step re-enqueues again, red); `third-attempt` (the classifier re-enqueues
/// while the count is below 2 → red); `reenqueue-twice` (two calls in one pass → red).
#[test]
fn a_first_never_start_is_re_enqueued_once_and_the_second_is_failed() {
    let r = submitted();
    let fake = Fake::new(&r, server(&r, vec![never_started(), never_started_again()]));
    let poller = r.poller(&fake);
    assert_eq!(poller.status_step(ID).unwrap(), StatusStep::ReEnqueued { tsp_id: 5 });
    assert_eq!(fake.calls(), [Label, Collect, Prepare, ReEnqueue], "one re-enqueue per pass, after the ready");
    let sent = &fake.reenqueues()[0];
    let c = expected_coords();
    let want: Vec<String> = [
        "reenqueue",
        c.job_dir.as_str(),
        ROOT,
        c.socket.as_str(),
        "0-23",
        "/opt/orca/orca",
        sha256_hex(WRAPPER).as_str(),
        "input.inp",
        sha256_hex(&uploaded_input()).as_str(),
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    assert_eq!(sent, &want, "the re-enqueue form, the current mask, the local dir's list");
    let j = r.w.job();
    assert_eq!(j.status, JobStatus::Queued);
    assert!(j.error_message.unwrap().contains("re-enqueued once (tsp id 5)"));
    assert_eq!(j.submit_pending_since, None, "the 5.4c pending flag is never set");

    assert_eq!(poller.status_step(ID).unwrap(), StatusStep::Shown(never_start()), "the first pass with the claim");
    r.clock.advance(NEVER_STARTED_CONFIRM);
    let step = poller.status_step(ID).unwrap();
    assert_eq!(step, StatusStep::Finalised { outcome: never_start(), status: JobStatus::Failed, drain_error: None });
    assert_eq!(fake.count(ReEnqueue), 1, "no second re-enqueue");
    assert_eq!(&fake.calls()[4..], [Label, Collect, Label, Collect, Download, List], "the markers and .tsp-out fetched as evidence");
    let message = r.w.job().error_message.unwrap();
    assert!(
        message.starts_with(
            "Failed: the job never started on the server after one re-enqueue (or the re-enqueue did not reach the \
             queue); see .tsp-out; its files were downloaded as evidence."
        ),
        "{message}"
    );
}

/// The verdict of a never-start with the claim.
fn never_start() -> Outcome {
    Outcome::Failed { reason: FailReason::WrapperNeverStarted }
}

/// o25.15: one never-start pass with the claim is **not** terminal — a re-enqueue call still running
/// on the server may have claimed and not yet enqueued. The row stays `queued` with the neutral text
/// and nothing is downloaded. NEGATIVE CONTROL (`never-started-terminal-at-first`): the first pass
/// takes the evidence path → red.
#[test]
fn one_never_start_pass_with_the_claim_is_not_terminal() {
    let r = submitted();
    let fake = Fake::new(&r, server(&r, vec![never_started_again()]));
    assert_eq!(r.poller(&fake).status_step(ID).unwrap(), StatusStep::Shown(never_start()));
    assert_eq!(fake.calls(), [Label, Collect], "no download, no call");
    assert_eq!(r.row(), (JobStatus::Queued, Some(super::NEVER_STARTED_UNCONFIRMED.into())));
}

/// o25.15: two consecutive passes **less than 45 s** apart are not terminal; the first pass at
/// least 45 s after the run's first one is. NEGATIVE CONTROL (`never-started-no-spacing`): no spacing
/// check → the second pass at 44 s is terminal, red.
#[test]
fn never_start_passes_under_45_s_apart_are_not_terminal() {
    let r = submitted();
    let fake = Fake::new(&r, server(&r, vec![never_started_again()]));
    let poller = r.poller(&fake);
    assert_eq!(poller.status_step(ID).unwrap(), StatusStep::Shown(never_start()));
    r.clock.advance(NEVER_STARTED_CONFIRM - Duration::from_secs(1));
    assert_eq!(poller.status_step(ID).unwrap(), StatusStep::Shown(never_start()), "44 s after the first: not yet");
    r.clock.advance(Duration::from_secs(1));
    let step = poller.status_step(ID).unwrap();
    assert!(matches!(&step, StatusStep::Finalised { status: JobStatus::Failed, .. }), "45 s after the first: {step:?}");
}

/// o25.15.1 (F2): the 45 s separate the two **server reads** — the first pass is remembered when its
/// collect **ended**, and a confirming pass counts only if its collect **started** 45 s or more after
/// that. With a slow collect (10 s, injected): a second pass starting 44 s after the first read ended
/// is not terminal, though its own collect ends 54 s after it; the next pass, starting later, is.
/// NEGATIVE CONTROLS: `never-started-first-before-collect` (the first pass timed at its collect's
/// start → the second pass is terminal, red); `never-started-confirm-after-collect` (the confirming
/// pass timed after its collect → red).
#[test]
fn the_45_s_separate_the_two_server_reads() {
    let r = submitted();
    let base = server(&r, vec![never_started_again()]);
    let fake = Fake::new(&r, |call, values| {
        if call == Collect {
            r.clock.advance(Duration::from_secs(10));
        }
        base(call, values)
    });
    let poller = r.poller(&fake);
    assert_eq!(poller.status_step(ID).unwrap(), StatusStep::Shown(never_start()), "the first read ends at 10 s");
    r.clock.advance(NEVER_STARTED_CONFIRM - Duration::from_secs(1));
    assert_eq!(poller.status_step(ID).unwrap(), StatusStep::Shown(never_start()), "the second read starts at 54 s: 44 s after");
    let step = poller.status_step(ID).unwrap();
    assert!(matches!(&step, StatusStep::Finalised { status: JobStatus::Failed, .. }), "the third read starts at 64 s: {step:?}");
}

/// o25.15.5 gate (F3): a label that says the job is not in the server's hands ends the re-enqueue's
/// waiting state, like a decided verdict. NEGATIVE CONTROL (`labelled-keeps-waiting`): `Labelled`
/// keeps it → red.
#[test]
fn a_label_ends_the_reenqueue_waiting_state() {
    let r = submitted();
    let labelled = Cell::new(false);
    let base = server(&r, vec![never_started()]);
    let fake = Fake::new(&r, |call, values| match call {
        ReEnqueue => submit_reply(values, &blocked_record()),
        Label if labelled.get() => crate::ssh_backend::tests::label_reply(values, crate::ssh_backend::tests::NOT_ON_SERVER),
        _ => base(call, values),
    });
    let poller = r.poller(&fake);
    poller.status_step(ID).unwrap();
    poller.status_step(ID).unwrap();
    assert_eq!(record(&r).unwrap().re_enqueue_waiting.map(|w| w.refusals), Some(2));
    labelled.set(true);
    assert!(matches!(poller.status_step(ID).unwrap(), StatusStep::Labelled(_)));
    assert_eq!(record(&r).unwrap().re_enqueue_waiting, None, "the label ends the waiting state");
    assert_eq!((r.memory.get(ID).reenqueue_waits, r.memory.get(ID).reenqueue_wait), (0, None));
}

/// o25.15: the passes must be **consecutive** — any other verdict between them ends the run, and the
/// next never-start starts a new one. NEGATIVE CONTROL (`never-started-no-reset`): the run kept across
/// another verdict → the pass after it is terminal, red.
#[test]
fn another_verdict_between_never_start_passes_restarts_the_confirmation() {
    let r = submitted();
    let fake = Fake::new(&r, server(&r, vec![never_started_again(), snapshot_with_row("queued"), never_started_again()]));
    let poller = r.poller(&fake);
    assert_eq!(poller.status_step(ID).unwrap(), StatusStep::Shown(never_start()));
    r.clock.advance(NEVER_STARTED_CONFIRM);
    assert_eq!(poller.status_step(ID).unwrap(), StatusStep::Shown(Outcome::Queued), "the re-enqueue's row appeared");
    assert_eq!(r.memory.get(ID).never_started_at, None);
    r.clock.advance(NEVER_STARTED_CONFIRM);
    assert_eq!(poller.status_step(ID).unwrap(), StatusStep::Shown(never_start()), "a new run's first pass");
    assert_eq!(r.w.job().status, JobStatus::Queued);
}

/// o25.14.1–2, o25.11 and o25.14.7: when the profile's core count changed since the submit, the
/// in-memory derivation differs from the local dir — the job is `failed` at once with the ADR's text,
/// **no call** is made (not even the ready), and the local dir is byte-identical. NEGATIVE CONTROLS:
/// `changed-mask-sent` (the comparison ignored → a re-enqueue is sent, red); `changed-mask-rewrites`
/// (the local dir rewritten before the comparison → red).
#[test]
fn a_changed_core_count_fails_the_job_at_once_without_a_call() {
    let r = submitted();
    r.w.sql("UPDATE server_profiles SET core_mask = '0-11'");
    let before = files(&r.local());
    let fake = Fake::new(&r, server(&r, vec![never_started()]));
    let step = r.poller(&fake).status_step(ID).unwrap();
    assert_eq!(fake.calls(), [Label, Collect], "no ready, no re-enqueue");
    assert_eq!(files(&r.local()), before, "the local dir is untouched");
    assert_eq!(step, StatusStep::Finalised { outcome: Outcome::ReEnqueue, status: JobStatus::Failed, drain_error: None });
    let j = r.w.job();
    assert_eq!(j.status, JobStatus::Failed);
    assert_eq!(
        j.error_message.as_deref(),
        Some(
            "Never started on the server, and the core count of the profile changed since it was submitted (the \
             input's %pal no longer matches); start it again as a new job."
        )
    );
    assert!(j.completed_at.is_some());
}

/// o25.14.2: a mask edit that keeps the CPU count re-enqueues on the new cores (the digest binds only
/// the `%pal` count); the call carries the current mask.
#[test]
fn a_mask_edit_that_keeps_the_count_re_enqueues_on_the_new_cores() {
    let r = submitted();
    r.w.sql("UPDATE server_profiles SET core_mask = '24-47'");
    let fake = Fake::new(&r, server(&r, vec![never_started()]));
    assert_eq!(r.poller(&fake).status_step(ID).unwrap(), StatusStep::ReEnqueued { tsp_id: 5 });
    assert_eq!(fake.reenqueues()[0][4], "24-47");
}

/// o25.4, o25.11 ("slot busy → no claim, `ReEnqueueWaiting` notice at the 3rd") and o25.14.6: slot
/// busy is transient — the row stays `queued`, nothing counted toward `failed`, the record keeps
/// `known = Shown(ReEnqueue)` and its `re_enqueue_waiting` shows the notice from the 3rd refusal; a
/// decided verdict other than `ReEnqueue` ends the waiting state. NEGATIVE CONTROLS:
/// `waiting-not-reset` (a decided verdict keeps the waiting state → red); `waiting-notice-at-first`
/// (the notice from the 1st → red).
#[test]
fn a_busy_slot_waits_with_the_notice_at_the_third_refusal_and_a_decided_verdict_ends_it() {
    let r = submitted();
    let queued = Cell::new(false);
    let base = server(&r, vec![never_started()]);
    let fake = Fake::new(&r, |call, values| match call {
        ReEnqueue => submit_reply(values, &blocked_record()),
        Collect if queued.get() => crate::ssh_backend::tests::ran(values, 0, &snapshot_with_row("queued"), b""),
        _ => base(call, values),
    });
    let poller = r.poller(&fake);
    // The ADR's 3rd refusal, as a literal: a constant-relative loop would follow a changed bound.
    for n in 1..=3u32 {
        let step = poller.status_step(ID).unwrap();
        assert!(matches!(&step, StatusStep::ReEnqueueWaiting(ReEnqueueWait::Slot(Wait::SlotBusy { .. }))), "{step:?}");
        let state = record(&r).unwrap();
        assert_eq!(state.known, Some(Known::Shown(Outcome::ReEnqueue)), "the view gate");
        let waiting = state.re_enqueue_waiting.unwrap();
        assert_eq!(waiting.refusals, n);
        assert_eq!(waiting.notified, n >= 3, "refusal {n}");
        if n >= 3 {
            assert!(waiting.notice.unwrap().contains("Waiting for the slot: held by pid 4242"), "the blockers named");
        }
    }
    assert_eq!(r.memory.get(ID).reenqueue_refusals, 0, "transient: nothing counted toward failed");
    assert_eq!(r.w.job().status, JobStatus::Queued);
    assert_eq!(r.w.job().submit_pending_since, None);

    queued.set(true);
    assert_eq!(poller.status_step(ID).unwrap(), StatusStep::Shown(Outcome::Queued));
    assert_eq!(record(&r).unwrap().re_enqueue_waiting, None, "a decided verdict ends the waiting state");
    assert_eq!(r.memory.get(ID).reenqueue_waits, 0);
}

/// o25.4: lock busy and a slot check error are transient too — five of them never fail the job.
#[test]
fn lock_busy_and_slot_check_errors_wait_and_never_fail_the_job() {
    let r = submitted();
    let n = Cell::new(0);
    let base = server(&r, vec![never_started()]);
    let fake = Fake::new(&r, |call, values| match call {
        ReEnqueue => {
            n.set(n.get() + 1);
            let outcome = if n.get() % 2 == 0 {
                crate::ssh_backend::tests::rec("lock-busy", "123 bash")
            } else {
                crate::ssh_backend::tests::rec("slot-check-error", "tsp -l exited 1")
            };
            submit_reply(values, &outcome)
        }
        _ => base(call, values),
    });
    let poller = r.poller(&fake);
    for _ in 0..5 {
        assert!(matches!(poller.status_step(ID).unwrap(), StatusStep::ReEnqueueWaiting(ReEnqueueWait::Slot(_))));
    }
    assert_eq!(r.w.job().status, JobStatus::Queued);
    assert_eq!(r.memory.get(ID).reenqueue_refusals, 0);
}

/// o25.14.4, o25.11 ("3 non-slot refusals → `failed`"): an answered refusal outside the transient
/// set is counted; the 3rd ends the job `failed` with that refusal's text. NEGATIVE CONTROL
/// (`refusals-unbounded`): no bound → red.
#[test]
fn three_answered_refusals_fail_the_job_with_the_last_ones_text() {
    let r = submitted();
    let base = server(&r, vec![never_started()]);
    let fake = Fake::new(&r, |call, values| match call {
        ReEnqueue => submit_reply(values, &crate::ssh_backend::tests::rec("refused", "wrapper: the sha256 of x is not the one its name carries")),
        _ => base(call, values),
    });
    let poller = r.poller(&fake);
    for n in 1..MAX_REENQUEUE_REFUSALS {
        let step = poller.status_step(ID).unwrap();
        assert!(matches!(&step, StatusStep::ReEnqueueRefused { refusals, why } if *refusals == n && why.starts_with("wrapper:")), "{step:?}");
        assert_eq!(record(&r).unwrap().known, Some(Known::Shown(Outcome::ReEnqueue)));
        assert!(record(&r).unwrap().check_error.unwrap().answered);
    }
    let step = poller.status_step(ID).unwrap();
    assert_eq!(step, StatusStep::Finalised { outcome: Outcome::ReEnqueue, status: JobStatus::Failed, drain_error: None });
    let message = r.w.job().error_message.unwrap();
    assert!(message.contains("re-enqueue was refused 3 times") && message.contains("wrapper: the sha256"), "{message}");
    assert_eq!(fake.count(ReEnqueue), MAX_REENQUEUE_REFUSALS as usize);
}

/// o25.14.4 ("count kept only while the next classify is `ReEnqueue` again"): a refusal the next
/// classify re-decides — a live row appeared — is reset by that decided verdict, so it never adds
/// toward `failed`.
#[test]
fn a_refusal_followed_by_another_verdict_is_not_kept() {
    let r = submitted();
    let row = Cell::new(false);
    let base = server(&r, vec![never_started()]);
    let fake = Fake::new(&r, |call, values| match call {
        ReEnqueue => submit_reply(values, &crate::ssh_backend::tests::rec("refused", "row: a tsp row already holds it")),
        Collect if row.get() => crate::ssh_backend::tests::ran(values, 0, &snapshot_with_row("queued"), b""),
        _ => base(call, values),
    });
    let poller = r.poller(&fake);
    poller.status_step(ID).unwrap();
    poller.status_step(ID).unwrap();
    assert_eq!(r.memory.get(ID).reenqueue_refusals, 2);
    row.set(true);
    poller.status_step(ID).unwrap();
    assert_eq!(r.memory.get(ID).reenqueue_refusals, 0, "a decided verdict resets the count");
    row.set(false);
    assert!(matches!(poller.status_step(ID).unwrap(), StatusStep::ReEnqueueRefused { refusals: 1, .. }));
}

/// o25.14.3, o25.14.7 ("an unverified profile waits with no call"): a profile that is not a run
/// target makes the re-enqueue **wait** — no ssh call, nothing counted, the notice at once — and the
/// next step after a passing connection test re-enqueues. NEGATIVE CONTROL (`unverified-terminal`):
/// the unverified profile ends the job → red.
#[test]
fn an_unverified_profile_waits_without_a_call_until_it_is_re_tested() {
    let r = submitted();
    r.w.sql("UPDATE server_profiles SET verified_at = NULL");
    let fake = Fake::new(&r, server(&r, vec![never_started()]));
    let poller = r.poller(&fake);
    for _ in 0..4 {
        let step = poller.status_step(ID).unwrap();
        assert!(matches!(&step, StatusStep::ReEnqueueWaiting(ReEnqueueWait::Unverified(_))), "{step:?}");
    }
    assert_eq!(fake.count(Prepare) + fake.count(ReEnqueue), 0, "no call to the server for the re-enqueue");
    assert_eq!(r.w.job().status, JobStatus::Queued);
    let waiting = record(&r).unwrap().re_enqueue_waiting.unwrap();
    assert!(waiting.notified && waiting.notice.unwrap().contains("re-test the server profile to continue"));
    assert_eq!((r.memory.get(ID).reenqueue_waits, r.memory.get(ID).reenqueue_refusals), (0, 0), "not counted");

    r.w.sql("UPDATE server_profiles SET verified_at = datetime('now')");
    assert_eq!(poller.status_step(ID).unwrap(), StatusStep::ReEnqueued { tsp_id: 5 });
}

/// o25.14.4, o25.14.7 ("a lost reply is not counted"): a re-enqueue call that got no answer is a
/// failed check — not counted, the next step re-decides from the server. NEGATIVE CONTROL
/// (`lost-reply-counted`): count it → red (the 3rd fails the job).
#[test]
fn a_lost_reenqueue_reply_is_not_counted() {
    let r = submitted();
    let base = server(&r, vec![never_started()]);
    let fake = Fake::new(&r, |call, values| match call {
        ReEnqueue => timeout(),
        _ => base(call, values),
    });
    let poller = r.poller(&fake);
    for _ in 0..4 {
        assert!(matches!(poller.status_step(ID).unwrap(), StatusStep::CheckFailed(_)));
    }
    assert_eq!(r.memory.get(ID).reenqueue_refusals, 0);
    assert_eq!(r.w.job().status, JobStatus::Queued);
    assert_eq!(record(&r).unwrap().last_contact.map(|c| matches!(c, Contact::Failed(_))), Some(true));
}

/// o13.3 / o25.14.3, o25.14.7 ("`refused-kup` clears `verified_at`"): a `refused-kup` is one counted
/// refusal that clears the profile's stamp, so the next step waits for a connection test, with no
/// call.
#[test]
fn a_refused_kup_clears_the_stamp_and_the_next_step_waits() {
    let r = submitted();
    let base = server(&r, vec![never_started()]);
    let fake = Fake::new(&r, |call, values| match call {
        ReEnqueue => {
            let ev = format!("rc 0\n{}{}", crate::ssh_backend::tests::rec("stdout", "b true\n"), crate::ssh_backend::tests::rec("stderr", ""));
            submit_reply(values, &format!("refused-kup {}\n{ev}\n", ev.len()))
        }
        _ => base(call, values),
    });
    let poller = r.poller(&fake);
    assert!(matches!(poller.status_step(ID).unwrap(), StatusStep::ReEnqueueRefused { refusals: 1, .. }));
    let verified: Option<String> =
        r.w.db.lock().unwrap().query_row("SELECT verified_at FROM server_profiles", [], |row| row.get(0)).unwrap();
    assert_eq!(verified, None, "the stamp is cleared");
    assert!(matches!(poller.status_step(ID).unwrap(), StatusStep::ReEnqueueWaiting(ReEnqueueWait::Unverified(_))));
    assert_eq!(fake.count(ReEnqueue), 1);
}

/// o25.14.1: the server's `upload-mismatch` is an ordinary counted refusal — the plan binding already
/// proved the local dir is the current derivation — never the "profile changed" text.
#[test]
fn a_server_upload_mismatch_is_a_counted_refusal() {
    let r = submitted();
    let base = server(&r, vec![never_started()]);
    let fake = Fake::new(&r, |call, values| match call {
        ReEnqueue => submit_reply(
            values,
            &format!("upload-mismatch\nmissing 0\nextra 1\n{}differing 0\n", crate::ssh_backend::tests::rec("name", ".enqueued.tmp.9")),
        ),
        _ => base(call, values),
    });
    let step = r.poller(&fake).status_step(ID).unwrap();
    assert!(matches!(&step, StatusStep::ReEnqueueRefused { refusals: 1, why } if why.starts_with("upload:")), "{step:?}");
    assert_eq!(r.w.job().status, JobStatus::Queued);
}

/// The pure table (o25.4, o25.14.4): outcome first, the call's own contact for a reply that did not
/// come or did not parse.
#[test]
fn the_reenqueue_decision_table() {
    let submit = |refusal| SubmitOutcome::NotClaimed { step: SubmitStep::Submit, refusal };
    assert_eq!(decide(&SubmitOutcome::Enqueued { tsp_id: 9 }, Some(&Contact::Ok)), Decision::Enqueued(9));
    assert!(matches!(decide(&submit(Refusal::Blocked { blockers: vec![] }), None), Decision::Wait(Wait::SlotBusy { .. })));
    assert!(matches!(decide(&submit(Refusal::SlotCheckFailed("x".into())), None), Decision::Wait(Wait::SlotCheckFailed(_))));
    assert!(matches!(decide(&submit(Refusal::LockBusy { openers: "x".into() }), None), Decision::Wait(Wait::LockBusy { .. })));
    for counted in [
        submit(Refusal::Other("marker: .started exists".into())),
        submit(Refusal::UploadMismatch { missing: vec![], extra: vec!["a".into()], differing: vec![] }),
        SubmitOutcome::NotClaimed { step: SubmitStep::Install, refusal: Refusal::Other("install: refused".into()) },
        SubmitOutcome::FailedAfterClaim { reason: "enqueue: tsp exited 1".into() },
    ] {
        assert!(matches!(decide(&counted, Some(&Contact::Ok)), Decision::Refused(_)), "{counted:?}");
    }
    let unknown = SubmitOutcome::Unknown { reason: "broken".into() };
    assert!(matches!(decide(&unknown, Some(&Contact::Ok)), Decision::Refused(_)), "answered but unreadable: counted");
    // o25.15.5 gate: an `Unknown` with **no call made** (a value refused before ssh ran) is counted —
    // nothing will change by itself. NEGATIVE CONTROL (`unknown-no-call-not-counted`): read it as a
    // lost reply → red.
    assert!(matches!(decide(&unknown, None), Decision::Refused(_)), "no call made: counted");
    assert!(matches!(decide(&unknown, Some(&Contact::Failed("t".into()))), Decision::Lost(_)), "no answer: not counted");
}
