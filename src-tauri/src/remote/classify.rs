//! The remote-job classifier: `classify(snapshot, reenqueue_count)`, ADR-024 Decision l.
//!
//! Raw facts in, decision here (rule #9). The 11-row precedence table is implemented in
//! [`classify`] in exactly the ADR's order — first match wins — and each row is marked in the
//! code with its number. The predicates the rows use ("alive", "ours", the job session, the
//! SID-reuse guard) are public so Part B can test the shell scripts against the same
//! definitions.

use std::borrow::Cow;

use super::markers::{parse_exit_code, parse_started, BootId, Started};
use super::procfs::{parse_stat, split_cmdline, ProcStat};
use super::snapshot::{Attempt, JobIdentity, SessionMember, Snapshot, SocketState};
use super::tsp::{match_job_row, TspState};
use super::FactError;
use crate::local_backend::{has_normal_termination, TAIL_BYTES};

/// The classifier's verdict on a job (ADR-024 l). `Lost` and `Cancelling` are not
/// `JobStatus` values yet; they join it in unit 5.4.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Waiting in a `tsp` queue (row 9).
    Queued,
    /// The wrapper is alive and ours (row 7).
    Running,
    /// `.exit_code` = 0 and `ORCA TERMINATED NORMALLY` (rows 2, 5). `late_cancel`: the user
    /// cancelled, but the job had already finished cleanly; the result is kept.
    Completed { late_cancel: bool },
    /// Terminal failure (rows 1, 6, 11).
    Failed { reason: FailReason },
    /// The job started and then vanished without an `.exit_code` (row 8). `orphans` are the
    /// job-session PIDs still holding the slot's cores; non-empty only with a current `boot_id`.
    Lost { orphans: Vec<u32> },
    /// Cancelled, but some of the job's processes still run (row 3). Transient.
    Cancelling,
    /// Cancelled, nothing of it runs (row 4).
    Cancelled,
    /// Not enough information to act safely; take no action this pass (row 10, or a retake
    /// whose `.started` reads still disagree).
    Indeterminate,
    /// Nothing ever ran; re-enqueue the original input (row 11, first time).
    ReEnqueue,
}

/// Why a job is `Failed`, in words the UI can show.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FailReason {
    /// Row 1.
    #[error("corrupt .started marker ({detail}); without its sid the job's processes cannot be swept")]
    CorruptStarted { detail: String },
    /// Row 6: `.exit_code` is a number other than 0.
    #[error("the job exited with code {code}")]
    NonZeroExit { code: u8 },
    /// Row 6: `.exit_code` is present but not a valid exit code (e.g. empty).
    #[error("unreadable .exit_code ({detail})")]
    BadExitCode { detail: String },
    /// Row 6: exit code 0, but no `ORCA TERMINATED NORMALLY` in the output tail (rule #6).
    #[error("exit code 0 but ORCA did not terminate normally")]
    NoNormalTermination,
    /// Row 11 after the one automatic re-enqueue.
    #[error("wrapper never started")]
    WrapperNeverStarted,
}

/// What [`classify`] asks the caller to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Classification {
    /// A verdict.
    Decided(Outcome),
    /// The two `.started` reads of a first snapshot disagree: discard it, collect a new one with
    /// [`Attempt::Retake`] and classify that. Never returned for a retake, so there is at most
    /// one.
    Retake,
}

/// The snapshot itself is unusable — a broken collection, not a fact about the job. The caller
/// takes no action this pass, exactly as for `Indeterminate`, and reports the error.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SnapshotError {
    #[error("invalid job identity: {0}")]
    BadIdentity(String),
    #[error("the snapshot has no socket facts")]
    NoSocketFacts,
    #[error("current boot id: {0}")]
    CurrentBootId(FactError),
    #[error("wrapper stat: {0}")]
    WrapperStat(FactError),
    #[error("wrapper stat is for pid {found}, .started says {expected}")]
    StatPidMismatch { expected: u32, found: u32 },
    #[error("SID stat: {0}")]
    SidStat(FactError),
    #[error("SID stat is for pid {found}, .started's sid is {expected}")]
    SidStatPidMismatch { expected: u32, found: u32 },
}

/// Classify one job from its snapshot. `reenqueue_count` is how many times the job was already
/// re-enqueued as never-started (persisted by 5.4 before each re-enqueue).
pub fn classify(snap: &Snapshot, reenqueue_count: u32) -> Result<Classification, SnapshotError> {
    validate_identity(&snap.identity)?;
    if snap.sockets.is_empty() {
        return Err(SnapshotError::NoSocketFacts);
    }
    let current_boot = BootId::parse(&snap.current_boot_id).map_err(SnapshotError::CurrentBootId)?;

    // The bracketing `.started` reads. If they differ, the job started (or something odd
    // happened) during collection, so no other fact can be trusted together with either read.
    if snap.started_first != snap.started_last {
        return Ok(match snap.attempt {
            Attempt::First => Classification::Retake,
            Attempt::Retake => Classification::Decided(Outcome::Indeterminate),
        });
    }
    decide(snap, &current_boot, reenqueue_count).map(Classification::Decided)
}

