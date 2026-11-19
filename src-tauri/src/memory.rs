//! The memory and disk preflight (ADR-024 o26, unit 5.5; Decision h). Pure: the input's
//! `%maxcore` ([`read_maxcore`], [`apply_maxcore`]), the cap-based estimate ([`estimate`]), the
//! warning thresholds and the proposal P, the parsers of the host facts (`/proc/meminfo`, `df`) and
//! the submit-time notice. The local host's facts are read by [`host_resources_at`] (a path seam for
//! the tests); the Tauri commands are `commands::memory`.
//!
//! **One estimator** (Anton, o26.2): the New Job screen (through `commands::memory::memory_estimate`)
//! and the remote submit (`ssh_backend::upload_and_submit`) call the same [`estimate`].
//!
//! **Units** (rule #11). `/proc/meminfo` prints `kB`, which is **KiB** (measured by a determiner,
//! `wiki/orca/memory.md`), so its values stay KiB here (`*_kib`); `df -B1` and `statvfs` are bytes
//! (`*_bytes`); ORCA's `%maxcore` and the estimate are **MiB** (`*_mib`; ORCA's "MB" is MiB, I).
//! No bare number leaves this module without its unit in its name.
//!
//! **The estimate** (o26.1): `U = n × (m + 100) + 140` MiB — an estimate **from the cap**, not an
//! upper bound: real use is usually far lower, and a cap that binds is exceeded (memory.md).

use std::path::Path;

use serde::Serialize;

use crate::ssh_backend::align_remote_pal;

/// ORCA's `%maxcore` when the input sets none: 4096 MiB on 6.1.x (the value measured, its unit
/// inferred — memory.md; other versions unmeasured, an over-estimate at worst, o26.13.7).
pub const ORCA_DEFAULT_MAXCORE_MIB: u64 = 4096;
/// The runtime overhead per rank (memory.md's table, I).
pub const RANK_OVERHEAD_MIB: u64 = 100;
/// The overhead once per job: the master `orca` and `mpirun` (memory.md, I).
pub const JOB_OVERHEAD_MIB: u64 = 140;
/// The share of RAM a job may take before the warning (policy, o26.3): 9/10, 10 % for the OS.
const RAM_SHARE: (u64, u64) = (9, 10);
/// The free space on the job root below which the warning shows (policy, o26.3).
pub const MIN_FREE_DISK_BYTES: u64 = 10 * 1024 * 1024 * 1024;
/// The builder's `%maxcore` step; P is rounded down to it (o26.4).
pub const MAXCORE_STEP_MIB: u64 = 100;
/// Below this P proposes nothing: the target is too small for n ranks (o26.4).
pub const MIN_PROPOSAL_MIB: u64 = 300;
/// The local default when the host facts are unreadable — today's builder value (o26.5, o26.13.4).
pub const FALLBACK_MAXCORE_MIB: u64 = 2000;
/// `/proc/meminfo` is ~1.5 KB; a reader refuses a file larger than this (rule #5's spirit).
const MEMINFO_CAP_BYTES: u64 = 64 * 1024;
const KIB_PER_MIB: u64 = 1024;

// --- The input's %maxcore ---------------------------------------------------------------------

/// A line with its `#` comment stripped — the app's existing rule (`src/editor/enclosing-block.ts`,
/// `line.replace(/#.*$/, "")`; ORCA's comment syntax has no rule-#10 record, o26.13.5).
fn strip_comment(line: &str) -> &str {
    match line.find('#') {
        Some(i) => &line[..i],
        None => line,
    }
}

/// The locator shared by [`read_maxcore`] and [`apply_maxcore`] (o26.13.5): the indices of the
/// lines whose first token, comments stripped, begins with `%maxcore` (any case). A commented-out
/// `# %maxcore 3000` is not one.
pub(crate) fn maxcore_lines(lines: &[&str]) -> Vec<usize> {
    lines
        .iter()
        .enumerate()
        .filter(|(_, l)| {
            strip_comment(l)
                .split_whitespace()
                .next()
                .is_some_and(|t| t.to_ascii_lowercase().starts_with("%maxcore"))
        })
        .map(|(i, _)| i)
        .collect()
}

