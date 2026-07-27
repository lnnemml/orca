//! Molecule commands: the CRUD surface over the `molecules` table.
//!
//! Same shape as `commands::jobs`: each Tauri command is a thin wrapper that
//! locks the shared connection and delegates to a `*_conn` helper taking a
//! `&Connection` directly, so the logic is unit-testable without a running
//! Tauri app.

use rusqlite::{params, Connection, OptionalExtension};
use tauri::State;
use uuid::Uuid;

use crate::commands::settings::DbState;
use crate::error::AppError;
use crate::models::molecule::Molecule;

// --- Connection-level helpers (testable) ------------------------------------

/// Insert a fresh molecule and return it fully hydrated.
fn create_molecule_conn(
    conn: &Connection,
    name: &str,
    formula: &str,
    xyz: &str,
    charge: i32,
    multiplicity: i32,
    tags: &str,
) -> Result<Molecule, AppError> {
    let id = Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO molecules (id, name, formula, xyz, charge, multiplicity, tags) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![id, name, formula, xyz, charge, multiplicity, tags],
    )?;
    get_molecule_conn(conn, &id)
}

/// All molecules, newest first.
fn list_molecules_conn(conn: &Connection) -> Result<Vec<Molecule>, AppError> {
    let sql = format!(
        "SELECT {} FROM molecules ORDER BY created_at DESC",
        Molecule::COLUMNS
    );
    let mut stmt = conn.prepare(&sql)?;
    let molecules = stmt
        .query_map([], Molecule::from_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(molecules)
}

/// A single molecule by id, or [`AppError::NotFound`].
fn get_molecule_conn(conn: &Connection, id: &str) -> Result<Molecule, AppError> {
    let sql = format!("SELECT {} FROM molecules WHERE id = ?1", Molecule::COLUMNS);
    conn.query_row(&sql, params![id], Molecule::from_row)
        .optional()?
        .ok_or_else(|| AppError::NotFound(format!("molecule {id}")))
}

/// Full update of an existing molecule; [`AppError::NotFound`] if the id is
/// absent. Returns the updated molecule.
fn update_molecule_conn(
    conn: &Connection,
    id: &str,
    name: &str,
    formula: &str,
    xyz: &str,
    charge: i32,
    multiplicity: i32,
    tags: &str,
) -> Result<Molecule, AppError> {
    let affected = conn.execute(
        "UPDATE molecules SET name = ?1, formula = ?2, xyz = ?3, charge = ?4, \
         multiplicity = ?5, tags = ?6 WHERE id = ?7",
        params![name, formula, xyz, charge, multiplicity, tags, id],
    )?;
    if affected == 0 {
        return Err(AppError::NotFound(format!("molecule {id}")));
    }
    get_molecule_conn(conn, id)
}

/// Delete a molecule; [`AppError::NotFound`] if the id is absent.
fn delete_molecule_conn(conn: &Connection, id: &str) -> Result<(), AppError> {
    let affected = conn.execute("DELETE FROM molecules WHERE id = ?1", params![id])?;
    if affected == 0 {
        return Err(AppError::NotFound(format!("molecule {id}")));
    }
    Ok(())
}

// --- Tauri commands ---------------------------------------------------------

#[tauri::command]
pub fn create_molecule(
    db: State<'_, DbState>,
    name: String,
    formula: String,
    xyz: String,
    charge: i32,
    multiplicity: i32,
    tags: String,
) -> Result<Molecule, AppError> {
    let conn = db.lock()?;
    create_molecule_conn(&conn, &name, &formula, &xyz, charge, multiplicity, &tags)
}

#[tauri::command]
pub fn list_molecules(db: State<'_, DbState>) -> Result<Vec<Molecule>, AppError> {
    let conn = db.lock()?;
    list_molecules_conn(&conn)
}

#[tauri::command]
pub fn get_molecule(db: State<'_, DbState>, id: String) -> Result<Molecule, AppError> {
    let conn = db.lock()?;
    get_molecule_conn(&conn, &id)
}

#[tauri::command]
pub fn update_molecule(
    db: State<'_, DbState>,
    id: String,
    name: String,
    formula: String,
    xyz: String,
    charge: i32,
    multiplicity: i32,
    tags: String,
) -> Result<Molecule, AppError> {
    let conn = db.lock()?;
    update_molecule_conn(&conn, &id, &name, &formula, &xyz, charge, multiplicity, &tags)
}

#[tauri::command]
pub fn delete_molecule(db: State<'_, DbState>, id: String) -> Result<(), AppError> {
    let conn = db.lock()?;
    delete_molecule_conn(&conn, &id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::init_db;

    /// A migrated database in a throwaway temp dir. A process-wide atomic
    /// counter keeps each test's directory unique even under parallel runs.
    fn test_db() -> (Connection, std::path::PathBuf) {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "orcastudio-molecules-test-{}-{}",
            std::process::id(),
            n
        ));
        std::fs::remove_dir_all(&dir).ok();
        let conn = init_db(&dir).expect("init_db should succeed");
        (conn, dir)
    }

    const ETHANOL_XYZ: &str = "3\nethanol\nC 0.0 0.0 0.0\nC 1.5 0.0 0.0\nO 2.1 1.2 0.0\n";

    #[test]
    fn create_lists_molecule() {
        let (conn, dir) = test_db();

        let mol = create_molecule_conn(&conn, "ethanol", "C2H6O", ETHANOL_XYZ, 0, 1, "substrate")
            .unwrap();
        assert_eq!(mol.name, "ethanol");
        assert_eq!(mol.formula, "C2H6O");
        assert_eq!(mol.tags, "substrate");

        let all = list_molecules_conn(&conn).unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].id, mol.id);
        assert_eq!(all[0].xyz, ETHANOL_XYZ);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn get_missing_molecule_is_not_found() {
        let (conn, dir) = test_db();

        let err = get_molecule_conn(&conn, "no-such-id").unwrap_err();
        assert!(matches!(err, AppError::NotFound(_)));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn update_molecule_changes_fields() {
        let (conn, dir) = test_db();

        let mol =
            create_molecule_conn(&conn, "ethanol", "C2H6O", ETHANOL_XYZ, 0, 1, "").unwrap();
        let updated = update_molecule_conn(
            &conn,
            &mol.id,
            "ethanolate",
            "C2H5O",
            ETHANOL_XYZ,
            -1,
            1,
            "reagent,anion",
        )
        .unwrap();
        assert_eq!(updated.name, "ethanolate");
        assert_eq!(updated.formula, "C2H5O");
        assert_eq!(updated.charge, -1);
        assert_eq!(updated.tags, "reagent,anion");

        // Persisted.
        let reloaded = get_molecule_conn(&conn, &mol.id).unwrap();
        assert_eq!(reloaded.name, "ethanolate");
        assert_eq!(reloaded.charge, -1);

        // Updating a missing id is NotFound.
        let err = update_molecule_conn(&conn, "no-such-id", "x", "", "", 0, 1, "").unwrap_err();
        assert!(matches!(err, AppError::NotFound(_)));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn delete_molecule_removes_it() {
        let (conn, dir) = test_db();

        let mol =
            create_molecule_conn(&conn, "ethanol", "C2H6O", ETHANOL_XYZ, 0, 1, "").unwrap();
        delete_molecule_conn(&conn, &mol.id).unwrap();

        assert!(list_molecules_conn(&conn).unwrap().is_empty());
        assert!(matches!(
            get_molecule_conn(&conn, &mol.id).unwrap_err(),
            AppError::NotFound(_)
        ));

        // Deleting a missing id is NotFound.
        let err = delete_molecule_conn(&conn, "no-such-id").unwrap_err();
        assert!(matches!(err, AppError::NotFound(_)));

        std::fs::remove_dir_all(&dir).ok();
    }
}
