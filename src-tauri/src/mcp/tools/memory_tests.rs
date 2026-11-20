//! Tests for `memory_estimate` (ADR-027 decisions 4 and 7).

use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

use super::memory::memory_estimate;
use super::test_support::{assert_fits, unitless_numeric_keys};
use super::ToolError;
use crate::commands::settings::DbState;
use crate::mcp::classify::MemoryVerdict;
use crate::mcp::preflight::MaxcoreSource;
use crate::mcp::test_db::TestDb;
use crate::memory::EstimateState;

const MEMINFO: &str = "MemTotal:       15988408 kB\nMemFree:         2683044 kB\n";
const HEADER: &str = "! wB97X-3c CPCM(DMF) Opt\n";

struct World {
    _db: TestDb,
    state: Arc<DbState>,
    dir: std::path::PathBuf,
}

fn world() -> World {
    let db = TestDb::new("tools-memory");
    std::fs::write(db.dir.join("meminfo"), MEMINFO).unwrap();
    db.conn
        .execute_batch(
            "INSERT INTO server_profiles (id, name, host, remote_orca_path, remote_scratch_dir, core_mask, mem_total_kib, disk_avail_bytes) VALUES
                ('uni', 'uni', 'uni', '/opt/orca/orca', '/s', '0-23', 65795232, 78360924160),
                ('dark', 'dark', 'lab', '/opt/orca/orca', '/s', '0-7', NULL, NULL);",
        )
        .unwrap();
    let state = Arc::new(DbState(Mutex::new(db.open_another())));
    let dir = db.dir.clone();
    World { _db: db, state, dir }
}

/// The estimate is of the header with the app's `%maxcore`: the check's `%maxcore` is the one the
/// app would set, and the verdict is the preflight's.
#[test]
fn the_estimate_uses_the_apps_maxcore_and_gives_the_verdict() {
    let w = world();
    let local = memory_estimate(&w.state, &w.dir.join("meminfo"), &w.dir, HEADER, "local").unwrap();
    assert_eq!((local.target.as_str(), local.app_maxcore.source), ("local", MaxcoreSource::LocalFitted));
    let EstimateState::Estimated(e) = &local.check.estimate else { panic!("an estimate: {:?}", local.check.estimate) };
    assert_eq!((e.maxcore_mib, e.maxcore_from_input), (local.app_maxcore.mib, true));
    assert_fits(&local, "local");

    let uni = memory_estimate(&w.state, &w.dir.join("meminfo"), &w.dir, HEADER, "uni").unwrap();
    assert_eq!(uni.app_maxcore.source, MaxcoreSource::ServerProposal);
    let dark = memory_estimate(&w.state, &w.dir.join("meminfo"), &w.dir, HEADER, "dark").unwrap();
    assert_eq!((dark.app_maxcore.source, dark.verdict), (MaxcoreSource::ServerDefault, MemoryVerdict::Unknown));

    let v = serde_json::to_value(&uni).unwrap();
    assert_eq!(unitless_numeric_keys(&v, &["ranks", "mib"]), Vec::<String>::new());
}

#[test]
fn a_header_with_its_own_maxcore_or_an_unknown_target_is_refused() {
    let w = world();
    let own = format!("{HEADER}%maxcore 3000\n");
    assert!(matches!(memory_estimate(&w.state, &w.dir.join("meminfo"), &w.dir, &own, "local"), Err(ToolError::Refused(_))));
    assert!(matches!(memory_estimate(&w.state, &w.dir.join("meminfo"), &w.dir, HEADER, "nowhere"), Err(ToolError::Refused(_))));
}

/// The core takes no lock of its own: `mcp_maxcore` and `memory_estimate_with` lock `DbState`
/// themselves, so a lock held across them would deadlock. Run on a thread with a deadline.
/// NEGATIVE CONTROL (`ctl:memory_no_lock`): hold the lock in the core → red (the deadline passes).
#[test]
fn the_estimate_takes_no_lock_of_its_own() {
    let w = world();
    let (state, dir) = (w.state.clone(), w.dir.clone());
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let r = memory_estimate(&state, &dir.join("meminfo"), &dir, HEADER, "uni").map(|v| v.verdict);
        let _ = tx.send(r.is_ok());
    });
    assert_eq!(rx.recv_timeout(Duration::from_secs(20)), Ok(true), "[ctl:memory_no_lock] the estimate returns");
}
