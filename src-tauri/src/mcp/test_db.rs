//! A migrated scratch database for the M1b tests: a fresh temp dir per test, `init_db` (so every
//! migration arm runs, v24 included), and a second connection on the same file when a test needs
//! two (the concurrency controls).

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use rusqlite::Connection;

use super::classify::Target;
use super::grant::{create_grant_conn, Grant, NewGrant};

/// The study's level (ADR-027's worked example), as Anton would paste it.
pub const LEVEL: &str = "! wB97X-3c CPCM(DMF)\n";
/// A time well before every test's expiry, and the expiry itself.
pub const NOW: &str = "2026-11-19 12:00:00";
pub const LATER: &str = "2026-11-19 13:00:00";
pub const EXPIRES: &str = "2026-11-20 12:00:00";

pub struct TestDb {
    pub dir: PathBuf,
    pub conn: Connection,
}

impl TestDb {
    pub fn new(tag: &str) -> TestDb {
        static N: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "orcastudio-mcp-{tag}-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::remove_dir_all(&dir).ok();
        let conn = crate::db::init_db(&dir).unwrap();
        TestDb { dir, conn }
    }

    /// A second connection on the same database file.
    pub fn open_another(&self) -> Connection {
        Connection::open(self.dir.join("orcastudio.db")).unwrap()
    }

    /// A job row in `status`, with an optional grant (an MCP job when set). A `cancelling` job is
    /// remote (the v21 CHECK), with all three coordinates.
    pub fn job(&self, id: &str, status: &str, grant: Option<&str>) {
        let origin = if grant.is_some() { "mcp" } else { "user" };
        let host = (status == "cancelling").then_some("uni");
        let dir = host.map(|_| format!("/r/jobs/{id}"));
        let sock = host.map(|_| "/r/tsp/slot0.sock");
        self.conn
            .execute(
                "INSERT INTO jobs (id, title, input_content, status, origin, grant_id, remote_host, remote_job_dir, remote_socket) \
                 VALUES (?1, 't', '! SP', ?2, ?3, ?4, ?5, ?6, ?7)",
                rusqlite::params![id, status, origin, grant, host, dir, sock],
            )
            .unwrap();
    }

    pub fn charged(&self, grant: &str) -> i64 {
        self.conn.query_row("SELECT charged_jobs FROM mcp_grants WHERE id = ?1", [grant], |r| r.get(0)).unwrap()
    }
}

impl Drop for TestDb {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.dir).ok();
    }
}

/// A grant on Local at [`LEVEL`], expiring at [`EXPIRES`].
pub fn new_grant(max_jobs: u32, max_in_flight: u32) -> NewGrant {
    NewGrant {
        targets: vec![Target::Local],
        levels: vec![LEVEL.into()],
        max_jobs,
        max_in_flight,
        expires_at: EXPIRES.into(),
    }
}

pub fn grant(conn: &Connection, max_jobs: u32, max_in_flight: u32) -> Grant {
    create_grant_conn(conn, &new_grant(max_jobs, max_in_flight), NOW).unwrap()
}
