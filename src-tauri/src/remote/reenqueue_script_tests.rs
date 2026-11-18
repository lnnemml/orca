//! `submit.sh reenqueue` (ADR-024 o25.3–4, o25.14) against the **real** script: the 5.2 [`Lab`]
//! (stub `tsp`, stub `busctl`), each job first submitted for real and then its queue dropped — the
//! stub daemon's rows emptied, standing in for `tsp -K` or a reboot (the restart simulation of
//! o25.10). The slot check scans this machine's real processes, so every test holds [`serial`] and
//! uses the submit tests' slot mask.
//!
//! Each guard has a **negative control**: a check function run on a mutated copy of the script
//! (in-suite, [`mutate`]) or a source mutation in `run_54b_controls.sh`, named at its test.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use super::call_script_tests::{
    enqueues, lab_listen_with_rows, mutate, prepared, refused, serial, slot_cpus, slot_socket, submit_with, tree,
};
use super::classify::{FailReason, Outcome};
use super::script_tests::{tracked, tsp_listing, wait_for, Lab};
use super::scripts::{LIST, SUBMIT};
use super::submit::{Refusal, SubmitArgs, SubmitReply};
use super::sync::{parse_list_reply, Digest, ListArgs};
use crate::execution_backend::FetchPolicy;

/// `<sock>.rows`: what the stub daemon's `tsp -l` prints.
fn set_rows(sock: &Path, rows: &[String]) {
    fs::write(format!("{}.rows", Lab::str(sock)), tsp_listing(rows)).unwrap();
}

/// A row of the stub's shape for `job` in `state`: `queued` as the stub enqueues it, `running` and
/// `finished` with an output path (and an exit code and times for a finished one), as P4 recorded.
fn row(lab: &Lab, job: &Path, args: &SubmitArgs, id: u32, state: &str) -> String {
    let command = format!("bash {} {} {} {}", Lab::str(&lab.wrapper), Lab::str(job), args.core_mask, args.orca_path);
    match state {
        "queued" => format!("{id:<4} queued     (file)                                       {command}"),
        "running" => format!("{id:<4} running    /tmp/ts-out.S0SWpm                           {command}"),
        "finished" => format!("{id:<4} finished   /tmp/ts-out.QG5j6G   1        0.00/0.00/0.00 {command}"),
        other => panic!("no {other} row"),
    }
}

/// A job submitted for real — the claim `.submitting`, `.enqueued` (id 0), `.tsp-out/`, a queued row
/// of the stub daemon — whose queue was then dropped: the rows emptied, the daemon still listening.
/// Returns the job dir, the slot socket and the re-enqueue's values (the first submit's files).
fn dropped(lab: &Lab, name: &str) -> (PathBuf, PathBuf, SubmitArgs) {
    let sock = slot_socket(lab, 0);
    let (job, args) = prepared(lab, name, &sock);
    let first = submit_with(lab, SUBMIT, &args, &[]);
    assert!(matches!(first, SubmitReply::Enqueued(_)), "the first submit: {first:?}");
    set_rows(&sock, &[]);
    (job, sock, args.reenqueue())
}

