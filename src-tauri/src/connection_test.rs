//! Pure parsers for the remote connection-test (Phase 5 unit 5.1, ADR-023).
//!
//! The connection-test runs three shell commands on a remote server and parses their
//! stdout to establish the server's specs (domain rule #10 — every third-party fact
//! from a real run). This module is the **pure parsing half**: it takes captured
//! `stdout` strings and extracts the ORCA version, the OpenMPI version, and the logical
//! CPU count. It performs **no** process spawning and **no** SSH — that execution is
//! Part B's concern. Keeping the parsers pure makes every stdout shape testable against
//! the prober's verbatim fixtures (`wiki/orca/remote-server-probe-commands.md`) with no
//! network.
//!
//! The three targeted formats, measured 2026-08-27 on the dev laptop (same distro family
//! as the university target):
//! - ORCA: the heavily-indented banner line `Program Version 6.1.0  -  RELEASE   -`
//!   emitted by `<path> --version 2>&1` (rule #1 — invoked by absolute path).
//! - OpenMPI: `Open MPI v4.1.6` — line 1 of `ompi_info --version` (rule #2 — the version
//!   must match the ORCA build). The `mpirun (Open MPI) 4.1.6` fallback shape is also
//!   accepted so a server without `ompi_info` still resolves.
//! - CPU: a bare trimmed integer from `nproc` (rule #8 — the taskset ceiling).
//!
//! Honest-or-absent: a version parser returns `None` when the expected line is absent or
//! malformed (never a bogus version scraped from unrelated digits); `parse_nproc` returns
//! an `AppError::Parse`-free typed error (`AppError::Backend`) rather than a guessed count.
//! The **executable-presence** gate (`test -x <path>`) is a pure exit-code concern with no
//! stdout to parse beyond a literal `ok`, so it is Part B's shell responsibility; a thin
//! [`parse_presence`] is offered only for the `echo ok` convention and documented as such.

use std::sync::LazyLock;

use regex::Regex;

use crate::error::AppError;

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

/// The `test -x <path> && echo ok` executable-presence gate. This is fundamentally an
/// **exit-code** concern (exit 0 = present + executable); the only stdout is the literal
/// `ok`. Part B owns the exit-code decision — this helper merely recognises the `ok`
/// convention for a shell that echoes on success, so a caller can treat "`ok` on stdout"
/// as the presence signal. Any other stdout (empty, an error message) is `false`.
pub fn parse_presence(stdout: &str) -> bool {
    stdout.trim() == "ok"
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
