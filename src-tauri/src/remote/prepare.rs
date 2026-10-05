//! The two calls that ready the server before an upload or a withdraw (ADR-024 o items 3.2, 13.1,
//! 14.1, 14.3): the read-only **prepare** call, and the **install** call that creates `<root>/bin`
//! and `<root>/tsp` and uploads the three job scripts. Pure parts only: the values sent, the strict
//! reply parsers, and the decision over the prepare call's facts. The scripts are
//! [`super::scripts::PREPARE`] and [`super::scripts::INSTALL`]; `crate::ssh_backend` runs them.
//!
//! **The uploaded scripts** ([`UPLOADED`]): the wrapper (tsp runs it), `cancel` and `collect` (the
//! trampoline runs them, o item 14.1), each as `<root>/bin/<name>-<sha256>.sh`.
//!
//! **Prepare** reports raw facts about `<root>`, `<root>/jobs`, the job dir, `<root>/bin`,
//! `<root>/tsp` and each uploaded script; [`check_prepare`] decides (rule #9):
//! - every component that exists must be a directory whose realpath is itself — so the upload
//!   never writes through a symlinked component (o item 3.2), `<root>/bin` and `<root>/tsp` included
//!   (o items 13.1, 14.3); a component that does not exist is fine (rsync `--mkpath` and the install
//!   call create them);
//! - a script "hashes right" iff it is a regular file whose realpath is itself and whose sha256 is
//!   the one its name carries. Anything else is not a refusal: the install call replaces it by
//!   rename.
//!
//! **Install** answers `installed` or `kept` per script. Its result is not trusted on its word: the
//! caller runs the prepare call again and requires [`Prepared::ready`] (the post-condition).

use super::classify::is_valid_path;
use super::poll::{check_echo, PollError};
use super::scripts::{sha256_hex, CANCEL, COLLECT, WRAPPER};
use super::submit::remote_job_dir;
use super::wire::{Reader, WireError};

/// The first line of every prepare reply; the number is the format version.
pub const PREPARE_HEADER: &str = "orcastudio-prepare 1";
/// The first line of every install reply; the number is the format version.
pub const INSTALL_HEADER: &str = "orcastudio-install 1";

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PrepareError {
    #[error("{what} {path:?} breaks the path rule")]
    Path { what: &'static str, path: String },
    #[error(transparent)]
    Reply(#[from] PollError),
    #[error("prepare reply: {0}")]
    Inconsistent(String),
}

impl From<WireError> for PrepareError {
    fn from(e: WireError) -> Self {
        PrepareError::Reply(PollError::Wire(e))
    }
}

/// The uploaded job scripts, in the order every call lists them: name and embedded bytes.
pub const UPLOADED: [(&str, &str); 3] = [("wrapper", WRAPPER), ("cancel", CANCEL), ("collect", COLLECT)];

/// The sha256 of each uploaded script, in [`UPLOADED`] order.
pub fn uploaded_shas() -> [String; 3] {
    UPLOADED.map(|(_, bytes)| sha256_hex(bytes))
}

/// What one prepare call sends: the root, the job's dir under it, and each uploaded script's sha.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrepareArgs {
    pub root: String,
    pub job_dir: String,
    /// In [`UPLOADED`] order: wrapper, cancel, collect.
    pub shas: [String; 3],
}

impl PrepareArgs {
    /// The job dir must be exactly `<root>/jobs/<id>`, both by the one path rule.
    pub fn new(root: &str, job_dir: &str) -> Result<Self, PrepareError> {
        let id = job_dir
            .strip_prefix(root)
            .and_then(|rest| rest.strip_prefix("/jobs/"))
            .ok_or_else(|| PrepareError::Path { what: "job dir", path: job_dir.to_string() })?;
        let expected = remote_job_dir(root, id)
            .map_err(|_| PrepareError::Path { what: "job dir", path: job_dir.to_string() })?;
        if expected != job_dir {
            return Err(PrepareError::Path { what: "job dir", path: job_dir.to_string() });
        }
        Ok(PrepareArgs { root: root.to_string(), job_dir: job_dir.to_string(), shas: uploaded_shas() })
    }

    /// The NUL-list values, in the order the script reads them.
    pub fn values(&self) -> Vec<String> {
        let mut values = vec![self.root.clone(), self.job_dir.clone()];
        values.extend(self.shas.iter().cloned());
        values
    }

    /// `<root>/bin/<name>-<sha>.sh` of the `i`-th uploaded script (o items 13.1, 14.1).
    pub fn script_path(&self, i: usize) -> String {
        format!("{}/bin/{}-{}.sh", self.root, UPLOADED[i].0, self.shas[i])
    }
}

/// One existing path as the server saw it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathFact {
    /// `stat -c %F` (does not follow a symlink): `directory`, `regular file`, `symbolic link`, …
    pub kind: String,
    /// `realpath -e`, or `None` when it does not resolve (a dangling symlink, a loop).
    pub realpath: Option<String>,
}

