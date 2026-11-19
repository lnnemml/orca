//! The memory and disk preflight's commands (ADR-024 o26, unit 5.5): thin shells over the pure
//! `crate::memory`. Nothing here writes anything.
//!
//! - [`memory_estimate`] — the New Job screen's check of the editor buffer (o26.7, o26.13.1): always
//!   **Local** (what "Create Job" makes can only run here, o20.2) and, when a server is selected, that
//!   server too (what "Create & Run" runs on). Each request carries a sequence number that comes back
//!   unchanged, so the view keeps only the latest reply (o26.13.6).
//! - [`apply_maxcore`] — the button's edit of the buffer, by `read_maxcore`'s own locator (o26.13.5).
//! - [`host_resources`] — this machine's RAM and free space under the data dir, read now (o26.6).
//! - [`local_fitted_maxcore`] — the local P for the current CPU preset, and the builder's local
//!   default derived from it (o26.5, o26.13.4).
//!
//! A server's facts are the ones its last full-pass connection test stored (schema v23); NULL is
//! "unknown — re-run the connection test", never 0.

use std::path::Path;

use rusqlite::Connection;
use serde::Serialize;
use tauri::State;

use crate::commands::server_profiles::get_profile_conn;
use crate::commands::settings::DbState;
use crate::error::AppError;
use crate::local_backend::{resolve_cpu_config, JobRunner};
use crate::memory::{
    check, estimate, fitted_default_mib, host_resources_at, proposal_mib, Fact, HostResources, MemoryCheck, RankRule,
};

/// This machine's memory file. A constant here, a parameter of every core below (the tests' seam).
pub(crate) const PROC_MEMINFO: &str = "/proc/meminfo";

/// What a server's NULL fact says (o26.7).
const SERVER_UNKNOWN: &str = "server memory unknown — re-run the connection test";
const SERVER_DISK_UNKNOWN: &str = "server free disk unknown — re-run the connection test";

/// One server's verdict, named by its profile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ServerMemoryCheck {
    pub profile_id: String,
    #[serde(flatten)]
    pub check: MemoryCheck,
}

/// The reply to one [`memory_estimate`] request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MemoryEstimateReply {
    /// The request's sequence number, unchanged (o26.13.6).
    pub seq: u64,
    /// "Create Job": this machine, the CPU preset's ranks.
    pub local: MemoryCheck,
    /// "Create & Run" on the selected server; `None` when the selected target is this machine.
    pub server: Option<ServerMemoryCheck>,
}

/// The local CPU preset's rank count (`resolve_cpu_config`, the count `align_pal_nprocs` will write).
fn preset_ranks(conn: &Connection) -> u32 {
    resolve_cpu_config(conn).1
}

/// The local check of `input`: the preset's ranks, this machine's facts read now.
fn local_check(input: &str, ranks: u32, meminfo: &Path, data_dir: &Path) -> MemoryCheck {
    let HostResources { mem_total_kib, disk_avail_bytes } = host_resources_at(meminfo, data_dir);
    check(estimate(input, RankRule::Local { preset_nprocs: ranks }), mem_total_kib, disk_avail_bytes, None)
}

/// A stored fact as a [`Fact`]: NULL is unknown, with the reason the view shows.
fn stored<T: Copy>(value: Option<T>, unknown: &str) -> Fact<T> {
    match value {
        Some(value) => Fact::Known { value },
        None => Fact::Unknown { reason: unknown.to_string() },
    }
}

/// The core of [`memory_estimate`]: the database read under the caller's lock, the host read after.
pub(crate) fn memory_estimate_with(
    db: &DbState,
    meminfo: &Path,
    data_dir: &Path,
    seq: u64,
    input: &str,
    target: Option<&str>,
) -> Result<MemoryEstimateReply, AppError> {
    let (ranks, profile) = {
        let conn = db.lock()?;
        let profile = target.map(|id| get_profile_conn(&conn, id)).transpose()?;
        (preset_ranks(&conn), profile)
    };
    let local = local_check(input, ranks, meminfo, data_dir);
    let server = profile.map(|p| {
        let est = match p.core_mask.as_deref() {
            Some(mask) => estimate(input, RankRule::Remote { core_mask: mask }),
            None => Err(format!("server profile {} has no core mask", p.name)),
        };
        ServerMemoryCheck {
            profile_id: p.id.clone(),
            check: check(
                est,
                stored(p.mem_total_kib, SERVER_UNKNOWN),
                stored(p.disk_avail_bytes, SERVER_DISK_UNKNOWN),
                p.host_facts_at.clone(),
            ),
        }
    });
    Ok(MemoryEstimateReply { seq, local, server })
}

/// The local P and the builder's local default (o26.5, o26.13.4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LocalFittedMaxcore {
    /// The CPU preset's ranks.
    pub ranks: u32,
    pub mem_total_kib: Fact<u64>,
    /// P, when the RAM is known and P ≥ 300.
    pub proposal_mib: Option<u64>,
    /// The builder's local `%maxcore`: P; 300 when P is below it; 2000 when the RAM is unknown.
    pub default_maxcore_mib: u64,
}