/// o25.4 + o25.14.1 (the success half): a dropped queue is re-enqueued — `Enqueued(1)`, the claim
/// `.reenqueue-1` (a symlink to `x`), `.enqueued` replaced by the new id, a new queued row — with
/// `.submitting` and `.enqueued` present and left out of the file check (o25.14.7). The collector then
/// reports the claim: the job reads `Queued`; once that queue is dropped too, the second never-start
/// is `Failed { WrapperNeverStarted }` (o25.10's "a second empty queue → failed"). A second
/// re-enqueue finds the claim and refuses, enqueuing nothing. `Ok` iff all of it holds.
fn success_check(script: &str) -> Result<(), String> {
    let _serial = serial();
    let lab = Lab::new();
    let (job, sock, args) = dropped(&lab, "rq-ok");
    let reply = submit_with(&lab, script, &args, &[]);
    if reply != SubmitReply::Enqueued(1) {
        return Err(format!("the re-enqueue: {reply:?}"));
    }
    assert_eq!(fs::read_link(job.join(".reenqueue-1")).unwrap(), Path::new("x"), "the claim is a symlink to x");
    assert!(fs::symlink_metadata(job.join(".submitting")).is_ok(), "the first claim stays");
    assert_eq!(fs::read_to_string(job.join(".enqueued")).unwrap(), format!("socket={}\nid=1\n", Lab::str(&sock)));
    let rows = fs::read_to_string(format!("{}.rows", Lab::str(&sock))).unwrap();
    assert!(rows.lines().any(|l| l.starts_with("1 ") && l.contains(" queued ") && l.contains(Lab::str(&job))), "{rows}");
    assert_eq!(enqueues(&lab).len(), 2, "one enqueue per call that got through");
    assert_eq!(lab.classify(&job, &[sock.clone()]), Outcome::Queued, "row 9 on the new row");

    set_rows(&sock, &[]);
    assert_eq!(
        lab.classify(&job, &[sock.clone()]),
        Outcome::Failed { reason: FailReason::WrapperNeverStarted },
        "the claim makes the second never-start terminal"
    );
    let again = submit_with(&lab, script, &args, &[]);
    assert!(refused(&again).starts_with(&format!("marker: {}/.reenqueue-1 exists", Lab::str(&job))), "{again:?}");
    assert_eq!(enqueues(&lab).len(), 2, "a second re-enqueue enqueues nothing");
    Ok(())
}

#[test]
fn a_dropped_queue_is_re_enqueued_once_and_the_claim_makes_the_next_never_start_terminal() {
    success_check(SUBMIT).unwrap();
}

/// NEGATIVE CONTROL (o25.14.7 "a success with `.submitting` and `.enqueued` present (control: no
/// exclusion → red)"): the file check without the re-enqueue's exclusions sees the first submit's
/// markers as extra files.
#[test]
fn a_reenqueue_file_check_without_the_exclusions_turns_red() {
    let red = success_check(&mutate(SUBMIT, "                .submitting | .enqueued) continue ;;\n", "")).unwrap_err();
    assert!(red.contains("UploadMismatch") && red.contains(".enqueued"), "{red}");
}

/// What a re-enqueue on a fresh dropped job does after `setup` changes its dir or its queue: the
/// reply, whether a claim was made, and whether anything was enqueued.
fn attempt(lab: &mut Lab, name: &str, setup: impl FnOnce(&mut Lab, &Path, &Path, &SubmitArgs), script: &str) -> (SubmitReply, bool, bool) {
    let (job, sock, args) = dropped(lab, name);
    setup(lab, &job, &sock, &args);
    let before = enqueues(lab).len();
    let reply = submit_with(lab, script, &args, &[]);
    let claimed = fs::symlink_metadata(job.join(".reenqueue-1")).is_ok();
    (reply, claimed, enqueues(lab).len() > before)
}

