//! Tests for the app's `%maxcore` and the memory verdict (ADR-027 decision 4; C8; M1b control).

use std::sync::Mutex;

use super::assemble::SERVER_DEFAULT_MAXCORE_MIB;
use super::classify::{MemoryVerdict, Target};
use super::preflight::{mcp_maxcore, memory_verdict_for, verdict_of, MaxcoreSource, McpMaxcore};
use super::test_db::TestDb;
use crate::commands::memory::local_fitted_maxcore_with;
use crate::commands::settings::DbState;
use crate::memory::{apply_maxcore, check, estimate, Fact, RankRule};

const MEMINFO: &str = "MemTotal:       15988408 kB\nMemFree:         2683044 kB\n";
const HEADER: &str = "! wB97X-3c CPCM(DMF) Opt\n";

struct World {
    _db: TestDb,
    state: DbState,
    dir: std::path::PathBuf,
}

fn world() -> World {
    let db = TestDb::new("preflight");
    std::fs::write(db.dir.join("meminfo"), MEMINFO).unwrap();
    db.conn
        .execute_batch(
            "INSERT INTO server_profiles (id, name, host, remote_orca_path, remote_scratch_dir, core_mask, mem_total_kib, disk_avail_bytes) VALUES
                ('uni', 'uni', 'uni', '/opt/orca/orca', '/s', '0-23', 65795232, 78360924160),
                ('dark', 'dark', 'lab', '/opt/orca/orca', '/s', '0-7', NULL, NULL),
                ('nomask', 'nomask', 'lab2', '/opt/orca/orca', '/s', NULL, 65795232, 78360924160);",
        )
        .unwrap();
    let state = DbState(Mutex::new(db.open_another()));
    let dir = db.dir.clone();
    World { _db: db, state, dir }
}

fn server(id: &str) -> Target {
    Target::Server { profile_id: id.into() }
}

/// The verdict of each fact state, on hand-built checks (pure): a warning is `Warns`; any missing
/// fact or estimate is `Unknown`; only all-known-and-quiet is `Fits`. NEGATIVE CONTROL: map an
/// unknown fact to `Fits` → red.
#[test]
fn the_verdict_is_fits_only_when_every_fact_is_known_and_quiet() {
    let est = |m: u64| estimate(&format!("{HEADER}%maxcore {m}\n"), RankRule::Local { preset_nprocs: 8 });
    let known = |v: u64| Fact::Known { value: v };
    let unknown = || Fact::Unknown { reason: "unknown".into() };
    let big_disk = 100 * 1024 * 1024 * 1024u64;
    assert_eq!(verdict_of(&check(est(1000), known(16_000_000), known(big_disk), None)), MemoryVerdict::Fits);
    assert_eq!(verdict_of(&check(est(4000), known(16_000_000), known(big_disk), None)), MemoryVerdict::Warns, "RAM");
    assert_eq!(verdict_of(&check(est(1000), known(16_000_000), known(1024), None)), MemoryVerdict::Warns, "disk");
    assert_eq!(verdict_of(&check(est(1000), unknown(), known(big_disk), None)), MemoryVerdict::Unknown, "RAM unknown");
    assert_eq!(verdict_of(&check(est(1000), known(16_000_000), unknown(), None)), MemoryVerdict::Unknown, "disk unknown");
    assert_eq!(
        verdict_of(&check(Err("no estimate".into()), known(16_000_000), known(big_disk), None)),
        MemoryVerdict::Unknown,
        "no estimate"
    );
}

/// A server with stored facts gets the 5.5 proposal, and the assembled input at it fits.
#[test]
fn a_known_server_gets_its_proposal_and_fits() {
    let w = world();
    let m = mcp_maxcore(&w.state, &w.dir.join("meminfo"), &w.dir, HEADER, &server("uni")).unwrap();
    assert_eq!(m.source, MaxcoreSource::ServerProposal);
    assert!(m.mib >= 300 && m.mib != SERVER_DEFAULT_MAXCORE_MIB, "{m:?}");
    let input = apply_maxcore(HEADER, m.mib).unwrap();
    assert_eq!(memory_verdict_for(&w.state, &w.dir.join("meminfo"), &w.dir, &input, &server("uni")).unwrap(), MemoryVerdict::Fits);
    let too_much = apply_maxcore(HEADER, m.mib * 4).unwrap();
    assert_eq!(
        memory_verdict_for(&w.state, &w.dir.join("meminfo"), &w.dir, &too_much, &server("uni")).unwrap(),
        MemoryVerdict::Warns
    );
}

/// A server whose memory is unknown gets the builder's default and an `Unknown` verdict (P4); one
/// without a core mask has no estimate and is `Unknown` too.
#[test]
fn an_unknown_server_gets_the_default_and_unknown() {
    let w = world();
    for id in ["dark", "nomask"] {
        let m = mcp_maxcore(&w.state, &w.dir.join("meminfo"), &w.dir, HEADER, &server(id)).unwrap();
        assert_eq!(m, McpMaxcore { mib: SERVER_DEFAULT_MAXCORE_MIB, source: MaxcoreSource::ServerDefault }, "{id}");
        let input = apply_maxcore(HEADER, m.mib).unwrap();
        assert_eq!(memory_verdict_for(&w.state, &w.dir.join("meminfo"), &w.dir, &input, &server(id)).unwrap(), MemoryVerdict::Unknown, "{id}");
    }
    assert!(mcp_maxcore(&w.state, &w.dir.join("meminfo"), &w.dir, HEADER, &server("gone")).is_err(), "no such profile");
}

/// Local takes `local_fitted_maxcore_with`'s default; with this machine's RAM unreadable the verdict
/// is never `Fits` (`Unknown`, or `Warns` if the real disk under the temp dir is short).
#[test]
fn local_takes_the_fitted_default_and_an_unreadable_ram_never_fits() {
    let w = world();
    let meminfo = w.dir.join("meminfo");
    let m = mcp_maxcore(&w.state, &meminfo, &w.dir, HEADER, &Target::Local).unwrap();
    assert_eq!(m.source, MaxcoreSource::LocalFitted);
    assert_eq!(m.mib, local_fitted_maxcore_with(&w.state, &meminfo, &w.dir).unwrap().default_maxcore_mib);
    let input = apply_maxcore(HEADER, m.mib).unwrap();
    let missing = w.dir.join("no-meminfo");
    assert_ne!(memory_verdict_for(&w.state, &missing, &w.dir, &input, &Target::Local).unwrap(), MemoryVerdict::Fits);
}