pub(crate) fn local_fitted_maxcore_with(db: &DbState, meminfo: &Path, data_dir: &Path) -> Result<LocalFittedMaxcore, AppError> {
    let ranks = preset_ranks(&*db.lock()?);
    let mem_total_kib = host_resources_at(meminfo, data_dir).mem_total_kib;
    let kib = mem_total_kib.value();
    Ok(LocalFittedMaxcore {
        ranks,
        proposal_mib: kib.and_then(|kib| proposal_mib(kib, ranks)),
        default_maxcore_mib: fitted_default_mib(kib, ranks),
        mem_total_kib,
    })
}

// --- Tauri commands ---------------------------------------------------------

/// The New Job screen's check of `input` (o26.7): Local always, and the server `target` when one is
/// selected (`null` = this machine). `seq` comes back unchanged.
#[tauri::command]
pub fn memory_estimate(
    db: State<'_, DbState>,
    runner: State<'_, JobRunner>,
    seq: u64,
    input: String,
    target: Option<String>,
) -> Result<MemoryEstimateReply, AppError> {
    memory_estimate_with(&db, Path::new(PROC_MEMINFO), runner.data_dir(), seq, &input, target.as_deref())
}

/// The button "Set `%maxcore` to P" (o26.4): the buffer with `%maxcore` set, or why not.
#[tauri::command]
pub fn apply_maxcore(input: String, maxcore_mib: u64) -> Result<String, AppError> {
    crate::memory::apply_maxcore(&input, maxcore_mib).map_err(AppError::Invalid)
}

/// This machine's RAM and the free space under the app's data dir, read now.
#[tauri::command]
pub fn host_resources(runner: State<'_, JobRunner>) -> Result<HostResources, AppError> {
    Ok(host_resources_at(Path::new(PROC_MEMINFO), runner.data_dir()))
}

