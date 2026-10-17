//! Whether the server answered: the **transport** result of one ssh or rsync call (ADR-024 o22.8).
//!
//! A job's remote record carries `last_contact` so the view can disable Retry while the server is
//! unreachable. **In short (ADR-024 o22.11.5): ssh exit 255 and rsync exit 255 write `Failed`;
//! rsync exit 12 writes no contact.** It is written from the transport result of every guarded call
//! the poller or a command makes for the job — never from what the server said. "The server answered" is
//! [`Contact::Ok`] even when the answer is a refusal, a broken reply or an `Indeterminate`; only a
//! call that got no answer is [`Contact::Failed`]:
//! - the process could not be run to completion — it could not start, it timed out (the runner
//!   killed it), or its pipes failed ([`TransportError`]); a reply over the size cap is an answer;
//! - it exited **255**, ssh's own failure (connect, auth, host key). rsync over ssh exits 255 too
//!   when ssh cannot connect — measured with rsync 3.2.7 / OpenSSH 9.6p1 for an unresolvable host, a
//!   refused port and a connect timeout, upload and download alike
//!   (`wiki/orca/remote-sync-probe.md`, "rsync when ssh cannot connect");
//! - it was ended by a signal without the runner's timeout: no reply came, so nothing says the
//!   server answered.
//!
//! **Undetermined, so not written:** rsync's exit **12** ("error in rsync protocol data stream").
//! A connection dropped mid-transfer gives it (probe 5.3a, item 9), but it is rsync's generic stream
//! error and its other causes are not measured, so it is not read as "no answer" either. Such a
//! call leaves the last contact as it was rather than guess either way.
//!
//! [`ContactRunner`] wraps the [`CommandRunner`] a step or a command uses and remembers the contact
//! of its **last** call whose contact is determined, so the record shows the most recent answer or
//! silence, whatever the step did with it. It records nothing for a call it never ran (an argv or a
//! value refused before the process started): no call, no contact.

use std::sync::Mutex;
use std::time::Duration;

use serde::Serialize;

use super::ssh::{CommandRunner, ProcessOutput, TransportError};
use super::sync::RSYNC_PROGRAM;

/// The transport result of a job's last ssh or rsync call: `{ "kind": "ok" }` or
/// `{ "kind": "failed", "detail": "<why>" }`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", content = "detail", rename_all = "snake_case")]
pub enum Contact {
    /// The server answered (whatever it answered).
    Ok,
    /// No answer: why, in the transport's words.
    Failed(String),
}

/// The contact one call's result shows; `None` when the result does not say (rsync's exit 12).
/// `program` names the call in the failure text.
pub fn contact_of(program: &str, result: &Result<ProcessOutput, TransportError>) -> Option<Contact> {
    Some(match result {
        // The server sent more than the cap: it answered.
        Err(TransportError::OutputTooLarge { .. }) => Contact::Ok,
        Err(e) => Contact::Failed(e.to_string()),
        Ok(out) => match out.code {
            Some(255) => Contact::Failed(format!("{program} exited with 255: {}", stderr_tail(&out.stderr))),
            Some(12) if program == RSYNC_PROGRAM => return None,
            None => Contact::Failed(format!("{program} was ended by a signal before it answered")),
            Some(_) => Contact::Ok,
        },
    })
}

/// The last 300 characters of stderr, trimmed, for the failure text.
fn stderr_tail(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    let text = text.trim();
    let skip = text.chars().count().saturating_sub(300);
    text.chars().skip(skip).collect()
}

/// A [`CommandRunner`] that passes every call through and remembers the contact of the last one.
pub struct ContactRunner<'a> {
    inner: &'a dyn CommandRunner,
    last: Mutex<Option<Contact>>,
}

impl<'a> ContactRunner<'a> {
    pub fn new(inner: &'a dyn CommandRunner) -> Self {
        ContactRunner { inner, last: Mutex::new(None) }
    }

    /// The contact of the last call run through this runner; `None` if it ran none.
    pub fn last(&self) -> Option<Contact> {
        self.last.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).clone()
    }
}