/// The precedence table, rows 1–11, first match wins.
fn decide(snap: &Snapshot, current_boot: &BootId, reenqueue_count: u32) -> Result<Outcome, SnapshotError> {
    // Row 1: `.started` exists but does not parse.
    let started: Option<Started> = match &snap.started_first {
        None => None,
        Some(raw) => match parse_started(raw) {
            Ok(started) => Some(started),
            Err(e) => {
                return Ok(Outcome::Failed {
                    reason: FailReason::CorruptStarted { detail: e.to_string() },
                })
            }
        },
    };

    let exit_code = snap.exit_code.as_deref().map(parse_exit_code);
    let finished_cleanly =
        matches!(exit_code, Some(Ok(0))) && tail_terminated_normally(&snap.output_tail);
    // `Some` only when `.started` exists and was written in this boot.
    let started_this_boot = started.as_ref().filter(|s| &s.boot_id == current_boot);

    if snap.cancelled {
        // Row 2: the cancel came too late; a clean result is never discarded.
        if finished_cleanly {
            return Ok(Outcome::Completed { late_cancel: true });
        }
        // Row 3: something of the job still runs. The session is checked first: if it is
        // non-empty the wrapper's state does not change the answer.
        if let Some(started) = started_this_boot {
            if !guarded_job_session(snap, started)?.is_empty()
                || wrapper_alive_and_ours(snap, started, current_boot)?
            {
                return Ok(Outcome::Cancelling);
            }
        }
        // Row 4.
        return Ok(Outcome::Cancelled);
    }

    // Row 5.
    if finished_cleanly {
        return Ok(Outcome::Completed { late_cancel: false });
    }

    // Row 6: `.exit_code` present, but not a clean completion (rule #6).
    if let Some(parsed) = exit_code {
        let reason = match parsed {
            Ok(0) => FailReason::NoNormalTermination,
            Ok(code) => FailReason::NonZeroExit { code },
            Err(e) => FailReason::BadExitCode { detail: e.to_string() },
        };
        return Ok(Outcome::Failed { reason });
    }

    // Row 7.
    if let Some(started) = started_this_boot {
        if wrapper_alive_and_ours(snap, started, current_boot)? {
            return Ok(Outcome::Running);
        }
    }
    // Row 8: started, and its wrapper is gone (another boot, dead, or not ours). Orphans exist
    // only within the same boot.
    if started.is_some() {
        let orphans = match started_this_boot {
            Some(started) => guarded_job_session(snap, started)?,
            None => Vec::new(),
        };
        return Ok(Outcome::Lost { orphans });
    }

    // Rows 9–11: no `.started`. Read what the sockets say about this job.
    let mut queued_or_running = false;
    let mut any_error = false;
    for fact in &snap.sockets {
        match &fact.state {
            // The queue lived in the dead daemon's memory: no rows.
            SocketState::NoDaemon => {}
            SocketState::Error(_) => any_error = true,
            SocketState::Rows(rows) => {
                for row in rows {
                    match match_job_row(row, &snap.identity.job_dir) {
                        Ok(None) => {}
                        Ok(Some(row)) => {
                            if matches!(row.state, TspState::Queued | TspState::Running) {
                                queued_or_running = true;
                            }
                        }
                        // Our row, unreadable: the query did not give a usable answer.
                        Err(_) => any_error = true,
                    }
                }
            }
        }
    }

    // Row 9.
    if queued_or_running {
        return Ok(Outcome::Queued);
    }
    // Row 10.
    if any_error {
        return Ok(Outcome::Indeterminate);
    }
    // Row 11: a finished row (the wrapper crashed before `.started`) or no row at all (a
    // restart dropped the queue) — never started.
    if reenqueue_count == 0 {
        Ok(Outcome::ReEnqueue)
    } else {
        Ok(Outcome::Failed { reason: FailReason::WrapperNeverStarted })
    }
}

/// Rule #6's marker test over the same 5 KiB window the local backend reads.
fn tail_terminated_normally(tail: &[u8]) -> bool {
    let window = usize::try_from(TAIL_BYTES).unwrap_or(usize::MAX);
    let start = tail.len().saturating_sub(window);
    let text: Cow<'_, str> = String::from_utf8_lossy(&tail[start..]);
    has_normal_termination(&text)
}

/// Rows 3 and 7: is the `.started` wrapper alive and ours? A missing stat means dead. A stat
/// line that does not parse, or is about another PID, is a broken snapshot — reading it as
/// "dead" would declare a job that still holds cores `Lost`.
fn wrapper_alive_and_ours(snap: &Snapshot, started: &Started, current_boot: &BootId) -> Result<bool, SnapshotError> {
    let stat = match &snap.wrapper_stat {
        None => None,
        Some(raw) => {
            let stat = parse_stat(raw).map_err(SnapshotError::WrapperStat)?;
            if stat.pid != started.pid {
                return Err(SnapshotError::StatPidMismatch { expected: started.pid, found: stat.pid });
            }
            Some(stat)
        }
    };
    Ok(is_alive(stat.as_ref(), started, current_boot)
        && is_our_wrapper(&snap.wrapper_cmdline, &snap.identity))
}

