//! The `ServerProfile` model (schema v19, Phase 5 unit 5.1, ADR-023, ADR-024 n) and its pure
//! rules: save-time validation, the verification target, and the run-target predicate.
//!
//! A **server profile** is the runtime configuration of one remote execution target:
//! a `~/.ssh/config` host alias (ADR-005 — the app stores NO credentials, auth stays
//! with the user's SSH setup), the remote absolute ORCA path (rule #1), the remote
//! root (`remote_scratch_dir`, ADR-024 n item 4) under which the job dirs, the `tsp/` sockets and
//! `bin/` live (rule #3), and the core-pinning mask (rule #8). It is **data, not code** — "add a
//! server" is a settings action, not a build (ADR-023).
//!
//! The `verified_*` fields (`orca_version`, `openmpi_version`, `core_count`, `verified_at`) are
//! the connection-test's rule-#10 measurement, stored as DISCRETE typed columns (never a JSON
//! blob). They are stamped only by a full pass of the connection test and cleared together
//! whenever the target they certified changes or a re-test is not a full pass (ADR-024 n items
//! 5–6). A run target additionally needs a core mask within the measured cores
//! ([`is_run_target`]).

use std::sync::LazyLock;

use regex::Regex;
use rusqlite::Row;
use serde::{Deserialize, Serialize};

use crate::remote::classify::is_valid_path;
use crate::remote::{slot_socket_path, MAX_SOCKET_PATH_BYTES};

/// A remote execution target. Mirrors the `server_profiles` table one-to-one.
#[derive(Debug, Clone, Serialize)]
pub struct ServerProfile {
    pub id: String,
    /// User-facing display name (e.g. "uni cluster").
    pub name: String,
    /// `~/.ssh/config` host alias — the transport handle (ADR-005). NOT a hostname the
    /// app resolves; the user's SSH config owns connection + auth details.
    pub host: String,
    /// Absolute path to the remote `orca` binary (rule #1 — ORCA is always invoked by
    /// its full absolute path, or OpenMPI parallelization silently fails).
    pub remote_orca_path: String,
    /// The remote root (ADR-024 n item 4): job dirs, `tsp/` sockets and `bin/` live under it.
    /// Absolute, under the one path rule, short enough for the socket bound ([`validate_root`]).
    pub remote_scratch_dir: String,
    /// `taskset` CPU list for pinning (rule #8). `None` until measured; a profile without a mask
    /// is not a run target.
    pub core_mask: Option<String>,
    /// Verified remote ORCA version (connection-test, rule #10). `None` until verified.
    pub orca_version: Option<String>,
    /// OpenMPI version reported by `ompi_info --version` at the last full pass. Recorded, not
    /// matched (ADR-024 n item 9); `None` when not verified or when the host reported none.
    pub openmpi_version: Option<String>,
    /// Verified logical CPU count (`nproc` ceiling, rule #8). `None` until verified.
    pub core_count: Option<u32>,
    /// Timestamp of the last full-pass connection test. **`None` = not verified = not a run
    /// target.** Gates new submits only (ADR-024 n item 6a).
    pub verified_at: Option<String>,
    pub created_at: String,
    /// Number of `tsp` slots. Always 1: the database CHECK rejects anything else until per-slot
    /// masks are measured (ADR-024 n item 1).
    pub slot_count: u32,
    /// `HH:MM-HH:MM` in the laptop's local time, or `None` for no window (ADR-024 n item 3). An
    /// informational label only.
    pub availability_window: Option<String>,
}

impl ServerProfile {
    /// Column list used by every `SELECT` that hydrates a [`ServerProfile`]. The order
    /// here is the contract [`ServerProfile::from_row`] relies on.
    pub const COLUMNS: &'static str = "id, name, host, remote_orca_path, remote_scratch_dir, \
         core_mask, orca_version, openmpi_version, core_count, verified_at, created_at, \
         slot_count, availability_window";