/// An uploaded script's facts: the path's, plus the sha256 the server computed for a regular file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptFact {
    pub path: PathFact,
    pub sha256: Option<String>,
}

/// The raw facts of one prepare call. `None` = the path does not exist (ENOENT only).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PrepareFacts {
    pub root: Option<PathFact>,
    pub jobs: Option<PathFact>,
    pub job: Option<PathFact>,
    pub bin: Option<PathFact>,
    pub tsp: Option<PathFact>,
    /// In [`UPLOADED`] order: wrapper, cancel, collect.
    pub scripts: [Option<ScriptFact>; 3],
}

const DIRECTORY: &str = "directory";

fn is_regular(kind: &str) -> bool {
    kind == "regular file" || kind == "regular empty file"
}

/// Parse one prepare reply strictly. Rust re-checks what the script decided (rule #9): a sha256
/// appears exactly for a regular file, and nothing exists below a component reported absent.
pub fn parse_prepare_reply(output: &[u8], sent: &PrepareArgs) -> Result<PrepareFacts, PrepareError> {
    let mut r = Reader::new(output);
    let (name, arg) = r.line()?;
    if PREPARE_HEADER.split_once(' ') != Some((name, arg.unwrap_or_default())) {
        return Err(r.malformed(format!("expected {PREPARE_HEADER:?}")).into());
    }
    check_echo(&mut r, &sent.values())?;
    let mut facts = PrepareFacts {
        root: path_fact(&mut r, "root")?,
        jobs: path_fact(&mut r, "jobs")?,
        job: path_fact(&mut r, "job")?,
        bin: path_fact(&mut r, "bin")?,
        tsp: path_fact(&mut r, "tsp")?,
        scripts: [None, None, None],
    };
    for (i, (name, _)) in UPLOADED.iter().enumerate() {
        let Some(path) = path_fact(&mut r, name)? else { continue };
        let sha256 = match r.word("sha256")? {
            "-" => None,
            hex if hex.len() == 64 && hex.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) => {
                Some(hex.to_string())
            }
            other => return Err(r.malformed(format!("sha256 {other:?}")).into()),
        };
        if sha256.is_some() != is_regular(&path.kind) {
            return Err(PrepareError::Inconsistent(format!(
                "{name} is a {:?} but the server {} a sha256",
                path.kind,
                if sha256.is_some() { "sent" } else { "did not send" }
            )));
        }
        facts.scripts[i] = Some(ScriptFact { path, sha256 });
    }
    if r.line()? != ("end", None) {
        return Err(r.malformed("expected end".into()).into());
    }
    if !r.at_end() {
        return Err(r.malformed("bytes after end".into()).into());
    }
    // A path cannot exist below a component that does not.
    for (parent, child, what) in [
        (facts.root.is_some(), facts.jobs.is_some() || facts.bin.is_some() || facts.tsp.is_some(), "root"),
        (facts.jobs.is_some(), facts.job.is_some(), "jobs"),
        (facts.bin.is_some(), facts.scripts.iter().any(Option::is_some), "bin"),
    ] {
        if child && !parent {
            return Err(PrepareError::Inconsistent(format!("{what} is absent but a path below it exists")));
        }
    }
    Ok(facts)
}

/// `<name> absent`, or `<name> present` + `kind <len>` + `realpath <len>|-`.
fn path_fact(r: &mut Reader<'_>, name: &str) -> Result<Option<PathFact>, PrepareError> {
    match r.word(name)? {
        "absent" => Ok(None),
        "present" => {
            let kind = r.bytes("kind")?.ok_or_else(|| r.malformed("kind is required".into()))?;
            let kind = String::from_utf8(kind)
                .map_err(|_| PrepareError::Inconsistent(format!("{name}: kind is not UTF-8")))?;
            let realpath = match r.bytes("realpath")? {
                None => None,
                Some(bytes) => Some(String::from_utf8(bytes).map_err(|_| {
                    PrepareError::Inconsistent(format!("{name}: realpath is not UTF-8"))
                })?),
            };
            Ok(Some(PathFact { kind, realpath }))
        }
        other => Err(r.malformed(format!("{name} {other:?}")).into()),
    }
}

