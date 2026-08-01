//! Manual-index commands (Phase 4.3, ADR-013). No UI — the panel + hover provider
//! are 4.4. `build_manual_index` is author-run indexing (like the fetch script);
//! `search_manual` is the query surface the panel will call.

use std::path::PathBuf;

use tauri::State;

use crate::commands::settings::DbState;
use crate::error::AppError;
use crate::manual::index::{self, IngestReport, ManualHit, ManualPage, ManualSection, ManualStatus};

/// Resolve the manual corpus root honestly for BOTH the source and the bundled run —
/// no longer a debt, because page display (below) reads the corpus off disk, not just
/// the one-off indexer.
///
/// - **Source/dev run:** the repo tree the fetch script writes,
///   `CARGO_MANIFEST_DIR/../resources/manual`. That path is baked at compile time, so on
///   a bundled app running on another machine it does not exist — which is exactly the
///   discriminator: if it is on disk, we are running from source.
/// - **Bundled run:** the corpus lives under the **user data dir**, alongside the SQLite
///   DB it is indexed into (`<data_dir>/orcastudio/manual`, the same `dirs::data_dir()`
///   base `lib.rs` uses for the DB). **Not** an app-resource dir: the ORCA manual is
///   never bundled or redistributed (domain rule #7), so it cannot ship inside the app
///   bundle — the user fetches it locally, and it belongs next to their data.
///
/// If neither resolves, this is an explicit error naming **where it looked**, never an
/// empty corpus that reads downstream as "no sections".
fn manual_root() -> Result<PathBuf, AppError> {
    let src = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../resources/manual");
    if src.is_dir() {
        return Ok(src);
    }
    let bundled = dirs::data_dir()
        .ok_or_else(|| AppError::Internal("could not determine user data directory".into()))?
        .join("orcastudio")
        .join("manual");
    if bundled.is_dir() {
        return Ok(bundled);
    }
    Err(AppError::NotFound(format!(
        "manual corpus not found. Looked in {} (source run) and {} (bundled run). \
         Fetch it with scripts/fetch-manual.py.",
        src.display(),
        bundled.display()
    )))
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
    let root = manual_root()?;
    let version = version.unwrap_or_else(|| corpus_version(&root));
    let mut conn = db.lock()?;
    index::build_index(&mut conn, &root, &version)
}

/// The full text of one manual file plus the line-bounds of every section indexed from
/// it (id, level, title, anchor, line_start, line_end), in line order — so the panel can
/// scroll to and highlight a section without a second request. **The page is read from
/// the FILE ON DISK, not rebuilt from stored sections** (the preamble and the exact
/// heading lines are not byte-reproducible from the DB). A post-condition (rule #9)
/// inside `get_page` asserts the file on disk matches the index and fails loudly on a
/// drift ("page on disk does not match the index; rebuild") rather than showing a
/// plausible-but-wrong page.
#[tauri::command]
pub fn get_manual_page(db: State<'_, DbState>, file: String) -> Result<ManualPage, AppError> {
    let root = manual_root()?;
    let conn = db.lock()?;
    index::get_page(&conn, &root, &file)
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
