//! Moving a remote job's files (ADR-024 o items 3.2, 3.3.4 and 6): the rsync argv for upload and
//! download, the download's filter, and the file lists both post-conditions compare.
//!
//! Nothing here spawns a process. The argv builders return what `SystemRunner` will run; the
//! listing functions read the **local** job dir only.
//!
//! **The download set has one source.** The filter is [`crate::artifacts::ARTIFACT_PATTERNS`] —
//! the list the curated group export also uses — plus the download-only extras: `stderr.log`, the
//! job markers ([`MARKERS`]), the `tsp` output dir `.tsp-out/` (it needs both `--include=.tsp-out/`
//! and `--include=.tsp-out/**`, measured, probe 5.3a) and `*.gbw` only when the job opts in. Then
//! `--exclude=*`, last: rsync takes the first matching rule, and excluding `*` also stops it from
//! entering any directory not listed (`sub/deep.xyz` stays behind, measured).
//!
//! **rsync rules (measured, probe 5.3a, `wiki/orca/remote-sync-probe.md`).** Both directions carry
//! the same ssh options as every other ssh call ([`super::ssh::ssh_options`]). Upload is
//! `-a --checksum --mkpath`: without `--mkpath` a missing parent fails with rc 11, and `--checksum`
//! keeps a retry from trusting size+mtime over a wrong remote file. Download is `-a --checksum` for
//! the same reason, in the other direction. **Never `--partial`** (an
//! interrupted transfer would leave a half file under its final name, rc 12) and **never
//! `--delete`** (it would run before the server's no-marker assert and could touch a live dir).

use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::path::Path;

use sha2::{Digest as _, Sha256};

use super::classify::is_valid_path;
use super::poll::{check_echo, PollError};
use super::ssh::ssh_options;
use super::wire::{Reader, WireError};
use crate::artifacts::{glob_match, is_artifact, ARTIFACT_PATTERNS};
use crate::execution_backend::FetchPolicy;
use crate::models::server_profile::{validate_host, InvalidProfile};

/// The rsync client, found on `PATH` (ADR-005: system tools).
pub const RSYNC_PROGRAM: &str = "rsync";

/// The job markers (ADR-024 l, o item 3.3.3). All of them come down with the results.
pub const MARKERS: &[&str] = &[".exit_code", ".started", ".enqueued", ".cancelled", ".submitting"];

/// The wrapper's stderr, written next to `output.out`.
pub const STDERR_LOG: &str = "stderr.log";

/// The per-job directory `tsp` writes its output file into (ADR-024 m item 1).
pub const TSP_OUT_DIR: &str = ".tsp-out";

/// The large wavefunction, fetched only when the job opts in.
pub const GBW_PATTERN: &str = "*.gbw";

/// A job dir holding more files than this is refused before any ssh (ADR-024 o item 3.3): the
/// submit call carries two NUL values per file.
pub const MAX_UPLOAD_FILES: usize = 1000;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SyncError {
    #[error(transparent)]
    Host(#[from] InvalidProfile),
    #[error("remote path {0:?} breaks the path rule")]
    RemotePath(String),
    #[error("local path {0:?} is not absolute")]
    LocalPath(String),
    #[error("the job dir holds {count} files, more than {MAX_UPLOAD_FILES}")]
    TooManyFiles { count: usize },
    #[error("{0:?}: a file name outside [A-Za-z0-9._-] cannot be uploaded")]
    BadName(String),
    #[error("{0:?} is not a regular file or directory and cannot be uploaded")]
    NotARegularFile(String),
    #[error("{path}: {message}")]
    Io { path: String, message: String },
    #[error(transparent)]
    Reply(#[from] PollError),
    #[error("listing reply: {0}")]
    Listing(String),
}

impl From<WireError> for SyncError {
    fn from(e: WireError) -> Self {
        SyncError::Reply(PollError::Wire(e))
    }
}

/// Every artifact pattern, `stderr.log` and the markers, each once, in that order.
fn leaf_includes() -> Vec<&'static str> {
    let mut includes: Vec<&'static str> = Vec::new();
    for name in ARTIFACT_PATTERNS.iter().chain([&STDERR_LOG]).chain(MARKERS) {
        if !includes.contains(name) {
            includes.push(name);
        }
    }
    includes
}

/// The leaf-name patterns the download selects at the top of the job dir: [`leaf_includes`] plus
/// `*.gbw` if opted in. `.tsp-out/` is not among them (the post-condition leaves it out). These
/// are the values the server's listing ([`super::scripts::LIST`]) matches names against.
pub fn download_patterns(policy: FetchPolicy) -> Vec<&'static str> {
    let mut patterns = leaf_includes();
    if policy.include_gbw {
        patterns.push(GBW_PATTERN);
    }
    patterns
}

