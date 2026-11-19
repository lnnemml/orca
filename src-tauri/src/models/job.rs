//! The `Job` model and its status state machine.
//!
//! A job is one ORCA calculation: an `.inp` payload plus everything we learn
//! about running it. The local lifecycle is
//! `draft -> queued -> running -> completed | failed | cancelled` (the queue and
//! cancellation arrived in Phase 2). Remote-execution states (uploading, syncing)
//! and the post-run `parsed` state arrive with later phases — see
//! `wiki/modules/tauri-core.md`.

use rusqlite::Row;
use serde::Serialize;

use crate::error::AppError;

/// The lifecycle state of a job.
///
/// Serialized to lowercase strings (`draft`, `queued`, `running`, `cancelling`,
/// `completed`, `parsed`, `failed`, `cancelled`, `lost`) both on the wire (to the frontend)
/// and in the `jobs.status` column.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum JobStatus {
    Draft,
    /// Submitted, waiting for the single execution slot (sequential queue).
    Queued,
    Running,
    /// Remote only (ADR-024 o22.5; the v21 CHECK): a cancel reached the server and some of the
    /// job's processes still run there (classifier row 3). Transient and **not terminal**: it is
    /// live for every remote-live predicate, and only the server's next verdict ends it.
    Cancelling,
    Completed,
    /// Completed AND its `.property.txt` results were parsed, verified, and stored
    /// (Phase 3). A post-`completed` state: the calculation already succeeded; this
    /// only says our own parse of it did too. A `completed` job with a parse error
    /// is a calculation that ran fine but whose results we could not read.
    Parsed,
    Failed,
    /// Stopped by the user (queued job dropped, or running process killed).
    Cancelled,
    /// Remote only, **terminal** (ADR-024 Decision d, o25.1): the job started on the server and its
    /// wrapper is gone without an `.exit_code` — the server restarted under it, or the wrapper died
    /// in the same boot (classifier row 8). Its files are fetched as evidence first; nothing is
    /// restarted automatically (the user starts a new job). Written only by the poller's finalise
    /// of a remote row; it is live for no predicate.
    Lost,
}

impl JobStatus {
    /// The canonical lowercase spelling stored in SQLite.
    pub fn as_str(&self) -> &'static str {
        match self {
            JobStatus::Draft => "draft",
            JobStatus::Queued => "queued",
            JobStatus::Running => "running",
            JobStatus::Cancelling => "cancelling",
            JobStatus::Completed => "completed",
            JobStatus::Parsed => "parsed",
            JobStatus::Failed => "failed",
            JobStatus::Cancelled => "cancelled",
            JobStatus::Lost => "lost",
        }
    }

    /// Every status, in declaration order (for the tests that pin SQL status sets to this enum).
    pub const ALL: [JobStatus; 9] = [
        JobStatus::Draft,
        JobStatus::Queued,
        JobStatus::Running,
        JobStatus::Cancelling,
        JobStatus::Completed,
        JobStatus::Parsed,
        JobStatus::Failed,
        JobStatus::Cancelled,
        JobStatus::Lost,
    ];

    /// True for a state no job leaves on its own: `completed`, `parsed`, `failed`, `cancelled`,
    /// `lost`. `cancelling` is not terminal (o22.5). ADR-027's `max_in_flight` counts every
    /// non-terminal job with a non-null `grant_id`, a `draft` in step B included.
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            JobStatus::Completed | JobStatus::Parsed | JobStatus::Failed | JobStatus::Cancelled | JobStatus::Lost
        )
    }

    /// True for a terminal state that is a success: `completed` (ORCA terminated normally and the
    /// marker was written, rule #6) or `parsed` (that, and our own parse of it succeeded).
    pub fn is_terminal_success(&self) -> bool {
        matches!(self, JobStatus::Completed | JobStatus::Parsed)
    }

    /// Parse a status string coming from the database or an IPC caller.
    /// Rejects anything outside the known state set.
    pub fn from_db(s: &str) -> Result<JobStatus, AppError> {
        match s {
            "draft" => Ok(JobStatus::Draft),
            "queued" => Ok(JobStatus::Queued),
            "running" => Ok(JobStatus::Running),
            // Explicit, never left to the wildcard: without this arm every read of a cancelling
            // row fails, and the poller and the guards lose the job (o22.5).
            "cancelling" => Ok(JobStatus::Cancelling),
            "completed" => Ok(JobStatus::Completed),
            "parsed" => Ok(JobStatus::Parsed),
            "failed" => Ok(JobStatus::Failed),
            "cancelled" => Ok(JobStatus::Cancelled),
            // Explicit for the same reason as `cancelling` (o25.1): a missing arm breaks every read
            // of a lost row.
            "lost" => Ok(JobStatus::Lost),
            other => Err(AppError::Internal(format!("unknown job status: {other}"))),
        }
    }
}

