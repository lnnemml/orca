//! Running an uploaded job script on the server (ADR-024 o item 14.1), and making a withdrawn job's
//! dir (o items 2, 14.1). Pure parts only: the values sent and the strict reply parsers. The scripts
//! are [`super::scripts::RUN`] (the trampoline) and [`super::scripts::MKJOB`]; `crate::ssh_backend`
//! runs them.
//!
//! **The trampoline** is the one way an uploaded job script runs: its NUL list is `<root> <name>
//! <sha> <args…>`, read from stdin, so no per-job value passes through the remote login shell. The
//! script's name is a [`JobScript`] — a closed set, mirrored by the trampoline's own allow-list
//! (`BUDGET`, which also holds each script's time budget). The reply is [`RunReply`]: `Refused`
//! (a value, the name, or the file's kind, realpath or sha256 is wrong), `NotInstalled` (no such
//! file), or `Ran` with the script's rc and both streams verbatim — the collector's snapshot is the
//! `stdout` payload, unwrapped before `wire::parse_snapshot`.

use super::classify::is_valid_path;
use super::poll::{check_echo, PollError};
use super::scripts::{sha256_hex, CANCEL, COLLECT};
use super::submit::remote_job_dir;
use super::wire::{parse_decimal, Reader, WireError};

/// The first line of every trampoline reply; the number is the format version.
pub const RUN_HEADER: &str = "orcastudio-run 1";
/// The first line of every mkjob reply; the number is the format version.
pub const MKJOB_HEADER: &str = "orcastudio-mkjob 1";

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RunError {
    #[error("{what} {path:?} breaks the path rule")]
    Path { what: &'static str, path: String },
    #[error(transparent)]
    Reply(#[from] PollError),
    #[error("{0}")]
    Shape(String),
}

impl From<WireError> for RunError {
    fn from(e: WireError) -> Self {
        RunError::Reply(PollError::Wire(e))
    }
}

/// The uploaded job scripts the trampoline may run. The wrapper is not one of them: it is tsp's,
/// and run here it would start ORCA outside the queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobScript {
    Cancel,
    Collect,
}

impl JobScript {
    pub fn name(self) -> &'static str {
        match self {
            JobScript::Cancel => "cancel",
            JobScript::Collect => "collect",
        }
    }

    fn bytes(self) -> &'static str {
        match self {
            JobScript::Cancel => CANCEL,
            JobScript::Collect => COLLECT,
        }
    }
}

/// What one trampoline call sends.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunArgs {
    pub root: String,
    pub name: String,
    pub sha: String,
    pub args: Vec<String>,
}

impl RunArgs {
    /// Run `script` (by the sha of its embedded bytes) under `root` with `args`.
    pub fn new(root: &str, script: JobScript, args: Vec<String>) -> Result<Self, RunError> {
        if !is_valid_path(root) {
            return Err(RunError::Path { what: "root", path: root.to_string() });
        }
        Ok(RunArgs { root: root.to_string(), name: script.name().to_string(), sha: sha256_hex(script.bytes()), args })
    }

    /// The NUL-list values, in the order the trampoline reads them.
    pub fn values(&self) -> Vec<String> {
        let mut values = vec![self.root.clone(), self.name.clone(), self.sha.clone()];
        values.extend(self.args.iter().cloned());
        values
    }
}

/// What the trampoline did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunReply {
    /// A value, the name, or the file's kind, realpath or sha256 is wrong; nothing ran.
    Refused(String),
    /// `<root>/bin/<name>-<sha>.sh` does not exist; nothing ran.
    NotInstalled,
    /// The script ran (under its time budget, stdin at EOF): its exit status and both streams.
    Ran { rc: u8, stdout: Vec<u8>, stderr: Vec<u8> },
}

/// Parse one trampoline reply strictly; the echoed values must be exactly the ones sent.
pub fn parse_run_reply(output: &[u8], sent: &RunArgs) -> Result<RunReply, RunError> {
    let mut r = Reader::new(output);
    let (name, arg) = r.line()?;
    if RUN_HEADER.split_once(' ') != Some((name, arg.unwrap_or_default())) {
        return Err(r.malformed(format!("expected {RUN_HEADER:?}")).into());
    }
    check_echo(&mut r, &sent.values())?;
    let reply = match r.line()? {
        ("refused", arg) => RunReply::Refused(r.text_after(arg)?),
        ("not-installed", None) => RunReply::NotInstalled,
        ("ran", None) => {
            let rc = r.word("rc")?;
            let rc = parse_decimal(rc)
                .and_then(|n| u8::try_from(n).ok())
                .ok_or_else(|| r.malformed(format!("rc {rc:?}")))?;
            let stdout = r.bytes("stdout")?.ok_or_else(|| r.malformed("stdout is required".into()))?;
            let stderr = r.bytes("stderr")?.ok_or_else(|| r.malformed("stderr is required".into()))?;
            RunReply::Ran { rc, stdout, stderr }
        }
        (other, _) => return Err(r.malformed(format!("expected refused, not-installed or ran, got {other:?}")).into()),
    };
    if r.line()? != ("end", None) {
        return Err(r.malformed("expected end".into()).into());
    }
    if !r.at_end() {
        return Err(r.malformed("bytes after end".into()).into());
    }
    Ok(reply)
}

/// What one mkjob call sends: the root and the job dir `<root>/jobs/<id>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MkjobArgs {
    pub root: String,
    pub job_dir: String,
}

impl MkjobArgs {
    pub fn new(root: &str, job_dir: &str) -> Result<Self, RunError> {
        let bad = || RunError::Path { what: "job dir", path: job_dir.to_string() };
        let id = job_dir.strip_prefix(root).and_then(|r| r.strip_prefix("/jobs/")).ok_or_else(bad)?;
        if remote_job_dir(root, id).ok().as_deref() != Some(job_dir) {
            return Err(bad());
        }
        Ok(MkjobArgs { root: root.to_string(), job_dir: job_dir.to_string() })
    }