/// The rsync filter of a download, in order: every artifact pattern, `stderr.log`, the markers,
/// `.tsp-out/` and its contents, `*.gbw` if opted in, then `--exclude=*`.
pub fn download_filter_args(policy: FetchPolicy) -> Vec<String> {
    let mut args: Vec<String> = leaf_includes().iter().map(|p| format!("--include={p}")).collect();
    args.push(format!("--include={TSP_OUT_DIR}/"));
    args.push(format!("--include={TSP_OUT_DIR}/**"));
    if policy.include_gbw {
        args.push(format!("--include={GBW_PATTERN}"));
    }
    args.push("--exclude=*".into());
    args
}

/// Whether the download filter selects `rel_path` (relative to the job dir, `/`-separated). The
/// same rules as [`download_filter_args`], for the local side of the download post-condition;
/// a test checks the two against the real rsync on every fixture path.
pub fn download_selects(rel_path: &str, policy: FetchPolicy) -> bool {
    if let Some(inside) = rel_path.strip_prefix(TSP_OUT_DIR).and_then(|r| r.strip_prefix('/')) {
        return !inside.is_empty();
    }
    if rel_path.contains('/') {
        return false;
    }
    is_artifact(rel_path)
        || rel_path == STDERR_LOG
        || MARKERS.contains(&rel_path)
        || (policy.include_gbw && glob_match(GBW_PATTERN, rel_path))
}

/// rsync's `-e`: the ssh command with the options every OrcaStudio ssh carries.
pub fn rsync_ssh_transport() -> String {
    format!("ssh {}", ssh_options().join(" "))
}

/// `rsync -a --checksum --mkpath -e '<ssh>' <local>/ <host>:<remote>/` (ADR-024 o item 3.2). The
/// trailing slashes copy the dir's contents into the remote job dir, not a nested dir (measured).
pub fn upload_argv(local_dir: &str, host: &str, remote_dir: &str) -> Result<Vec<String>, SyncError> {
    let (local, remote) = checked_ends(local_dir, host, remote_dir)?;
    Ok(vec![
        "-a".into(),
        "--checksum".into(),
        "--mkpath".into(),
        "-e".into(),
        rsync_ssh_transport(),
        format!("{local}/"),
        format!("{host}:{remote}/"),
    ])
}

/// `rsync -a --checksum <filter> -e '<ssh>' <host>:<remote>/ <local>/` (ADR-024 o item 6). No
/// `--partial`. `--checksum` (Anton, 2026-10-05, mirroring upload) so a retried fetch re-sends a
/// local file that was corrupted with its size and mtime intact, instead of keeping it and failing
/// the hash post-condition again.
pub fn download_argv(
    host: &str,
    remote_dir: &str,
    local_dir: &str,
    policy: FetchPolicy,
) -> Result<Vec<String>, SyncError> {
    let (local, remote) = checked_ends(local_dir, host, remote_dir)?;
    let mut argv = vec!["-a".to_string(), "--checksum".to_string()];
    argv.extend(download_filter_args(policy));
    argv.extend(["-e".into(), rsync_ssh_transport(), format!("{host}:{remote}/"), format!("{local}/")]);
    Ok(argv)
}

/// Validate both ends of a transfer: the host as for ssh, the remote dir by the one path rule of
/// ADR-024 l detail 4 (it is the recorded job dir), the local dir absolute. Returns both without a
/// trailing slash.
fn checked_ends<'a>(local: &'a str, host: &str, remote: &'a str) -> Result<(&'a str, &'a str), SyncError> {
    validate_host(host)?;
    if !is_valid_path(remote) {
        return Err(SyncError::RemotePath(remote.to_string()));
    }
    let local_trimmed = local.trim_end_matches('/');
    if !local.starts_with('/') || local_trimmed.is_empty() {
        return Err(SyncError::LocalPath(local.to_string()));
    }
    Ok((local_trimmed, remote))
}

/// What a listed entry holds: a regular file's sha256 (lowercase hex), or a symlink's target (the
/// `.submitting` claim is a dangling symlink, ADR-024 o item 3.3.7, and has no content to hash).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Digest {
    Sha256(String),
    Symlink(String),
}

/// One entry of a job dir listing, by its path relative to the job dir (`/`-separated).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct FileDigest {
    pub name: String,
    pub digest: Digest,
}

/// List the entries under `dir` that `select` accepts (by relative path), sorted by name: each
/// regular file with its sha256, each symlink with its target. Directories are walked, not listed.
/// Anything else (a socket, a fifo) is [`SyncError::NotARegularFile`].
pub fn list_dir(dir: &Path, select: &dyn Fn(&str) -> bool) -> Result<Vec<FileDigest>, SyncError> {
    let mut paths = Vec::new();
    walk(dir, "", &mut paths)?;
    let mut out = Vec::new();
    for (name, kind) in paths {
        if !select(&name) {
            continue;
        }
        let full = dir.join(&name);
        let digest = match kind {
            Kind::File => Digest::Sha256(sha256_file(&full)?),
            Kind::Symlink => Digest::Symlink(
                std::fs::read_link(&full)
                    .map_err(|e| io_error(&full, e))?
                    .to_string_lossy()
                    .into_owned(),
            ),
            Kind::Other => return Err(SyncError::NotARegularFile(name)),
        };
        out.push(FileDigest { name, digest });
    }
    out.sort();
    Ok(out)
}