/// Who created a job (schema v24, ADR-027): Anton through the app, or the MCP server.
/// Serialized lowercase on the wire and in `jobs.origin`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum JobOrigin {
    User,
    Mcp,
}

impl JobOrigin {
    /// The spelling stored in `jobs.origin` (its CHECK admits exactly these two).
    pub fn as_str(&self) -> &'static str {
        match self {
            JobOrigin::User => "user",
            JobOrigin::Mcp => "mcp",
        }
    }

    /// Parse `jobs.origin`; anything else is refused, never defaulted.
    pub fn from_db(s: &str) -> Result<JobOrigin, AppError> {
        match s {
            "user" => Ok(JobOrigin::User),
            "mcp" => Ok(JobOrigin::Mcp),
            other => Err(AppError::Internal(format!("unknown job origin: {other}"))),
        }
    }
}

/// A single ORCA calculation and everything persisted about it.
///
/// Mirrors the `jobs` table one-to-one. `Option` fields are `NULL` until the
/// relevant lifecycle step fills them (e.g. `energy`/`wall_time` after parsing).
#[derive(Debug, Clone, Serialize)]
pub struct Job {
    pub id: String,
    pub title: String,
    pub input_content: String,
    pub status: JobStatus,
    pub job_dir: Option<String>,
    pub energy: Option<f64>,
    pub wall_time: Option<f64>,
    pub error_message: Option<String>,
    pub created_at: String,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
    /// Versioned SceneFragment snapshot (ADR-008 #5), written once at create
    /// time. `None` for jobs created before schema v4, or with no scene. It
    /// *annotates* `input_content`; the input text stays authoritative for
    /// geometry (restore reconciles them — see `restoreScene` in the frontend).
    pub scene_json: Option<String>,
    /// Serialized operation log (ADR-017 unit 2b), co-written with `scene_json`.
    /// `None` for jobs created before schema v11, or with no scene. "New
    /// iteration" restores it, cross-checked against the snapshot (`restoreSceneLog`).
    pub scene_log_json: Option<String>,
    /// Grouping FK into `pathways` (schema v13, Phase 4.5 C1). `None` is the normal
    /// state for a standalone job — every job today. Set by `attach_job_to_pathway`
    /// and nulled by `detach_job_from_pathway` / a reaction-or-pathway deletion; the
    /// job itself is never deleted by any of those (the jobs-survive invariant). The
    /// reaction is derived by joining `pathways` — no `reaction_id` on jobs. Exposed
    /// here so the reaction UI (C2a) can map a pathway to its attached job.
    pub pathway_id: Option<String>,
    /// Grouping FK into `groups` (schema v16, Phase 4.7.2, ADR-019). `None` = ungrouped
    /// (root / "All jobs"). Set by `move_job`; nulled when its group is deleted (the
    /// command re-parents to the deleted group's parent — [`Group`](crate::models::group::Group);
    /// `ON DELETE SET NULL` is only the fallback). Orthogonal to `pathway_id` and the
    /// re-opt/reference links: a job can sit in a group AND be a re-opt child AND be a
    /// reaction reference at once (ADR-019 Decision 5).
    pub group_id: Option<String>,
    /// Execution-backend FK into `server_profiles` (schema v18, Phase 5 unit 5.1, ADR-023).
    /// **`None` = the local backend** — the normal state for every job today. There is NO
    /// `'local'` profile row (the v13 `pathway_id` precedent): a `NULL` here *is* "run
    /// locally". A non-null value names the remote `ServerProfile` a job runs on; the
    /// per-job "Run on:" selector (a later Phase 5 unit) is a `match` on this. Nulled if
    /// its profile is deleted (`delete_server_profile` nulls children explicitly AND
    /// `ON DELETE SET NULL` is enforced) — the job survives, dropping back to local.
    /// Orthogonal to `pathway_id` / `group_id` / the re-opt links.
    pub backend_id: Option<String>,
    /// The remote job's coordinates (schema v20, ADR-024 o item 1 / n 6b): the profile host it was
    /// submitted to, its absolute job dir (`<root>/jobs/<job_id>`) and its `tsp` slot socket.
    /// Written once at submit, before any ssh, and never rewritten from the profile — reconcile,
    /// cancel and fetch use these, never the profile's current values. All three are `None` for a
    /// local job; the database keeps them all-set or all-NULL (a CHECK). See [`Job::is_remote`].
    pub remote_host: Option<String>,
    pub remote_job_dir: Option<String>,
    pub remote_socket: Option<String>,
    /// A remote job's pending cancel (schema v21, ADR-024 o22.2, o22.14.5): when the user asked —
    /// `None`: no cancel pending — and when a delivery marked the send (`None` while Undo is still
    /// possible). `datetime('now')` values: **UTC, laptop clock**, only ever displayed (the view
    /// converts them, `formatLaptopUtc`). Set only on a remote `queued`/`running`/`cancelling` job,
    /// the sent mark only with the request (the v21 CHECKs). Every write of either emits `job:status`.
    pub cancel_requested_at: Option<String>,
    pub cancel_sent_at: Option<String>,
    /// A remote job's pending submit (schema v22, ADR-024 o23.5): when a submit the user started was
    /// first refused for the slot (slot busy / slot check error) — `None`: no submit pending. The
    /// poller re-checks such a row every status period and submits it once the slot frees (o23.3).
    /// `datetime('now')`: **UTC, laptop clock**, only ever displayed. Set only on a remote `queued`
    /// job (the v22 CHECK); every write of it emits `job:status`.
    pub submit_pending_since: Option<String>,
    /// Who created the job (schema v24, ADR-027). Every job before v24 is the user's. Only
    /// `mcp::steps::charge_and_insert_conn` and `insert_proposed_conn` write `mcp`.
    pub origin: JobOrigin,
    /// An MCP job's reasons, a JSON array of `mcp::steps::McpReason` (`{row, message}`): why it was
    /// proposed, or why its charge was refunded. `None` for a user job and a charged one.
    pub mcp_reasons: Option<String>,
    /// The grant an MCP job is charged to (schema v24). `None` for a user job, a proposed one, and
    /// a refunded one. A non-terminal job with a grant counts against every grant's
    /// `max_in_flight` (ADR-027 decision 5).
    pub grant_id: Option<String>,
    /// `Some("submitting")` while the MCP server is between step A and step C: the `submit_job`
    /// command refuses the job (`mcp::steps::submit_refusal_for_mcp_state`). The v24 CHECK admits
    /// nothing else, and only on a charged job.
    pub mcp_state: Option<String>,
}