/// What the prepare call's facts allow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Prepared {
    /// Per uploaded script ([`UPLOADED`] order): a regular file at its own realpath with the sha
    /// its name carries.
    pub scripts_ok: [bool; 3],
    pub bin_exists: bool,
    pub tsp_exists: bool,
}

impl Prepared {
    /// Nothing to install: the upload, the submit call or the trampoline may follow.
    pub fn ready(&self) -> bool {
        self.scripts_ok.iter().all(|ok| *ok) && self.bin_exists && self.tsp_exists
    }

    /// The scripts that do not hash right, by name.
    pub fn missing(&self) -> Vec<&'static str> {
        UPLOADED.iter().zip(self.scripts_ok).filter(|(_, ok)| !ok).map(|((name, _), _)| *name).collect()
    }
}

/// Why an upload must not go ahead: a component exists with the wrong shape (o item 3.2).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{component} {path} {why}")]
pub struct ShapeRefusal {
    pub component: &'static str,
    pub path: String,
    pub why: String,
}

/// Decide over the prepare call's facts (ADR-024 o items 3.2, 13.1). Every existing component of
/// `<root>`, `<root>/jobs`, the job dir, `<root>/bin` and `<root>/tsp` must be a directory whose
/// realpath is exactly its own path; the first that is not refuses the upload, named.
pub fn check_prepare(facts: &PrepareFacts, sent: &PrepareArgs) -> Result<Prepared, ShapeRefusal> {
    let jobs = format!("{}/jobs", sent.root);
    let bin = format!("{}/bin", sent.root);
    let tsp = format!("{}/tsp", sent.root);
    for (component, path, fact) in [
        ("root", sent.root.as_str(), &facts.root),
        ("jobs", jobs.as_str(), &facts.jobs),
        ("job dir", sent.job_dir.as_str(), &facts.job),
        ("bin", bin.as_str(), &facts.bin),
        ("tsp", tsp.as_str(), &facts.tsp),
    ] {
        let Some(fact) = fact else { continue };
        let refuse = |why: String| ShapeRefusal { component, path: path.to_string(), why };
        if fact.kind != DIRECTORY {
            return Err(refuse(format!("is a {}, not a directory", fact.kind)));
        }
        if fact.realpath.as_deref() != Some(path) {
            return Err(refuse(match &fact.realpath {
                Some(real) => format!("is reached through a symlink (realpath {real})"),
                None => "does not resolve (realpath failed)".to_string(),
            }));
        }
    }
    let mut scripts_ok = [false; 3];
    for (i, ok) in scripts_ok.iter_mut().enumerate() {
        let path = sent.script_path(i);
        *ok = facts.scripts[i].as_ref().is_some_and(|f| {
            is_regular(&f.path.kind)
                && f.path.realpath.as_deref() == Some(path.as_str())
                && f.sha256.as_deref() == Some(sent.shas[i].as_str())
        });
    }
    Ok(Prepared { scripts_ok, bin_exists: facts.bin.is_some(), tsp_exists: facts.tsp.is_some() })
}

/// What one install call sends: the root, then each uploaded script's sha and bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallArgs {
    pub root: String,
    /// In [`UPLOADED`] order: (sha256, bytes).
    pub scripts: [(String, String); 3],
}

impl InstallArgs {
    pub fn new(root: &str) -> Result<Self, PrepareError> {
        if !is_valid_path(root) {
            return Err(PrepareError::Path { what: "root", path: root.to_string() });
        }
        Ok(InstallArgs { root: root.to_string(), scripts: UPLOADED.map(|(_, b)| (sha256_hex(b), b.to_string())) })
    }

    /// The NUL-list values, in the order the script reads them: root, then sha + bytes per script.
    pub fn values(&self) -> Vec<String> {
        let mut values = vec![self.root.clone()];
        for (sha, bytes) in &self.scripts {
            values.push(sha.clone());
            values.push(bytes.clone());
        }
        values
    }
}

