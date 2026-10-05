//! The remote connection test (Phase 5 unit 5.1, ADR-023, ADR-024 n): the transport bytes, the
//! strict parser of the script's output, and the verdict. Pure — no ssh, no process. The Tauri
//! command `commands::server_profiles::test_server_profile` runs `ssh … -- <host> bash -s`
//! (`remote::ssh::ssh_bash_argv`) with [`conntest_stdin`] and hands the stdout to [`run`].
//!
//! **Transport (ADR-024 n item 11, probe 5.1c).** One `bash -s` stdin carries the static script
//! [`CONNTEST`] and then the values as a NUL-separated list: ORCA path, root, core mask (empty when
//! unset). The script's last line reads that list and calls `main`. The script echoes the values it
//! received, and [`parse_output`] asserts they are exactly the values sent (rule #9) before it
//! reads any fact. A script line after the read loop, or a child that reads stdin, breaks that
//! post-condition instead of producing a plausible-looking result.
//!
//! **Output format** (records, read by the shared strict [`Reader`] of the 5.2 collector): a
//! record line is `<name>` or `<name> <arg>`; a byte record `<name> <len>` is followed by exactly
//! `<len>` raw bytes and a newline.
//!
//! ```text
//! orcastudio-conntest 1
//! argc <n>                  the number of values received
//! arg <len>   (n times)     each value, verbatim
//! <check> <rc>|skipped      for each check, in this fixed order:
//!   out <len>                 mkdir realpath findmnt busctl id nproc orca_x orca ompi
//!   err <len>               (out/err follow only a check that ran; orca's err is empty: merged)
//! end
//! ```
//!
//! If `argc` is not 3, the script prints `end` right after the values. An `error <len>` record in
//! place of any record line means the script itself failed (e.g. `mktemp`); it exits 3.
//! Anything unknown, missing, duplicated or out of order, and anything after `end`, is
//! [`ConnTestError::Malformed`].
//!
//! **Verdict** ([`evaluate`], ADR-024 n item 8): [`Verdict::FullPass`] only if every mandatory
//! check passes — ORCA, cores, the core mask (when set), KillUserProcesses, the root. Anything
//! undetermined is "not passed", with a reason per check. OpenMPI's version is recorded when
//! reported (item 9), and membership in `sudo` is a warning. The formats are the ones recorded in
//! `wiki/orca/remote-server-probe-commands.md` (rule #10).
//!
//! The version and `nproc` parsers below predate the script; the verdict reuses them.
//! Honest-or-absent: a version parser returns `None` when the expected line is absent or
//! malformed (never a bogus version scraped from unrelated digits).

use std::sync::LazyLock;

use regex::Regex;
use serde::Serialize;

use crate::error::AppError;
use crate::models::server_profile::{cpu_out_of_range, parse_core_mask, ProfileTarget};
use crate::remote::classify::is_valid_path;
use crate::remote::scripts::CONNTEST;
use crate::remote::wire::{Reader, WireError};

// The patterns are static literals verified by the unit tests below, so each `expect`
// here is a compile-time-constant invariant, not runtime input handling (the
// no-`.unwrap()`-in-prod rule targets fallible *runtime* values).

/// `Program Version 6.1.0  -  RELEASE   -` (heavy leading indent). Matches the version
/// token immediately after the literal `Program Version`, so unrelated digits elsewhere
/// in the banner cannot be mistaken for the version (the negative-control property).
static ORCA_VERSION_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"Program Version\s+(\d+\.\d+\.\d+)").expect("static ORCA version regex is valid")
});

/// `Open MPI v4.1.6` — line 1 of `ompi_info --version`. The `v` prefix is required so the
/// key-value form (`Open MPI: 4.1.6`, no `v`) does NOT accidentally match this primary
/// pattern; the fallback pattern below handles the `mpirun` shape.
static OPENMPI_VERSION_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"Open MPI v(\d+\.\d+\.\d+)").expect("static OpenMPI version regex is valid")
});

/// `mpirun (Open MPI) 4.1.6` — line 1 of `mpirun --version`, the fallback when `ompi_info`
/// is absent. Anchored on the literal `Open MPI)` so it cannot match the primary shape's
/// `Open MPI v...` and double-count.
static MPIRUN_VERSION_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"Open MPI\)\s+(\d+\.\d+\.\d+)").expect("static mpirun version regex is valid")
});

/// Extract the ORCA version (e.g. `"6.1.0"`) from the `<path> --version 2>&1` banner.
///
/// Targets the `Program Version <MAJOR>.<MINOR>.<PATCH>` line verbatim. Returns `None` if
/// that line is absent or malformed — a banner that is present but carries no version, an
/// empty string, or unrelated text all yield `None`, never a version scraped from stray
/// digits (honest-or-absent).
pub fn parse_orca_version(stdout: &str) -> Option<String> {
    ORCA_VERSION_RE
        .captures(stdout)
        .map(|caps| caps[1].to_string())
}

/// Extract the OpenMPI version (e.g. `"4.1.6"`) from `ompi_info --version` (line 1
/// `Open MPI v<version>`), falling back to the `mpirun --version` shape
/// (`mpirun (Open MPI) <version>`) if the primary form is absent. Returns `None` if
/// neither shape is present (honest-or-absent).
pub fn parse_openmpi_version(stdout: &str) -> Option<String> {
    OPENMPI_VERSION_RE
        .captures(stdout)
        .or_else(|| MPIRUN_VERSION_RE.captures(stdout))
        .map(|caps| caps[1].to_string())
}

/// Parse the logical CPU count from `nproc` — a single bare integer on its own line.
///
/// Trims surrounding whitespace and parses the whole remaining token as a `u32`. Returns
/// [`AppError::Backend`] (a user-facing spawn/config failure) when the stdout is empty or
/// is not exactly one non-negative integer — never a guessed count (rule #9 post-condition:
/// a bad `nproc` blocks the profile rather than silently defaulting a core ceiling).
pub fn parse_nproc(stdout: &str) -> Result<u32, AppError> {
    let trimmed = stdout.trim();
    trimmed.parse::<u32>().map_err(|_| {
        AppError::Backend(format!(
            "unexpected `nproc` output: expected a single integer, got {trimmed:?}"
        ))
    })
}

/// *Superseded by the connection-test script, which records `test -x`'s rc itself (`orca_x`).*
/// The `test -x <path> && echo ok` executable-presence gate. This is fundamentally an
/// **exit-code** concern (exit 0 = present + executable); the only stdout is the literal
/// `ok`. Part B owns the exit-code decision — this helper merely recognises the `ok`
/// convention for a shell that echoes on success, so a caller can treat "`ok` on stdout"
/// as the presence signal. Any other stdout (empty, an error message) is `false`.
pub fn parse_presence(stdout: &str) -> bool {
    stdout.trim() == "ok"
}