impl CommandRunner for ContactRunner<'_> {
    fn run(&self, program: &str, args: &[String], stdin: &[u8], timeout: Duration) -> Result<ProcessOutput, TransportError> {
        let result = self.inner.run(program, args, stdin, timeout);
        if let Some(contact) = contact_of(program, &result) {
            *self.last.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(contact);
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn exited(code: Option<i32>) -> Result<ProcessOutput, TransportError> {
        Ok(ProcessOutput { code, stdout: Vec::new(), stderr: b"ssh: connect to host uni port 22: Connection refused\n".to_vec() })
    }

    /// o22.8: only a call that got no answer is `Failed`; a refusal, a usage error, a broken reply
    /// (any exit but 255) and an over-long reply are answers; rsync's 12 does not say.
    /// NEGATIVE CONTROL: map every non-zero exit to `Failed` → red (the rc 3 and rc 1 rows).
    #[test]
    fn only_a_call_without_an_answer_is_a_failed_contact() {
        let failed = |r: &Result<ProcessOutput, TransportError>| matches!(contact_of("ssh", r), Some(Contact::Failed(_)));
        assert!(failed(&Err(TransportError::Timeout { program: "ssh".into(), secs: 60 })));
        assert!(failed(&Err(TransportError::Spawn { program: "ssh".into(), message: "not found".into() })));
        assert!(failed(&Err(TransportError::Io { program: "ssh".into(), message: "broken pipe".into() })));
        assert!(failed(&exited(Some(255))));
        assert!(failed(&exited(None)));
        for code in [0, 1, 2, 3, 12, 23] {
            assert_eq!(contact_of("ssh", &exited(Some(code))), Some(Contact::Ok), "ssh exit {code} is an answer");
        }
        for code in [0, 1, 23] {
            assert_eq!(contact_of("rsync", &exited(Some(code))), Some(Contact::Ok), "rsync exit {code} is an answer");
        }
        assert_eq!(contact_of("rsync", &exited(Some(12))), None, "a dropped transfer or a broken stream: undetermined");
        assert_eq!(contact_of("ssh", &Err(TransportError::OutputTooLarge { program: "ssh".into(), stream: "stdout" })), Some(Contact::Ok));
        let Some(Contact::Failed(why)) = contact_of("rsync", &exited(Some(255))) else { panic!("255 fails") };
        assert!(why.starts_with("rsync exited with 255: ssh: connect to host"), "{why}");
    }

    struct Answers(Vec<Result<ProcessOutput, TransportError>>, Mutex<usize>);

    impl CommandRunner for Answers {
        fn run(&self, _: &str, _: &[String], _: &[u8], _: Duration) -> Result<ProcessOutput, TransportError> {
            let mut i = self.1.lock().unwrap();
            *i += 1;
            self.0[*i - 1].clone()
        }
    }

    /// The runner passes each result through unchanged and keeps the last determined contact.
    #[test]
    fn the_runner_remembers_the_last_calls_contact() {
        let inner = Answers(vec![exited(Some(0)), exited(Some(255)), exited(Some(3)), exited(Some(12))], Mutex::new(0));
        let runner = ContactRunner::new(&inner);
        assert_eq!(runner.last(), None, "no call, no contact");
        assert_eq!(runner.run("ssh", &[], b"", Duration::from_secs(1)), exited(Some(0)));
        assert_eq!(runner.last(), Some(Contact::Ok));
        runner.run("ssh", &[], b"", Duration::from_secs(1)).unwrap();
        assert!(matches!(runner.last(), Some(Contact::Failed(_))));
        runner.run("ssh", &[], b"", Duration::from_secs(1)).unwrap();
        assert_eq!(runner.last(), Some(Contact::Ok), "the last call decides");
        runner.run("rsync", &[], b"", Duration::from_secs(1)).unwrap();
        assert_eq!(runner.last(), Some(Contact::Ok), "rsync's 12 leaves it as it was");
    }

    #[test]
    fn the_contact_wire_shape() {
        assert_eq!(serde_json::to_value(Contact::Ok).unwrap(), serde_json::json!({ "kind": "ok" }));
        assert_eq!(serde_json::to_value(Contact::Failed("x".into())).unwrap(), serde_json::json!({ "kind": "failed", "detail": "x" }));
    }
}