    pub fn values(&self) -> Vec<String> {
        vec![self.root.clone(), self.job_dir.clone()]
    }
}

/// Parse one mkjob reply and check o item 1's shapes **after** the `mkdir` (rule #9): the job
/// dir's realpath is itself and its parent's is `<root>/jobs`, so `.cancelled` is never published
/// through a symlinked component.
pub fn parse_mkjob_reply(output: &[u8], sent: &MkjobArgs) -> Result<(), RunError> {
    let mut r = Reader::new(output);
    let (name, arg) = r.line()?;
    if MKJOB_HEADER.split_once(' ') != Some((name, arg.unwrap_or_default())) {
        return Err(r.malformed(format!("expected {MKJOB_HEADER:?}")).into());
    }
    check_echo(&mut r, &sent.values())?;
    let job = r.bytes("job")?.ok_or_else(|| r.malformed("job is required".into()))?;
    let parent = r.bytes("parent")?.ok_or_else(|| r.malformed("parent is required".into()))?;
    if r.line()? != ("end", None) {
        return Err(r.malformed("expected end".into()).into());
    }
    if !r.at_end() {
        return Err(r.malformed("bytes after end".into()).into());
    }
    let jobs = format!("{}/jobs", sent.root);
    if job != sent.job_dir.as_bytes() || parent != jobs.as_bytes() {
        return Err(RunError::Shape(format!(
            "after mkdir, realpath {} = {:?} and realpath {}/.. = {:?}; expected the paths themselves (a symlinked component)",
            sent.job_dir,
            String::from_utf8_lossy(&job),
            sent.job_dir,
            String::from_utf8_lossy(&parent)
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROOT: &str = "/home/anton/.orcastudio";
    const JOB: &str = "/home/anton/.orcastudio/jobs/j1";

    fn rec(name: &str, text: &str) -> String {
        format!("{name} {}\n{text}\n", text.len())
    }

    fn wire(header: &str, values: &[String], body: &str) -> Vec<u8> {
        let mut out = format!("{header}\nargc {}\n", values.len());
        for v in values {
            out.push_str(&rec("arg", v));
        }
        out.push_str(body);
        out.push_str("end\n");
        out.into_bytes()
    }

    #[test]
    fn run_values_are_root_name_sha_then_the_arguments() {
        let a = RunArgs::new(ROOT, JobScript::Collect, vec![JOB.into(), "/s.sock".into()]).unwrap();
        assert_eq!(a.values(), [ROOT, "collect", &sha256_hex(COLLECT), JOB, "/s.sock"]);
        let c = RunArgs::new(ROOT, JobScript::Cancel, vec![]).unwrap();
        assert_eq!((c.name.as_str(), c.sha.as_str()), ("cancel", sha256_hex(CANCEL).as_str()));
        assert!(RunArgs::new("rel", JobScript::Cancel, vec![]).is_err());
    }

    #[test]
    fn run_replies_parse_and_broken_ones_are_errors() {
        let a = RunArgs::new(ROOT, JobScript::Cancel, vec!["cancel".into(), JOB.into(), ROOT.into()]).unwrap();
        let v = a.values();
        let body = format!("ran\nrc 3\n{}{}", rec("stdout", "error x\n"), rec("stderr", "cancel: x"));
        assert_eq!(
            parse_run_reply(&wire(RUN_HEADER, &v, &body), &a).unwrap(),
            RunReply::Ran { rc: 3, stdout: b"error x\n".to_vec(), stderr: b"cancel: x".to_vec() }
        );
        assert_eq!(parse_run_reply(&wire(RUN_HEADER, &v, "not-installed\n"), &a).unwrap(), RunReply::NotInstalled);
        assert_eq!(parse_run_reply(&wire(RUN_HEADER, &v, &rec("refused", "script: no")), &a).unwrap(), RunReply::Refused("script: no".into()));
        for body in ["", "ran\nrc 300\nstdout 0\n\nstderr 0\n\n", "ran\nrc 0\n", "installed\n", &rec("error", "mktemp failed")] {
            assert!(parse_run_reply(&wire(RUN_HEADER, &v, body), &a).is_err(), "{body:?}");
        }
        let mut other = a.clone();
        other.args[1] = "/home/anton/.orcastudio/jobs/j2".into();
        assert!(parse_run_reply(&wire(RUN_HEADER, &other.values(), "not-installed\n"), &a).is_err(), "the echo must match");
    }

    #[test]
    fn mkjob_checks_the_shapes_after_the_mkdir() {
        let a = MkjobArgs::new(ROOT, JOB).unwrap();
        let v = a.values();
        let ok = format!("{}{}", rec("job", JOB), rec("parent", &format!("{ROOT}/jobs")));
        parse_mkjob_reply(&wire(MKJOB_HEADER, &v, &ok), &a).unwrap();
        for (case, body) in [
            ("a symlinked job dir", format!("{}{}", rec("job", "/data/j1"), rec("parent", &format!("{ROOT}/jobs")))),
            ("a symlinked jobs", format!("{}{}", rec("job", JOB), rec("parent", "/data/jobs"))),
            ("an error", rec("error", "mkdir -p failed")),
        ] {
            assert!(parse_mkjob_reply(&wire(MKJOB_HEADER, &v, &body), &a).is_err(), "{case}");
        }
        assert!(MkjobArgs::new(ROOT, "/home/anton/.orcastudio/jobs/a/b").is_err());
        assert!(MkjobArgs::new(ROOT, "/elsewhere/jobs/j1").is_err());
    }
}