/// The input's `%maxcore` in MiB: `Ok(None)` when it sets none (ORCA's default applies), or why it
/// cannot be read — two `%maxcore` lines, or a line that is not exactly `%maxcore <positive
/// integer>` after the comment is stripped (a simple directive, no `end`). Never a guess (rule #9).
pub fn read_maxcore(input: &str) -> Result<Option<u64>, String> {
    let lines: Vec<&str> = input.lines().collect();
    match maxcore_lines(&lines).as_slice() {
        [] => Ok(None),
        [i] => {
            let tokens: Vec<&str> = strip_comment(lines[*i]).split_whitespace().collect();
            match tokens.as_slice() {
                [name, value] if name.eq_ignore_ascii_case("%maxcore") => value
                    .parse::<u64>()
                    .ok()
                    .filter(|v| *v >= 1 && value.bytes().all(|b| b.is_ascii_digit()))
                    .map(Some)
                    .ok_or_else(|| format!("cannot read the input's memory setting: {:?}", lines[*i].trim())),
                _ => Err(format!("cannot read the input's memory setting: {:?}", lines[*i].trim())),
            }
        }
        more => Err(format!("cannot read the input's memory setting: {} %maxcore lines", more.len())),
    }
}

/// The button's edit (o26.4, o26.13.5): the input with `%maxcore` set to `maxcore_mib` — the single
/// existing `%maxcore` line (found by [`read_maxcore`]'s own locator) replaced, else `%maxcore P`
/// inserted right after the **first** `!` line, else at the top. Two `%maxcore` lines are refused
/// (no deterministic choice). The input's line ending (LF, or CRLF throughout) is kept. Post-condition
/// (rule #9): the result reads back as exactly that value.
pub fn apply_maxcore(input: &str, maxcore_mib: u64) -> Result<String, String> {
    if maxcore_mib == 0 {
        return Err("%maxcore must be a positive number of MiB".into());
    }
    let line = format!("%maxcore {maxcore_mib}");
    let mut lines: Vec<&str> = input.lines().collect();
    match maxcore_lines(&lines).as_slice() {
        [] => {
            let at = lines.iter().position(|l| l.trim_start().starts_with('!')).map_or(0, |i| i + 1);
            lines.insert(at, line.as_str());
        }
        [i] => lines[*i] = line.as_str(),
        more => return Err(format!("the input has {} %maxcore lines; edit it by hand", more.len())),
    }
    // The input's own line ending is kept (a CRLF input stays CRLF): CRLF only when every newline is.
    let newline = if input.contains("\r\n") && input.matches('\n').count() == input.matches("\r\n").count() {
        "\r\n"
    } else {
        "\n"
    };
    let mut out = lines.join(newline);
    if input.ends_with('\n') || input.is_empty() {
        out.push_str(newline);
    }
    match read_maxcore(&out) {
        Ok(Some(v)) if v == maxcore_mib => Ok(out),
        other => Err(format!("after setting %maxcore to {maxcore_mib} the input reads {other:?}")),
    }
}

/// The largest `PALn` simple keyword on the input's `!` lines, if any (`! … PAL8`, any case).
fn read_pal_keyword(input: &str) -> Option<u32> {
    input
        .lines()
        .map(strip_comment)
        .filter(|l| l.trim_start().starts_with('!'))
        .flat_map(|l| l.trim_start().trim_start_matches('!').split_whitespace().collect::<Vec<_>>())
        .filter_map(|t| {
            let lower = t.to_ascii_lowercase();
            let digits = lower.strip_prefix("pal")?;
            if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            digits.parse::<u32>().ok().filter(|n| *n >= 1)
        })
        .max()
}

// --- The estimate -----------------------------------------------------------------------------

/// Where the job will run, as far as its rank count goes (o26.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RankRule<'a> {
    /// This machine: always the CPU preset's count (`local_backend::resolve_cpu_config`) —
    /// `align_pal_nprocs` rewrites an explicit `%pal` in both directions, so the input's is ignored.
    Local { preset_nprocs: u32 },
    /// A server profile's core mask: `%pal` aligned downward to the mask's distinct CPUs, or the
    /// mask's count without one — derived by the submit's own [`align_remote_pal`] (o14.2).
    Remote { core_mask: &'a str },
}