impl Job {
    /// Column list used by every `SELECT` that hydrates a [`Job`]. The order
    /// here is the contract [`Job::from_row`] relies on.
    pub const COLUMNS: &'static str = "id, title, input_content, status, job_dir, \
         energy, wall_time, error_message, created_at, started_at, completed_at, \
         scene_json, scene_log_json, pathway_id, group_id, backend_id, \
         remote_host, remote_job_dir, remote_socket, cancel_requested_at, cancel_sent_at, \
         submit_pending_since, origin, mcp_reasons, grant_id, mcp_state";

    /// Whether this job runs on a server. **A job is remote iff its coordinates are non-NULL**
    /// (ADR-024 o item 1) — never decided by `backend_id`, which only names the profile and is
    /// nulled when a profile is deleted. Keyed on `remote_host`, the same column every local-only
    /// query filters on (`remote_host IS NULL`), so this predicate and that SQL cannot disagree;
    /// the v20 CHECK keeps the other two coordinates set exactly when `remote_host` is.
    // Its callers: the cancel/delete refusal `ssh_backend::refuse_if_remote_live`.
    pub fn is_remote(&self) -> bool {
        self.remote_host.is_some()
    }

    /// Build a [`Job`] from a row selected in [`Job::COLUMNS`] order.
    pub fn from_row(row: &Row) -> rusqlite::Result<Job> {
        let status_str: String = row.get(3)?;
        let status = JobStatus::from_db(&status_str).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(
                3,
                rusqlite::types::Type::Text,
                Box::new(std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string())),
            )
        })?;

        let origin_str: String = row.get(22)?;
        let origin = JobOrigin::from_db(&origin_str).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(
                22,
                rusqlite::types::Type::Text,
                Box::new(std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string())),
            )
        })?;

        Ok(Job {
            id: row.get(0)?,
            title: row.get(1)?,
            input_content: row.get(2)?,
            status,
            job_dir: row.get(4)?,
            energy: row.get(5)?,
            wall_time: row.get(6)?,
            error_message: row.get(7)?,
            created_at: row.get(8)?,
            started_at: row.get(9)?,
            completed_at: row.get(10)?,
            scene_json: row.get(11)?,
            scene_log_json: row.get(12)?,
            pathway_id: row.get(13)?,
            group_id: row.get(14)?,
            backend_id: row.get(15)?,
            remote_host: row.get(16)?,
            remote_job_dir: row.get(17)?,
            remote_socket: row.get(18)?,
            cancel_requested_at: row.get(19)?,
            cancel_sent_at: row.get(20)?,
            submit_pending_since: row.get(21)?,
            origin,
            mcp_reasons: row.get(23)?,
            grant_id: row.get(24)?,
            mcp_state: row.get(25)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    /// A minimal `jobs` table carrying the two columns that changed in v18-adjacent
    /// work, so `Job::COLUMNS` + `Job::from_row` round-trip `backend_id` in BOTH states:
    /// NULL (local) and a set value (a remote profile). Guards the COLUMNS-order contract
    /// against a drift where `backend_id`'s SELECT position and `row.get(15)` disagree.
    fn setup(conn: &Connection) {
        conn.execute_batch(
            "CREATE TABLE jobs (
                id TEXT PRIMARY KEY, title TEXT NOT NULL, input_content TEXT NOT NULL,
                status TEXT NOT NULL DEFAULT 'draft', job_dir TEXT, energy REAL,
                wall_time REAL, error_message TEXT,
                created_at TEXT NOT NULL DEFAULT (datetime('now')), started_at TEXT,
                completed_at TEXT, scene_json TEXT, scene_log_json TEXT,
                pathway_id TEXT, group_id TEXT, backend_id TEXT,
                remote_host TEXT, remote_job_dir TEXT, remote_socket TEXT,
                cancel_requested_at TEXT, cancel_sent_at TEXT, submit_pending_since TEXT,
                origin TEXT NOT NULL DEFAULT 'user', mcp_reasons TEXT, grant_id TEXT, mcp_state TEXT
            );",
        )
        .unwrap();
    }

    /// Every status survives the database spelling both ways, `cancelling` (o22.5) and `lost`
    /// (o25.1) included. NEGATIVE CONTROL: `from_db` has a wildcard arm, so the compiler does not
    /// force a `"cancelling"` or `"lost"` arm; drop either and this goes red, as would every read of
    /// such a row.
    #[test]
    fn every_status_round_trips_through_its_database_spelling() {
        let all = [
            JobStatus::Draft,
            JobStatus::Queued,
            JobStatus::Running,
            JobStatus::Cancelling,
            JobStatus::Completed,
            JobStatus::Parsed,
            JobStatus::Failed,
            JobStatus::Cancelled,
            JobStatus::Lost,
        ];
        for status in all {
            assert_eq!(JobStatus::from_db(status.as_str()).unwrap(), status, "{status:?}");
            assert_eq!(serde_json::to_value(status).unwrap(), status.as_str(), "the wire spelling is the column's");
        }
        assert_eq!(JobStatus::Cancelling.as_str(), "cancelling");
        assert_eq!(JobStatus::Lost.as_str(), "lost");
        assert!(JobStatus::from_db("lose").is_err(), "an unknown spelling is refused");
    }

    fn get(conn: &Connection, id: &str) -> Job {
        let sql = format!("SELECT {} FROM jobs WHERE id = ?1", Job::COLUMNS);
        conn.query_row(&sql, rusqlite::params![id], Job::from_row).unwrap()
    }

    #[test]
    fn from_row_round_trips_backend_id_null_and_set() {
        let conn = Connection::open_in_memory().unwrap();
        setup(&conn);
        conn.execute_batch(
            "INSERT INTO jobs (id, title, input_content, status) \
                VALUES ('local', 'runs local', '! SP', 'draft');
             INSERT INTO jobs (id, title, input_content, status, backend_id) \
                VALUES ('remote', 'runs remote', '! SP', 'draft', 'profile-1');",
        )
        .unwrap();

        assert_eq!(
            get(&conn, "local").backend_id,
            None,
            "NULL backend_id hydrates to None (= local backend, ADR-023)"
        );
        assert_eq!(
            get(&conn, "remote").backend_id.as_deref(),
            Some("profile-1"),
            "a set backend_id hydrates to the named profile id"
        );
        // Sanity that COLUMNS order is still coherent: an adjacent field survives too.
        assert_eq!(get(&conn, "local").title, "runs local");
    }

    /// ADR-024 o22.14.5: the two pending-cancel columns reach the view through `Job`, each in its own
    /// field — two distinct values, so a swap cannot pass — and a row without a cancel reads both
    /// `None`. NEGATIVE CONTROL: swap `row.get(19)` and `row.get(20)` in `from_row` → red.
    #[test]
    fn from_row_reads_the_pending_cancel_columns_in_order() {
        let conn = Connection::open_in_memory().unwrap();
        setup(&conn);
        conn.execute_batch(
            "INSERT INTO jobs (id, title, input_content, status, remote_host, remote_job_dir, remote_socket, \
                cancel_requested_at, cancel_sent_at) \
                VALUES ('pending', 't', '! SP', 'running', 'uni', '/r/jobs/pending', '/r/tsp/slot0.sock', \
                '2026-10-18 06:18:01', '2026-10-18 06:18:05');
             INSERT INTO jobs (id, title, input_content, status) VALUES ('plain', 't', '! SP', 'queued');",
        )
        .unwrap();
        let pending = get(&conn, "pending");
        assert_eq!(pending.cancel_requested_at.as_deref(), Some("2026-10-18 06:18:01"), "when the user asked");
        assert_eq!(pending.cancel_sent_at.as_deref(), Some("2026-10-18 06:18:05"), "when the send was marked");
        let plain = get(&conn, "plain");
        assert_eq!((plain.cancel_requested_at, plain.cancel_sent_at), (None, None));
        let wire = serde_json::to_value(&pending).unwrap();
        assert_eq!(
            (wire["cancel_requested_at"].clone(), wire["cancel_sent_at"].clone()),
            (serde_json::json!("2026-10-18 06:18:01"), serde_json::json!("2026-10-18 06:18:05")),
            "the TS `Job` reads these names"
        );
    }

    /// ADR-024 o23.5: the pending submit reaches the view through `Job` in its own field, after the
    /// cancel columns; a row without one reads `None`. NEGATIVE CONTROL: read `row.get(20)` for it in
    /// `from_row` → red (it would read the sent mark).
    #[test]
    fn from_row_reads_the_pending_submit_after_the_cancel_columns() {
        let conn = Connection::open_in_memory().unwrap();
        setup(&conn);
        conn.execute_batch(
            "INSERT INTO jobs (id, title, input_content, status, remote_host, remote_job_dir, remote_socket, \
                cancel_requested_at, cancel_sent_at, submit_pending_since) \
                VALUES ('pending', 't', '! SP', 'queued', 'uni', '/r/jobs/pending', '/r/tsp/slot0.sock', \
                '2026-10-19 06:00:01', '2026-10-19 06:00:05', '2026-10-19 05:59:00');
             INSERT INTO jobs (id, title, input_content, status) VALUES ('plain', 't', '! SP', 'queued');",
        )
        .unwrap();
        let pending = get(&conn, "pending");
        assert_eq!(pending.submit_pending_since.as_deref(), Some("2026-10-19 05:59:00"));
        assert_eq!(pending.cancel_sent_at.as_deref(), Some("2026-10-19 06:00:05"), "the neighbour survives");
        assert_eq!(get(&conn, "plain").submit_pending_since, None);
        assert_eq!(serde_json::to_value(&pending).unwrap()["submit_pending_since"], serde_json::json!("2026-10-19 05:59:00"));
    }

    /// `is_remote` follows the coordinates, never `backend_id`: a job whose profile was deleted
    /// (`backend_id` nulled) but which has coordinates is still remote, and a job naming a profile
    /// without coordinates (not yet submitted) is not.
    #[test]
    fn is_remote_follows_the_coordinates_not_backend_id() {
        let conn = Connection::open_in_memory().unwrap();
        setup(&conn);
        conn.execute_batch(
            "INSERT INTO jobs (id, title, input_content, status, remote_host, remote_job_dir, remote_socket) \
                VALUES ('orphaned', 't', '! SP', 'running', 'uni', '/r/jobs/orphaned', '/r/tsp/slot0.sock');
             INSERT INTO jobs (id, title, input_content, status, backend_id) \
                VALUES ('draft', 't', '! SP', 'draft', 'profile-1');
             INSERT INTO jobs (id, title, input_content, status) VALUES ('local', 't', '! SP', 'queued');",
        )
        .unwrap();
        let orphaned = get(&conn, "orphaned");
        assert!(orphaned.is_remote());
        assert_eq!(orphaned.backend_id, None);
        assert_eq!(orphaned.remote_job_dir.as_deref(), Some("/r/jobs/orphaned"));
        assert_eq!(orphaned.remote_socket.as_deref(), Some("/r/tsp/slot0.sock"));
        assert!(!get(&conn, "draft").is_remote(), "a profile without coordinates is not remote yet");
        assert!(!get(&conn, "local").is_remote());
    }
    /// ADR-027 (schema v24): the four MCP columns reach `Job` each in its own field — four distinct
    /// values, so a shifted index cannot pass — and a row without them reads the user's defaults.
    /// An unknown `origin` fails the read instead of defaulting. NEGATIVE CONTROL: swap
    /// `row.get(24)` and `row.get(25)` in `from_row` → red.
    #[test]
    fn from_row_reads_the_mcp_columns_in_order() {
        let conn = Connection::open_in_memory().unwrap();
        setup(&conn);
        conn.execute_batch(
            "INSERT INTO jobs (id, title, input_content, status, origin, mcp_reasons, grant_id, mcp_state) \
                VALUES ('mcp', 't', '! SP', 'draft', 'mcp', '[{\"row\":\"P5\",\"message\":\"m\"}]', 'g1', 'submitting');
             INSERT INTO jobs (id, title, input_content, status) VALUES ('user', 't', '! SP', 'queued');
             INSERT INTO jobs (id, title, input_content, status, origin) VALUES ('odd', 't', '! SP', 'queued', 'claude');",
        )
        .unwrap();
        let mcp = get(&conn, "mcp");
        assert_eq!(mcp.origin, JobOrigin::Mcp);
        assert_eq!(mcp.mcp_reasons.as_deref(), Some("[{\"row\":\"P5\",\"message\":\"m\"}]"));
        assert_eq!(mcp.grant_id.as_deref(), Some("g1"));
        assert_eq!(mcp.mcp_state.as_deref(), Some("submitting"));
        assert_eq!(mcp.submit_pending_since, None, "the neighbour stays put");
        let user = get(&conn, "user");
        assert_eq!((user.origin, user.mcp_reasons, user.grant_id, user.mcp_state), (JobOrigin::User, None, None, None));
        assert_eq!(serde_json::to_value(&mcp).unwrap()["origin"], serde_json::json!("mcp"), "the wire spelling");
        let sql = format!("SELECT {} FROM jobs WHERE id = 'odd'", Job::COLUMNS);
        assert!(conn.query_row(&sql, [], Job::from_row).is_err(), "an unknown origin is refused, never defaulted");
    }

    /// The terminal set is exactly completed, parsed, failed, cancelled and lost; the success subset
    /// is completed and parsed. NEGATIVE CONTROL: add `Cancelling` to `is_terminal` → red.
    #[test]
    fn the_terminal_states_are_the_five_end_states() {
        let terminal: Vec<&str> = JobStatus::ALL.iter().filter(|s| s.is_terminal()).map(|s| s.as_str()).collect();
        assert_eq!(terminal, ["completed", "parsed", "failed", "cancelled", "lost"]);
        let success: Vec<&str> = JobStatus::ALL.iter().filter(|s| s.is_terminal_success()).map(|s| s.as_str()).collect();
        assert_eq!(success, ["completed", "parsed"]);
        assert!(JobStatus::ALL.iter().all(|s| !s.is_terminal_success() || s.is_terminal()));
    }
}