/// The job session as rows 3 and 8 use it: empty if `.started`'s SID has been reused (the
/// cancel script's guard, ADR-024 l), otherwise the cwd-filtered members. Without the guard a
/// foreign process sitting in the job dir under a reused SID would hold a cancelled job in
/// `Cancelling` forever (the guarded sweep signals nothing) or show up as a phantom orphan.
/// Called only with a `.started` from the current boot. A SID stat line that does not parse, or
/// names another pid, is a broken snapshot.
fn guarded_job_session(snap: &Snapshot, started: &Started) -> Result<Vec<u32>, SnapshotError> {
    let sid_stat = match &snap.sid_stat {
        None => None,
        Some(raw) => {
            let stat = parse_stat(raw).map_err(SnapshotError::SidStat)?;
            if stat.pid != started.sid {
                return Err(SnapshotError::SidStatPidMismatch { expected: started.sid, found: stat.pid });
            }
            Some(stat)
        }
    };
    if is_sid_reused(sid_stat.as_ref(), started) {
        return Ok(Vec::new());
    }
    Ok(job_session(&snap.session_members, &snap.identity.job_dir))
}

/// **Alive** (ADR-024 l): `.started` is from the current boot, the stat line exists, its state is
/// not `Z`, and its start time (field 22) equals the one recorded in `.started`. A zombie is dead
/// for every rule; a different start time means the PID now belongs to another process.
pub fn is_alive(stat: Option<&ProcStat>, started: &Started, current_boot: &BootId) -> bool {
    match stat {
        None => false,
        Some(stat) => {
            &started.boot_id == current_boot
                && !stat.is_zombie()
                && stat.starttime == started.starttime
        }
    }
}

/// **Ours** (ADR-024 l): the cmdline runs our wrapper for **this** job — argv[0] is `bash`,
/// argv[1] is `<root>/bin/wrapper-<sha>.sh` for any lowercase-hex sha (an older wrapper keeps
/// running after an upgrade), and argv[2] is exactly this job's dir.
pub fn is_our_wrapper(cmdline: &[u8], identity: &JobIdentity) -> bool {
    let argv = split_cmdline(cmdline);
    if argv.len() < 3 || argv[0] != b"bash" || argv[2] != identity.job_dir.as_bytes() {
        return false;
    }
    let prefix = format!("{}/bin/wrapper-", identity.root);
    argv[1]
        .strip_prefix(prefix.as_bytes())
        .and_then(|rest| rest.strip_suffix(b".sh"))
        .is_some_and(|sha| !sha.is_empty() && sha.iter().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(b)))
}

/// The **job session**: the PIDs of the `ps -s <sid>` members whose cwd is exactly this job's
/// dir. A member whose cwd could not be read (ENOENT) is not in it.
pub fn job_session(members: &[SessionMember], job_dir: &str) -> Vec<u32> {
    members
        .iter()
        .filter(|m| m.cwd.as_deref() == Some(job_dir.as_bytes()))
        .map(|m| m.pid)
        .collect()
}

/// The **SID-reuse guard** (ADR-024 i/l, used by the cancel sweep): the session id in `.started`
/// has been reused only if a process exists at that number with a **different** start time.
/// Absent, or the same start time (alive, or a zombie not yet reaped), means the SID is still
/// ours. A stat line that does not parse is an error, never a guess.
pub fn sid_reused(sid_stat: Option<&[u8]>, started: &Started) -> Result<bool, FactError> {
    let stat = sid_stat.map(parse_stat).transpose()?;
    Ok(is_sid_reused(stat.as_ref(), started))
}

/// [`sid_reused`] over an already-parsed stat line.
fn is_sid_reused(sid_stat: Option<&ProcStat>, started: &Started) -> bool {
    sid_stat.is_some_and(|stat| stat.starttime != started.starttime)
}

