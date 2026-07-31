//! Manual-index commands (Phase 4.3, ADR-013). No UI — the panel + hover provider
//! are 4.4. `build_manual_index` is author-run indexing (like the fetch script);
//! `search_manual` is the query surface the panel will call.

use std::path::PathBuf;

use tauri::State;

use crate::commands::settings::DbState;
use crate::error::AppError;
use crate::manual::index::{self, IngestReport, ManualHit};

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