/// The cap-based estimate of one input on one target (o26.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Estimate {
    /// n: the ranks the backend will run; with a `PALn` keyword, the larger of the two.
    pub ranks: u32,
    /// m: the input's `%maxcore`, or ORCA's default.
    pub maxcore_mib: u64,
    /// Whether m is the input's own (false: ORCA's default 4096).
    pub maxcore_from_input: bool,
    /// U = n × (m + 100) + 140.
    pub estimate_mib: u64,
}

/// n for `input` on `rule`, the `PALn` keyword taken as the larger (which of the two ORCA honours
/// is ADR-025's open question, so the larger is assumed).
pub fn ranks(input: &str, rule: RankRule<'_>) -> Result<u32, String> {
    let backend = match rule {
        RankRule::Local { preset_nprocs } => preset_nprocs,
        RankRule::Remote { core_mask } => align_remote_pal(input, core_mask)?.1.nprocs,
    };
    Ok(read_pal_keyword(input).map_or(backend, |keyword| keyword.max(backend)))
}

/// U for `input` on `rule` (o26.1), or why there is none ("cannot read the input's memory setting",
/// an unreadable `%pal` on a remote target, an overflow).
pub fn estimate(input: &str, rule: RankRule<'_>) -> Result<Estimate, String> {
    let ranks = ranks(input, rule)?;
    let from_input = read_maxcore(input)?;
    let maxcore_mib = from_input.unwrap_or(ORCA_DEFAULT_MAXCORE_MIB);
    let estimate_mib = maxcore_mib
        .checked_add(RANK_OVERHEAD_MIB)
        .and_then(|per_rank| per_rank.checked_mul(u64::from(ranks)))
        .and_then(|all| all.checked_add(JOB_OVERHEAD_MIB))
        .ok_or_else(|| format!("%maxcore {maxcore_mib} × {ranks} ranks is too large to estimate"))?;
    Ok(Estimate { ranks, maxcore_mib, maxcore_from_input: from_input.is_some(), estimate_mib })
}

// --- Thresholds and the proposal --------------------------------------------------------------

/// 0.9 × `mem_kib`, in whole MiB, rounded down. For an integer U, `U > 0.9 × RAM` ⇔ `U >` this.
pub fn ram_threshold_mib(mem_kib: u64) -> u64 {
    let scaled = u128::from(mem_kib) * u128::from(RAM_SHARE.0) / (u128::from(RAM_SHARE.1) * u128::from(KIB_PER_MIB));
    u64::try_from(scaled).unwrap_or(u64::MAX)
}

/// The RAM warning (o26.3): U > 0.9 × the RAM it is compared with.
pub fn ram_warns(estimate_mib: u64, mem_kib: u64) -> bool {
    estimate_mib > ram_threshold_mib(mem_kib)
}

/// The disk warning (o26.3): less than 10 GiB free on the job root.
pub fn disk_warns(avail_bytes: u64) -> bool {
    avail_bytes < MIN_FREE_DISK_BYTES
}

/// P before the cut at 300: ⌊((0.9 × MemTotal − 140) / n − 100) / 100⌋ × 100 MiB, in exact integer
/// arithmetic (each floor of a floor equals the floor of the whole). May be negative.
pub fn raw_proposal_mib(mem_total_kib: u64, ranks: u32) -> i64 {
    let ranks = i64::from(ranks.max(1));
    let budget = i64::try_from(ram_threshold_mib(mem_total_kib)).unwrap_or(i64::MAX) - JOB_OVERHEAD_MIB as i64;
    let step = MAXCORE_STEP_MIB as i64;
    (budget.div_euclid(ranks) - RANK_OVERHEAD_MIB as i64).div_euclid(step) * step
}

/// P (o26.4), or `None` when it is below 300 — the target is too small for n ranks at any useful
/// `%maxcore`, and nothing is proposed.
pub fn proposal_mib(mem_total_kib: u64, ranks: u32) -> Option<u64> {
    u64::try_from(raw_proposal_mib(mem_total_kib, ranks)).ok().filter(|p| *p >= MIN_PROPOSAL_MIB)
}