/// The job dir and root must be absolute paths over `[A-Za-z0-9._/-]` without a trailing `/` —
/// what submit asserts. Re-checked here because whole-token row matching and exact cwd
/// comparison both depend on it.
fn validate_identity(identity: &JobIdentity) -> Result<(), SnapshotError> {
    for (name, path) in [("job_dir", &identity.job_dir), ("root", &identity.root)] {
        let ok = path.len() > 1
            && path.starts_with('/')
            && !path.ends_with('/')
            && path
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'/' | b'-'));
        if !ok {
            return Err(SnapshotError::BadIdentity(format!("{name} {path:?}")));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote::procfs::fixtures::{stat_bash_live, stat_bash_zombie, CMDLINE_P2};
    use crate::remote::snapshot::SocketFact;
    use crate::remote::tsp::fixtures::{finished_row_for, queued_row_for, running_row_for};

    // The recorded probe-5.2 root and job dir (probe P2).
    const ROOT: &str = "/home/anton/.orcastudio/probe-5.2";
    const JOB: &str = "/home/anton/.orcastudio/probe-5.2/jobs/j1";
    // Synthetic boot ids: the probe pages record none.
    const BOOT: &str = "0f6e3c1a-5b7d-4c2e-9a8b-1d2e3f4a5b6c";
    const OTHER_BOOT: &str = "9b1d0e2f-3a4c-4d5e-8f60-718293a4b5c6";
    // The wrapper's identity comes from the recorded live bash of probe 5.2c: PID 66401,
    // starttime 1891976 (`stat_bash_live`).
    const WRAPPER_PID: u32 = 66401;
    const WRAPPER_STARTTIME: u64 = 1891976;
    const SOCKET_A: &str = "/home/anton/.orcastudio/tsp/slot0.sock";
    const SOCKET_B: &str = "/home/anton/.orcastudio/tsp/slot1.sock";

    fn started_bytes(boot: &str, starttime: u64) -> Vec<u8> {
        format!(
            "pid={WRAPPER_PID}\npgid={WRAPPER_PID}\nsid={WRAPPER_PID}\nboot_id={boot}\nstarttime={starttime}\nstarted_at=1759490000\n"
        )
        .into_bytes()
    }

    /// The recorded P2 cmdline with the content-addressed name 5.3 uploads
    /// (`bin/wrapper-<sha>.sh`). The probe ran `bin/wrapper.sh` without a sha (ADR-024 l,
    /// round 2 LOW-5); the sha here is synthetic.
    fn our_cmdline() -> Vec<u8> {
        replace_bytes(CMDLINE_P2, b"/bin/wrapper.sh", b"/bin/wrapper-3f2a9c1b.sh")
    }

    fn replace_bytes(haystack: &[u8], from: &[u8], to: &[u8]) -> Vec<u8> {
        let text = String::from_utf8_lossy(haystack).replace(
            std::str::from_utf8(from).unwrap(),
            std::str::from_utf8(to).unwrap(),
        );
        text.into_bytes()
    }

    fn identity() -> JobIdentity {
        JobIdentity { job_dir: JOB.into(), root: ROOT.into() }
    }

    fn member(pid: u32, cwd: Option<&str>) -> SessionMember {
        SessionMember { pid, cwd: cwd.map(|c| c.as_bytes().to_vec()) }
    }

    fn socket(path: &str, state: SocketState) -> SocketFact {
        SocketFact { socket_path: path.into(), state }
    }

    /// A job running normally: `.started` from this boot, the wrapper alive and ours, its
    /// session in the job dir, a `running` row on slot 0.
    fn running() -> Snapshot {
        let started = started_bytes(BOOT, WRAPPER_STARTTIME);
        Snapshot {
            identity: identity(),
            attempt: Attempt::First,
            current_boot_id: format!("{BOOT}\n"),
            started_first: Some(started.clone()),
            wrapper_stat: Some(stat_bash_live().into_bytes()),
            wrapper_cmdline: our_cmdline(),
            // PID = SID: the wrapper's own line.
            sid_stat: Some(stat_bash_live().into_bytes()),
            session_members: vec![member(WRAPPER_PID, Some(JOB)), member(WRAPPER_PID + 1, Some(JOB))],
            sockets: vec![
                socket(SOCKET_A, SocketState::Rows(vec![running_row_for(JOB)])),
                socket(SOCKET_B, SocketState::NoDaemon),
            ],
            exit_code: None,
            cancelled: false,
            output_tail: b"SCF ITERATIONS\n".to_vec(),
            started_last: Some(started),
        }
    }

    /// The wrapper and its whole session are gone.
    fn dead(mut snap: Snapshot) -> Snapshot {
        snap.wrapper_stat = None;
        snap.wrapper_cmdline = Vec::new();
        snap.sid_stat = None;
        snap.session_members = Vec::new();
        snap
    }

    /// Finished cleanly: exit 0 and ORCA's marker in the tail.
    fn finished_cleanly(snap: Snapshot) -> Snapshot {
        let mut snap = dead(snap);
        snap.exit_code = Some(b"0\n".to_vec());
        snap.output_tail = b"TOTAL RUN TIME: 0 days 0 hours 0 minutes 8 seconds\n****ORCA TERMINATED NORMALLY****\n".to_vec();
        snap
    }

    /// Never started: no `.started` anywhere, wrapper facts empty.
    fn not_started(sockets: Vec<SocketFact>) -> Snapshot {
        let mut snap = dead(running());
        snap.started_first = None;
        snap.started_last = None;
        snap.sockets = sockets;
        snap
    }

    fn outcome(snap: &Snapshot) -> Outcome {
        outcome_with(snap, 0)
    }

    fn outcome_with(snap: &Snapshot, reenqueue_count: u32) -> Outcome {
        match classify(snap, reenqueue_count) {
            Ok(Classification::Decided(o)) => o,
            other => panic!("expected a verdict, got {other:?}"),
        }
    }

    fn failed(reason: FailReason) -> Outcome {
        Outcome::Failed { reason }
    }

    // --- Row 1 -----------------------------------------------------------------------------

    #[test]
    fn row1_corrupt_started_fails() {
        for raw in [&b""[..], b"pid=66401\n", b"garbage"] {
            let mut snap = running();
            snap.started_first = Some(raw.to_vec());
            snap.started_last = Some(raw.to_vec());
            assert!(
                matches!(outcome(&snap), Outcome::Failed { reason: FailReason::CorruptStarted { .. } }),
                "{raw:?}"
            );
        }
    }

    #[test]
    fn row1_precedes_even_a_clean_cancelled_completion() {
        // Literal precedence: without a parseable `.started` the job cannot be swept, and the
        // user must see that (ADR-024 l, round 2 LOW-3).
        let mut snap = finished_cleanly(running());
        snap.cancelled = true;
        snap.started_first = Some(Vec::new());
        snap.started_last = Some(Vec::new());
        assert!(matches!(outcome(&snap), Outcome::Failed { reason: FailReason::CorruptStarted { .. } }));
    }

    // --- Rows 2–4: cancelled ---------------------------------------------------------------

    #[test]
    fn row2_late_cancel_keeps_a_clean_result() {
        let mut snap = finished_cleanly(running());
        snap.cancelled = true;
        assert_eq!(outcome(&snap), Outcome::Completed { late_cancel: true });
    }

    #[test]
    fn row3_cancelled_with_a_live_wrapper_is_cancelling() {
        let mut snap = running();
        snap.cancelled = true;
        assert_eq!(outcome(&snap), Outcome::Cancelling);
    }

    #[test]
    fn row3_cancelled_with_only_orphans_left_is_cancelling() {
        let mut snap = dead(running());
        snap.cancelled = true;
        snap.session_members = vec![member(70001, Some(JOB))];
        assert_eq!(outcome(&snap), Outcome::Cancelling);
    }

    #[test]
    fn row4_cancelled_and_nothing_runs_is_cancelled() {
        let mut snap = dead(running());
        snap.cancelled = true;
        // A member that exited (cwd ENOENT) and one elsewhere do not count.
        snap.session_members = vec![member(70001, None), member(70002, Some("/home/anton"))];
        assert_eq!(outcome(&snap), Outcome::Cancelled);
    }

    #[test]
    fn row4_cancelled_from_another_boot_is_cancelled() {
        let mut snap = running();
        snap.cancelled = true;
        snap.current_boot_id = OTHER_BOOT.into();
        assert_eq!(outcome(&snap), Outcome::Cancelled);
    }

    #[test]
    fn row4_cancelled_before_it_started_is_cancelled_never_reenqueued() {
        let mut snap = not_started(vec![socket(SOCKET_A, SocketState::NoDaemon)]);
        snap.cancelled = true;
        assert_eq!(outcome(&snap), Outcome::Cancelled);
    }

    #[test]
    fn row4_cancelled_with_exit_zero_but_no_marker_is_cancelled() {
        let mut snap = dead(running());
        snap.cancelled = true;
        snap.exit_code = Some(b"0\n".to_vec());
        assert_eq!(outcome(&snap), Outcome::Cancelled);
    }

    // --- Rows 5–6: `.exit_code` ------------------------------------------------------------

    #[test]
    fn row5_exit_zero_and_marker_is_completed() {
        assert_eq!(outcome(&finished_cleanly(running())), Outcome::Completed { late_cancel: false });
    }

    #[test]
    fn row5_marker_must_be_in_the_last_5_kib() {
        // The same window the local backend reads (`TAIL_BYTES`): a marker further back does
        // not count.
        let mut snap = finished_cleanly(running());
        snap.output_tail.extend(std::iter::repeat_n(b'x', 5 * 1024));
        assert_eq!(outcome(&snap), failed(FailReason::NoNormalTermination));
    }

    #[test]
    fn row6_nonzero_exit_fails() {
        let mut snap = finished_cleanly(running());
        snap.exit_code = Some(b"3\n".to_vec());
        assert_eq!(outcome(&snap), failed(FailReason::NonZeroExit { code: 3 }));
    }

    #[test]
    fn row6_wrapper_self_check_code_97_fails() {
        let mut snap = dead(running());
        snap.exit_code = Some(b"97\n".to_vec());
        assert_eq!(outcome(&snap), failed(FailReason::NonZeroExit { code: 97 }));
    }

    #[test]
    fn row6_exit_zero_without_marker_fails() {
        let mut snap = dead(running());
        snap.exit_code = Some(b"0\n".to_vec());
        assert_eq!(outcome(&snap), failed(FailReason::NoNormalTermination));
    }

    #[test]
    fn row6_empty_or_garbage_exit_code_fails() {
        for raw in [&b""[..], b"abc", b"-1"] {
            let mut snap = finished_cleanly(running());
            snap.exit_code = Some(raw.to_vec());
            assert!(
                matches!(outcome(&snap), Outcome::Failed { reason: FailReason::BadExitCode { .. } }),
                "{raw:?}"
            );
        }
    }

    #[test]
    fn row6_exit_code_wins_over_a_live_wrapper() {
        // `.exit_code` is written by rename just before the wrapper exits; once present the
        // job's verdict is final.
        let mut snap = running();
        snap.exit_code = Some(b"1\n".to_vec());
        assert_eq!(outcome(&snap), failed(FailReason::NonZeroExit { code: 1 }));
    }

    // --- Rows 7–8: started, no `.exit_code` --------------------------------------------------

    #[test]
    fn row7_live_wrapper_is_running() {
        assert_eq!(outcome(&running()), Outcome::Running);
    }

    #[test]
    fn row7_running_needs_no_tsp_row() {
        // A running job survives `tsp -K` with its row gone (probe P4).
        let mut snap = running();
        snap.sockets = vec![socket(SOCKET_A, SocketState::NoDaemon)];
        assert_eq!(outcome(&snap), Outcome::Running);
    }

    #[test]
    fn row8_stale_boot_id_is_lost_without_orphans() {
        // Restart simulation: the host rebooted under the job. Field 22 counts from the new
        // boot, so the old session's facts mean nothing; even members passed in are ignored.
        let mut snap = running();
        snap.current_boot_id = OTHER_BOOT.into();
        assert_eq!(outcome(&snap), Outcome::Lost { orphans: Vec::new() });
    }

    #[test]
    fn row8_dead_wrapper_with_orphans_is_lost_with_them() {
        let mut snap = dead(running());
        snap.session_members =
            vec![member(70001, Some(JOB)), member(70002, None), member(70003, Some("/home/anton"))];
        assert_eq!(outcome(&snap), Outcome::Lost { orphans: vec![70001] });
    }

    #[test]
    fn zombie_wrapper_is_not_alive() {
        let mut snap = running();
        snap.wrapper_stat = Some(stat_bash_zombie().into_bytes());
        snap.wrapper_cmdline = Vec::new(); // a zombie's cmdline is empty (probe 5.2c)
        snap.session_members = Vec::new();
        assert_eq!(outcome(&snap), Outcome::Lost { orphans: Vec::new() });

        // Even with the cmdline still ours, state Z alone makes it dead.
        snap.wrapper_cmdline = our_cmdline();
        assert_eq!(outcome(&snap), Outcome::Lost { orphans: Vec::new() });
    }

    #[test]
    fn forged_starttime_is_not_alive() {
        // The PID now belongs to a process with another start time (PID reuse), even though
        // its cmdline is ours.
        let mut snap = running();
        let forged = started_bytes(BOOT, WRAPPER_STARTTIME + 5);
        snap.started_first = Some(forged.clone());
        snap.started_last = Some(forged);
        snap.session_members = Vec::new();
        assert_eq!(outcome(&snap), Outcome::Lost { orphans: Vec::new() });
    }

    #[test]
    fn another_jobs_wrapper_at_our_pid_is_not_ours() {
        let mut snap = running();
        snap.wrapper_cmdline = replace_bytes(&our_cmdline(), b"/jobs/j1\0", b"/jobs/j10\0");
        snap.session_members = Vec::new();
        assert_eq!(outcome(&snap), Outcome::Lost { orphans: Vec::new() });
    }

    // --- The SID-reuse guard on the job session ---------------------------------------------

    /// A process now at the wrapper's SID number with another start time — the recorded live
    /// bash line with field 22 changed (synthetic; forced PID reuse was not measured).
    fn reused_sid_stat() -> Vec<u8> {
        stat_bash_live().replace(" 1891976 ", " 2000000 ").into_bytes()
    }

    #[test]
    fn reused_sid_foreign_process_in_job_dir_does_not_hold_a_cancel() {
        let mut snap = dead(running());
        snap.cancelled = true;
        snap.sid_stat = Some(reused_sid_stat());
        snap.session_members = vec![member(70001, Some(JOB))];
        assert_eq!(outcome(&snap), Outcome::Cancelled);
    }

    #[test]
    fn reused_sid_foreign_process_in_job_dir_is_not_an_orphan() {
        let mut snap = dead(running());
        snap.sid_stat = Some(reused_sid_stat());
        snap.session_members = vec![member(70001, Some(JOB))];
        assert_eq!(outcome(&snap), Outcome::Lost { orphans: Vec::new() });
    }

    #[test]
    fn zombie_wrapper_at_the_sid_keeps_the_session() {
        // Probe 5.2c: the zombie keeps field 22, so the SID is still ours and the live rank
        // in the job dir is ours too.
        let mut snap = running();
        snap.wrapper_stat = Some(stat_bash_zombie().into_bytes());
        snap.wrapper_cmdline = Vec::new();
        snap.sid_stat = Some(stat_bash_zombie().into_bytes());
        snap.session_members = vec![member(WRAPPER_PID, None), member(70001, Some(JOB))];
        assert_eq!(outcome(&snap), Outcome::Lost { orphans: vec![70001] });
        snap.cancelled = true;
        assert_eq!(outcome(&snap), Outcome::Cancelling);
    }

    #[test]
    fn broken_sid_stat_is_an_error_only_when_needed() {
        let mut snap = dead(running());
        snap.sid_stat = Some(b"garbage".to_vec());
        assert!(matches!(classify(&snap, 0), Err(SnapshotError::SidStat(_))));

        let mut snap = dead(running());
        snap.sid_stat = Some(stat_bash_live().replacen("66401", "66402", 1).into_bytes());
        assert_eq!(
            classify(&snap, 0),
            Err(SnapshotError::SidStatPidMismatch { expected: 66401, found: 66402 })
        );

        // Another boot: no job session is computed, so the line is irrelevant.
        let mut snap = dead(running());
        snap.current_boot_id = OTHER_BOOT.into();
        snap.sid_stat = Some(b"garbage".to_vec());
        assert_eq!(outcome(&snap), Outcome::Lost { orphans: Vec::new() });

        // A clean finish never needs it.
        let mut snap = finished_cleanly(running());
        snap.sid_stat = Some(b"garbage".to_vec());
        assert_eq!(outcome(&snap), Outcome::Completed { late_cancel: false });
    }

    // --- Rows 9–11: never started ----------------------------------------------------------

    #[test]
    fn row9_queued_or_running_row_is_queued() {
        let snap = not_started(vec![socket(SOCKET_A, SocketState::Rows(vec![queued_row_for(JOB)]))]);
        assert_eq!(outcome(&snap), Outcome::Queued);
        let snap = not_started(vec![socket(SOCKET_A, SocketState::Rows(vec![running_row_for(JOB)]))]);
        assert_eq!(outcome(&snap), Outcome::Queued);
    }

    #[test]
    fn row9_queued_row_beats_an_error_on_another_socket() {
        let snap = not_started(vec![
            socket(SOCKET_A, SocketState::Error("ssh: connection reset".into())),
            socket(SOCKET_B, SocketState::Rows(vec![queued_row_for(JOB)])),
        ]);
        assert_eq!(outcome(&snap), Outcome::Queued);
    }

    #[test]
    fn row10_socket_error_is_indeterminate() {
        let snap = not_started(vec![
            socket(SOCKET_A, SocketState::NoDaemon),
            socket(SOCKET_B, SocketState::Error("tsp -l exited 255".into())),
        ]);
        assert_eq!(outcome(&snap), Outcome::Indeterminate);
        // Not even after the one re-enqueue: no action at all.
        assert_eq!(outcome_with(&snap, 1), Outcome::Indeterminate);
    }

    #[test]
    fn row10_unreadable_row_of_ours_is_indeterminate() {
        let row = queued_row_for(JOB).replace("queued ", "weird  ");
        let snap = not_started(vec![socket(SOCKET_A, SocketState::Rows(vec![row]))]);
        assert_eq!(outcome(&snap), Outcome::Indeterminate);
    }

    #[test]
    fn row11_no_daemon_anywhere_is_never_started() {
        let snap = not_started(vec![
            socket(SOCKET_A, SocketState::NoDaemon),
            socket(SOCKET_B, SocketState::NoDaemon),
        ]);
        assert_eq!(outcome_with(&snap, 0), Outcome::ReEnqueue);
        assert_eq!(outcome_with(&snap, 1), failed(FailReason::WrapperNeverStarted));
    }

    #[test]
    fn row11_empty_rows_is_never_started() {
        // Restart simulation for a queued job: `tsp -K` (or a reboot) drops the queue.
        let snap = not_started(vec![socket(SOCKET_A, SocketState::Rows(Vec::new()))]);
        assert_eq!(outcome_with(&snap, 0), Outcome::ReEnqueue);
    }

    #[test]
    fn row11_finished_row_without_started_is_never_started() {
        // The wrapper crashed before writing `.started`.
        let snap = not_started(vec![socket(SOCKET_A, SocketState::Rows(vec![finished_row_for(JOB)]))]);
        assert_eq!(outcome_with(&snap, 0), Outcome::ReEnqueue);
        assert_eq!(outcome_with(&snap, 1), failed(FailReason::WrapperNeverStarted));
    }

    #[test]
    fn foreign_row_sharing_a_prefix_is_not_matched() {
        // `/jobs/j10` is queued; our job `/jobs/j1` is not in any queue.
        let foreign = queued_row_for(&format!("{JOB}0"));
        let snap = not_started(vec![socket(SOCKET_A, SocketState::Rows(vec![foreign]))]);
        assert_eq!(outcome(&snap), Outcome::ReEnqueue);
    }

    // --- The bracketing `.started` reads -----------------------------------------------------

    #[test]
    fn started_appearing_during_collection_asks_for_one_retake() {
        let mut snap = not_started(vec![socket(SOCKET_A, SocketState::NoDaemon)]);
        snap.started_last = Some(started_bytes(BOOT, WRAPPER_STARTTIME));
        assert_eq!(classify(&snap, 0), Ok(Classification::Retake));

        snap.attempt = Attempt::Retake;
        assert_eq!(classify(&snap, 0), Ok(Classification::Decided(Outcome::Indeterminate)));
    }

    #[test]
    fn started_vanishing_during_collection_is_never_trusted() {
        let mut snap = running();
        snap.started_last = None;
        assert_eq!(classify(&snap, 0), Ok(Classification::Retake));
        snap.attempt = Attempt::Retake;
        assert_eq!(classify(&snap, 0), Ok(Classification::Decided(Outcome::Indeterminate)));
    }

    #[test]
    fn a_consistent_retake_is_classified_normally() {
        let mut snap = running();
        snap.attempt = Attempt::Retake;
        assert_eq!(classify(&snap, 0), Ok(Classification::Decided(Outcome::Running)));
    }

    // --- Broken snapshots ------------------------------------------------------------------

    #[test]
    fn broken_snapshots_are_errors_not_verdicts() {
        let mut snap = running();
        snap.current_boot_id = "not-a-uuid".into();
        assert!(matches!(classify(&snap, 0), Err(SnapshotError::CurrentBootId(_))));

        let mut snap = running();
        snap.sockets.clear();
        assert_eq!(classify(&snap, 0), Err(SnapshotError::NoSocketFacts));

        // A garbled stat must not read as "dead" (that would be `Lost` for a computing job).
        let mut snap = running();
        snap.wrapper_stat = Some(b"66401 (bash) S garbage".to_vec());
        assert!(matches!(classify(&snap, 0), Err(SnapshotError::WrapperStat(_))));

        let mut snap = running();
        snap.wrapper_stat = Some(stat_bash_live().replacen("66401", "66402", 1).into_bytes());
        assert_eq!(
            classify(&snap, 0),
            Err(SnapshotError::StatPidMismatch { expected: 66401, found: 66402 })
        );

        for bad in ["jobs/j1", "/home/anton/jobs/j1/", "/home/anton/jobs/j 1", "/"] {
            let mut snap = running();
            snap.identity.job_dir = bad.into();
            assert!(matches!(classify(&snap, 0), Err(SnapshotError::BadIdentity(_))), "{bad:?}");
        }
    }

    #[test]
    fn a_garbled_stat_is_irrelevant_from_another_boot() {
        let mut snap = running();
        snap.current_boot_id = OTHER_BOOT.into();
        snap.wrapper_stat = Some(b"garbage".to_vec());
        assert_eq!(outcome(&snap), Outcome::Lost { orphans: Vec::new() });
    }

    // --- Predicates ------------------------------------------------------------------------

    fn started() -> Started {
        parse_started(&started_bytes(BOOT, WRAPPER_STARTTIME)).unwrap()
    }

    fn boot() -> BootId {
        BootId::parse(BOOT).unwrap()
    }

    #[test]
    fn alive_needs_a_live_state_the_same_starttime_and_this_boot() {
        let live = parse_stat(stat_bash_live().as_bytes()).unwrap();
        let zombie = parse_stat(stat_bash_zombie().as_bytes()).unwrap();
        assert!(is_alive(Some(&live), &started(), &boot()));
        assert!(!is_alive(None, &started(), &boot()));
        assert!(!is_alive(Some(&zombie), &started(), &boot()));
        assert!(!is_alive(Some(&live), &started(), &BootId::parse(OTHER_BOOT).unwrap()));
        let mut forged = started();
        forged.starttime += 1;
        assert!(!is_alive(Some(&live), &forged, &boot()));
    }

    #[test]
    fn ours_needs_bash_our_wrapper_and_this_job_dir() {
        let id = identity();
        assert!(is_our_wrapper(&our_cmdline(), &id));
        // The recorded P2 shape has no sha in the wrapper name, so it is not an uploaded one.
        assert!(!is_our_wrapper(CMDLINE_P2, &id));
        assert!(!is_our_wrapper(b"", &id), "zombie");
        assert!(!is_our_wrapper(b"sleep\x0060\0", &id), "the taskset child");
        let variants: [(&[u8], &[u8]); 6] = [
            (b"bash\0", b"/bin/bash\0"),
            (b"/jobs/j1\0", b"/jobs/j10\0"),
            (b"/jobs/j1\0", b"/jobs/j\0"),
            (b"wrapper-3f2a9c1b.sh", b"wrapper-.sh"),
            (b"wrapper-3f2a9c1b.sh", b"wrapper-3F2A9C1B.sh"),
            (b"/probe-5.2/bin/", b"/probe-5.3/bin/"),
        ];
        for (from, to) in variants {
            let cmdline = replace_bytes(&our_cmdline(), from, to);
            assert_ne!(cmdline, our_cmdline());
            assert!(!is_our_wrapper(&cmdline, &id), "{}", String::from_utf8_lossy(&cmdline));
        }
    }

    #[test]
    fn job_session_is_exact_cwd_and_skips_enoent() {
        let members = [
            member(1, Some(JOB)),
            member(2, None),
            member(3, Some(&format!("{JOB}0"))),
            member(4, Some(&format!("{JOB}/"))),
            member(5, Some(&format!("{JOB} (deleted)"))),
            member(6, Some(JOB)),
        ];
        assert_eq!(job_session(&members, JOB), vec![1, 6]);
    }

    #[test]
    fn sid_guard_reused_only_on_a_different_starttime() {
        let live = stat_bash_live();
        let zombie = stat_bash_zombie();
        assert!(!sid_reused(None, &started()).unwrap(), "absent: ours (nothing to reuse)");
        assert!(!sid_reused(Some(live.as_bytes()), &started()).unwrap(), "alive: ours");
        assert!(!sid_reused(Some(zombie.as_bytes()), &started()).unwrap(), "zombie: still ours");
        let mut forged = started();
        forged.starttime += 5;
        assert!(sid_reused(Some(live.as_bytes()), &forged).unwrap(), "different start: reused");
        assert!(sid_reused(Some(b"junk"), &started()).is_err());
    }
}
