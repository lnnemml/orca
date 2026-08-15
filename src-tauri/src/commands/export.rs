//! Export write-commands (unit 3.16). The FRONTEND builds the export content from the
//! already-loaded, already-parsed results (no re-parse) and picks a destination via the
//! native save dialog; these commands only **write** it. Rust owns file I/O (ADR-009).
//!
//! **Never into the app data directory (rule #3).** Job artifacts live under
//! `{data_dir}/orcastudio` — exports must not litter it. The save dialog defaults
//! elsewhere, and these commands additionally **refuse** any path inside that tree, so a
//! user who manually navigates there is stopped rather than corrupting a job dir.

use std::collections::HashSet;
use std::io::Write;
use std::path::{Path, PathBuf};

use rusqlite::{params, params_from_iter, Connection, OptionalExtension};
use tauri::State;

use crate::commands::export_group::{
    build_manifest, build_single_job_manifest, slugify, CopyMode, GroupMeta, GroupNode, JobRow,
    ManifestV1, ResultRow,
};
use crate::commands::settings::DbState;
use crate::error::AppError;
use crate::local_backend::path_is_within;

/// Refuse to write anywhere under the app data dir (rule #3).
fn reject_if_in_data_dir(path: &Path) -> Result<(), AppError> {
    if let Some(dir) = dirs::data_dir() {
        let app_root = dir.join("orcastudio");
        if path.starts_with(&app_root) {
            return Err(AppError::Internal(
                "refusing to export into the app data directory — job artifacts live there \
                 (rule #3); choose another location"
                    .into(),
            ));
        }
    }
    Ok(())
}

/// Write a UTF-8 text export (xyz / CSV) to `path`. `_db` keeps the command signature
/// uniform with the rest and reserves a hook for future per-job export logging.
#[tauri::command]
pub fn write_export_text(
    _db: State<'_, DbState>,
    path: String,
    content: String,
) -> Result<(), AppError> {
    let p = Path::new(&path);
    reject_if_in_data_dir(p)?;
    let mut f = std::fs::File::create(p)?;
    f.write_all(content.as_bytes())?;
    Ok(())
}

/// Write binary bytes (a PNG) to `path`. The frontend passes the PNG as a byte array.
#[tauri::command]
pub fn write_export_bytes(
    _db: State<'_, DbState>,
    path: String,
    bytes: Vec<u8>,
) -> Result<(), AppError> {
    let p = Path::new(&path);
    reject_if_in_data_dir(p)?;
    let mut f = std::fs::File::create(p)?;
    f.write_all(&bytes)?;
    Ok(())
}

// ===========================================================================
// Group export (ADR-021) — a PROJECTION of a job group onto a self-contained,
// human-readable, UUID-traceable directory tree + `manifest.json`. The canonical
// `<UUID>/` dirs and the SQLite rows are the source of truth and are NEVER touched:
// this reads them and WRITES a fresh copy elsewhere. The pure core (naming, the
// manifest shape, the curated allowlist) lives in `commands::export_group`; this is
// the impure wiring (DB reads, the dir listing, the copy, the post-condition).
// ===========================================================================

/// A job's columns needed to project it — the `JobRow` fields minus `present_files`
/// (filled from a dir listing) and `job_type` (no such column; always `None` in v1).
const JOB_EXPORT_COLUMNS: &str = "id, title, status, created_at, job_dir, group_id, \
     pathway_id, source_ensemble_job_id, source_conformer_index";

