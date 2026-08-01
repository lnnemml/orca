//! Manual-index commands (Phase 4.3, ADR-013). No UI — the panel + hover provider
//! are 4.4. `build_manual_index` is author-run indexing (like the fetch script);
//! `search_manual` is the query surface the panel will call.

use std::path::PathBuf;

use tauri::State;

use crate::commands::settings::DbState;
use crate::error::AppError;
use crate::manual::index::{self, IngestReport, ManualHit, ManualSection, ManualStatus};

/// The manual corpus root. Author-run indexing reads the repo's `resources/manual/`
/// (the same tree the fetch script writes); bundled-resource resolution for a shipped
/// app is a later concern (4.4+).
fn manual_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../resources/manual")
}

fn corpus_version(root: &std::path::Path) -> String {
    std::fs::read_to_string(root.join("manifest.json"))
        .ok()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .and_then(|v| v.get("orca_version").and_then(|x| x.as_str()).map(String::from))
        .unwrap_or_else(|| "6.1".to_string())
}

/// Build (or rebuild) the manual index for `version` (default: the fetched corpus's
/// version). Idempotent — replaces that version's rows.
#[tauri::command]
pub fn build_manual_index(
    db: State<'_, DbState>,
    version: Option<String>,
) -> Result<IngestReport, AppError> {
    let root = manual_root();
    let version = version.unwrap_or_else(|| corpus_version(&root));
    let mut conn = db.lock()?;
    index::build_index(&mut conn, &root, &version)
}

/// Full-text search over the manual index. Empty query → empty result.
#[tauri::command]
pub fn search_manual(
    db: State<'_, DbState>,
    query: String,
    limit: Option<usize>,
) -> Result<Vec<ManualHit>, AppError> {
    let conn = db.lock()?;
    index::search_manual(&conn, &query, limit.unwrap_or(20))
}

/// One full section by id (for the panel/`SectionView`) — `search_manual` returns only
/// a snippet. A missing id is an error, not an empty section.
#[tauri::command]
pub fn get_manual_section(db: State<'_, DbState>, id: i64) -> Result<ManualSection, AppError> {
    let conn = db.lock()?;
    index::get_section(&conn, id)
}

/// Is the manual index built, and with what tallies? `None` → the panel shows a
/// "Build index" state rather than an empty (mis-readable) result list.
#[tauri::command]
pub fn manual_index_status(db: State<'_, DbState>) -> Result<Option<ManualStatus>, AppError> {
    let conn = db.lock()?;
    index::index_status(&conn)
}

/// Resolve a `keywords.json` section descriptor to the full section (the hover→drawer
/// path). `map_version` is `keywords.json`'s `orca_version`: if it disagrees with the
/// built index, the map is **stale** — say so, don't silently resolve against a
/// different corpus. The descriptor must resolve to exactly one row (`resolve_descriptor`).
#[tauri::command]
pub fn resolve_manual_section(
    db: State<'_, DbState>,
    file: String,
    breadcrumb: Vec<String>,
    title: String,
    nth: usize,
    map_version: String,
) -> Result<ManualSection, AppError> {
    let conn = db.lock()?;
    let status = index::index_status(&conn)?
        .ok_or_else(|| AppError::NotFound("manual index not built".into()))?;
    if status.orca_version != map_version {
        return Err(AppError::Internal(format!(
            "keywords.json is for ORCA {map_version} but the index is {} — rebuild the index",
            status.orca_version
        )));
    }
    let id = index::resolve_descriptor(&conn, &status.orca_version, &file, &breadcrumb, &title, nth)?;
    index::get_section(&conn, id)
}
