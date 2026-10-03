//! The raw-fact snapshot of one remote job (ADR-024 l, "Snapshot fields").
//!
//! Everything here is a **raw fact** read on the server — file bytes, `/proc` lines, `tsp -l`
//! rows — never a verdict computed there (rule #9). The collector (unit 5.2 Part B) reads them
//! in a fixed order: `boot_id` → `.started` → `/proc` and `ps -s` → `tsp -l` → `.exit_code`,
//! `.cancelled`, tail → `.started` again.

/// Who the job is: the inputs the "ours" and job-session rules compare against.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobIdentity {
    /// The job's own directory on the server, absolute, e.g. `/home/anton/.orcastudio/jobs/j1`.
    /// It is the wrapper's argv[2], every job-session member's cwd, and a whole token of the
    /// job's `tsp -l` row.
    pub job_dir: String,
    /// The app's root on the server, e.g. `/home/anton/.orcastudio`. The wrapper runs from
    /// `<root>/bin/wrapper-<sha>.sh`.
    pub root: String,
}

/// Whether this snapshot is the first one taken for this classification, or the single retake
/// allowed when the two `.started` reads disagreed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Attempt {
    First,
    Retake,
}

/// One member of `ps -s <sid>`: its PID and the raw target of `/proc/<pid>/cwd`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionMember {
    pub pid: u32,
    /// `None` when the cwd could not be read (ENOENT: the process exited, or it is a zombie —
    /// probe 5.2c). Such a member is not in the job session; this is not an error.
    pub cwd: Option<Vec<u8>>,
}

/// What a `tsp` socket says about this job. Three-way on purpose: a failed query is never read
/// as "no rows" (ADR-024 l, round 2 MED-4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SocketState {
    /// Nothing listens on the socket path (absent from `/proc/net/unix`). The queue lived in the
    /// dead daemon's memory, so this counts as no rows. `tsp` was not run to find out.
    NoDaemon,
    /// A daemon listens; these are the raw `tsp -l` lines that contain the job dir. The
    /// classifier keeps only those where it is a whole token.
    Rows(Vec<String>),
    /// Any other failure, with what went wrong.
    Error(String),
}

/// One socket fact: each of the profile's slot sockets, plus the socket in `.enqueued`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SocketFact {
    pub socket_path: String,
    pub state: SocketState,
}

/// The raw facts about one job, as collected on the server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    pub identity: JobIdentity,
    pub attempt: Attempt,
    /// Raw `/proc/sys/kernel/random/boot_id` now.
    pub current_boot_id: String,
    /// Raw `.started`, first read; `None` if absent.
    pub started_first: Option<Vec<u8>>,
    /// Raw `/proc/<.started pid>/stat`; `None` if the process does not exist.
    pub wrapper_stat: Option<Vec<u8>>,
    /// Raw `/proc/<.started pid>/cmdline` (empty for a zombie or an absent process).
    pub wrapper_cmdline: Vec<u8>,
    /// Raw `/proc/<.started sid>/stat`; `None` if no process has that number. The SID-reuse
    /// guard compares its start time with `.started`'s. The wrapper is PID = SID, so for a live
    /// wrapper this is the same line as `wrapper_stat`; it is read separately so the guard never
    /// depends on that.
    pub sid_stat: Option<Vec<u8>>,
    /// The members of `ps -s <.started sid>`. Collected only with a current `boot_id`;
    /// the classifier ignores them otherwise.
    pub session_members: Vec<SessionMember>,
    /// One fact per socket; never empty for a real collection.
    pub sockets: Vec<SocketFact>,
    /// Raw `.exit_code`; `None` if absent.
    pub exit_code: Option<Vec<u8>>,
    /// Whether `.cancelled` exists.
    pub cancelled: bool,
    /// The last bytes of `output.out` (at most `TAIL_BYTES`, rule #5); empty if absent.
    pub output_tail: Vec<u8>,
    /// Raw `.started`, re-read last; brackets the collection.
    pub started_last: Option<Vec<u8>>,
}