/// The expected list of an upload (ADR-024 o item 3.3): every file of the local job dir with its
/// sha256. Refused before any ssh for more than [`MAX_UPLOAD_FILES`] files, a name outside the
/// path rule's characters (it travels as a NUL value and is compared by the server's listing), or
/// anything that is not a regular file — a job dir we prepared holds nothing else.
pub fn upload_expected(dir: &Path) -> Result<Vec<FileDigest>, SyncError> {
    let mut paths = Vec::new();
    walk(dir, "", &mut paths)?;
    if paths.len() > MAX_UPLOAD_FILES {
        return Err(SyncError::TooManyFiles { count: paths.len() });
    }
    for (name, kind) in &paths {
        if !is_valid_path(&format!("/{name}")) {
            return Err(SyncError::BadName(name.clone()));
        }
        if *kind != Kind::File {
            return Err(SyncError::NotARegularFile(name.clone()));
        }
    }
    list_dir(dir, &|_| true)
}

/// The submit call's file values: two NUL values per file, name then sha256 (ADR-024 o item 3.3).
/// A symlink has no sha256 to send, so it is refused ([`upload_expected`] never yields one).
pub fn expected_values(files: &[FileDigest]) -> Result<Vec<String>, SyncError> {
    let mut values = Vec::with_capacity(files.len() * 2);
    for f in files {
        let Digest::Sha256(hex) = &f.digest else {
            return Err(SyncError::NotARegularFile(f.name.clone()));
        };
        values.push(f.name.clone());
        values.push(hex.clone());
    }
    Ok(values)
}

/// How the downloaded files differ from the server's listing. Every list names files, sorted.
#[derive(Debug, Clone, PartialEq, Eq, Default, thiserror::Error)]
#[error("download post-condition failed: missing {missing:?}, extra {extra:?}, differing {differing:?}")]
pub struct DownloadMismatch {
    /// On the server, not here.
    pub missing: Vec<String>,
    /// Here, not on the server.
    pub extra: Vec<String>,
    /// On both, with a different hash (or symlink target).
    pub differing: Vec<String>,
}

/// The download post-condition (ADR-024 o item 6, rule #9): the filter-selected local subset must
/// equal the server's listing of the same filter, every hash matching. `.tsp-out/` is left out on
/// both sides (it is tsp's, and can still change).
pub fn compare_download(local: &[FileDigest], server: &[FileDigest]) -> Result<(), DownloadMismatch> {
    let index = |list: &[FileDigest]| -> BTreeMap<String, Digest> {
        list.iter()
            .filter(|f| !f.name.starts_with(&format!("{TSP_OUT_DIR}/")))
            .map(|f| (f.name.clone(), f.digest.clone()))
            .collect()
    };
    let (here, there) = (index(local), index(server));
    let mut mismatch = DownloadMismatch::default();
    for (name, digest) in &there {
        match here.get(name) {
            None => mismatch.missing.push(name.clone()),
            Some(d) if d != digest => mismatch.differing.push(name.clone()),
            Some(_) => {}
        }
    }
    let there_names: BTreeSet<&String> = there.keys().collect();
    mismatch.extra = here.keys().filter(|n| !there_names.contains(n)).cloned().collect();
    if mismatch == DownloadMismatch::default() {
        Ok(())
    } else {
        Err(mismatch)
    }
}

/// The first line of every listing reply; the number is the format version.
pub const LIST_HEADER: &str = "orcastudio-list 1";

/// What one server listing sends: the job's recorded dir and the download's patterns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListArgs {
    pub job_dir: String,
    pub policy: FetchPolicy,
}

impl ListArgs {
    pub fn new(job_dir: &str, policy: FetchPolicy) -> Result<Self, SyncError> {
        if !is_valid_path(job_dir) {
            return Err(SyncError::RemotePath(job_dir.to_string()));
        }
        Ok(ListArgs { job_dir: job_dir.to_string(), policy })
    }

    /// The NUL-list values: the job dir, then [`download_patterns`].
    pub fn values(&self) -> Vec<String> {
        let mut values = vec![self.job_dir.clone()];
        values.extend(download_patterns(self.policy).into_iter().map(String::from));
        values
    }
}