// --- The connection-test script: transport, output, verdict -------------------

/// The first record of the script's output; the number is the format version.
pub const OUTPUT_HEADER: &str = "orcastudio-conntest 1";

/// The only filesystem types the root may be on (ADR-024 n item 8). A type joins the list only
/// after a run on a real host measures it (rule #10): ext4 is the one measured (uni, `/home`).
pub const ROOT_FS_ALLOW_LIST: &[&str] = &["ext4"];

/// The checks, in the order the script runs and prints them.
const CHECK_ORDER: [&str; 9] =
    ["mkdir", "realpath", "findmnt", "busctl", "id", "nproc", "orca_x", "orca", "ompi"];

/// The values the connection test is fed, in NUL-list order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnTestArgs {
    pub orca_path: String,
    pub root: String,
    pub core_mask: Option<String>,
}

impl ConnTestArgs {
    pub fn for_target(target: &ProfileTarget) -> Self {
        ConnTestArgs {
            orca_path: target.remote_orca_path.clone(),
            root: target.remote_scratch_dir.clone(),
            core_mask: target.core_mask.clone(),
        }
    }

    /// The values as sent: an unset mask is the empty string.
    fn values(&self) -> [&str; 3] {
        [&self.orca_path, &self.root, self.core_mask.as_deref().unwrap_or("")]
    }
}

/// The connection test's output could not be read, so there is no verdict. Part B treats this
/// as "not a full pass" too (ADR-024 n item 6), and reports this reason.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ConnTestError {
    #[error("a value to send contains a NUL byte, which the NUL-separated list cannot carry")]
    ValueHasNul,
    #[error("the script received {received} values but {sent} were sent: the transport is broken")]
    ArgCount { sent: usize, received: usize },
    #[error("the script received value {index} as {received:?} but {sent:?} was sent: the transport is broken")]
    ArgMismatch { index: usize, sent: String, received: String },
    #[error("the connection-test script failed: {0}")]
    Script(String),
    #[error("connection-test output: {0}")]
    Malformed(WireError),
}

impl From<WireError> for ConnTestError {
    fn from(e: WireError) -> Self {
        match e {
            WireError::Collector(message) => ConnTestError::Script(message),
            other => ConnTestError::Malformed(other),
        }
    }
}

/// The bytes for `ssh <host> bash -s`'s stdin: the script, then each value followed by a NUL.
pub fn conntest_stdin(args: &ConnTestArgs) -> Result<Vec<u8>, ConnTestError> {
    let mut stdin = CONNTEST.as_bytes().to_vec();
    for value in args.values() {
        if value.contains('\0') {
            return Err(ConnTestError::ValueHasNul);
        }
        stdin.extend_from_slice(value.as_bytes());
        stdin.push(0);
    }
    Ok(stdin)
}

/// One check as the script ran it: its exit status and raw output, or not run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckRun {
    Ran { rc: u8, out: Vec<u8>, err: Vec<u8> },
    Skipped,
}

/// The raw facts of one connection test, one per check of [`CHECK_ORDER`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawFacts {
    pub mkdir: CheckRun,
    pub realpath: CheckRun,
    pub findmnt: CheckRun,
    pub busctl: CheckRun,
    pub id: CheckRun,
    pub nproc: CheckRun,
    pub orca_x: CheckRun,
    pub orca: CheckRun,
    pub ompi: CheckRun,
}

/// Parse the script's stdout strictly. The values the script echoes must be exactly `sent` (rule
/// #9: the transport's post-condition) before any fact is read.
pub fn parse_output(output: &[u8], sent: &ConnTestArgs) -> Result<RawFacts, ConnTestError> {
    let mut r = Reader::new(output);
    let (name, arg) = r.line()?;
    if OUTPUT_HEADER.split_once(' ') != Some((name, arg.unwrap_or_default())) {
        return Err(r.malformed(format!("expected {OUTPUT_HEADER:?}")).into());
    }

    let sent_values = sent.values();
    let received = r.count("argc")?;
    if received != sent_values.len() {
        return Err(ConnTestError::ArgCount { sent: sent_values.len(), received });
    }
    for (index, want) in sent_values.iter().enumerate() {
        let got = r.bytes("arg")?.ok_or_else(|| r.malformed("arg is required".into()))?;
        if got != want.as_bytes() {
            return Err(ConnTestError::ArgMismatch {
                index,
                sent: want.to_string(),
                received: String::from_utf8_lossy(&got).into_owned(),
            });
        }
    }

    let mut runs = Vec::with_capacity(CHECK_ORDER.len());
    for check in CHECK_ORDER {
        let run = match r.word(check)? {
            "skipped" => CheckRun::Skipped,
            rc_text => {
                let rc = parse_rc(rc_text)
                    .ok_or_else(|| r.malformed(format!("{check} rc {rc_text:?}")))?;
                let out = r.bytes("out")?.ok_or_else(|| r.malformed("out is required".into()))?;
                let err = r.bytes("err")?.ok_or_else(|| r.malformed("err is required".into()))?;
                CheckRun::Ran { rc, out, err }
            }
        };
        runs.push(run);
    }
    if r.line()? != ("end", None) {
        return Err(r.malformed("expected end".into()).into());
    }
    if !r.at_end() {
        return Err(r.malformed("bytes after end".into()).into());
    }

    let [mkdir, realpath, findmnt, busctl, id, nproc, orca_x, orca, ompi]: [CheckRun; 9] = runs
        .try_into()
        .map_err(|_| r.malformed("check count".into()))?;
    Ok(RawFacts { mkdir, realpath, findmnt, busctl, id, nproc, orca_x, orca, ompi })
}

/// An exit status: decimal 0–255, no sign, no leading zero.
fn parse_rc(text: &str) -> Option<u8> {
    let canonical = !text.is_empty()
        && text.bytes().all(|b| b.is_ascii_digit())
        && (text == "0" || !text.starts_with('0'));
    if canonical {
        text.parse().ok()
    } else {
        None
    }
}

/// A mandatory check of ADR-024 n item 8.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Check {
    Orca,
    Cores,
    CoreMask,
    KillUserProcesses,
    Root,
}

/// A mandatory check that did not pass, and why. "Undetermined" is a failure too.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CheckFailure {
    pub check: Check,
    pub reason: String,
}

/// Something worth telling the user that does not block the profile (ADR-024 n item 9).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "detail")]
pub enum Warning {
    /// The profile user is in the `sudo` group (Decision k: the profile user should have no sudo).
    SudoGroup,
    /// `id -nG` did not answer, so `sudo` membership is unknown.
    GroupsUndetermined(String),
    /// `ompi_info --version` reported no version; NULL is recorded.
    OpenMpiNotReported(String),
}