    /// Build a [`ServerProfile`] from a row selected in [`ServerProfile::COLUMNS`] order.
    pub fn from_row(row: &Row) -> rusqlite::Result<ServerProfile> {
        Ok(ServerProfile {
            id: row.get(0)?,
            name: row.get(1)?,
            host: row.get(2)?,
            remote_orca_path: row.get(3)?,
            remote_scratch_dir: row.get(4)?,
            core_mask: row.get(5)?,
            orca_version: row.get(6)?,
            openmpi_version: row.get(7)?,
            core_count: row.get(8)?,
            verified_at: row.get(9)?,
            created_at: row.get(10)?,
            slot_count: row.get(11)?,
            availability_window: row.get(12)?,
        })
    }

    /// The fields a verification certifies.
    pub fn target(&self) -> ProfileTarget {
        ProfileTarget {
            host: self.host.clone(),
            remote_orca_path: self.remote_orca_path.clone(),
            remote_scratch_dir: self.remote_scratch_dir.clone(),
            core_mask: self.core_mask.clone(),
            slot_count: self.slot_count,
        }
    }
}

/// The **target** of a verification (ADR-024 n item 5): the fields whose value, once changed,
/// makes the profile a different target from the one the connection test measured. The name and
/// the availability window are not part of it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileTarget {
    pub host: String,
    pub remote_orca_path: String,
    pub remote_scratch_dir: String,
    pub core_mask: Option<String>,
    pub slot_count: u32,
}

/// A profile field that fails save-time validation (ADR-024 n items 2–4). Nothing is written.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum InvalidProfile {
    #[error("the host alias is empty")]
    EmptyHost,
    #[error("the remote ORCA path must be absolute (rule #1): {0:?}")]
    OrcaPathNotAbsolute(String),
    #[error("the remote root must be an absolute path over [A-Za-z0-9._-] components with no empty, '.' or '..' component and no trailing '/': {0:?}")]
    RootNotValidPath(String),
    #[error("the socket path {socket:?} under the remote root is {len} bytes, more than {MAX_SOCKET_PATH_BYTES}; choose a shorter root")]
    SocketPathTooLong { socket: String, len: usize },
    #[error("the core mask {mask:?} is not a taskset CPU list of single CPUs and ascending ranges (e.g. 0-11,24-35): {why}")]
    CoreMask { mask: String, why: String },
    #[error("the availability window {window:?} is not HH:MM-HH:MM with two different valid times: {why}")]
    Window { window: String, why: String },
    #[error("slot_count must be 1 until per-slot masks are measured (ADR-024 n item 1), got {0}")]
    SlotCount(u32),
}

/// Why a profile is not a run target (ADR-024 n item 2). Submit refuses with this reason.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum NotRunTarget {
    #[error("the profile has not passed the connection test")]
    NotVerified,
    #[error("the profile has no core mask")]
    NoCoreMask,
    #[error("the core mask is invalid: {0}")]
    BadCoreMask(String),
    #[error("the verified core count is missing")]
    NoCoreCount,
    #[error("CPU {cpu} of the core mask is outside 0..{} of the {core_count} cores", core_count.saturating_sub(1))]
    CpuOutOfRange { cpu: u32, core_count: u32 },
}

/// `taskset`'s list syntax as the wrapper validates it (ADR-024 l Part B item 8).
static MASK_SYNTAX_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^[0-9]+([,-][0-9]+)*$").expect("static core-mask regex is valid")
});

/// `HH:MM-HH:MM`.
static WINDOW_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^([0-9]{2}):([0-9]{2})-([0-9]{2}):([0-9]{2})$").expect("static window regex is valid")
});

/// One element of a core mask: a single CPU or an inclusive ascending range.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CpuRange {
    pub first: u32,
    pub last: u32,
}