/// The local P for the current CPU preset and the builder's local default.
#[tauri::command]
pub fn local_fitted_maxcore(db: State<'_, DbState>, runner: State<'_, JobRunner>) -> Result<LocalFittedMaxcore, AppError> {
    local_fitted_maxcore_with(&db, Path::new(PROC_MEMINFO), runner.data_dir())
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use crate::db::init_db;
    use crate::memory::{DiskCheck, EstimateState, RamCheck};

    const LAPTOP_MEMINFO: &str = "MemTotal:       15988408 kB\nMemFree:         2683044 kB\nMemAvailable:   10368556 kB\n";

    struct World {
        db: DbState,
        dir: std::path::PathBuf,
    }

    impl World {
        fn new() -> World {
            use std::sync::atomic::{AtomicU64, Ordering};
            static N: AtomicU64 = AtomicU64::new(0);
            let dir = std::env::temp_dir().join(format!("orcastudio-memcmd-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
            std::fs::remove_dir_all(&dir).ok();
            let conn = init_db(&dir).unwrap();
            std::fs::write(dir.join("meminfo"), LAPTOP_MEMINFO).unwrap();
            conn.execute(
                "INSERT INTO server_profiles (id, name, host, remote_orca_path, remote_scratch_dir, core_mask) \
                 VALUES ('uni', 'uni', 'uni', '/opt/orca/orca', '/home/anton/.orcastudio', '0-23'), \
                        ('nomask', 'nomask', 'lab', '/opt/orca/orca', '/home/x/.orcastudio', NULL)",
                [],
            )
            .unwrap();
            World { db: DbState(Mutex::new(conn)), dir }
        }

        fn ask(&self, seq: u64, input: &str, target: Option<&str>) -> MemoryEstimateReply {
            memory_estimate_with(&self.db, &self.dir.join("meminfo"), &self.dir, seq, input, target).unwrap()
        }

        fn sql(&self, sql: &str) {
            self.db.lock().unwrap().execute_batch(sql).unwrap();
        }
    }

    impl Drop for World {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.dir).ok();
        }
    }

    fn est(c: &MemoryCheck) -> crate::memory::Estimate {
        match &c.estimate {
            EstimateState::Estimated(e) => *e,
            other => panic!("no estimate: {other:?}"),
        }
    }

    /// The local verdict uses the CPU preset's ranks (Interactive: 8; Custom: its `cpu_nprocs`) and
    /// this machine's facts; `seq` comes back unchanged (o26.13.6).
    #[test]
    fn the_local_check_uses_the_preset_and_echoes_the_sequence_number() {
        let w = World::new();
        let r = w.ask(41, "! HF\n%pal nprocs 2 end\n", None);
        assert_eq!(r.seq, 41);
        assert!(r.server.is_none());
        assert_eq!(est(&r.local).ranks, 8, "the default preset (Interactive), not the input's 2");
        assert!(matches!(r.local.ram, RamCheck::Known { mem_total_kib: 15_988_408, .. }));
        assert!(matches!(r.local.disk, DiskCheck::Known { .. }), "statvfs of the data dir");
        assert_eq!(r.local.facts_measured_at, None, "read now");
        assert_eq!(r.local.proposal_mib, Some(1600));
        w.sql("INSERT INTO settings (key, value) VALUES ('cpu_preset', 'custom'), ('cpu_nprocs', '4') \
               ON CONFLICT(key) DO UPDATE SET value = excluded.value");
        assert_eq!(est(&w.ask(42, "! HF\n", None).local).ranks, 4);
        assert_eq!(w.ask(7, "! HF\n", None).seq, 7, "an older number is echoed as it is; the view drops it");
    }

    /// o26.13.1: with a server selected and an input without `%maxcore`, the reply carries both
    /// verdicts — the Local one (Create) warns on the laptop even where the server's verdict is
    /// unknown. NEGATIVE CONTROL: evaluate `local` against the selected target → red.
    #[test]
    fn create_is_checked_against_local_and_create_and_run_against_the_selected_server() {
        let w = World::new();
        let r = w.ask(1, "! r2SCAN-3c Opt\n", Some("uni"));
        assert_eq!(est(&r.local).ranks, 8);
        assert!(r.local.warns, "8 × 4196 + 140 MiB > 0.9 × 15.2 GiB: {:?}", r.local.ram);
        let server = r.server.unwrap();
        assert_eq!(server.profile_id, "uni");
        assert_eq!(est(&server.check).ranks, 24);
        assert_eq!(server.check.ram, RamCheck::Unknown { reason: SERVER_UNKNOWN.into() }, "never tested: unknown, not 0");
        assert_eq!(server.check.disk, DiskCheck::Unknown { reason: SERVER_DISK_UNKNOWN.into() });
        assert_eq!(server.check.proposal_mib, None);

        w.sql("UPDATE server_profiles SET mem_total_kib = 65795232, disk_avail_bytes = 78360924160, \
               host_facts_at = '2026-11-18 10:00:00' WHERE id = 'uni'");
        let server = w.ask(2, "! r2SCAN-3c Opt\n", Some("uni")).server.unwrap();
        assert!(matches!(server.check.ram, RamCheck::Known { warns: Some(true), .. }));
        assert_eq!(server.check.proposal_mib, Some(2300));
        assert_eq!(server.check.facts_measured_at.as_deref(), Some("2026-11-18 10:00:00"));
        let fits = w.ask(3, "! r2SCAN-3c Opt\n%maxcore 2000\n", Some("uni")).server.unwrap();
        assert!(!fits.check.warns);

        let nomask = w.ask(4, "! HF\n", Some("nomask")).server.unwrap();
        assert!(matches!(nomask.check.estimate, EstimateState::NotEstimated { ref reason } if reason.contains("no core mask")));
        assert!(matches!(memory_estimate_with(&w.db, &w.dir.join("meminfo"), &w.dir, 5, "", Some("no-such")), Err(AppError::NotFound(_))));
    }

    /// The JSON Part B reads: `seq`, `local`, `server` flattened with its `profile_id`.
    #[test]
    fn the_reply_serializes_flat_for_the_view() {
        let w = World::new();
        let json = serde_json::to_value(w.ask(9, "! HF\n%maxcore 1000\n", Some("uni"))).unwrap();
        assert_eq!(json["seq"], 9);
        assert_eq!(json["local"]["estimate"]["state"], "estimated");
        assert_eq!(json["server"]["profile_id"], "uni");
        assert_eq!(json["server"]["estimate"]["ranks"], 24);
        assert_eq!(json["server"]["ram"]["state"], "unknown");
        assert_eq!(json["server"]["facts_measured_at"], serde_json::Value::Null);
    }

    /// The builder's local default (o26.5, o26.13.4) follows the preset: 1600 at 8 ranks, 1000 at 12;
    /// 2000 when the RAM cannot be read. CONTROL: a fixed 2000 → red.
    #[test]
    fn the_local_default_is_p_for_the_preset_or_2000_when_unreadable() {
        let w = World::new();
        let fitted = local_fitted_maxcore_with(&w.db, &w.dir.join("meminfo"), &w.dir).unwrap();
        assert_eq!((fitted.ranks, fitted.proposal_mib, fitted.default_maxcore_mib), (8, Some(1600), 1600));
        w.sql("INSERT INTO settings (key, value) VALUES ('cpu_preset', 'max_throughput') \
               ON CONFLICT(key) DO UPDATE SET value = excluded.value");
        let fitted = local_fitted_maxcore_with(&w.db, &w.dir.join("meminfo"), &w.dir).unwrap();
        assert_eq!((fitted.ranks, fitted.default_maxcore_mib), (12, 1000));
        let unreadable = local_fitted_maxcore_with(&w.db, &w.dir.join("no-such"), &w.dir).unwrap();
        assert!(matches!(unreadable.mem_total_kib, Fact::Unknown { .. }));
        assert_eq!((unreadable.proposal_mib, unreadable.default_maxcore_mib), (None, 2000));
    }

    #[test]
    fn apply_maxcore_refuses_with_invalid() {
        assert_eq!(apply_maxcore("! HF\n".into(), 1600).unwrap(), "! HF\n%maxcore 1600\n");
        assert!(matches!(apply_maxcore("%maxcore 1\n%maxcore 2\n".into(), 1600), Err(AppError::Invalid(_))));
    }
}
