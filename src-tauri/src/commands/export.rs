//! Export write-commands (unit 3.16). The FRONTEND builds the export content from the
//! already-loaded, already-parsed results (no re-parse) and picks a destination via the
//! native save dialog; these commands only **write** it. Rust owns file I/O (ADR-009).
//!
//! **Never into the app data directory (rule #3).** Job artifacts live under
//! `{data_dir}/orcastudio` — exports must not litter it. The save dialog defaults
//! elsewhere, and these commands additionally **refuse** any path inside that tree, so a
//! user who manually navigates there is stopped rather than corrupting a job dir.

use std::io::Write;
use std::path::Path;

use tauri::State;

use crate::commands::settings::DbState;
use crate::error::AppError;

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
}