/// Parse the server's listing into the selected entries, sorted — the `server` side of
/// [`compare_download`]. The reply (records, the 5.2 format):
///
/// ```text
/// orcastudio-list 1
/// argc <n>
/// arg <len>              × n: the job dir, then the patterns, verbatim
/// entries <n>            then n times:
///   entry <len>          a top-level name (never `.tsp-out`), then one of
///     sha256 <hex>       selected regular file
///     link <len>         selected symlink: its target bytes (never followed)
///     dir                selected directory (not entered)
///     other              selected, but not a file, symlink or directory
///     unselected         not selected (not read)
/// end
/// ```
///
/// Rust re-derives the selection (rule #9): every verdict must equal [`download_selects`] for the
/// name, so a script that matched the patterns differently is caught, not trusted. A selected
/// `other` (a fifo named `output.out`, say), a name listed twice, a name with `/`, or `.tsp-out`
/// is [`SyncError::Listing`].
pub fn parse_list_reply(output: &[u8], sent: &ListArgs) -> Result<Vec<FileDigest>, SyncError> {
    let mut r = Reader::new(output);
    let (name, arg) = r.line()?;
    if LIST_HEADER.split_once(' ') != Some((name, arg.unwrap_or_default())) {
        return Err(r.malformed(format!("expected {LIST_HEADER:?}")).into());
    }
    check_echo(&mut r, &sent.values())?;
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for _ in 0..r.count("entries")? {
        let raw = r.bytes("entry")?.ok_or_else(|| r.malformed("entry is required".into()))?;
        let name = String::from_utf8_lossy(&raw).into_owned();
        if name.is_empty() || name.contains('/') || name == "." || name == ".." || name == TSP_OUT_DIR {
            return Err(SyncError::Listing(format!("{name:?} is not a top-level entry the listing may report")));
        }
        if !seen.insert(name.clone()) {
            return Err(SyncError::Listing(format!("{name:?} is listed twice")));
        }
        let (kind, arg) = r.line()?;
        let digest = match (kind, arg) {
            ("unselected", None) => None,
            ("dir", None) => None,
            ("other", None) => {
                return Err(SyncError::Listing(format!("{name:?} is selected but is not a file, symlink or directory")))
            }
            ("sha256", Some(hex)) if hex.len() == 64 && hex.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) => {
                Some(Digest::Sha256(hex.to_string()))
            }
            ("link", arg) => {
                Some(Digest::Symlink(r.text_after(arg)?))
            }
            _ => return Err(r.malformed(format!("expected an entry kind for {name:?}, got {kind:?}")).into()),
        };
        let selected = kind != "unselected";
        if selected != download_selects(&name, sent.policy) {
            return Err(SyncError::Listing(format!(
                "the server {} {name:?}, the download filter {}",
                if selected { "selected" } else { "did not select" },
                if selected { "does not" } else { "does" }
            )));
        }
        if let Some(digest) = digest {
            out.push(FileDigest { name, digest });
        }
    }
    if r.line()? != ("end", None) {
        return Err(r.malformed("expected end".into()).into());
    }
    if !r.at_end() {
        return Err(r.malformed("bytes after end".into()).into());
    }
    out.sort();
    Ok(out)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    File,
    Symlink,
    Other,
}

/// Collect every non-directory entry under `dir` as `(relative path, kind)`; symlinks are not
/// followed.
fn walk(dir: &Path, prefix: &str, out: &mut Vec<(String, Kind)>) -> Result<(), SyncError> {
    let entries = std::fs::read_dir(dir).map_err(|e| io_error(dir, e))?;
    for entry in entries {
        let entry = entry.map_err(|e| io_error(dir, e))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let rel = if prefix.is_empty() { name } else { format!("{prefix}/{name}") };
        let file_type = entry.file_type().map_err(|e| io_error(&entry.path(), e))?;
        if file_type.is_dir() {
            walk(&entry.path(), &rel, out)?;
        } else if file_type.is_file() {
            out.push((rel, Kind::File));
        } else if file_type.is_symlink() {
            out.push((rel, Kind::Symlink));
        } else {
            out.push((rel, Kind::Other));
        }
    }
    Ok(())
}

/// The sha256 of a file, streamed (a downloaded `.gbw` can be large).
fn sha256_file(path: &Path) -> Result<String, SyncError> {
    let mut file = std::fs::File::open(path).map_err(|e| io_error(path, e))?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf).map_err(|e| io_error(path, e))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