/// The local default `%maxcore` (o26.5, o26.13.4): P for the preset's ranks; 2000 when the host's
/// RAM is unknown; 300 when P is below it.
pub fn fitted_default_mib(mem_total_kib: Option<u64>, ranks: u32) -> u64 {
    match mem_total_kib {
        None => FALLBACK_MAXCORE_MIB,
        Some(kib) => u64::try_from(raw_proposal_mib(kib, ranks)).unwrap_or(0).max(MIN_PROPOSAL_MIB),
    }
}

// --- Host facts -------------------------------------------------------------------------------

/// The value of `key` in `/proc/meminfo`'s text (or `head -5` of it), in KiB: exactly one line
/// `<key>:<spaces><digits> kB` (the file's literal `kB` = KiB, memory.md). Anything else is an `Err`.
pub fn meminfo_kib(text: &str, key: &str) -> Result<u64, String> {
    let prefix = format!("{key}:");
    let found: Vec<&str> = text.lines().filter(|l| l.starts_with(&prefix)).collect();
    let [line] = found.as_slice() else {
        return Err(format!("meminfo: {} {key} lines", found.len()));
    };
    match line[prefix.len()..].split_whitespace().collect::<Vec<_>>().as_slice() {
        [value, "kB"] if value.bytes().all(|b| b.is_ascii_digit()) => {
            value.parse::<u64>().map_err(|_| format!("meminfo: {key} {value:?} does not fit"))
        }
        _ => Err(format!("meminfo: unexpected {key} line {line:?}")),
    }
}

/// One row of `df --output=avail,target -B1 <path>` (memory.md): the header `Avail Mounted on`,
/// then `<bytes> <mount>` — the first token bytes, the rest the mount. Exactly those two lines.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DfRow {
    pub avail_bytes: u64,
    pub target: String,
}

pub fn parse_df_avail(text: &str) -> Result<DfRow, String> {
    let lines: Vec<&str> = text.lines().collect();
    let [header, row] = lines.as_slice() else {
        return Err(format!("df: {} lines, not a header and one row", lines.len()));
    };
    if header.split_whitespace().collect::<Vec<_>>() != ["Avail", "Mounted", "on"] {
        return Err(format!("df: unexpected header {header:?}"));
    }
    let row = row.trim_start();
    let (bytes, target) = row.split_once(char::is_whitespace).ok_or_else(|| format!("df: unexpected row {row:?}"))?;
    let target = target.trim();
    if bytes.is_empty() || !bytes.bytes().all(|b| b.is_ascii_digit()) || target.is_empty() {
        return Err(format!("df: unexpected row {row:?}"));
    }
    let avail_bytes = bytes.parse::<u64>().map_err(|_| format!("df: {bytes:?} does not fit"))?;
    Ok(DfRow { avail_bytes, target: target.to_string() })
}

/// A host fact, or why it is unknown — never a stand-in value (honest-or-absent).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Fact<T> {
    Known { value: T },
    Unknown { reason: String },
}

impl<T: Copy> Fact<T> {
    pub fn from_result(r: Result<T, String>) -> Self {
        match r {
            Ok(value) => Fact::Known { value },
            Err(reason) => Fact::Unknown { reason },
        }
    }

    pub fn value(&self) -> Option<T> {
        match self {
            Fact::Known { value } => Some(*value),
            Fact::Unknown { .. } => None,
        }
    }
}

/// This machine's RAM and the free space under the app's data dir, read now (o26.6, nothing stored).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HostResources {
    pub mem_total_kib: Fact<u64>,
    pub disk_avail_bytes: Fact<u64>,
}

/// [`HostResources`] from `meminfo` (`/proc/meminfo` in the app; a fixture in the tests) and
/// `statvfs(data_dir)`: `f_bavail × f_frsize`, the space an unprivileged user may write — df's
/// `Avail`.
pub fn host_resources_at(meminfo: &Path, data_dir: &Path) -> HostResources {
    HostResources {
        mem_total_kib: Fact::from_result(read_meminfo(meminfo).and_then(|text| meminfo_kib(&text, "MemTotal"))),
        disk_avail_bytes: Fact::from_result(statvfs_avail_bytes(data_dir)),
    }
}