/// o25.4 / o25.11 (Scripts, the refusals) and o25.14.1: a re-enqueue refuses — with no claim and no
/// enqueue — on each of `.started`, `.exit_code`, `.cancelled` (the marker refusal **kind**, never
/// an upload mismatch, though each is also an extra file: step 3 runs before step 4), on an existing
/// claim, on a queued or a running row of the job, and on a file whose sha256 differs (`upload-
/// mismatch`). `Ok` iff every case is refused as stated. The controls (`run_54b_controls.sh rq-*`)
/// drop one check each, and `rq-file-check-first` moves step 4 before step 3.
fn refusals_check(script: &str) -> Result<(), String> {
    let _serial = serial();
    let mut lab = Lab::new();
    for marker in [".started", ".exit_code", ".cancelled"] {
        let name = format!("rq{}", marker.replace('.', "-"));
        let (reply, claimed, enqueued) =
            attempt(&mut lab, &name, |_, job, _, _| fs::write(job.join(marker), "x\n").unwrap(), script);
        let ok = matches!(&reply, SubmitReply::Refused(Refusal::Other(why)) if why.starts_with("marker: ") && why.ends_with(&format!("{marker} exists")));
        if !ok || claimed || enqueued {
            return Err(format!("{marker}: {reply:?}, claimed {claimed}, enqueued {enqueued}"));
        }
    }
    let (reply, _, enqueued) = attempt(
        &mut lab,
        "rq-claimed",
        |_, job, _, _| std::os::unix::fs::symlink("x", job.join(".reenqueue-1")).unwrap(),
        script,
    );
    if !matches!(&reply, SubmitReply::Refused(Refusal::Other(why)) if why.ends_with(".reenqueue-1 exists")) || enqueued {
        return Err(format!(".reenqueue-1: {reply:?}, enqueued {enqueued}"));
    }
    for state in ["queued", "running"] {
        let (reply, claimed, enqueued) = attempt(
            &mut lab,
            &format!("rq-{state}"),
            |lab, job, sock, args| set_rows(sock, &[row(lab, job, args, 0, state)]),
            script,
        );
        if !matches!(&reply, SubmitReply::Refused(Refusal::Other(why)) if why.starts_with("row: ")) || claimed || enqueued {
            return Err(format!("a {state} row: {reply:?}, claimed {claimed}, enqueued {enqueued}"));
        }
    }
    let (reply, claimed, enqueued) = attempt(
        &mut lab,
        "rq-sha",
        |_, job, _, _| fs::write(job.join("input.xyz"), "1\n\nH 0 0 9\n").unwrap(),
        script,
    );
    let mismatch = matches!(&reply, SubmitReply::Refused(Refusal::UploadMismatch { differing, .. }) if differing == &["input.xyz"]);
    if !mismatch || claimed || enqueued {
        return Err(format!("a sha mismatch: {reply:?}, claimed {claimed}, enqueued {enqueued}"));
    }
    Ok(())
}

#[test]
fn a_reenqueue_refuses_on_each_marker_the_claim_a_live_row_and_a_sha_mismatch() {
    refusals_check(SUBMIT).unwrap();
}

/// o25.4: a **finished** row of the job (a wrapper that crashed before `.started`, row 11) does not
/// refuse — only queued and running rows do. `Ok` iff it is enqueued with its claim.
fn finished_row_check(script: &str) -> Result<(), String> {
    let _serial = serial();
    let mut lab = Lab::new();
    let (reply, claimed, enqueued) = attempt(
        &mut lab,
        "rq-finished",
        |lab, job, sock, args| set_rows(sock, &[row(lab, job, args, 0, "finished")]),
        script,
    );
    if reply != SubmitReply::Enqueued(1) || !claimed || !enqueued {
        return Err(format!("a finished row: {reply:?}, claimed {claimed}, enqueued {enqueued}"));
    }
    Ok(())
}

#[test]
fn a_reenqueue_accepts_a_finished_row() {
    finished_row_check(SUBMIT).unwrap();
}

/// NEGATIVE CONTROL (o25.11 "accepts with a finished row (control: any row refuses → red)").
#[test]
fn a_reenqueue_that_refuses_any_row_turns_red() {
    let red = finished_row_check(&mutate(SUBMIT, "&& [[ $RW_JOB == \"$job\" && $RW_STATE == finished ]]", "&& false")).unwrap_err();
    assert!(red.starts_with("a finished row: Refused"), "{red}");
}

/// o25.3 / o25.14.7 (a Lab gate — the fake server cannot see it): the claim is made **after** the
/// slot check. A re-enqueue the scan blocks (a pinned process on the mask) or whose slot cannot be
/// checked refuses with its transient kind and writes nothing in the job dir — no claim. `Ok` iff so.
fn claim_after_slot_check(script: &str) -> Result<(), String> {
    let _serial = serial();
    let mut lab = Lab::new();
    let (job, sock, args) = dropped(&lab, "rq-blocked");
    let elsewhere = lab.root.join("elsewhere");
    fs::create_dir_all(&elsewhere).unwrap();
    let mut cmd = Command::new("taskset");
    cmd.arg("-c").arg(slot_cpus().0.to_string()).arg("sleep").arg("20").current_dir(&elsewhere).stdin(Stdio::null());
    let pid = lab.spawn(cmd);
    wait_for("pinned", || fs::read_to_string(format!("/proc/{pid}/cmdline")).is_ok_and(|c| c.starts_with("sleep")));
    tracked(pid);

    let before = tree(&job);
    let reply = submit_with(&lab, script, &args, &[]);
    if !matches!(&reply, SubmitReply::Refused(Refusal::Blocked { .. })) {
        return Err(format!("slot busy: {reply:?}"));
    }
    if tree(&job) != before {
        return Err(format!("slot busy wrote into the job dir: {:?}", tree(&job)));
    }
    lab_listen_with_rows(&mut lab, &sock, &[]);
    fs::write(format!("{}.fail", Lab::str(&sock)), "").unwrap();
    let reply = submit_with(&lab, script, &args, &[]);
    if !matches!(&reply, SubmitReply::Refused(Refusal::SlotCheckFailed(_))) {
        return Err(format!("slot check error: {reply:?}"));
    }
    if tree(&job) != before {
        return Err(format!("slot check error wrote into the job dir: {:?}", tree(&job)));
    }
    Ok(())
}