/// Parse a core mask into its CPU ranges. The text must match the wrapper's regex
/// `^[0-9]+([,-][0-9]+)*$`, and additionally every comma-separated element must be `N` or `N-M`
/// with `N <= M`, each fitting a `u32`. The regex alone admits `1-2-3` and `5-2`, whose CPU sets
/// `taskset` would decide by rules we have not measured; those are refused rather than guessed,
/// so "every CPU in the mask" is always well defined.
pub fn parse_core_mask(mask: &str) -> Result<Vec<CpuRange>, String> {
    if !MASK_SYNTAX_RE.is_match(mask) {
        return Err("not of the form ^[0-9]+([,-][0-9]+)*$".into());
    }
    mask.split(',')
        .map(|element| {
            let mut bounds = element.split('-');
            let first = parse_cpu(bounds.next().unwrap_or_default())?;
            let last = match bounds.next() {
                None => first,
                Some(text) => parse_cpu(text)?,
            };
            if bounds.next().is_some() {
                return Err(format!("{element:?} has more than one '-'"));
            }
            if first > last {
                return Err(format!("{element:?} is a descending range"));
            }
            Ok(CpuRange { first, last })
        })
        .collect()
}

fn parse_cpu(text: &str) -> Result<u32, String> {
    text.parse::<u32>().map_err(|_| format!("CPU {text:?} does not fit a u32"))
}

/// The highest CPU a parsed mask names.
fn max_cpu(ranges: &[CpuRange]) -> Option<u32> {
    ranges.iter().map(|r| r.last).max()
}

/// The first CPU of `mask` that is not within `0..core_count-1`, if any (ADR-024 n items 2, 8).
/// Shared by [`is_run_target`] and the connection test's verdict, so both apply one rule.
pub fn cpu_out_of_range(ranges: &[CpuRange], core_count: u32) -> Option<u32> {
    max_cpu(ranges).filter(|cpu| *cpu >= core_count)
}

/// Validate an availability window: `HH:MM-HH:MM`, hours 00–23, minutes 00–59, the two ends
/// different. A window may wrap past midnight (`22:00-08:00`). Equal ends are rejected (ADR-024 n
/// item 3), because "from 08:00 to 08:00" means neither "always" nor "never" unambiguously.
pub fn validate_window(window: &str) -> Result<(), InvalidProfile> {
    let bad = |why: &str| InvalidProfile::Window { window: window.to_string(), why: why.to_string() };
    let caps = WINDOW_RE.captures(window).ok_or_else(|| bad("not of the form HH:MM-HH:MM"))?;
    let field = |i: usize| caps[i].parse::<u32>().unwrap_or(u32::MAX);
    let (h1, m1, h2, m2) = (field(1), field(2), field(3), field(4));
    if h1 > 23 || h2 > 23 {
        return Err(bad("an hour is above 23"));
    }
    if m1 > 59 || m2 > 59 {
        return Err(bad("a minute is above 59"));
    }
    if (h1, m1) == (h2, m2) {
        return Err(bad("the two ends are equal"));
    }
    Ok(())
}

/// Validate the remote root (ADR-024 n item 4): an absolute path under the one path rule of
/// Decision l item 4 ([`is_valid_path`], shared with the shell's `valid_path`), and every slot
/// socket under it ([`slot_socket_path`]) within [`MAX_SOCKET_PATH_BYTES`].
pub fn validate_root(root: &str, slot_count: u32) -> Result<(), InvalidProfile> {
    if !is_valid_path(root) {
        return Err(InvalidProfile::RootNotValidPath(root.to_string()));
    }
    for slot in 0..slot_count {
        let socket = slot_socket_path(root, slot);
        if socket.len() > MAX_SOCKET_PATH_BYTES {
            let len = socket.len();
            return Err(InvalidProfile::SocketPathTooLong { socket, len });
        }
    }
    Ok(())
}