fn read_meminfo(path: &Path) -> Result<String, String> {
    use std::io::Read;
    let file = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut text = String::new();
    file.take(MEMINFO_CAP_BYTES + 1)
        .read_to_string(&mut text)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    if text.len() as u64 > MEMINFO_CAP_BYTES {
        return Err(format!("{}: larger than {MEMINFO_CAP_BYTES} bytes", path.display()));
    }
    Ok(text)
}

fn statvfs_avail_bytes(path: &Path) -> Result<u64, String> {
    use std::os::unix::ffi::OsStrExt;
    let c_path = std::ffi::CString::new(path.as_os_str().as_bytes())
        .map_err(|_| format!("{}: a NUL in the path", path.display()))?;
    // SAFETY: `c_path` is a valid NUL-terminated string and `stat` a properly sized out-parameter;
    // statvfs(3) writes only into it.
    let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(c_path.as_ptr(), &mut stat) } != 0 {
        return Err(format!("statvfs {}: {}", path.display(), std::io::Error::last_os_error()));
    }
    #[allow(clippy::unnecessary_cast)] // the field types differ between libc targets
    let (blocks, size) = (stat.f_bavail as u64, stat.f_frsize as u64);
    blocks.checked_mul(size).ok_or_else(|| format!("statvfs {}: the free space overflows", path.display()))
}

// --- The checks the New Job screen shows ------------------------------------------------------

/// The RAM side of a check: the target's RAM and, with an estimate, whether U exceeds 0.9 × it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum RamCheck {
    /// The RAM is not known: no comparison, no proposal (o26.7: "server memory unknown").
    Unknown { reason: String },
    Known {
        mem_total_kib: u64,
        /// 0.9 × RAM, whole MiB: U warns iff it is above this.
        threshold_mib: u64,
        /// `None` without an estimate.
        warns: Option<bool>,
    },
}

/// The disk side of a check: the free space on the target's job root and whether it is below 10 GiB.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum DiskCheck {
    Unknown { reason: String },
    Known { avail_bytes: u64, min_free_bytes: u64, warns: bool },
}

/// The estimate's side: U, or why there is none.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum EstimateState {
    Estimated(Estimate),
    NotEstimated { reason: String },
}

/// One target's verdict for the current buffer (o26.7): the estimate, RAM, disk and P.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MemoryCheck {
    pub estimate: EstimateState,
    pub ram: RamCheck,
    pub disk: DiskCheck,
    /// P, when the RAM and n are known and P ≥ 300.
    pub proposal_mib: Option<u64>,
    /// The RAM and n are known but P < 300: too small for n ranks at any useful `%maxcore`.
    pub too_small_for_ranks: bool,
    /// When the facts were measured: the server's last connection test (`host_facts_at`); `None`
    /// for this machine (read now) or when nothing was measured.
    pub facts_measured_at: Option<String>,
    /// Whether the RAM or the disk warns (the row shows a line, with the button, for this target).
    pub warns: bool,
}

/// The check of `estimate` against a target's facts. P needs n, so an estimate that failed only for
/// the input's `%maxcore` still proposes nothing.
pub fn check(
    estimate: Result<Estimate, String>,
    mem_total_kib: Fact<u64>,
    disk_avail_bytes: Fact<u64>,
    facts_measured_at: Option<String>,
) -> MemoryCheck {
    let u = estimate.as_ref().ok().map(|e| e.estimate_mib);
    let ranks = estimate.as_ref().ok().map(|e| e.ranks);
    let ram = match mem_total_kib {
        Fact::Unknown { reason } => RamCheck::Unknown { reason },
        Fact::Known { value } => RamCheck::Known {
            mem_total_kib: value,
            threshold_mib: ram_threshold_mib(value),
            warns: u.map(|u| ram_warns(u, value)),
        },
    };
    let disk = match disk_avail_bytes {
        Fact::Unknown { reason } => DiskCheck::Unknown { reason },
        Fact::Known { value } => DiskCheck::Known { avail_bytes: value, min_free_bytes: MIN_FREE_DISK_BYTES, warns: disk_warns(value) },
    };
    let (proposal, too_small) = match (&ram, ranks) {
        (RamCheck::Known { mem_total_kib, .. }, Some(n)) => {
            let p = proposal_mib(*mem_total_kib, n);
            (p, p.is_none())
        }
        _ => (None, false),
    };
    let warns = matches!(ram, RamCheck::Known { warns: Some(true), .. }) || matches!(disk, DiskCheck::Known { warns: true, .. });
    MemoryCheck {
        warns,
        estimate: match estimate {
            Ok(e) => EstimateState::Estimated(e),
            Err(reason) => EstimateState::NotEstimated { reason },
        },
        ram,
        disk,
        proposal_mib: proposal,
        too_small_for_ranks: too_small,
        facts_measured_at,
    }
}