#[test]
fn a_blocked_reenqueue_claims_nothing() {
    claim_after_slot_check(SUBMIT).unwrap();
}

/// NEGATIVE CONTROL (o25.11 "claim before the slot check → red").
#[test]
fn a_claim_before_the_slot_check_turns_red() {
    let broken = mutate(
        SUBMIT,
        "    : >\"$T/blockers\"\n",
        "    [[ $mode == reenqueue ]] && ln -sT x \"$job/.reenqueue-1\"\n    : >\"$T/blockers\"\n",
    );
    let red = claim_after_slot_check(&broken).unwrap_err();
    assert!(red.starts_with("slot busy wrote into the job dir"), "{red}");
}

/// o25.4 / o25.14.5: `.enqueued`'s own socket, when it differs from the slot's, is listed too — a
/// queued row there refuses, a finished one does not — and only then.
#[test]
fn a_reenqueue_checks_the_enqueued_socket_when_it_differs() {
    let _serial = serial();
    let mut lab = Lab::new();
    let other = lab.root.join("tsp").join("old.sock");
    let (reply, claimed, _) = attempt(
        &mut lab,
        "rq-other-queued",
        |lab, job, _, args| {
            fs::write(job.join(".enqueued"), format!("socket={}\nid=3\n", Lab::str(&other))).unwrap();
            let queued = row(lab, job, args, 3, "queued");
            lab_listen_with_rows(lab, &other, &[queued]);
        },
        SUBMIT,
    );
    assert!(matches!(&reply, SubmitReply::Refused(Refusal::Other(why)) if why.starts_with(&format!("row: a tsp row on {}", Lab::str(&other)))), "{reply:?}");
    assert!(!claimed);
    let (reply, claimed, _) = attempt(
        &mut lab,
        "rq-other-finished",
        |lab, job, _, args| {
            fs::write(job.join(".enqueued"), format!("socket={}\nid=3\n", Lab::str(&other))).unwrap();
            let finished = row(lab, job, args, 3, "finished");
            lab_listen_with_rows(lab, &other, &[finished]);
        },
        SUBMIT,
    );
    assert!(matches!(reply, SubmitReply::Enqueued(_)), "{reply:?}");
    assert!(claimed);
}

/// o25.2: `.reenqueue-1` is in the download set's server listing (the `list` script), as the dangling
/// symlink the re-enqueue makes; the rsync half is `sync`'s download gate. NEGATIVE CONTROL
/// (`run_54b_controls.sh markers-no-claim`): drop it from `sync::MARKERS` → red.
#[test]
fn the_reenqueue_claim_is_listed_for_the_download() {
    let lab = Lab::new();
    let job = lab.job("listed");
    fs::write(job.join(".started"), "x").unwrap();
    std::os::unix::fs::symlink("x", job.join(".reenqueue-1")).unwrap();
    let args = ListArgs::new(Lab::str(&job), FetchPolicy::SMALL_ONLY).unwrap();
    let out = super::call_script_tests::run(&lab, LIST, &args.values(), &[]);
    let server = parse_list_reply(&out.stdout, &args).unwrap();
    assert!(
        server.iter().any(|f| f.name == ".reenqueue-1" && f.digest == Digest::Symlink("x".into())),
        "{server:?}"
    );
}