/// Save-time validation of every user-owned target field and the window (ADR-024 n items 2–4).
/// Called before any write; an error means nothing is written.
pub fn validate_profile(
    target: &ProfileTarget,
    availability_window: Option<&str>,
) -> Result<(), InvalidProfile> {
    if target.host.trim().is_empty() {
        return Err(InvalidProfile::EmptyHost);
    }
    if !target.remote_orca_path.starts_with('/') {
        return Err(InvalidProfile::OrcaPathNotAbsolute(target.remote_orca_path.clone()));
    }
    if target.slot_count != 1 {
        return Err(InvalidProfile::SlotCount(target.slot_count));
    }
    validate_root(&target.remote_scratch_dir, target.slot_count)?;
    if let Some(mask) = &target.core_mask {
        parse_core_mask(mask)
            .map_err(|why| InvalidProfile::CoreMask { mask: mask.clone(), why })?;
    }
    if let Some(window) = availability_window {
        validate_window(window)?;
    }
    Ok(())
}

/// Is this profile a run target (ADR-024 n item 2)? Verified, with a core mask whose every CPU
/// lies within `0..core_count-1` of the verified core count (rule #8). Gates **new submits only**
/// (item 6a): reconcile, cancel and fetch never consult it.
// The caller is 5.3's submit; until it lands this is exercised by its tests only.
#[allow(dead_code)]
pub fn is_run_target(profile: &ServerProfile) -> Result<(), NotRunTarget> {
    if profile.verified_at.is_none() {
        return Err(NotRunTarget::NotVerified);
    }
    let mask = profile.core_mask.as_deref().ok_or(NotRunTarget::NoCoreMask)?;
    let ranges = parse_core_mask(mask).map_err(NotRunTarget::BadCoreMask)?;
    let core_count = profile.core_count.ok_or(NotRunTarget::NoCoreCount)?;
    match cpu_out_of_range(&ranges, core_count) {
        Some(cpu) => Err(NotRunTarget::CpuOutOfRange { cpu, core_count }),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target() -> ProfileTarget {
        ProfileTarget {
            host: "uni".into(),
            remote_orca_path: "/opt/orca/orca".into(),
            remote_scratch_dir: "/home/anton/.orcastudio".into(),
            core_mask: Some("0-23".into()),
            slot_count: 1,
        }
    }

    fn verified(mask: Option<&str>, core_count: Option<u32>) -> ServerProfile {
        ServerProfile {
            id: "p1".into(),
            name: "uni".into(),
            host: "uni".into(),
            remote_orca_path: "/opt/orca/orca".into(),
            remote_scratch_dir: "/home/anton/.orcastudio".into(),
            core_mask: mask.map(str::to_string),
            orca_version: Some("6.1.1".into()),
            openmpi_version: Some("4.1.6".into()),
            core_count,
            verified_at: Some("2026-10-03 10:00:00".into()),
            created_at: "2026-10-03 09:00:00".into(),
            slot_count: 1,
            availability_window: None,
        }
    }

    #[test]
    fn a_good_profile_validates() {
        assert_eq!(validate_profile(&target(), None), Ok(()));
        assert_eq!(validate_profile(&target(), Some("22:00-08:00")), Ok(()));
        let mut t = target();
        t.core_mask = None;
        assert_eq!(validate_profile(&t, Some("08:00-18:30")), Ok(()));
    }

    #[test]
    fn orca_path_must_be_absolute() {
        let mut t = target();
        t.remote_orca_path = "orca".into();
        assert!(matches!(validate_profile(&t, None), Err(InvalidProfile::OrcaPathNotAbsolute(_))));
        t.remote_orca_path = String::new();
        assert!(matches!(validate_profile(&t, None), Err(InvalidProfile::OrcaPathNotAbsolute(_))));
    }

    #[test]
    fn root_follows_the_one_path_rule() {
        for bad in ["relative/dir", "/home/anton/", "/home//anton", "/home/./anton", "/home/../anton",
                    "/home/an ton", "/home/anton;rm", "", "/"] {
            let mut t = target();
            t.remote_scratch_dir = bad.into();
            assert!(
                matches!(validate_profile(&t, None), Err(InvalidProfile::RootNotValidPath(_))),
                "{bad:?} must be refused"
            );
        }
    }

    // The socket layout is `<root>/tsp/slot<N>.sock` (15 bytes after the root for slot 0), so the
    // longest root that fits the 100-byte bound is 85 bytes. Bites if the bound or the layout
    // drifts: an 85-byte root passes, an 86-byte root is refused.
    #[test]
    fn root_is_bounded_by_the_socket_path_length() {
        let root_of = |len: usize| format!("/{}", "a".repeat(len - 1));
        assert_eq!(slot_socket_path(&root_of(85), 0).len(), 100);
        assert_eq!(validate_root(&root_of(85), 1), Ok(()));
        assert!(matches!(
            validate_root(&root_of(86), 1),
            Err(InvalidProfile::SocketPathTooLong { len: 101, .. })
        ));
    }

    #[test]
    fn slot_count_other_than_one_is_refused() {
        let mut t = target();
        t.slot_count = 2;
        assert_eq!(validate_profile(&t, None), Err(InvalidProfile::SlotCount(2)));
    }

    #[test]
    fn core_mask_syntax() {
        assert_eq!(parse_core_mask("0-23").unwrap(), vec![CpuRange { first: 0, last: 23 }]);
        assert_eq!(
            parse_core_mask("0,2,4-7").unwrap(),
            vec![CpuRange { first: 0, last: 0 }, CpuRange { first: 2, last: 2 }, CpuRange { first: 4, last: 7 }]
        );
        for bad in ["", "-1", "0-", "a", "0 - 3", "0,,1", "1-2-3", "5-2", "99999999999", "0x3", " 0"] {
            assert!(parse_core_mask(bad).is_err(), "{bad:?} must be refused");
            let mut t = target();
            t.core_mask = Some(bad.into());
            assert!(matches!(validate_profile(&t, None), Err(InvalidProfile::CoreMask { .. })), "{bad:?}");
        }
    }

    #[test]
    fn window_syntax_wrap_and_equal_ends() {
        for good in ["00:00-23:59", "22:00-08:00", "08:00-08:01"] {
            assert_eq!(validate_window(good), Ok(()), "{good}");
        }
        for bad in ["08:00-08:00", "24:00-08:00", "08:60-09:00", "8:00-09:00", "08:00", "08:00-09:00 ",
                    "08-09", "", "08:00–09:00"] {
            assert!(matches!(validate_window(bad), Err(InvalidProfile::Window { .. })), "{bad:?}");
        }
    }

    #[test]
    fn run_target_needs_verification_mask_and_range() {
        assert_eq!(is_run_target(&verified(Some("0-23"), Some(48))), Ok(()));
        assert_eq!(is_run_target(&verified(Some("0-47"), Some(48))), Ok(()));

        let mut unverified = verified(Some("0-23"), Some(48));
        unverified.verified_at = None;
        assert_eq!(is_run_target(&unverified), Err(NotRunTarget::NotVerified));
        assert_eq!(is_run_target(&verified(None, Some(48))), Err(NotRunTarget::NoCoreMask));
        assert!(matches!(is_run_target(&verified(Some("1-2-3"), Some(48))), Err(NotRunTarget::BadCoreMask(_))));
        assert_eq!(is_run_target(&verified(Some("0-23"), None)), Err(NotRunTarget::NoCoreCount));
    }

    // NEGATIVE CONTROL (bites, control g): CPU `nproc` itself is out of range — the CPUs are
    // 0..nproc-1. An off-by-one (`cpu <= core_count` accepted) lets `0-48` on 48 cores through.
    #[test]
    fn run_target_mask_range_is_zero_to_nproc_minus_one() {
        assert_eq!(
            is_run_target(&verified(Some("0-48"), Some(48))),
            Err(NotRunTarget::CpuOutOfRange { cpu: 48, core_count: 48 })
        );
        assert_eq!(
            is_run_target(&verified(Some("0,50,3"), Some(48))),
            Err(NotRunTarget::CpuOutOfRange { cpu: 50, core_count: 48 })
        );
        assert_eq!(
            is_run_target(&verified(Some("0"), Some(0))),
            Err(NotRunTarget::CpuOutOfRange { cpu: 0, core_count: 0 })
        );
    }
}