fn io_error(path: &Path, e: std::io::Error) -> SyncError {
    SyncError::Io { path: path.display().to_string(), message: e.to_string() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::export_group::curated_match;
    use std::path::PathBuf;
    use std::process::Command;

    fn scratch(tag: &str) -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("orcastudio-sync-{tag}-{}-{n}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    // --- argv -------------------------------------------------------------------------------

    const JOB: &str = "/home/anton/.orcastudio/jobs/0b7c1d2e-3f40-4a5b-8c6d-7e8f90a1b2c3";

    #[test]
    fn upload_argv_is_exact() {
        assert_eq!(
            upload_argv("/data/jobs/j1/", "uni", JOB).unwrap(),
            [
                "-a",
                "--checksum",
                "--mkpath",
                "-e",
                "ssh -o BatchMode=yes -o ConnectTimeout=10",
                "/data/jobs/j1/",
                &format!("uni:{JOB}/"),
            ]
        );
    }

    #[test]
    fn download_argv_is_exact_and_carries_the_filter() {
        let argv = download_argv("anton@uni", JOB, "/data/jobs/j1", FetchPolicy::SMALL_ONLY).unwrap();
        let mut want = vec!["-a".to_string(), "--checksum".to_string()];
        want.extend(download_filter_args(FetchPolicy::SMALL_ONLY));
        want.extend([
            "-e".to_string(),
            "ssh -o BatchMode=yes -o ConnectTimeout=10".to_string(),
            format!("anton@uni:{JOB}/"),
            "/data/jobs/j1/".to_string(),
        ]);
        assert_eq!(argv, want);
    }

    /// The transport options are the ones `ssh_bash_argv` uses, word for word.
    #[test]
    fn rsync_and_ssh_share_the_options() {
        let ssh = super::super::ssh::ssh_bash_argv("uni").unwrap();
        let options = ssh.split(|a| a == "--").next().unwrap().join(" ");
        assert_eq!(rsync_ssh_transport(), format!("ssh {options}"));
    }

    #[test]
    fn no_transfer_ever_carries_partial_or_delete() {
        let up = upload_argv("/l", "uni", JOB).unwrap();
        for policy in [FetchPolicy::SMALL_ONLY, FetchPolicy::WITH_GBW] {
            let down = download_argv("uni", JOB, "/l", policy).unwrap();
            for argv in [&up, &down] {
                assert!(
                    !argv.iter().any(|a| a.starts_with("--partial") || a.starts_with("--delete") || a == "-P"),
                    "{argv:?}"
                );
            }
        }
    }

    #[test]
    fn transfers_refuse_bad_ends() {
        assert!(matches!(upload_argv("/l", "-oProxyCommand=x", JOB), Err(SyncError::Host(_))));
        assert!(matches!(upload_argv("/l", "uni", "/r/../etc"), Err(SyncError::RemotePath(_))));
        assert!(matches!(upload_argv("/l", "uni", "/r/a b"), Err(SyncError::RemotePath(_))));
        assert!(matches!(upload_argv("rel/dir", "uni", JOB), Err(SyncError::LocalPath(_))));
        assert!(matches!(upload_argv("/", "uni", JOB), Err(SyncError::LocalPath(_))));
        assert!(matches!(download_argv("uni", "relative", "/l", FetchPolicy::SMALL_ONLY), Err(SyncError::RemotePath(_))));
    }

    // --- the download set through the real rsync (ADR-024 o item 6, gates a and b) ------------

    /// Example names of every artifact pattern (a test checks every pattern has one).
    const PATTERN_EXAMPLES: &[(&str, &[&str])] = &[
        ("input.inp", &["input.inp"]),
        ("output.out", &["output.out"]),
        ("input.xyz", &["input.xyz"]),
        (".exit_code", &[".exit_code"]),
        ("*.property.txt", &["input.property.txt", "input_atom53.property.txt"]),
        ("*.hess", &["input.hess"]),
        ("*_trj.xyz", &["input_trj.xyz", "input_MEP_trj.xyz", "input_MEP_ALL_trj.xyz"]),
        ("*.NEB.log", &["input.NEB.log"]),
        ("*.final.interp", &["input.final.interp"]),
        ("*_converged.xyz", &["input_NEB-TS_converged.xyz", "input_NEB-CI_converged.xyz"]),
        ("*.relaxscan*.dat", &["input.relaxscanact.dat", "input.relaxscanscf.dat"]),
        ("input.[0-9]*.xyz", &["input.001.xyz", "input.010.xyz"]),
        ("*.finalensemble.xyz", &["input.finalensemble.xyz"]),
    ];

    /// Selected for the download only, never by the curated export. `.submitting` is a dangling
    /// symlink, as the submit's claim makes it.
    const DOWNLOAD_ONLY: &[&str] =
        &["stderr.log", ".started", ".enqueued", ".cancelled", ".submitting", ".tsp-out/ts-out.AbC"];

    const GBW: &str = "input.gbw";

    /// Never downloaded.
    const NEGATIVES: &[&str] = &[
        "input.tmp",
        "input.densities",
        "input.densitiesinfo",
        "orbital.mo7.g80.cube",
        "input.grid.tmp",
        "input.interp",
        "input.allxyz",
        "input.foo.xyz",
        "input.json",
        ".input.gbw.lMlWCT",
        ".tmp/x",
        ".tmp/pmix.1/lit",
        "sub/deep.xyz",
        "sub/output.out",
    ];

    fn positives(include_gbw: bool) -> BTreeSet<String> {
        let mut set: BTreeSet<String> = PATTERN_EXAMPLES
            .iter()
            .flat_map(|(_, names)| names.iter())
            .chain(DOWNLOAD_ONLY)
            .map(|s| s.to_string())
            .collect();
        if include_gbw {
            set.insert(GBW.into());
        }
        set
    }

    fn all_fixture_paths() -> Vec<&'static str> {
        PATTERN_EXAMPLES
            .iter()
            .flat_map(|(_, names)| names.iter().copied())
            .chain(DOWNLOAD_ONLY.iter().copied())
            .chain([GBW])
            .chain(NEGATIVES.iter().copied())
            .collect()
    }

    /// A job dir holding every fixture path; each file's content is its own name.
    fn materialise(dir: &Path) {
        for path in all_fixture_paths() {
            let full = dir.join(path);
            std::fs::create_dir_all(full.parent().unwrap()).unwrap();
            if path == ".submitting" {
                std::os::unix::fs::symlink("x", &full).unwrap();
            } else {
                std::fs::write(&full, path).unwrap();
            }
        }
    }

    /// Run the real rsync, dir to dir, with `filter`, and list what arrived.
    fn rsync_down(filter: &[String]) -> BTreeSet<String> {
        let root = scratch("rsync");
        let (src, dst) = (root.join("src"), root.join("dst"));
        std::fs::create_dir_all(&src).unwrap();
        std::fs::create_dir_all(&dst).unwrap();
        materialise(&src);
        let out = Command::new(RSYNC_PROGRAM)
            .arg("-a")
            .args(filter)
            .arg(format!("{}/", src.display()))
            .arg(format!("{}/", dst.display()))
            .output()
            .expect("rsync must be installed for this gate (it is the program the download runs)");
        assert!(out.status.success(), "rsync failed: {}", String::from_utf8_lossy(&out.stderr));
        let came = list_dir(&dst, &|_| true).unwrap().into_iter().map(|f| f.name).collect();
        std::fs::remove_dir_all(&root).ok();
        came
    }

    /// Gate a: what came down is exactly the positives — nothing missing, nothing extra.
    fn download_gate(filter: &[String], include_gbw: bool) -> Result<(), String> {
        let came = rsync_down(filter);
        let want = positives(include_gbw);
        let missing: Vec<_> = want.difference(&came).collect();
        let extra: Vec<_> = came.difference(&want).collect();
        if missing.is_empty() && extra.is_empty() {
            Ok(())
        } else {
            Err(format!("missing {missing:?}, extra {extra:?}"))
        }
    }

    #[test]
    fn every_pattern_has_examples_that_it_matches() {
        assert_eq!(
            PATTERN_EXAMPLES.iter().map(|(p, _)| *p).collect::<Vec<_>>(),
            ARTIFACT_PATTERNS,
            "every artifact pattern needs fixture examples, in list order"
        );
        for (pattern, names) in PATTERN_EXAMPLES {
            assert!(!names.is_empty());
            for name in *names {
                assert!(glob_match(pattern, name), "{name} does not match {pattern}");
            }
        }
        for name in NEGATIVES {
            assert!(!download_selects(name, FetchPolicy::WITH_GBW), "{name} is meant as a negative");
        }
    }

    #[test]
    fn real_rsync_downloads_exactly_the_artifact_set() {
        download_gate(&download_filter_args(FetchPolicy::SMALL_ONLY), false).unwrap();
        download_gate(&download_filter_args(FetchPolicy::WITH_GBW), true).unwrap();
    }

    /// NEGATIVE CONTROL (gate a bites): without any one include, or without the final exclude,
    /// the real rsync no longer brings down exactly the set. Covers both `.tsp-out/` rules.
    #[test]
    fn dropping_any_filter_rule_turns_the_download_gate_red() {
        let filter = download_filter_args(FetchPolicy::SMALL_ONLY);
        for i in 0..filter.len() {
            let mut broken = filter.clone();
            let dropped = broken.remove(i);
            assert!(
                download_gate(&broken, false).is_err(),
                "the gate stayed green without {dropped}"
            );
        }
    }

    /// NEGATIVE CONTROL (gate a bites on the opt-in): `*.gbw` without the opt-in, or the opt-in
    /// without `*.gbw`, fails the gate.
    #[test]
    fn gbw_without_opt_in_turns_the_download_gate_red() {
        assert!(download_gate(&download_filter_args(FetchPolicy::WITH_GBW), false).is_err());
        assert!(download_gate(&download_filter_args(FetchPolicy::SMALL_ONLY), true).is_err());
    }

    /// Gate b: for every leaf fixture name the curated export would take, the download takes it,
    /// and the other way round, apart from the download-only extras and the opt-in `.gbw`.
    fn parity(curated: &dyn Fn(&str) -> bool) -> Result<(), Vec<String>> {
        let came = rsync_down(&download_filter_args(FetchPolicy::SMALL_ONLY));
        let disagree: Vec<String> = all_fixture_paths()
            .into_iter()
            .filter(|p| !p.contains('/') && !DOWNLOAD_ONLY.contains(p) && *p != GBW)
            .filter(|p| curated(p) != came.contains(*p))
            .map(String::from)
            .collect();
        if disagree.is_empty() {
            Ok(())
        } else {
            Err(disagree)
        }
    }

    #[test]
    fn curated_export_and_download_take_the_same_artifacts() {
        parity(&curated_match).unwrap();
    }

    /// NEGATIVE CONTROL (gate b bites): a rule added to the curated export only.
    #[test]
    fn a_rule_only_in_curated_match_turns_parity_red() {
        let curated_plus_tmp = |n: &str| curated_match(n) || glob_match("*.tmp", n);
        assert_eq!(parity(&curated_plus_tmp), Err(vec!["input.tmp".into(), "input.grid.tmp".into()]));
    }

    /// `download_selects` (the local side of the post-condition) says what the real rsync did,
    /// for every fixture path, under both policies.
    #[test]
    fn download_selects_agrees_with_the_real_rsync() {
        for policy in [FetchPolicy::SMALL_ONLY, FetchPolicy::WITH_GBW] {
            let came = rsync_down(&download_filter_args(policy));
            for path in all_fixture_paths() {
                assert_eq!(download_selects(path, policy), came.contains(path), "{path} under {policy:?}");
            }
        }
    }

    /// Every file a reader opens in a job dir comes down (main risk: a fetch that "completes" with
    /// nothing to parse). Each entry names the reader. There is no allowed gap.
    #[test]
    fn reader_artifacts_are_downloaded() {
        const READERS: &[(&str, &str)] = &[
            ("input.property.txt", "results::parse_and_store (ADR-012 parse source)"),
            ("input.hess", "results.rs .hess reader"),
            ("input_trj.xyz", "results.rs trajectory reader"),
            ("input_MEP_trj.xyz", "results.rs NEB path reader"),
            ("input.relaxscanact.dat", "results.rs relaxed-scan curve"),
            ("input.001.xyz", "results.rs scan geometries (input.{k:03}.xyz)"),
            ("output.out", "convergence, output viewer, rule #6 tail"),
            ("input.inp", "results.rs input coordinate block"),
            (".exit_code", "detect_completion (rule #6)"),
            ("stderr.log", "detect_completion error message"),
            ("input.finalensemble.xyz", "commands::jobs::read_job_ensemble (GOAT)"),
        ];
        for (name, reader) in READERS {
            assert!(download_selects(name, FetchPolicy::SMALL_ONLY), "{name}, read by {reader}, is not downloaded");
        }
    }

    // --- listings and the post-conditions ------------------------------------------------------

    fn sha(text: &str) -> String {
        Sha256::digest(text.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
    }

    #[test]
    fn upload_expected_lists_every_file_with_its_sha256() {
        let dir = scratch("upload");
        std::fs::write(dir.join("input.inp"), "! SP\n").unwrap();
        std::fs::write(dir.join("product.xyz"), "1\n\nH 0 0 0\n").unwrap();
        let files = upload_expected(&dir).unwrap();
        assert_eq!(
            files,
            [
                FileDigest { name: "input.inp".into(), digest: Digest::Sha256(sha("! SP\n")) },
                FileDigest { name: "product.xyz".into(), digest: Digest::Sha256(sha("1\n\nH 0 0 0\n")) },
            ]
        );
        assert_eq!(
            expected_values(&files).unwrap(),
            ["input.inp", &sha("! SP\n"), "product.xyz", &sha("1\n\nH 0 0 0\n")]
        );
        let link = FileDigest { name: ".submitting".into(), digest: Digest::Symlink("x".into()) };
        assert_eq!(expected_values(&[link]), Err(SyncError::NotARegularFile(".submitting".into())));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn upload_expected_refuses_more_than_the_file_bound() {
        let dir = scratch("upload-many");
        for i in 0..MAX_UPLOAD_FILES {
            std::fs::write(dir.join(format!("f{i}")), "").unwrap();
        }
        assert_eq!(upload_expected(&dir).unwrap().len(), MAX_UPLOAD_FILES);
        std::fs::write(dir.join("one-more"), "").unwrap();
        assert_eq!(upload_expected(&dir), Err(SyncError::TooManyFiles { count: MAX_UPLOAD_FILES + 1 }));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn upload_expected_refuses_odd_names_and_symlinks() {
        let dir = scratch("upload-odd");
        std::fs::write(dir.join("a b.xyz"), "").unwrap();
        assert_eq!(upload_expected(&dir), Err(SyncError::BadName("a b.xyz".into())));
        std::fs::remove_file(dir.join("a b.xyz")).unwrap();
        std::os::unix::fs::symlink("/etc/passwd", dir.join("input.xyz")).unwrap();
        assert_eq!(upload_expected(&dir), Err(SyncError::NotARegularFile("input.xyz".into())));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn upload_expected_walks_subdirectories() {
        let dir = scratch("upload-sub");
        std::fs::create_dir_all(dir.join("aux")).unwrap();
        std::fs::write(dir.join("aux/product.xyz"), "x").unwrap();
        let names: Vec<_> = upload_expected(&dir).unwrap().into_iter().map(|f| f.name).collect();
        assert_eq!(names, ["aux/product.xyz"]);
        std::fs::remove_dir_all(&dir).ok();
    }

    fn file(name: &str, content: &str) -> FileDigest {
        FileDigest { name: name.into(), digest: Digest::Sha256(sha(content)) }
    }

    #[test]
    fn compare_download_accepts_an_exact_copy_and_ignores_tsp_out() {
        let server = [file("output.out", "a"), file(".tsp-out/ts-out.X", "late"), file(".exit_code", "0\n")];
        let local = [file(".exit_code", "0\n"), file("output.out", "a")];
        assert_eq!(compare_download(&local, &server), Ok(()));
    }

    /// NEGATIVE CONTROL: one changed byte, one file missing and one file too many are each named.
    #[test]
    fn compare_download_names_every_difference() {
        let server = [file("output.out", "abc"), file("input.hess", "h"), file(".exit_code", "0\n")];
        let local = [file("output.out", "abd"), file(".exit_code", "0\n"), file("input.xyz", "x")];
        assert_eq!(
            compare_download(&local, &server),
            Err(DownloadMismatch {
                missing: vec!["input.hess".into()],
                extra: vec!["input.xyz".into()],
                differing: vec!["output.out".into()],
            })
        );
        let link = |t: &str| FileDigest { name: ".submitting".into(), digest: Digest::Symlink(t.into()) };
        assert_eq!(
            compare_download(&[link("y")], &[link("x")]).unwrap_err().differing,
            [".submitting"]
        );
    }

    // --- the server listing's reply ---------------------------------------------------------

    fn list_wire(args: &ListArgs, entries: &[(&str, &str)]) -> Vec<u8> {
        let values = args.values();
        let mut out = format!("{LIST_HEADER}\nargc {}\n", values.len());
        for v in values {
            out.push_str(&format!("arg {}\n{v}\n", v.len()));
        }
        out.push_str(&format!("entries {}\n", entries.len()));
        for (name, kind) in entries {
            out.push_str(&format!("entry {}\n{name}\n{kind}\n", name.len()));
        }
        out.push_str("end\n");
        out.into_bytes()
    }

    #[test]
    fn a_listing_reply_parses_into_the_selected_entries() {
        let args = ListArgs::new(JOB, FetchPolicy::SMALL_ONLY).unwrap();
        let hex = sha("a");
        let wire = list_wire(
            &args,
            &[
                ("output.out", &format!("sha256 {hex}")),
                (".submitting", "link 1\nx"),
                ("input.gbw", "unselected"),
                (".tmp", "unselected"),
                ("input.inp", "dir"),
            ],
        );
        assert_eq!(
            parse_list_reply(&wire, &args).unwrap(),
            [
                FileDigest { name: ".submitting".into(), digest: Digest::Symlink("x".into()) },
                FileDigest { name: "output.out".into(), digest: Digest::Sha256(hex) },
            ]
        );
        assert_eq!(args.values()[1..], download_patterns(FetchPolicy::SMALL_ONLY).iter().map(|p| p.to_string()).collect::<Vec<_>>()[..]);
        assert!(download_patterns(FetchPolicy::WITH_GBW).contains(&GBW_PATTERN));
        assert!(!download_patterns(FetchPolicy::SMALL_ONLY).contains(&GBW_PATTERN));
    }

    /// NEGATIVE CONTROLS of the re-derivation: the server's verdict must be Rust's filter's, in
    /// both directions; a duplicate, `.tsp-out`, a path or a selected non-file is refused.
    #[test]
    fn a_listing_reply_that_disagrees_with_the_filter_is_refused() {
        let args = ListArgs::new(JOB, FetchPolicy::SMALL_ONLY).unwrap();
        let hex = format!("sha256 {}", sha("a"));
        let cases: &[&[(&str, &str)]] = &[
            &[("output.out", "unselected")],
            &[("input.gbw", &hex)],
            &[("output.out", &hex), ("output.out", &hex)],
            &[(".tsp-out", "unselected")],
            &[("sub/deep.xyz", "unselected")],
            &[("output.out", "other")],
            &[("output.out", "sha256 ABC")],
        ];
        for entries in cases {
            assert!(parse_list_reply(&list_wire(&args, entries), &args).is_err(), "{entries:?}");
        }
    }

    /// The local side of the post-condition: the filter-selected subset of a real downloaded dir,
    /// compared with a listing of the source under the same filter, is equal; a corrupted byte
    /// after the transfer is caught.
    #[test]
    fn listing_after_a_real_rsync_round_trips() {
        let root = scratch("roundtrip");
        let (src, dst) = (root.join("src"), root.join("dst"));
        std::fs::create_dir_all(&src).unwrap();
        std::fs::create_dir_all(&dst).unwrap();
        materialise(&src);
        let policy = FetchPolicy::SMALL_ONLY;
        let status = Command::new(RSYNC_PROGRAM)
            .arg("-a")
            .args(download_filter_args(policy))
            .arg(format!("{}/", src.display()))
            .arg(format!("{}/", dst.display()))
            .status()
            .unwrap();
        assert!(status.success());
        // A file that was already in the local job dir and is not selected must not matter.
        std::fs::write(dst.join("input.json"), "local only").unwrap();
        let select = |p: &str| download_selects(p, policy);
        let server = list_dir(&src, &select).unwrap();
        assert_eq!(compare_download(&list_dir(&dst, &select).unwrap(), &server), Ok(()));
        std::fs::write(dst.join("input.hess"), "corrupted").unwrap();
        assert_eq!(
            compare_download(&list_dir(&dst, &select).unwrap(), &server).unwrap_err().differing,
            ["input.hess"]
        );
        std::fs::remove_dir_all(&root).ok();
    }
}