/// What the install call did with one script. Neither is trusted on its word (the prepare call
/// runs again).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Installed {
    /// The wrapper was uploaded by unique temp name + rename.
    Installed,
    /// A wrapper with the right sha was already there; only `bin/`/`tsp/` were ensured.
    Kept,
}

/// Parse one install reply strictly: `<name> installed|kept` per script, in [`UPLOADED`] order;
/// an `error` record is an error, never an outcome.
pub fn parse_install_reply(output: &[u8], sent: &InstallArgs) -> Result<[Installed; 3], PrepareError> {
    let mut r = Reader::new(output);
    let (name, arg) = r.line()?;
    if INSTALL_HEADER.split_once(' ') != Some((name, arg.unwrap_or_default())) {
        return Err(r.malformed(format!("expected {INSTALL_HEADER:?}")).into());
    }
    check_echo(&mut r, &sent.values())?;
    let mut outcome = [Installed::Kept; 3];
    for (i, (script, _)) in UPLOADED.iter().enumerate() {
        outcome[i] = match r.word(script)? {
            "installed" => Installed::Installed,
            "kept" => Installed::Kept,
            other => return Err(r.malformed(format!("{script}: expected installed or kept, got {other:?}")).into()),
        };
    }
    if r.line()? != ("end", None) {
        return Err(r.malformed("expected end".into()).into());
    }
    if !r.at_end() {
        return Err(r.malformed("bytes after end".into()).into());
    }
    Ok(outcome)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    const ROOT: &str = "/home/anton/.orcastudio";
    const JOB: &str = "/home/anton/.orcastudio/jobs/j1";

    pub(crate) fn args() -> PrepareArgs {
        PrepareArgs::new(ROOT, JOB).unwrap()
    }

    fn rec(name: &str, text: &str) -> String {
        format!("{name} {}\n{text}\n", text.len())
    }

    /// One entry of a prepare reply: absent, or present with kind and realpath (`None` → `-`).
    pub(crate) fn entry(name: &str, fact: Option<(&str, Option<&str>)>) -> String {
        match fact {
            None => format!("{name} absent\n"),
            Some((kind, real)) => format!(
                "{name} present\n{}{}",
                rec("kind", kind),
                real.map_or("realpath -\n".to_string(), |r| rec("realpath", r))
            ),
        }
    }

    /// A whole prepare reply for `sent`: the echo, then `body`, then `end`.
    pub(crate) fn prepare_wire(sent: &PrepareArgs, body: &str) -> Vec<u8> {
        let values = sent.values();
        let mut out = format!("{PREPARE_HEADER}\nargc {}\n", values.len());
        for v in values {
            out.push_str(&rec("arg", &v));
        }
        out.push_str(body);
        out.push_str("end\n");
        out.into_bytes()
    }

    /// The entry of the `i`-th uploaded script with the right bytes.
    pub(crate) fn script_ok(sent: &PrepareArgs, i: usize) -> String {
        format!(
            "{}sha256 {}\n",
            entry(UPLOADED[i].0, Some(("regular file", Some(&sent.script_path(i))))),
            sent.shas[i]
        )
    }

    /// The facts of a server ready for the upload: every component a real directory, every
    /// uploaded script there with the right bytes.
    pub(crate) fn ready_body(sent: &PrepareArgs) -> String {
        let dir = |name: &str, path: &str| entry(name, Some((DIRECTORY, Some(path))));
        format!(
            "{}{}{}{}{}{}{}{}",
            dir("root", &sent.root),
            dir("jobs", &format!("{}/jobs", sent.root)),
            entry("job", None),
            dir("bin", &format!("{}/bin", sent.root)),
            dir("tsp", &format!("{}/tsp", sent.root)),
            script_ok(sent, 0),
            script_ok(sent, 1),
            script_ok(sent, 2),
        )
    }

    #[test]
    fn values_are_root_job_and_the_three_embedded_shas() {
        let a = args();
        let shas = [sha256_hex(WRAPPER), sha256_hex(CANCEL), sha256_hex(COLLECT)];
        assert_eq!(a.values(), [ROOT, JOB, &shas[0], &shas[1], &shas[2]]);
        assert_eq!(a.script_path(0), format!("{ROOT}/bin/wrapper-{}.sh", shas[0]));
        assert_eq!(a.script_path(1), format!("{ROOT}/bin/cancel-{}.sh", shas[1]));
        assert_eq!(a.script_path(2), format!("{ROOT}/bin/collect-{}.sh", shas[2]));
        for bad in ["/home/anton/.orcastudio/jobs", "/home/anton/.orcastudio/jobs/a/b", "/elsewhere/jobs/j1", "/home/anton/.orcastudio/jobs/.."] {
            assert!(PrepareArgs::new(ROOT, bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_ready_server_needs_no_install() {
        let a = args();
        let facts = parse_prepare_reply(&prepare_wire(&a, &ready_body(&a)), &a).unwrap();
        assert_eq!(check_prepare(&facts, &a), Ok(Prepared { scripts_ok: [true; 3], bin_exists: true, tsp_exists: true }));
        assert!(check_prepare(&facts, &a).unwrap().ready());
    }

    #[test]
    fn an_empty_server_is_fine_and_needs_the_install() {
        let a = args();
        let body = ["root", "jobs", "job", "bin", "tsp", "wrapper", "cancel", "collect"].map(|n| entry(n, None)).concat();
        let facts = parse_prepare_reply(&prepare_wire(&a, &body), &a).unwrap();
        assert_eq!(facts, PrepareFacts::default());
        let p = check_prepare(&facts, &a).unwrap();
        assert!(!p.ready() && !p.bin_exists && !p.tsp_exists);
        assert_eq!(p.missing(), ["wrapper", "cancel", "collect"]);
    }

    /// A script is only "ok" with the right bytes, as a regular file at its own realpath. Anything
    /// else is an install, never a refusal — for each of the three, independently.
    #[test]
    fn a_wrong_script_needs_the_install() {
        let a = args();
        for i in 0..3 {
            let mut facts = parse_prepare_reply(&prepare_wire(&a, &ready_body(&a)), &a).unwrap();
            let good = facts.scripts[i].clone().unwrap();
            for (case, f) in [
                ("other bytes", Some(ScriptFact { sha256: Some("0".repeat(64)), ..good.clone() })),
                ("a symlink", Some(ScriptFact { path: PathFact { kind: "symbolic link".into(), realpath: Some("/x".into()) }, sha256: None })),
                ("elsewhere", Some(ScriptFact { path: PathFact { realpath: Some("/x/w.sh".into()), ..good.path.clone() }, ..good.clone() })),
                ("absent", None),
            ] {
                facts.scripts[i] = f;
                let p = check_prepare(&facts, &a).unwrap_or_else(|e| panic!("{case}: {e}"));
                assert!(!p.ready(), "{} {case}", UPLOADED[i].0);
                assert_eq!(p.missing(), [UPLOADED[i].0], "{case}");
            }
        }
    }

    /// NEGATIVE CONTROL target (d): every existing component must be a directory at its own
    /// realpath. `<root>/bin` reached through a symlink refuses even when the scripts inside it have
    /// the right bytes (o item 13.1), and so does `<root>/tsp` (o item 14.3). Remove `bin` from the
    /// list in `check_prepare` and the `bin` rows go red.
    #[test]
    fn a_component_with_the_wrong_shape_refuses_the_upload_by_name() {
        let a = args();
        let jobs = format!("{ROOT}/jobs");
        let bin = format!("{ROOT}/bin");
        let tsp = format!("{ROOT}/tsp");
        let cases: &[(&str, usize, (&str, Option<&str>), &str)] = &[
            ("symlinked root", 0, ("symbolic link", Some("/data/os")), "root"),
            ("root under a symlinked parent", 0, (DIRECTORY, Some("/real/home/anton/.orcastudio")), "root"),
            ("jobs a file", 1, ("regular file", Some(jobs.as_str())), "jobs"),
            ("symlinked jobs", 1, ("symbolic link", Some("/data/jobs")), "jobs"),
            ("symlinked job dir", 2, ("symbolic link", Some("/data/j1")), "job dir"),
            ("dangling job dir", 2, ("symbolic link", None), "job dir"),
            ("symlinked bin", 3, ("symbolic link", Some("/data/bin")), "bin"),
            ("bin under another name", 3, (DIRECTORY, Some("/home/anton/other-bin")), "bin"),
            ("tsp a file", 4, ("regular file", Some(tsp.as_str())), "tsp"),
            ("symlinked tsp", 4, ("symbolic link", Some("/data/tsp")), "tsp"),
        ];
        let names = ["root", "jobs", "job", "bin", "tsp"];
        let paths = [ROOT, jobs.as_str(), JOB, bin.as_str(), tsp.as_str()];
        for (case, at, fact, component) in cases {
            let mut body = String::new();
            for i in 0..5 {
                body.push_str(&if i == *at { entry(names[i], Some(*fact)) } else { entry(names[i], Some((DIRECTORY, Some(paths[i])))) });
            }
            for i in 0..3 {
                body.push_str(&script_ok(&a, i));
            }
            let facts = parse_prepare_reply(&prepare_wire(&a, &body), &a).unwrap_or_else(|e| panic!("{case}: {e}"));
            let refusal = check_prepare(&facts, &a).expect_err(case);
            assert_eq!(refusal.component, *component, "{case}");
        }
    }

    /// The parser re-derives what it can (rule #9): no sha for a non-file, a sha for a file, no
    /// path below an absent one; anything malformed or an `error` record is an error.
    #[test]
    fn a_prepare_reply_that_contradicts_itself_is_refused() {
        let a = args();
        let dir = |n: &str, p: &str| entry(n, Some((DIRECTORY, Some(p))));
        let bin = format!("{ROOT}/bin");
        let lead = format!("{}{}{}", dir("root", ROOT), dir("jobs", &format!("{ROOT}/jobs")), entry("job", None));
        let rest = format!("{}{}", script_ok(&a, 1), script_ok(&a, 2));
        for body in [
            format!("{lead}{}{}{}sha256 -\n{rest}", dir("bin", &bin), entry("tsp", None), entry("wrapper", Some(("regular file", Some(&a.script_path(0)))))),
            format!("{lead}{}{}{}sha256 {}\n{rest}", dir("bin", &bin), entry("tsp", None), entry("wrapper", Some(("symbolic link", None))), "a".repeat(64)),
            format!("{}{}{}{}{}{}{}{}", entry("root", None), dir("jobs", "/x"), entry("job", None), entry("bin", None), entry("tsp", None), entry("wrapper", None), entry("cancel", None), entry("collect", None)),
            format!("{lead}{}{}{}{}{}", entry("bin", None), entry("tsp", None), entry("wrapper", None), entry("cancel", None), script_ok(&a, 2)),
            format!("{lead}{}{}{}sha256 ABC\n{rest}", dir("bin", &bin), entry("tsp", None), entry("wrapper", Some(("regular file", None)))),
            format!("{lead}{}{}{}", dir("bin", &bin), entry("tsp", None), script_ok(&a, 0)),
            "root maybe\n".to_string(),
            rec("error", "stat /x: Permission denied"),
        ] {
            assert!(parse_prepare_reply(&prepare_wire(&a, &body), &a).is_err(), "{body:?}");
        }
    }

    fn install_wire(sent: &InstallArgs, outcome: &str) -> Vec<u8> {
        let values = sent.values();
        let mut out = format!("{INSTALL_HEADER}\nargc {}\n", values.len());
        for v in values {
            out.push_str(&rec("arg", &v));
        }
        out.push_str(outcome);
        out.push_str("end\n");
        out.into_bytes()
    }

    #[test]
    fn install_replies_parse_and_errors_stay_errors() {
        let a = InstallArgs::new(ROOT).unwrap();
        let values = a.values();
        assert_eq!(values.len(), 7);
        assert_eq!((values[2].as_str(), values[4].as_str(), values[6].as_str()), (WRAPPER, CANCEL, COLLECT), "the embedded bytes");
        assert_eq!(values[1], sha256_hex(WRAPPER));
        assert_eq!(
            parse_install_reply(&install_wire(&a, "wrapper installed\ncancel kept\ncollect installed\n"), &a).unwrap(),
            [Installed::Installed, Installed::Kept, Installed::Installed]
        );
        for outcome in [
            "",
            "wrapper installed\n",
            "wrapper installed\ncollect kept\ncancel kept\n",
            "wrapper uploaded\ncancel kept\ncollect kept\n",
            &rec("error", "mv failed"),
        ] {
            assert!(parse_install_reply(&install_wire(&a, outcome), &a).is_err(), "{outcome:?}");
        }
        assert!(InstallArgs::new("relative").is_err());
    }
}