/// What a full pass stamps (ADR-024 n items 6, 9).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct VerifiedFacts {
    pub orca_version: String,
    pub openmpi_version: Option<String>,
    pub core_count: u32,
}

/// The connection test's verdict. Only a full pass stamps the profile; anything else clears it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case", tag = "verdict")]
pub enum Verdict {
    FullPass { facts: VerifiedFacts, warnings: Vec<Warning> },
    NotPassed { failures: Vec<CheckFailure>, warnings: Vec<Warning> },
}

/// Parse and evaluate in one step.
pub fn run(output: &[u8], sent: &ConnTestArgs) -> Result<Verdict, ConnTestError> {
    Ok(evaluate(&parse_output(output, sent)?, sent))
}

/// Decide every check from the raw facts (ADR-024 n items 8–9).
pub fn evaluate(facts: &RawFacts, sent: &ConnTestArgs) -> Verdict {
    let mut failures = Vec::new();
    let mut fail = |check: Check, reason: String| failures.push(CheckFailure { check, reason });

    let orca_version = check_orca(&facts.orca_x, &facts.orca)
        .map_err(|reason| fail(Check::Orca, reason))
        .ok();
    let core_count = check_cores(&facts.nproc)
        .map_err(|reason| fail(Check::Cores, reason))
        .ok();
    if let Some(mask) = &sent.core_mask {
        if let Err(reason) = check_mask(mask, core_count) {
            fail(Check::CoreMask, reason);
        }
    }
    if let Err(reason) = check_kill_user_processes(&facts.busctl) {
        fail(Check::KillUserProcesses, reason);
    }
    if let Err(reason) = check_root(&sent.root, &facts.mkdir, &facts.realpath, &facts.findmnt) {
        fail(Check::Root, reason);
    }

    let mut warnings = Vec::new();
    match &facts.id {
        CheckRun::Ran { rc: 0, out, .. } => {
            if String::from_utf8_lossy(out).split_whitespace().any(|g| g == "sudo") {
                warnings.push(Warning::SudoGroup);
            }
        }
        other => warnings.push(Warning::GroupsUndetermined(describe(other))),
    }
    let openmpi_version = match &facts.ompi {
        CheckRun::Ran { rc: 0, out, .. } => parse_openmpi_version(&String::from_utf8_lossy(out)),
        _ => None,
    };
    if openmpi_version.is_none() {
        warnings.push(Warning::OpenMpiNotReported(describe(&facts.ompi)));
    }

    match (orca_version, core_count) {
        (Some(orca_version), Some(core_count)) if failures.is_empty() => Verdict::FullPass {
            facts: VerifiedFacts { orca_version, openmpi_version, core_count },
            warnings,
        },
        _ => Verdict::NotPassed { failures, warnings },
    }
}

/// A short description of a check's run, for a reason or a warning.
fn describe(run: &CheckRun) -> String {
    match run {
        CheckRun::Skipped => "not run".into(),
        CheckRun::Ran { rc, out, err } => {
            let text = |b: &[u8]| String::from_utf8_lossy(b).trim().chars().take(200).collect::<String>();
            format!("rc {rc}, stdout {:?}, stderr {:?}", text(out), text(err))
        }
    }
}

/// `test -x`, then `--version`: a pass only with a `Program Version x.y.z` line. ORCA exits 2 on
/// `--version` (measured), so the rc is not the signal; 127 and 126 mean not runnable.
fn check_orca(orca_x: &CheckRun, orca: &CheckRun) -> Result<String, String> {
    match orca_x {
        CheckRun::Skipped => {
            return Err("the ORCA path is not absolute, so the script did not run it".into())
        }
        CheckRun::Ran { rc: 0, .. } => {}
        CheckRun::Ran { .. } => {
            return Err("ORCA is missing or not executable (test -x failed)".into())
        }
    }
    match orca {
        CheckRun::Skipped => Err("ORCA was not run".into()),
        CheckRun::Ran { rc: 127, .. } => {
            Err(format!("ORCA is not runnable: not found (rc 127): {}", describe(orca)))
        }
        CheckRun::Ran { rc: 126, .. } => {
            Err(format!("ORCA is not runnable: not executable (rc 126): {}", describe(orca)))
        }
        CheckRun::Ran { rc, out, .. } => parse_orca_version(&String::from_utf8_lossy(out))
            .ok_or_else(|| {
                format!("no `Program Version x.y.z` line in ORCA's --version output (rc {rc})")
            }),
    }
}

/// `nproc` with rc 0 and a positive integer.
fn check_cores(nproc: &CheckRun) -> Result<u32, String> {
    match nproc {
        CheckRun::Ran { rc: 0, out, .. } => match parse_nproc(&String::from_utf8_lossy(out)) {
            Ok(0) => Err("nproc reported 0 CPUs".into()),
            Ok(n) => Ok(n),
            Err(e) => Err(e.to_string()),
        },
        other => Err(format!("nproc failed: {}", describe(other))),
    }
}

/// Every CPU of the mask lies within `0..nproc-1` (ADR-024 n item 8, rule #8). Shares
/// [`cpu_out_of_range`] with the run-target rule.
fn check_mask(mask: &str, core_count: Option<u32>) -> Result<(), String> {
    let ranges =
        parse_core_mask(mask).map_err(|why| format!("the core mask {mask:?} is invalid: {why}"))?;
    let core_count =
        core_count.ok_or("the core count is undetermined, so the mask cannot be checked")?;
    match cpu_out_of_range(&ranges, core_count) {
        Some(cpu) => Err(format!(
            "CPU {cpu} of the core mask {mask:?} is outside 0..{} ({core_count} CPUs)",
            core_count.saturating_sub(1)
        )),
        None => Ok(()),
    }
}

/// `busctl` must print exactly `b false` with rc 0. `b true`, any other output or any rc ≠ 0 is
/// not passed (ADR-024 n item 8; so a host without logind cannot be a run target).
fn check_kill_user_processes(busctl: &CheckRun) -> Result<(), String> {
    match busctl {
        CheckRun::Ran { rc: 0, out, .. } if out.as_slice() == b"b false\n" => Ok(()),
        CheckRun::Ran { rc: 0, out, .. } if out.as_slice() == b"b true\n" => Err(
            "KillUserProcesses is true: logind kills a user's processes when the session ends, \
             so a detached job would die"
                .into(),
        ),
        other => Err(format!("KillUserProcesses is undetermined: {}", describe(other))),
    }
}