/// The source group plus ALL of its descendant sub-groups, as `GroupNode`s (mirrors the
/// bounded parent-walk of `commands::groups`, here walking DOWN by `parent_id`). The root
/// must exist ([`AppError::NotFound`] else). A `visited` set + a count bound make a
/// pre-existing corrupt cycle terminate rather than hang.
fn group_subtree(conn: &Connection, root_id: &str) -> Result<Vec<GroupNode>, AppError> {
    let root: Option<(String, String, Option<String>)> = conn
        .query_row(
            "SELECT id, name, parent_id FROM groups WHERE id = ?1",
            params![root_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    let (rid, rname, rparent) = root.ok_or_else(|| AppError::NotFound(format!("group {root_id}")))?;

    let total: i64 = conn.query_row("SELECT COUNT(*) FROM groups", [], |r| r.get(0))?;

    let mut nodes = vec![GroupNode { id: rid.clone(), name: rname, parent_id: rparent }];
    let mut visited: HashSet<String> = HashSet::from([rid.clone()]);
    let mut frontier = vec![rid];
    let mut guard: i64 = 0;
    while let Some(pid) = frontier.pop() {
        guard += 1;
        if guard > total + 1 {
            return Err(AppError::Internal(
                "group hierarchy is corrupt (cycle detected while collecting the subtree)".into(),
            ));
        }
        let mut stmt =
            conn.prepare("SELECT id, name, parent_id FROM groups WHERE parent_id = ?1")?;
        let children = stmt
            .query_map(params![pid], |r| {
                Ok(GroupNode { id: r.get(0)?, name: r.get(1)?, parent_id: r.get(2)? })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for child in children {
            if visited.insert(child.id.clone()) {
                frontier.push(child.id.clone());
                nodes.push(child);
            }
        }
    }
    Ok(nodes)
}

/// Every job whose `group_id` is in `group_ids` (a job lives in exactly one group —
/// ADR-019). `present_files` is filled afterwards from each `job_dir` listing.
fn jobs_in_groups(conn: &Connection, group_ids: &HashSet<String>) -> Result<Vec<JobRow>, AppError> {
    if group_ids.is_empty() {
        return Ok(Vec::new());
    }
    let ids: Vec<String> = group_ids.iter().cloned().collect();
    let placeholders = ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
    let sql = format!(
        "SELECT {JOB_EXPORT_COLUMNS} FROM jobs WHERE group_id IN ({placeholders})"
    );
    let mut stmt = conn.prepare(&sql)?;
    let mut jobs = stmt
        .query_map(params_from_iter(ids.iter()), |r| {
            Ok(JobRow {
                id: r.get(0)?,
                title: r.get(1)?,
                job_type: None, // no `job_type` column — honest null (ADR-021)
                status: r.get(2)?,
                created_at: r.get(3)?,
                job_dir: r.get(4)?,
                group_id: r.get(5)?,
                pathway_id: r.get(6)?,
                source_ensemble_job_id: r.get(7)?,
                source_conformer_index: r.get(8)?,
                present_files: Vec::new(),
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for job in &mut jobs {
        if let Some(dir) = &job.job_dir {
            job.present_files = list_present_files(Path::new(dir));
        }
    }
    Ok(jobs)
}

/// Parsed-results rows for the given job ids (`final_energy_eh` → the manifest's
/// `energy_eh`). Jobs without a row simply have no entry — the manifest then carries
/// `results: null`, never a fabricated zero.
fn results_for_jobs(conn: &Connection, job_ids: &[String]) -> Result<Vec<ResultRow>, AppError> {
    if job_ids.is_empty() {
        return Ok(Vec::new());
    }
    let placeholders = job_ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
    let sql = format!(
        "SELECT job_id, final_energy_eh, imaginary_count FROM results \
         WHERE job_id IN ({placeholders})"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt
        .query_map(params_from_iter(job_ids.iter()), |r| {
            Ok(ResultRow { job_id: r.get(0)?, energy_eh: r.get(1)?, imaginary_count: r.get(2)? })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// Leaf filenames of the regular files directly in `dir` (no subdirs, no `.`/`..`).
/// Best-effort: a missing/unreadable dir yields an empty list (recorded honestly as
/// "no files" rather than aborting the whole export).
fn list_present_files(dir: &Path) -> Vec<String> {
    let mut names = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for entry in rd.flatten() {
            if entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
                if let Some(n) = entry.file_name().to_str() {
                    names.push(n.to_string());
                }
            }
        }
    }
    names
}

/// A fresh export directory under `dest_parent`, NEVER clobbering a prior export:
/// `{group-slug}-export`, disambiguated with `stamp` (and a counter) if that name is
/// already taken.
fn fresh_export_dir(dest_parent: &Path, group_name: &str, stamp: &str) -> Result<PathBuf, AppError> {
    let base = format!("{}-export", slugify(group_name));
    let mut candidate = dest_parent.join(&base);
    if candidate.exists() {
        candidate = dest_parent.join(format!("{base}-{stamp}"));
        let mut n = 2;
        while candidate.exists() {
            candidate = dest_parent.join(format!("{base}-{stamp}-{n}"));
            n += 1;
        }
    }
    std::fs::create_dir_all(&candidate)?;
    Ok(candidate)
}

/// Copy each manifest job's included files from its canonical `<UUID>/` dir into
/// `export_root/<exported_dir>`. A job with no `job_dir` (draft/never-run) is recorded in the
/// manifest only — no directory, no empty-dir clobber. Best-effort per file (a vanished source
/// is skipped, not fatal — the manifest still records it as intended-included). Returns the set
/// of uuids that got a directory, for the post-condition. Shared by group + single-job export.
fn copy_manifest_job_dirs(
    export_root: &Path,
    manifest: &ManifestV1,
    job_dir_by_id: &std::collections::HashMap<&str, Option<&String>>,
) -> Result<HashSet<String>, AppError> {
    let mut copied_uuids: HashSet<String> = HashSet::new();
    for mjob in &manifest.jobs {
        let Some(Some(src_dir)) = job_dir_by_id.get(mjob.uuid.as_str()).copied() else {
            continue; // draft / no job_dir → recorded only
        };
        let dst = export_root.join(&mjob.exported_dir);
        std::fs::create_dir_all(&dst)?;
        for name in &mjob.files.included {
            let from = Path::new(src_dir).join(name);
            let to = dst.join(name);
            let _ = std::fs::copy(&from, &to);
        }
        copied_uuids.insert(mjob.uuid.clone());
    }
    Ok(copied_uuids)
}

/// Re-read the written `manifest.json` and assert it in OUR terms (rule #9): every
/// collected job appears exactly once, every `exported_dir` is unique, every copied
/// job's dir exists on disk, and every uuid resolves to a real `jobs` row. Any mismatch
/// fails loudly rather than leaving a plausible-but-wrong export.
fn verify_export_postcondition(
    conn: &Connection,
    export_root: &Path,
    collected_job_ids: &HashSet<String>,
    copied_uuids: &HashSet<String>,
) -> Result<(), AppError> {
    let text = std::fs::read_to_string(export_root.join("manifest.json"))?;
    let manifest: ManifestV1 = serde_json::from_str(&text)
        .map_err(|e| AppError::Internal(format!("export post-condition: manifest unreadable: {e}")))?;

    let uuids: Vec<&str> = manifest.jobs.iter().map(|j| j.uuid.as_str()).collect();
    let uuid_set: HashSet<&str> = uuids.iter().copied().collect();
    if uuid_set.len() != uuids.len() {
        return Err(AppError::Internal(
            "export post-condition: a job appears more than once in the manifest".into(),
        ));
    }
    let collected: HashSet<&str> = collected_job_ids.iter().map(String::as_str).collect();
    if uuid_set != collected {
        return Err(AppError::Internal(
            "export post-condition: manifest jobs do not match the collected group jobs".into(),
        ));
    }

    let dirs: Vec<&str> = manifest.jobs.iter().map(|j| j.exported_dir.as_str()).collect();
    if dirs.iter().collect::<HashSet<_>>().len() != dirs.len() {
        return Err(AppError::Internal(
            "export post-condition: two jobs share an exported_dir (collision)".into(),
        ));
    }

    for job in &manifest.jobs {
        if copied_uuids.contains(job.uuid.as_str())
            && !export_root.join(&job.exported_dir).is_dir()
        {
            return Err(AppError::Internal(format!(
                "export post-condition: exported dir {:?} missing on disk",
                job.exported_dir
            )));
        }
        let real: bool = conn
            .query_row("SELECT 1 FROM jobs WHERE id = ?1", params![job.uuid], |_| Ok(()))
            .optional()?
            .is_some();
        if !real {
            return Err(AppError::Internal(format!(
                "export post-condition: manifest uuid {} resolves to no job row",
                job.uuid
            )));
        }
    }
    Ok(())
}

/// The testable core of `export_group` (takes `&Connection` + resolved paths + injected
/// timestamps, so it is unit-testable without a running app or a real clock).
#[allow(clippy::too_many_arguments)]
fn export_group_conn(
    conn: &Connection,
    group_id: &str,
    dest_parent: &Path,
    copy_mode: CopyMode,
    jobs_root: &Path,
    exported_at: String,
    dir_stamp: &str,
) -> Result<PathBuf, AppError> {
    // INVERTED path guard (rule #3): the delete path uses `path_is_within` to CONFIRM a
    // path is inside the managed jobs root before removing it; here we REFUSE to export
    // INTO that root, so an export can never litter or overwrite a canonical job dir.
    if path_is_within(jobs_root, dest_parent) {
        return Err(AppError::Internal(
            "refusing to export into the managed jobs directory — canonical job artifacts \
             live there (rule #3); choose another location"
                .into(),
        ));
    }

    let tree = group_subtree(conn, group_id)?; // root existence checked here (NotFound)
    let group_meta = {
        let root = &tree[0];
        GroupMeta { id: root.id.clone(), name: root.name.clone() }
    };
    let group_ids: HashSet<String> = tree.iter().map(|n| n.id.clone()).collect();

    let jobs = jobs_in_groups(conn, &group_ids)?;
    let job_ids: Vec<String> = jobs.iter().map(|j| j.id.clone()).collect();
    let results = results_for_jobs(conn, &job_ids)?;

    let manifest = build_manifest(&group_meta, &jobs, &results, &tree, copy_mode, exported_at);

    // Fresh, never-clobber export root.
    let export_root = fresh_export_dir(dest_parent, &group_meta.name, dir_stamp)?;

    // Copy per job (shared with single-job export). A draft/never-run job (job_dir NULL) is
    // in the manifest but copies nothing and gets NO directory (no empty-dir clobber).
    let job_dir_by_id: std::collections::HashMap<&str, Option<&String>> =
        jobs.iter().map(|j| (j.id.as_str(), j.job_dir.as_ref())).collect();
    let copied_uuids = copy_manifest_job_dirs(&export_root, &manifest, &job_dir_by_id)?;

    // Write the manifest, then verify it in our terms.
    let json = serde_json::to_string_pretty(&manifest)
        .map_err(|e| AppError::Internal(format!("serializing manifest: {e}")))?;
    let mut f = std::fs::File::create(export_root.join("manifest.json"))?;
    f.write_all(json.as_bytes())?;

    let collected: HashSet<String> = job_ids.into_iter().collect();
    verify_export_postcondition(conn, &export_root, &collected, &copied_uuids)?;

    Ok(export_root)
}

/// Export a job group (and all its sub-groups) to `dest_parent` as an ordered, readable,
/// UUID-traceable tree + `manifest.json` (ADR-021). Returns the created export directory.
/// The canonical `<UUID>/` dirs and SQLite rows are untouched — this is a projection.
#[tauri::command]
pub fn export_group(
    db: State<'_, DbState>,
    group_id: String,
    dest_parent: String,
    copy_mode: CopyMode,
) -> Result<String, AppError> {
    let conn = db.lock()?;
    // The managed jobs root — mirror of `local_backend`'s `data_dir/jobs` (rule #3 guard).
    let jobs_root = dirs::data_dir()
        .map(|d| d.join("orcastudio").join("jobs"))
        .unwrap_or_else(|| PathBuf::from("/nonexistent-jobs-root"));
    // Timestamps from SQLite so they match the app's `created_at` format, no new dep.
    let exported_at: String = conn.query_row("SELECT datetime('now')", [], |r| r.get(0))?;
    let dir_stamp: String =
        conn.query_row("SELECT strftime('%Y%m%dT%H%M%S', 'now')", [], |r| r.get(0))?;
    let path = export_group_conn(
        &conn,
        &group_id,
        Path::new(&dest_parent),
        copy_mode,
        &jobs_root,
        exported_at,
        &dir_stamp,
    )?;
    Ok(path.to_string_lossy().into_owned())
}

// ===========================================================================
// Single-job export (ADR-021 sibling) — the SAME projection for one open job, into a
// directory NAMED AFTER THE JOB (slug of its title). Reuses the group-export machinery
// (the inverted rule-#3 guard, fresh_export_dir, the copy loop, the manifest, the
// post-condition); no group resolution beyond naming the job's own group in `source`.
// ===========================================================================

/// The single job's row (+ its dir listing), or [`AppError::NotFound`] if the id is unknown.
fn single_job_row(conn: &Connection, job_id: &str) -> Result<JobRow, AppError> {
    let sql = format!("SELECT {JOB_EXPORT_COLUMNS} FROM jobs WHERE id = ?1");
    let mut job = conn
        .query_row(&sql, params![job_id], |r| {
            Ok(JobRow {
                id: r.get(0)?,
                title: r.get(1)?,
                job_type: None, // no `job_type` column — honest null (ADR-021)
                status: r.get(2)?,
                created_at: r.get(3)?,
                job_dir: r.get(4)?,
                group_id: r.get(5)?,
                pathway_id: r.get(6)?,
                source_ensemble_job_id: r.get(7)?,
                source_conformer_index: r.get(8)?,
                present_files: Vec::new(),
            })
        })
        .optional()?
        .ok_or_else(|| AppError::NotFound(format!("job {job_id}")))?;
    if let Some(dir) = &job.job_dir {
        job.present_files = list_present_files(Path::new(dir));
    }
    Ok(job)
}

/// The job's group as a `GroupMeta` (id + name), or `None` when the job is ungrouped OR its
/// `group_id` dangles — honest absence, never a fabricated group (the manifest carries null).
fn group_meta_of(conn: &Connection, group_id: &str) -> Result<Option<GroupMeta>, AppError> {
    let meta = conn
        .query_row(
            "SELECT id, name FROM groups WHERE id = ?1",
            params![group_id],
            |r| Ok(GroupMeta { id: r.get(0)?, name: r.get(1)? }),
        )
        .optional()?;
    Ok(meta)
}

/// The testable core of `export_job` (takes `&Connection` + resolved paths + injected
/// timestamps). Mirrors [`export_group_conn`] MINUS the group tree: one job row, its result,
/// its own group named in `source` (null if ungrouped).
#[allow(clippy::too_many_arguments)]
fn export_job_conn(
    conn: &Connection,
    job_id: &str,
    dest_parent: &Path,
    copy_mode: CopyMode,
    jobs_root: &Path,
    exported_at: String,
    dir_stamp: &str,
) -> Result<PathBuf, AppError> {
    // Same inverted rule-#3 guard as the group export: refuse to export INTO the managed
    // jobs root, so an export can never litter or overwrite a canonical job dir.
    if path_is_within(jobs_root, dest_parent) {
        return Err(AppError::Internal(
            "refusing to export into the managed jobs directory — canonical job artifacts \
             live there (rule #3); choose another location"
                .into(),
        ));
    }

    let job = single_job_row(conn, job_id)?; // NotFound if the id is unknown
    let results = results_for_jobs(conn, &[job.id.clone()])?;
    let group = match &job.group_id {
        Some(gid) => group_meta_of(conn, gid)?,
        None => None,
    };

    let manifest =
        build_single_job_manifest(&job, results.first(), group.as_ref(), copy_mode, exported_at);

    // Fresh, never-clobber export root: `{slug(title)}-export`.
    let export_root = fresh_export_dir(dest_parent, &job.title, dir_stamp)?;

    let job_dir_by_id: std::collections::HashMap<&str, Option<&String>> =
        std::iter::once((job.id.as_str(), job.job_dir.as_ref())).collect();
    let copied_uuids = copy_manifest_job_dirs(&export_root, &manifest, &job_dir_by_id)?;

    let json = serde_json::to_string_pretty(&manifest)
        .map_err(|e| AppError::Internal(format!("serializing manifest: {e}")))?;
    let mut f = std::fs::File::create(export_root.join("manifest.json"))?;
    f.write_all(json.as_bytes())?;

    let collected: HashSet<String> = HashSet::from([job.id.clone()]);
    verify_export_postcondition(conn, &export_root, &collected, &copied_uuids)?;

    Ok(export_root)
}

/// Export a SINGLE open job's folder to `dest_parent` as a readable, UUID-traceable directory
/// named after the job (slug of its title) + `manifest.json` (ADR-021 sibling of group export).
/// Returns the created export directory. The canonical `<UUID>/` dir and SQLite rows are
/// untouched — this is a projection.
#[tauri::command]
pub fn export_job(
    db: State<'_, DbState>,
    job_id: String,
    dest_parent: String,
    copy_mode: CopyMode,
) -> Result<String, AppError> {
    let conn = db.lock()?;
    let jobs_root = dirs::data_dir()
        .map(|d| d.join("orcastudio").join("jobs"))
        .unwrap_or_else(|| PathBuf::from("/nonexistent-jobs-root"));
    let exported_at: String = conn.query_row("SELECT datetime('now')", [], |r| r.get(0))?;
    let dir_stamp: String =
        conn.query_row("SELECT strftime('%Y%m%dT%H%M%S', 'now')", [], |r| r.get(0))?;
    let path = export_job_conn(
        &conn,
        &job_id,
        Path::new(&dest_parent),
        copy_mode,
        &jobs_root,
        exported_at,
        &dir_stamp,
    )?;
    Ok(path.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_path_inside_the_data_dir_is_refused() {
        if let Some(dir) = dirs::data_dir() {
            let inside = dir.join("orcastudio").join("jobs").join("x").join("export.xyz");
            assert!(reject_if_in_data_dir(&inside).is_err());
        }
    }

    #[test]
    fn a_path_outside_is_allowed() {
        let outside = std::env::temp_dir().join("orcastudio_export_test.xyz");
        assert!(reject_if_in_data_dir(&outside).is_ok());
    }

    // --- Group export (Part B) ------------------------------------------------

    use crate::db::init_db;
    use rusqlite::params;

    /// A migrated DB + a unique scratch root per test (process id + atomic counter),
    /// so parallel test runs never collide.
    fn scratch(tag: &str) -> (Connection, PathBuf) {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "orcastudio-export-{}-{}-{}",
            tag,
            std::process::id(),
            n
        ));
        std::fs::remove_dir_all(&root).ok();
        std::fs::create_dir_all(&root).unwrap();
        let conn = init_db(&root.join("db")).expect("init_db");
        (conn, root)
    }

    fn mk_group(conn: &Connection, id: &str, name: &str, parent: Option<&str>) {
        conn.execute(
            "INSERT INTO groups (id, name, parent_id) VALUES (?1, ?2, ?3)",
            params![id, name, parent],
        )
        .unwrap();
    }

    /// Insert a job with a real on-disk `job_dir` under `jobs_root/<id>` containing the
    /// given files, and return that dir. `created_at` is set explicitly for ordering.
    fn mk_job_with_dir(
        conn: &Connection,
        jobs_root: &Path,
        id: &str,
        title: &str,
        group: &str,
        created_at: &str,
        files: &[&str],
    ) -> PathBuf {
        let dir = jobs_root.join(id);
        std::fs::create_dir_all(&dir).unwrap();
        for f in files {
            std::fs::write(dir.join(f), format!("contents-of-{f}")).unwrap();
        }
        conn.execute(
            "INSERT INTO jobs (id, title, input_content, status, job_dir, group_id, created_at) \
             VALUES (?1, ?2, ?3, 'parsed', ?4, ?5, ?6)",
            params![id, title, "! Opt", dir.to_string_lossy(), group, created_at],
        )
        .unwrap();
        dir
    }

    /// Snapshot every file under `dir` as (relative-name → bytes) for a byte-equality check.
    fn snapshot(dir: &Path) -> std::collections::BTreeMap<String, Vec<u8>> {
        let mut m = std::collections::BTreeMap::new();
        for e in std::fs::read_dir(dir).unwrap().flatten() {
            if e.file_type().unwrap().is_file() {
                m.insert(
                    e.file_name().to_string_lossy().into_owned(),
                    std::fs::read(e.path()).unwrap(),
                );
            }
        }
        m
    }

    /// m1: the headline path. A group with sub-groups exports to readable, ordered dirs
    /// and a manifest that round-trips uuid ↔ name ↔ structure. m4: curated mode leaves
    /// `.gbw` off disk but records it in `manifest.omitted`.
    #[test]
    fn export_projects_group_to_ordered_dirs_and_manifest() {
        let (conn, root) = scratch("m1");
        let jobs_root = root.join("jobs");
        let dest = root.join("dest");
        std::fs::create_dir_all(&dest).unwrap();

        mk_group(&conn, "g-root", "HCN reduction", None);
        mk_group(&conn, "g-sub", "si-face", Some("g-root"));
        let d1 = mk_job_with_dir(
            &conn, &jobs_root, "j1", "HCN opt", "g-root", "2026-08-14T10:00:00",
            &["input.inp", "input.property.txt", "input.gbw"],
        );
        mk_job_with_dir(
            &conn, &jobs_root, "j2", "HNC opt", "g-sub", "2026-08-14T11:00:00",
            &["input.inp", "input.property.txt"],
        );
        // A parsed-results row for j1.
        conn.execute(
            "INSERT INTO results (job_id, final_energy_eh, imaginary_count, data_json, parser_version) \
             VALUES ('j1', -93.42, 0, '{}', 1)",
            [],
        )
        .unwrap();

        let before = snapshot(&d1);

        let out = export_group_conn(
            &conn, "g-root", &dest, CopyMode::Curated, &jobs_root,
            "2026-08-14 12:00:00".into(), "20260814T120000",
        )
        .expect("export should succeed");

        // Manifest round-trips.
        let text = std::fs::read_to_string(out.join("manifest.json")).unwrap();
        let m: ManifestV1 = serde_json::from_str(&text).unwrap();
        assert_eq!(m.jobs.len(), 2);
        assert_eq!(m.source.group_name.as_deref(), Some("HCN reduction"));
        // Creation order: j1 first (01_), j2 second (02_).
        assert_eq!(m.jobs[0].uuid, "j1");
        assert_eq!(m.jobs[0].exported_dir, "1_hcn-opt");
        assert_eq!(m.jobs[1].uuid, "j2");
        assert_eq!(m.jobs[1].exported_dir, "2_hnc-opt");
        // group_path reflects the tree.
        assert_eq!(m.jobs[0].group_path, vec!["HCN reduction"]);
        assert_eq!(m.jobs[1].group_path, vec!["HCN reduction", "si-face"]);
        // j1 result carried, j2 has none (null, not zero).
        assert_eq!(m.jobs[0].results.as_ref().unwrap().energy_eh, Some(-93.42));
        assert!(m.jobs[1].results.is_none());

        // m4: curated — .gbw recorded as omitted, absent on disk; property.txt copied.
        assert!(m.jobs[0].files.omitted.contains(&"input.gbw".to_string()));
        assert!(m.jobs[0].files.included.contains(&"input.property.txt".to_string()));
        assert!(out.join("1_hcn-opt").join("input.property.txt").exists());
        assert!(!out.join("1_hcn-opt").join("input.gbw").exists());

        // m3: the canonical source dir is byte-for-byte unchanged (projection only).
        assert_eq!(before, snapshot(&d1), "canonical job dir must be untouched");

        std::fs::remove_dir_all(&root).ok();
    }

    /// m2 (negative): two jobs both titled "Opt" get distinct dirs — neither overwrites
    /// the other, and both are copied.
    #[test]
    fn two_same_titled_jobs_do_not_clobber() {
        let (conn, root) = scratch("m2");
        let jobs_root = root.join("jobs");
        let dest = root.join("dest");
        std::fs::create_dir_all(&dest).unwrap();

        mk_group(&conn, "g", "study", None);
        mk_job_with_dir(&conn, &jobs_root, "j1", "Opt", "g", "2026-08-14T10:00:00", &["input.inp"]);
        mk_job_with_dir(&conn, &jobs_root, "j2", "Opt", "g", "2026-08-14T11:00:00", &["input.inp"]);

        let out = export_group_conn(
            &conn, "g", &dest, CopyMode::Full, &jobs_root,
            "2026-08-14 12:00:00".into(), "stamp",
        )
        .unwrap();

        assert!(out.join("1_opt").join("input.inp").exists());
        assert!(out.join("2_opt").join("input.inp").exists());
    }

    /// Inverted path guard: exporting INTO the managed jobs root is refused (rule #3).
    #[test]
    fn export_into_jobs_root_is_refused() {
        let (conn, root) = scratch("guard");
        let jobs_root = root.join("jobs");
        std::fs::create_dir_all(&jobs_root).unwrap();
        mk_group(&conn, "g", "study", None);

        // dest inside jobs_root → refused.
        let inside = jobs_root.join("sneaky");
        std::fs::create_dir_all(&inside).unwrap();
        let err = export_group_conn(
            &conn, "g", &inside, CopyMode::Curated, &jobs_root, "t".into(), "s",
        );
        assert!(matches!(err, Err(AppError::Internal(_))), "must refuse export into jobs root");
    }

    /// A draft/never-run job (job_dir NULL) is recorded in the manifest with empty files
    /// and gets NO directory — no empty-dir clobber. The post-condition still passes.
    #[test]
    fn draft_job_is_recorded_but_not_copied() {
        let (conn, root) = scratch("draft");
        let jobs_root = root.join("jobs");
        let dest = root.join("dest");
        std::fs::create_dir_all(&dest).unwrap();

        mk_group(&conn, "g", "study", None);
        // A draft: no job_dir.
        conn.execute(
            "INSERT INTO jobs (id, title, input_content, status, group_id, created_at) \
             VALUES ('draft1', 'Untitled', '! Opt', 'draft', 'g', '2026-08-14T09:00:00')",
            [],
        )
        .unwrap();
        mk_job_with_dir(&conn, &jobs_root, "j1", "Opt", "g", "2026-08-14T10:00:00", &["input.inp"]);

        let out = export_group_conn(
            &conn, "g", &dest, CopyMode::Curated, &jobs_root, "t".into(), "s",
        )
        .unwrap();

        let m: ManifestV1 =
            serde_json::from_str(&std::fs::read_to_string(out.join("manifest.json")).unwrap()).unwrap();
        let draft = m.jobs.iter().find(|j| j.uuid == "draft1").unwrap();
        assert!(draft.files.included.is_empty() && draft.files.omitted.is_empty());
        assert!(!out.join(&draft.exported_dir).exists(), "draft must get no directory");
        // The real job WAS copied.
        let real = m.jobs.iter().find(|j| j.uuid == "j1").unwrap();
        assert!(out.join(&real.exported_dir).join("input.inp").exists());

        std::fs::remove_dir_all(&root).ok();
    }

    /// A fresh export never clobbers a prior one: a second export lands in a distinct dir.
    #[test]
    fn second_export_does_not_clobber_first() {
        let (conn, root) = scratch("fresh");
        let jobs_root = root.join("jobs");
        let dest = root.join("dest");
        std::fs::create_dir_all(&dest).unwrap();
        mk_group(&conn, "g", "study", None);
        mk_job_with_dir(&conn, &jobs_root, "j1", "Opt", "g", "2026-08-14T10:00:00", &["input.inp"]);

        let a = export_group_conn(
            &conn, "g", &dest, CopyMode::Curated, &jobs_root, "t".into(), "20260814T120000",
        )
        .unwrap();
        let b = export_group_conn(
            &conn, "g", &dest, CopyMode::Curated, &jobs_root, "t".into(), "20260814T120000",
        )
        .unwrap();
        assert_ne!(a, b, "second export must not reuse the first dir");
        assert!(a.exists() && b.exists());

        std::fs::remove_dir_all(&root).ok();
    }

    /// A missing group is NotFound (nothing is written).
    #[test]
    fn export_missing_group_is_not_found() {
        let (conn, root) = scratch("nf");
        let dest = root.join("dest");
        std::fs::create_dir_all(&dest).unwrap();
        let err = export_group_conn(
            &conn, "no-such", &dest, CopyMode::Curated, &root.join("jobs"), "t".into(), "s",
        );
        assert!(matches!(err, Err(AppError::NotFound(_))));
        std::fs::remove_dir_all(&root).ok();
    }

    // --- Single-job export (Part B) -------------------------------------------

    /// Insert an UNGROUPED job with a real on-disk `job_dir` (group_id NULL) — the group
    /// helper always sets a group, so single-job tests need this variant.
    fn mk_ungrouped_job_with_dir(
        conn: &Connection,
        jobs_root: &Path,
        id: &str,
        title: &str,
        files: &[&str],
    ) -> PathBuf {
        let dir = jobs_root.join(id);
        std::fs::create_dir_all(&dir).unwrap();
        for f in files {
            std::fs::write(dir.join(f), format!("contents-of-{f}")).unwrap();
        }
        conn.execute(
            "INSERT INTO jobs (id, title, input_content, status, job_dir, created_at) \
             VALUES (?1, ?2, ?3, 'parsed', ?4, '2026-08-14T10:00:00')",
            params![id, title, "! Opt", dir.to_string_lossy()],
        )
        .unwrap();
        dir
    }

    /// m1 + m3: a GROUPED single job exports to `{slug(title)}-export/{slug(title)}/…` + a
    /// manifest that round-trips uuid↔name and names the job's group; curated leaves `.gbw`
    /// off disk but records it in `omitted`; the canonical `<UUID>/` dir is byte-unchanged.
    #[test]
    fn export_job_projects_single_job_and_manifest() {
        let (conn, root) = scratch("sj-m1");
        let jobs_root = root.join("jobs");
        let dest = root.join("dest");
        std::fs::create_dir_all(&dest).unwrap();

        mk_group(&conn, "g", "HCN reduction", None);
        let d1 = mk_job_with_dir(
            &conn, &jobs_root, "j1", "HCN Opt Freq", "g", "2026-08-14T10:00:00",
            &["input.inp", "input.property.txt", "input.gbw"],
        );
        conn.execute(
            "INSERT INTO results (job_id, final_energy_eh, imaginary_count, data_json, parser_version) \
             VALUES ('j1', -93.42, 0, '{}', 1)",
            [],
        )
        .unwrap();
        let before = snapshot(&d1);

        let out = export_job_conn(
            &conn, "j1", &dest, CopyMode::Curated, &jobs_root, "2026-08-14 12:00:00".into(), "stamp",
        )
        .expect("single-job export should succeed");

        // The export root is named after the JOB (slug of its title).
        assert_eq!(out.file_name().unwrap(), "hcn-opt-freq-export");
        let m: ManifestV1 =
            serde_json::from_str(&std::fs::read_to_string(out.join("manifest.json")).unwrap()).unwrap();
        assert_eq!(m.jobs.len(), 1);
        assert_eq!(m.jobs[0].uuid, "j1");
        // NO numeric prefix — the artifact dir is just the slug.
        assert_eq!(m.jobs[0].exported_dir, "hcn-opt-freq");
        assert_eq!(m.source.group_name.as_deref(), Some("HCN reduction"));
        assert_eq!(m.jobs[0].results.as_ref().unwrap().energy_eh, Some(-93.42));

        // Curated: property.txt copied, .gbw omitted-not-copied but recorded.
        assert!(out.join("hcn-opt-freq").join("input.property.txt").exists());
        assert!(!out.join("hcn-opt-freq").join("input.gbw").exists());
        assert!(m.jobs[0].files.omitted.contains(&"input.gbw".to_string()));
        // m3: canonical source dir untouched (projection only).
        assert_eq!(before, snapshot(&d1), "canonical job dir must be untouched");

        std::fs::remove_dir_all(&root).ok();
    }

    /// m4: an UNGROUPED job's manifest carries `source.group = null` — no fabricated group.
    #[test]
    fn export_job_ungrouped_source_group_is_null() {
        let (conn, root) = scratch("sj-m4");
        let jobs_root = root.join("jobs");
        let dest = root.join("dest");
        std::fs::create_dir_all(&dest).unwrap();
        mk_ungrouped_job_with_dir(&conn, &jobs_root, "j1", "scratch", &["input.inp"]);

        let out = export_job_conn(
            &conn, "j1", &dest, CopyMode::Full, &jobs_root, "t".into(), "s",
        )
        .unwrap();
        let m: ManifestV1 =
            serde_json::from_str(&std::fs::read_to_string(out.join("manifest.json")).unwrap()).unwrap();
        assert!(m.source.group_id.is_none(), "ungrouped → null group, not fabricated");
        assert!(m.source.group_name.is_none());
        assert!(m.jobs[0].group_path.is_empty());
        assert!(out.join("scratch").join("input.inp").exists());

        std::fs::remove_dir_all(&root).ok();
    }

    /// m2 (negative): exporting a single job INTO the managed jobs root is refused (rule #3).
    #[test]
    fn export_job_into_jobs_root_is_refused() {
        let (conn, root) = scratch("sj-guard");
        let jobs_root = root.join("jobs");
        mk_ungrouped_job_with_dir(&conn, &jobs_root, "j1", "Opt", &["input.inp"]);
        let inside = jobs_root.join("sneaky");
        std::fs::create_dir_all(&inside).unwrap();
        let err = export_job_conn(
            &conn, "j1", &inside, CopyMode::Curated, &jobs_root, "t".into(), "s",
        );
        assert!(matches!(err, Err(AppError::Internal(_))), "must refuse export into jobs root");
        std::fs::remove_dir_all(&root).ok();
    }

    /// An unknown job id is NotFound (nothing written).
    #[test]
    fn export_job_missing_is_not_found() {
        let (conn, root) = scratch("sj-nf");
        let dest = root.join("dest");
        std::fs::create_dir_all(&dest).unwrap();
        let err = export_job_conn(
            &conn, "no-such", &dest, CopyMode::Curated, &root.join("jobs"), "t".into(), "s",
        );
        assert!(matches!(err, Err(AppError::NotFound(_))));
        std::fs::remove_dir_all(&root).ok();
    }

    /// A draft single job (job_dir NULL) exports the manifest only — no artifact dir, and the
    /// post-condition still passes (mirrors the group-export draft case).
    #[test]
    fn export_job_draft_writes_manifest_only() {
        let (conn, root) = scratch("sj-draft");
        let dest = root.join("dest");
        std::fs::create_dir_all(&dest).unwrap();
        conn.execute(
            "INSERT INTO jobs (id, title, input_content, status, created_at) \
             VALUES ('draft1', 'Untitled draft', '! Opt', 'draft', '2026-08-14T09:00:00')",
            [],
        )
        .unwrap();

        let out = export_job_conn(
            &conn, "draft1", &dest, CopyMode::Curated, &root.join("jobs"), "t".into(), "s",
        )
        .unwrap();
        let m: ManifestV1 =
            serde_json::from_str(&std::fs::read_to_string(out.join("manifest.json")).unwrap()).unwrap();
        assert_eq!(m.jobs.len(), 1);
        assert!(m.jobs[0].files.included.is_empty() && m.jobs[0].files.omitted.is_empty());
        assert!(!out.join(&m.jobs[0].exported_dir).exists(), "draft gets no artifact dir");

        std::fs::remove_dir_all(&root).ok();
    }
}