// --- The submit-time check (o26.6, o26.13.2) --------------------------------------------------

/// What the submit's prepare call read on the server (`remote::prepare`): each fact `None` when its
/// read failed — recorded, never gating.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SubmitHostFacts {
    pub mem_total_kib: Option<u64>,
    pub mem_available_kib: Option<u64>,
    pub root_avail_bytes: Option<u64>,
}

fn gib(mib: u64) -> String {
    format!("{:.1} GiB", mib as f64 / 1024.0)
}

fn gib_from_kib(kib: u64) -> String {
    gib(kib / KIB_PER_MIB)
}

/// The notice of a submit the user started (Create & Run, Retry), beside the `%pal` notice; `None`
/// when everything fits. Never a refusal (warn, never block — Anton). The RAM it compares U with
/// (o26.13.2): with **no** other live remote job of ours on the profile, MemAvailable (other users'
/// load counts now); **with** one, MemTotal — MemAvailable then has our own job subtracted, a false
/// positive in the common "queue two jobs" case — and MemAvailable is shown as context only.
pub fn submit_memory_notice(estimate: &Result<Estimate, String>, facts: &SubmitHostFacts, other_live_jobs: usize) -> Option<String> {
    let mut parts = Vec::new();
    match estimate {
        Err(reason) => parts.push(format!("memory not checked: {reason}")),
        Ok(e) => {
            let (compared, label) = if other_live_jobs == 0 {
                (facts.mem_available_kib, "available")
            } else {
                (facts.mem_total_kib, "in total")
            };
            match compared {
                None => parts.push(format!(
                    "memory not checked: the server's {} memory could not be read",
                    if other_live_jobs == 0 { "available" } else { "total" }
                )),
                Some(kib) if ram_warns(e.estimate_mib, kib) => {
                    let mut text = format!(
                        "memory: the job's estimate is {} ({} ranks × (%maxcore {} + {RANK_OVERHEAD_MIB}) + {JOB_OVERHEAD_MIB} MiB{}), \
                         above 0.9 × the server's {} {label}",
                        gib(e.estimate_mib),
                        e.ranks,
                        e.maxcore_mib,
                        if e.maxcore_from_input { "" } else { ", ORCA's default %maxcore" },
                        gib_from_kib(kib),
                    );
                    if other_live_jobs > 0 {
                        if let Some(avail) = facts.mem_available_kib {
                            text.push_str(&format!(" ({} available now, with {other_live_jobs} of our jobs on it)", gib_from_kib(avail)));
                        }
                    }
                    text.push_str("; it may swap. An estimate from the cap, not an upper bound");
                    if let Some(p) = facts.mem_total_kib.and_then(|kib| proposal_mib(kib, e.ranks)) {
                        text.push_str(&format!("; %maxcore {p} fits {} ranks", e.ranks));
                    }
                    parts.push(text);
                }
                Some(_) => {}
            }
        }
    }
    match facts.root_avail_bytes {
        None => parts.push("free disk not checked: the server's free space could not be read".into()),
        Some(bytes) if disk_warns(bytes) => parts.push(format!(
            "disk: {} free on the server's job root, below {}",
            gib(bytes / (1024 * 1024)),
            gib(MIN_FREE_DISK_BYTES / (1024 * 1024))
        )),
        Some(_) => {}
    }
    (!parts.is_empty()).then(|| format!("[OrcaStudio] {}.", parts.join(". ")))
}

#[cfg(test)]
mod tests;