/// The root: the one path form, created (`mkdir -p` rc 0), equal to its realpath, and on an
/// allow-listed filesystem type (the first token of `findmnt -no FSTYPE --target`).
fn check_root(
    root: &str,
    mkdir: &CheckRun,
    realpath: &CheckRun,
    findmnt: &CheckRun,
) -> Result<(), String> {
    if !is_valid_path(root) {
        return Err(format!("the root {root:?} is not a valid path"));
    }
    match mkdir {
        CheckRun::Ran { rc: 0, .. } => {}
        other => return Err(format!("could not create the root: {}", describe(other))),
    }
    match realpath {
        CheckRun::Ran { rc: 0, out, .. } if out.strip_suffix(b"\n") == Some(root.as_bytes()) => {}
        CheckRun::Ran { rc: 0, out, .. } => {
            return Err(format!(
                "the root is not its own realpath ({:?}); job dirs are matched by their canonical path",
                String::from_utf8_lossy(out).trim_end()
            ))
        }
        other => return Err(format!("realpath failed: {}", describe(other))),
    }
    let undetermined = || format!("the root's filesystem type is undetermined: {}", describe(findmnt));
    let CheckRun::Ran { rc: 0, out, .. } = findmnt else {
        return Err(undetermined());
    };
    let text = String::from_utf8_lossy(out);
    let mut lines = text.lines();
    let fs_type = match (lines.next(), lines.next()) {
        (Some(line), None) => line.split_whitespace().next(),
        _ => None,
    };
    match fs_type {
        Some(t) if ROOT_FS_ALLOW_LIST.contains(&t) => Ok(()),
        Some(t) => Err(format!(
            "the root is on a {t:?} filesystem; only {ROOT_FS_ALLOW_LIST:?} is measured to work"
        )),
        None => Err(undetermined()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The prober's verbatim ORCA banner tail (`<path> --version 2>&1`). Heavy indent is
    // significant and preserved exactly from wiki/orca/remote-server-probe-commands.md.
    const ORCA_BANNER: &str = "\
                         Program Version 6.1.0  -  RELEASE   -
";

    // The prober's verbatim `ompi_info --version`: three lines, version on line 1.
    const OMPI_INFO: &str = "\
Open MPI v4.1.6

http://www.open-mpi.org/community/help/
";

    // The prober's verbatim `mpirun --version` fallback: three lines, version on line 1.
    const MPIRUN: &str = "\
mpirun (Open MPI) 4.1.6

Report bugs to http://www.open-mpi.org/community/help/
";

    #[test]
    fn orca_version_from_real_banner() {
        assert_eq!(parse_orca_version(ORCA_BANNER).as_deref(), Some("6.1.0"));
    }

    #[test]
    fn orca_version_absent_or_garbage_is_none() {
        // Empty stdout, a banner with no version line, and unrelated text all → None.
        assert_eq!(parse_orca_version(""), None);
        assert_eq!(parse_orca_version("some unrelated line\nanother\n"), None);
        assert_eq!(
            parse_orca_version("ORCA failed to open --version\n"),
            None
        );
    }

    // NEGATIVE CONTROL (bites): a banner that carries plenty of digits but NOT after the
    // literal `Program Version` must yield None. If the parser were loosened to grab "any
    // x.y.z digits" (e.g. matching the date/size below), this assert would go red — proving
    // the anchor on `Program Version` is load-bearing, not decorative.
    #[test]
    fn orca_version_does_not_scrape_unrelated_digits() {
        let wrong_banner = "\
-rwxrwxr-x 1 root root 43453616 Jun 12 2025 /opt/orca/orca
Compiled with gcc 11.2.0 for x86_64
Build 3.14.1 tag
";
        assert_eq!(
            parse_orca_version(wrong_banner),
            None,
            "must not scrape a version from unrelated x.y.z digits — the \
             `Program Version` anchor is what makes this correct"
        );
    }

    #[test]
    fn openmpi_version_from_ompi_info() {
        assert_eq!(parse_openmpi_version(OMPI_INFO).as_deref(), Some("4.1.6"));
    }

    #[test]
    fn openmpi_version_falls_back_to_mpirun() {
        // No `Open MPI v...` line, but the `mpirun (Open MPI) X` fallback resolves.
        assert_eq!(parse_openmpi_version(MPIRUN).as_deref(), Some("4.1.6"));
    }

    #[test]
    fn openmpi_version_absent_is_none() {
        assert_eq!(parse_openmpi_version(""), None);
        assert_eq!(parse_openmpi_version("bash: ompi_info: command not found\n"), None);
        // The key-value form without the `v` prefix is NOT the primary shape and has no
        // `mpirun` marker either → None (we target line-1 shapes, per the prober).
        assert_eq!(parse_openmpi_version("                Open MPI: 4.1.6\n"), None);
    }

    #[test]
    fn nproc_from_bare_integer() {
        assert_eq!(parse_nproc("16\n").unwrap(), 16);
        // Extra surrounding whitespace is tolerated (trim), value still exact.
        assert_eq!(parse_nproc("  8  \n").unwrap(), 8);
    }

    #[test]
    fn nproc_garbage_is_error_not_guess() {
        assert!(matches!(parse_nproc(""), Err(AppError::Backend(_))));
        assert!(matches!(parse_nproc("nproc: not found\n"), Err(AppError::Backend(_))));
        // A negative or multi-token line is not a bare u32 → error, never a guessed count.
        assert!(matches!(parse_nproc("-1\n"), Err(AppError::Backend(_))));
        assert!(matches!(parse_nproc("16 32\n"), Err(AppError::Backend(_))));
    }

    #[test]
    fn presence_gate_recognises_ok() {
        assert!(parse_presence("ok\n"));
        assert!(parse_presence("ok"));
        assert!(!parse_presence(""));
        assert!(!parse_presence("bash: test: missing\n"));
    }
}

/// The connection-test script: its output parser and verdict over the recorded formats, and the
/// real script run locally under `bash -s` (stub `busctl`/`findmnt`/`id`/`nproc`/`ompi_info` on
/// `PATH`, a stub ORCA at an absolute path; nothing outside the test's own temp dir is touched).
#[cfg(test)]
pub(crate) mod conntest_tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};
    use std::process::{Command, Stdio};
    use std::sync::atomic::{AtomicU32, Ordering};

    // ---- Fixtures, verbatim from wiki/orca/remote-server-probe-commands.md ----------------

    /// "Connection-test formats on uni": `busctl get-property … KillUserProcesses`.
    const BUSCTL_FALSE: &[u8] = b"b false\n";
    /// The bad-property shape on stderr, rc 1 (recorded with its ellipses).
    const BUSCTL_BAD_PROPERTY_ERR: &[u8] =
        b"Failed to get property \xe2\x80\xa6 Unknown interface \xe2\x80\xa6 or property \xe2\x80\xa6\n";
    /// `id -nG` on uni.
    const ID_ANTON: &[u8] = b"anton users\n";
    /// `findmnt -no FSTYPE,SOURCE,TARGET --target <root>` on uni: the fs type is the first token.
    const FINDMNT_UNI: &[u8] = b"ext4   /dev/sdb4 /home\n";
    /// ORCA 6.1.1's `--version` line 54 on uni.
    const ORCA_VERSION_LINE: &str = "                         Program Version 6.1.1  -  RELEASE   -";
    /// The stderr tail of ORCA's `--version`.
    const ORCA_TAIL: &str = "Cannot open input file: --version";
    /// `ompi_info --version`: three lines.
    const OMPI_INFO_416: &[u8] = b"Open MPI v4.1.6\n\nhttp://www.open-mpi.org/community/help/\n";
    /// `nproc` on uni.
    const NPROC_UNI: &[u8] = b"48\n";

    const ROOT: &str = "/home/anton/.orcastudio";

    /// ORCA's merged `--version` output: the version on line 54 and the stderr tail last. Lines
    /// other than those two are not recorded on the wiki, so they are left empty, not invented.
    fn orca_banner() -> Vec<u8> {
        let mut text = "\n".repeat(53);
        text.push_str(ORCA_VERSION_LINE);
        text.push('\n');
        text.push_str(ORCA_TAIL);
        text.push('\n');
        text.into_bytes()
    }

    pub(crate) fn ran(rc: u8, out: &[u8]) -> CheckRun {
        CheckRun::Ran { rc, out: out.to_vec(), err: Vec::new() }
    }

    fn args(mask: Option<&str>) -> ConnTestArgs {
        ConnTestArgs {
            orca_path: "/opt/orca/orca".into(),
            root: ROOT.into(),
            core_mask: mask.map(str::to_string),
        }
    }

    /// Every check passing, in the recorded formats.
    pub(crate) fn uni_facts() -> RawFacts {
        RawFacts {
            mkdir: ran(0, b""),
            realpath: ran(0, format!("{ROOT}\n").as_bytes()),
            findmnt: ran(0, FINDMNT_UNI),
            busctl: ran(0, BUSCTL_FALSE),
            id: ran(0, ID_ANTON),
            nproc: ran(0, NPROC_UNI),
            orca_x: ran(0, b""),
            orca: ran(2, &orca_banner()),
            ompi: ran(0, OMPI_INFO_416),
        }
    }

    /// The script's output for `values` and `facts`, in the documented record format.
    pub(crate) fn encode(values: &[&str], facts: &RawFacts) -> Vec<u8> {
        let mut w = format!("{OUTPUT_HEADER}\nargc {}\n", values.len()).into_bytes();
        let bytes = |w: &mut Vec<u8>, name: &str, b: &[u8]| {
            w.extend_from_slice(format!("{name} {}\n", b.len()).as_bytes());
            w.extend_from_slice(b);
            w.push(b'\n');
        };
        for v in values {
            bytes(&mut w, "arg", v.as_bytes());
        }
        let runs = [
            &facts.mkdir, &facts.realpath, &facts.findmnt, &facts.busctl, &facts.id,
            &facts.nproc, &facts.orca_x, &facts.orca, &facts.ompi,
        ];
        for (name, run) in CHECK_ORDER.iter().zip(runs) {
            match run {
                CheckRun::Skipped => w.extend_from_slice(format!("{name} skipped\n").as_bytes()),
                CheckRun::Ran { rc, out, err } => {
                    w.extend_from_slice(format!("{name} {rc}\n").as_bytes());
                    bytes(&mut w, "out", out);
                    bytes(&mut w, "err", err);
                }
            }
        }
        w.extend_from_slice(b"end\n");
        w
    }

    fn failures(v: &Verdict) -> Vec<Check> {
        match v {
            Verdict::FullPass { .. } => Vec::new(),
            Verdict::NotPassed { failures, .. } => failures.iter().map(|f| f.check).collect(),
        }
    }

    pub(crate) fn with(f: impl FnOnce(&mut RawFacts)) -> RawFacts {
        let mut facts = uni_facts();
        f(&mut facts);
        facts
    }

    // ---- Parsing --------------------------------------------------------------------------

    #[test]
    fn the_recorded_formats_are_a_full_pass() {
        let sent = args(Some("0-23"));
        let wire = encode(&["/opt/orca/orca", ROOT, "0-23"], &uni_facts());
        assert_eq!(parse_output(&wire, &sent).unwrap(), uni_facts(), "the encoding round-trips");
        assert_eq!(
            run(&wire, &sent).unwrap(),
            Verdict::FullPass {
                facts: VerifiedFacts {
                    orca_version: "6.1.1".into(),
                    openmpi_version: Some("4.1.6".into()),
                    core_count: 48,
                },
                warnings: Vec::new(),
            }
        );
    }

    // The transport post-condition (rule #9): the echoed values must be exactly those sent.
    #[test]
    fn the_echoed_values_must_equal_the_sent_ones() {
        let sent = args(None);
        let facts = uni_facts();
        assert!(parse_output(&encode(&["/opt/orca/orca", ROOT, ""], &facts), &sent).is_ok());
        assert_eq!(
            parse_output(&encode(&["/opt/orca/orca", ROOT], &facts), &sent),
            Err(ConnTestError::ArgCount { sent: 3, received: 2 })
        );
        assert_eq!(
            parse_output(&encode(&["/opt/orca/orca", ROOT, "", ""], &facts), &sent),
            Err(ConnTestError::ArgCount { sent: 3, received: 4 })
        );
        assert!(matches!(
            parse_output(&encode(&["/opt/orca/orca", "/home/anton", ""], &facts), &sent),
            Err(ConnTestError::ArgMismatch { index: 1, .. })
        ));
    }

    #[test]
    fn the_output_is_parsed_strictly() {
        let sent = args(None);
        let good = encode(&["/opt/orca/orca", ROOT, ""], &uni_facts());
        let text = String::from_utf8(good.clone()).unwrap();
        let malformed = |bytes: &[u8]| matches!(parse_output(bytes, &sent), Err(ConnTestError::Malformed(_)));

        assert!(malformed(b""), "empty output");
        assert!(malformed(&good[..good.len() - 1]), "truncated");
        assert!(malformed(&[good.as_slice(), b"extra\n"].concat()), "bytes after end");
        assert!(malformed(text.replacen("orcastudio-conntest 1", "orcastudio-conntest 2", 1).as_bytes()));
        assert!(malformed(text.replacen("\nbusctl 0\n", "\nbusctl 02\n", 1).as_bytes()), "rc with a leading zero");
        assert!(malformed(text.replacen("\nbusctl 0\n", "\nbusctl 256\n", 1).as_bytes()), "rc above 255");
        assert!(malformed(text.replacen("\nbusctl 0\n", "\nkill 0\n", 1).as_bytes()), "unknown check");
        assert!(malformed(text.replacen("\nnproc 0\n", "\nid 0\n", 1).as_bytes()), "duplicate / out of order");
        // A missing check: drop `ompi` entirely.
        let without_ompi = text.split("ompi 0\n").next().unwrap().to_string() + "end\n";
        assert!(malformed(without_ompi.as_bytes()), "missing check");
        // A byte record whose length is wrong.
        assert!(malformed(text.replacen("out 3\n48\n", "out 4\n48\n", 1).as_bytes()));
        // The script's own failure record.
        let failed = format!("{OUTPUT_HEADER}\nargc 3\narg 14\n/opt/orca/orca\narg 23\n{ROOT}\narg 0\n\nerror 16\nmktemp -d failed\n");
        assert_eq!(
            parse_output(failed.as_bytes(), &sent),
            Err(ConnTestError::Script("mktemp -d failed".into()))
        );
    }

    #[test]
    fn a_nul_in_a_value_cannot_be_sent() {
        let mut a = args(None);
        a.root = "/a\0b".into();
        assert_eq!(conntest_stdin(&a), Err(ConnTestError::ValueHasNul));
        let stdin = conntest_stdin(&args(Some("0-3"))).unwrap();
        assert!(stdin.starts_with(CONNTEST.as_bytes()));
        assert!(stdin.ends_with(format!("/opt/orca/orca\0{ROOT}\00-3\0").as_bytes()));
    }

    // ---- The verdict ----------------------------------------------------------------------

    #[test]
    fn orca_passes_only_with_a_program_version_line() {
        let sent = args(None);
        // rc 2 with the line: the measured normal case.
        assert_eq!(failures(&evaluate(&uni_facts(), &sent)), vec![]);
        // rc 0 with the line passes too: the rc is not the signal.
        assert_eq!(failures(&evaluate(&with(|f| f.orca = ran(0, &orca_banner())), &sent)), vec![]);
        for orca in [
            ran(127, b"bash: line 1: /opt/orca/orca: No such file or directory\n"),
            ran(126, b"bash: line 1: /opt/orca/orca: Permission denied\n"),
            ran(126, b"bash: line 1: /opt/orca/orca: Is a directory\n"),
            ran(2, ORCA_TAIL.as_bytes()),
            ran(0, b""),
            CheckRun::Skipped,
        ] {
            assert_eq!(failures(&evaluate(&with(|f| f.orca = orca.clone()), &sent)), vec![Check::Orca], "{orca:?}");
        }
        // test -x failed, or the path was not absolute: not passed.
        assert_eq!(failures(&evaluate(&with(|f| f.orca_x = ran(1, b"")), &sent)), vec![Check::Orca]);
        assert_eq!(
            failures(&evaluate(&with(|f| { f.orca_x = CheckRun::Skipped; f.orca = CheckRun::Skipped; }), &sent)),
            vec![Check::Orca]
        );
    }

    #[test]
    fn cores_must_be_a_positive_integer() {
        let sent = args(None);
        for nproc in [ran(0, b"0\n"), ran(0, b"forty-eight\n"), ran(0, b""), ran(127, b""), CheckRun::Skipped] {
            assert_eq!(failures(&evaluate(&with(|f| f.nproc = nproc.clone()), &sent)), vec![Check::Cores], "{nproc:?}");
        }
    }

    // NEGATIVE CONTROL (bites, control g): the mask's CPUs are 0..nproc-1, so on 48 CPUs `0-47`
    // passes and `0-48` does not. An off-by-one (`<= nproc`) turns the second assert green-for-
    // the-wrong-reason red.
    #[test]
    fn the_mask_must_lie_within_zero_to_nproc_minus_one() {
        assert_eq!(failures(&evaluate(&uni_facts(), &args(Some("0-47")))), vec![]);
        assert_eq!(failures(&evaluate(&uni_facts(), &args(Some("0-48")))), vec![Check::CoreMask]);
        assert_eq!(failures(&evaluate(&uni_facts(), &args(Some("0,2,64")))), vec![Check::CoreMask]);
        assert_eq!(failures(&evaluate(&uni_facts(), &args(Some("1-2-3")))), vec![Check::CoreMask]);
        // No mask: nothing to check here (the profile is then not a run target, by is_run_target).
        assert_eq!(failures(&evaluate(&uni_facts(), &args(None))), vec![]);
        // Undetermined cores: the mask cannot be checked, so it does not pass either.
        assert_eq!(
            failures(&evaluate(&with(|f| f.nproc = ran(1, b"")), &args(Some("0-3")))),
            vec![Check::Cores, Check::CoreMask]
        );
    }

    // NEGATIVE CONTROL (bites, control c): only exactly `b false` with rc 0 passes. Accepting
    // `b true` (or anything else) would certify a host whose logind kills detached jobs.
    #[test]
    fn kill_user_processes_must_be_exactly_b_false() {
        let sent = args(None);
        for busctl in [
            ran(0, b"b true\n"),
            CheckRun::Ran { rc: 1, out: Vec::new(), err: BUSCTL_BAD_PROPERTY_ERR.to_vec() },
            ran(1, BUSCTL_FALSE),
            ran(0, b"b false"),
            ran(0, b"b false\nb false\n"),
            ran(0, b""),
            ran(127, b""),
            CheckRun::Skipped,
        ] {
            assert_eq!(
                failures(&evaluate(&with(|f| f.busctl = busctl.clone()), &sent)),
                vec![Check::KillUserProcesses],
                "{busctl:?}"
            );
        }
    }

    // NEGATIVE CONTROL (bites, control d): the root's fs type must be on the allow-list {ext4}.
    // Accepting any type would certify an NFS root, whose behaviour is not measured.
    #[test]
    fn the_root_must_be_on_an_allow_listed_filesystem() {
        let sent = args(None);
        assert_eq!(failures(&evaluate(&with(|f| f.findmnt = ran(0, b"ext4\n")), &sent)), vec![]);
        for findmnt in [
            ran(0, b"nfs4   server:/export /home\n"),
            ran(0, b"tmpfs\n"),
            ran(0, b"ext2/ext3\n"),
            ran(0, b""),
            ran(1, b""),
            ran(0, b"ext4\next4\n"),
            CheckRun::Skipped,
        ] {
            assert_eq!(failures(&evaluate(&with(|f| f.findmnt = findmnt.clone()), &sent)), vec![Check::Root], "{findmnt:?}");
        }
    }

    #[test]
    fn the_root_must_be_created_and_be_its_own_realpath() {
        let sent = args(None);
        let root_fails = |f: RawFacts| failures(&evaluate(&f, &sent)) == vec![Check::Root];
        assert!(root_fails(with(|f| f.mkdir = ran(1, b""))));
        assert!(root_fails(with(|f| f.realpath = ran(0, b"/data/anton/.orcastudio\n"))));
        assert!(root_fails(with(|f| f.realpath = ran(0, ROOT.as_bytes()))), "no newline: not the recorded shape");
        assert!(root_fails(with(|f| f.realpath = ran(1, b""))));
        assert!(root_fails(with(|f| {
            f.mkdir = CheckRun::Skipped;
            f.realpath = CheckRun::Skipped;
            f.findmnt = CheckRun::Skipped;
        })));
        let mut bad_root = args(None);
        bad_root.root = "/home/anton/../root".into();
        assert_eq!(failures(&evaluate(&uni_facts(), &bad_root)), vec![Check::Root]);
    }

    #[test]
    fn openmpi_is_recorded_and_sudo_is_a_warning() {
        let sent = args(None);
        let v = evaluate(&with(|f| { f.ompi = ran(127, b""); f.id = ran(0, b"anton sudo users\n"); }), &sent);
        match v {
            Verdict::FullPass { facts, warnings } => {
                assert_eq!(facts.openmpi_version, None, "absent OpenMPI is NULL, never guessed");
                assert!(warnings.contains(&Warning::SudoGroup));
                assert!(warnings.iter().any(|w| matches!(w, Warning::OpenMpiNotReported(_))));
            }
            other => panic!("OpenMPI and sudo do not gate: {other:?}"),
        }
        let v = evaluate(&with(|f| f.id = ran(1, b"")), &sent);
        assert!(matches!(v, Verdict::FullPass { ref warnings, .. }
            if matches!(warnings.as_slice(), [Warning::GroupsUndetermined(_)])));
        // `sudoers` is not `sudo`: whole tokens only.
        let v = evaluate(&with(|f| f.id = ran(0, b"anton sudoers\n")), &sent);
        assert!(matches!(v, Verdict::FullPass { ref warnings, .. } if warnings.is_empty()));
    }

    // ---- The script's shape ---------------------------------------------------------------

    /// ADR-024 n item 11, verbatim except for the called function.
    const LAST_LINE: &str =
        "args=(); while IFS= read -r -d '' a; do args+=(\"$a\"); done; main \"${args[@]}\"; exit";

    #[test]
    fn the_read_loop_is_the_last_line_and_nothing_follows_it() {
        assert!(CONNTEST.ends_with(&format!("\n{LAST_LINE}\n")), "the script must end with the read loop");
        assert_eq!(CONNTEST.matches("read -r -d ''").count(), 1);
    }

    // NEGATIVE CONTROL (bites, control b, structural half): every child command of the script
    // body reads /dev/null. Removing `</dev/null` from ORCA's call (`check_merged`) or from any
    // capture turns this red. The behavioural half: every stub below exits 99 if its stdin is
    // not /dev/null, so the real-script full pass goes red too.
    #[test]
    fn every_child_command_in_the_script_reads_dev_null() {
        let body = include_str!("remote/scripts/conntest.sh");
        let mut children = 0;
        for line in body.lines().map(str::trim).filter(|l| !l.starts_with('#')) {
            let runs_a_child = line.starts_with("\"$@\"")
                || ["stat ", "cat ", "mktemp", "rm "].iter().any(|c| line.contains(c));
            if runs_a_child {
                children += 1;
                assert!(line.contains("</dev/null"), "a child command without </dev/null: {line:?}");
            }
        }
        assert_eq!(children, 6, "check, check_merged, stat, cat, mktemp and rm");
    }

    // ---- The real script, run locally under `bash -s` -------------------------------------

    static NEXT_LAB: AtomicU32 = AtomicU32::new(0);

    /// A private temp dir with stub commands; removed on drop.
    struct Lab {
        dir: PathBuf,
    }

    /// Exit 99 unless this stub's stdin is /dev/null — the behavioural check of `</dev/null`.
    const STDIN_GUARD: &str = r#"[[ $(readlink /proc/$$/fd/0) == /dev/null ]] || { echo "stdin is not /dev/null" >&2; exit 99; }"#;

    impl Lab {
        fn new() -> Lab {
            let n = NEXT_LAB.fetch_add(1, Ordering::Relaxed);
            let dir = std::env::temp_dir().join(format!("orcastudio-conntest-{}-{n}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(dir.join("bin")).unwrap();
            let lab = Lab { dir };
            lab.stub("bin/busctl", r#"[[ "$*" == "get-property org.freedesktop.login1 /org/freedesktop/login1 org.freedesktop.login1.Manager KillUserProcesses" ]] || exit 1
printf '%s\n' "${STUB_BUSCTL_OUT-b false}""#);
            lab.stub("bin/findmnt", r#"[[ $# == 4 && "$1 $2 $3" == "-no FSTYPE --target" && -d $4 ]] || exit 1
printf '%s\n' "${STUB_FSTYPE-ext4}""#);
            lab.stub("bin/id", r#"[[ "$*" == "-nG" ]] || exit 1
printf 'anton users\n'"#);
            lab.stub("bin/nproc", r#"[[ $# == 0 ]] || exit 1
printf '48\n'"#);
            lab.stub("bin/ompi_info", r#"[[ "$*" == "--version" ]] || exit 1
printf 'Open MPI v4.1.6\n\nhttp://www.open-mpi.org/community/help/\n'"#);
            let banner_lines = "printf '\\n%.0s' {1..53}\n".to_string()
                + &format!("printf '%s\\n' '{ORCA_VERSION_LINE}'\n")
                + &format!("printf '%s\\n' '{ORCA_TAIL}' >&2\n");
            lab.stub("orca", &format!("[[ \"$*\" == \"--version\" ]] || exit 1\n{banner_lines}exit 2"));
            lab
        }

        fn path(&self, rel: &str) -> String {
            self.dir.join(rel).to_str().unwrap().to_string()
        }

        fn stub(&self, rel: &str, body: &str) {
            let p = self.dir.join(rel);
            std::fs::write(&p, format!("#!/bin/bash\n{STDIN_GUARD}\n{body}\n")).unwrap();
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        /// Run `bash -s` with `stdin`, stubs first on PATH; returns stdout.
        fn bash_s(&self, stdin: &[u8], env: &[(&str, &str)]) -> Vec<u8> {
            use std::io::Write;
            let mut cmd = Command::new("bash");
            cmd.arg("-s")
                .env("PATH", format!("{}:/usr/bin:/bin", self.path("bin")))
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            for (k, v) in env {
                cmd.env(k, v);
            }
            let mut child = cmd.spawn().unwrap();
            child.stdin.take().unwrap().write_all(stdin).unwrap();
            child.wait_with_output().unwrap().stdout
        }

        fn conntest(&self, sent: &ConnTestArgs, env: &[(&str, &str)]) -> Result<Verdict, ConnTestError> {
            run(&self.bash_s(&conntest_stdin(sent).unwrap(), env), sent)
        }

        fn args(&self, root: &str, mask: Option<&str>) -> ConnTestArgs {
            ConnTestArgs { orca_path: self.path("orca"), root: self.path(root), core_mask: mask.map(str::to_string) }
        }
    }

    impl Drop for Lab {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    #[test]
    fn real_script_full_pass_creates_the_root() {
        let lab = Lab::new();
        let sent = lab.args("home/anton/.orcastudio", Some("0-23"));
        assert!(!Path::new(&sent.root).exists());
        assert_eq!(
            lab.conntest(&sent, &[]).unwrap(),
            Verdict::FullPass {
                facts: VerifiedFacts {
                    orca_version: "6.1.1".into(),
                    openmpi_version: Some("4.1.6".into()),
                    core_count: 48,
                },
                warnings: Vec::new(),
            }
        );
        assert!(Path::new(&sent.root).is_dir(), "mkdir -p created the root");
        // An unset mask travels as an empty value and still counts as one of three.
        assert!(matches!(lab.conntest(&lab.args("home/anton/.orcastudio", None), &[]), Ok(Verdict::FullPass { .. })));
    }

    // The values reach `main` verbatim: an embedded newline, a quote, `$HOME`, `;` (probe 5.1c).
    #[test]
    fn real_script_carries_awkward_values_verbatim() {
        let lab = Lab::new();
        let sent = ConnTestArgs {
            orca_path: format!("{}/no such/it's $HOME;\nrm -x", lab.dir.display()),
            root: lab.path("root"),
            core_mask: Some("0-3\n4".into()),
        };
        let verdict = lab.conntest(&sent, &[]).unwrap();
        assert_eq!(failures(&verdict), vec![Check::Orca, Check::CoreMask]);
    }

    #[test]
    fn real_script_reports_b_true_and_a_non_ext4_root() {
        let lab = Lab::new();
        let verdict = lab
            .conntest(&lab.args("root", None), &[("STUB_BUSCTL_OUT", "b true"), ("STUB_FSTYPE", "tmpfs")])
            .unwrap();
        assert_eq!(failures(&verdict), vec![Check::KillUserProcesses, Check::Root]);
    }

    #[test]
    fn real_script_symlinked_root_is_not_its_realpath() {
        let lab = Lab::new();
        std::fs::create_dir(lab.dir.join("real")).unwrap();
        std::os::unix::fs::symlink(lab.dir.join("real"), lab.dir.join("link")).unwrap();
        let verdict = lab.conntest(&lab.args("link/root", None), &[]).unwrap();
        assert_eq!(failures(&verdict), vec![Check::Root]);
    }

    // A directory passes `test -x` and then fails to run with rc 126 (recorded on uni).
    #[test]
    fn real_script_orca_directory_is_not_runnable() {
        let lab = Lab::new();
        std::fs::create_dir(lab.dir.join("orca-dir")).unwrap();
        let mut sent = lab.args("root", None);
        sent.orca_path = lab.path("orca-dir");
        let verdict = lab.conntest(&sent, &[]).unwrap();
        match verdict {
            Verdict::NotPassed { failures, .. } => {
                assert_eq!(failures.len(), 1);
                assert!(failures[0].reason.contains("rc 126"), "{:?}", failures[0]);
            }
            other => panic!("{other:?}"),
        }
    }

    // A root that is not of the one path form is never touched: no mkdir runs.
    #[test]
    fn real_script_never_touches_an_invalid_root() {
        let lab = Lab::new();
        let sent = lab.args("a/../b", None);
        assert_eq!(failures(&lab.conntest(&sent, &[]).unwrap()), vec![Check::Root]);
        assert!(!lab.dir.join("a").exists() && !lab.dir.join("b").exists());
    }

    #[test]
    fn real_script_echoes_the_count_it_received() {
        let lab = Lab::new();
        let sent = lab.args("root", None);
        let mut two = CONNTEST.as_bytes().to_vec();
        two.extend_from_slice(format!("{}\0{}\0", sent.orca_path, sent.root).as_bytes());
        assert_eq!(
            parse_output(&lab.bash_s(&two, &[]), &sent),
            Err(ConnTestError::ArgCount { sent: 3, received: 2 })
        );
    }

    // NEGATIVE CONTROL (bites, control a, permanent form): a script line after the read loop is
    // read as data — it lands in the first value — and the echo post-condition catches it,
    // instead of the line silently never running.
    #[test]
    fn a_script_line_after_the_read_loop_breaks_the_post_condition() {
        let lab = Lab::new();
        let sent = lab.args("root", None);
        let mut stdin = CONNTEST.as_bytes().to_vec();
        stdin.extend_from_slice(b"echo one more line\n");
        stdin.extend_from_slice(&conntest_stdin(&sent).unwrap()[CONNTEST.len()..]);
        assert!(matches!(
            parse_output(&lab.bash_s(&stdin, &[]), &sent),
            Err(ConnTestError::ArgMismatch { index: 0, .. })
        ));
    }

    // A child that reads stdin before the loop swallows the rest of the script and the values
    // (probe 5.1c, measured with `cat`): no output at all, which is an error, not a verdict.
    #[test]
    fn a_stdin_reading_child_before_the_loop_breaks_the_post_condition() {
        let lab = Lab::new();
        let sent = lab.args("root", None);
        let script = CONNTEST.replacen(&format!("\n{LAST_LINE}\n"), &format!("\ncat >/dev/null\n{LAST_LINE}\n"), 1);
        assert_ne!(script, CONNTEST);
        let mut stdin = script.into_bytes();
        stdin.extend_from_slice(&conntest_stdin(&sent).unwrap()[CONNTEST.len()..]);
        let out = lab.bash_s(&stdin, &[]);
        assert!(out.is_empty(), "the swallowed script printed {:?}", String::from_utf8_lossy(&out));
        assert!(matches!(parse_output(&out, &sent), Err(ConnTestError::Malformed(_))));
    }
}
