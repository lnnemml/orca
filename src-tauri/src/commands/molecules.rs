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

/// All **library** molecules (role 0), newest first. Reagents (`is_reagent = 1`)
/// are deliberately excluded — they live in the reagent catalog (`list_reagents`),
/// not the molecule library. Existing rows are role 0, so this is unchanged for
/// pre-v12 data.
fn list_molecules_conn(conn: &Connection) -> Result<Vec<Molecule>, AppError> {
    let sql = format!(
        "SELECT {} FROM molecules WHERE is_reagent = 0 ORDER BY created_at DESC",
        Molecule::COLUMNS
    );
    let mut stmt = conn.prepare(&sql)?;
    let molecules = stmt
        .query_map([], Molecule::from_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(molecules)
}

/// The user reagent catalog: molecules with `is_reagent = 1`, newest first. The
/// sibling of `list_molecules_conn` over the same table, split by the role flag.
fn list_reagents_conn(conn: &Connection) -> Result<Vec<Molecule>, AppError> {
    let sql = format!(
        "SELECT {} FROM molecules WHERE is_reagent = 1 ORDER BY created_at DESC",
        Molecule::COLUMNS
    );
    let mut stmt = conn.prepare(&sql)?;
    let reagents = stmt
        .query_map([], Molecule::from_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(reagents)
}

/// Save a user reagent: a molecules row with `is_reagent = 1` and a **mandatory**
/// `charge` (ADR-014 — charge is never defaulted silently; the caller UI requires
/// it). Multiplicity is not asked (electron parity + charge determine it; the Scene
/// validates it) and defaults to 1; formula/tags are empty.
fn create_reagent_conn(
    conn: &Connection,
    name: &str,
    xyz: &str,
    charge: i32,
) -> Result<Molecule, AppError> {
    let id = Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO molecules (id, name, formula, xyz, charge, multiplicity, tags, is_reagent) \
         VALUES (?1, ?2, '', ?3, ?4, 1, '', 1)",
        params![id, name, xyz, charge],
    )?;
    get_molecule_conn(conn, &id)
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
pub fn list_reagents(db: State<'_, DbState>) -> Result<Vec<Molecule>, AppError> {
    let conn = db.lock()?;
    list_reagents_conn(&conn)
}

/// Save a user reagent. `charge` is required by the caller (ADR-014); this command
/// takes it as a plain `i32`, never an `Option` — there is no silent default.
#[tauri::command]
pub fn create_reagent(
    db: State<'_, DbState>,
    name: String,
    xyz: String,
    charge: i32,
) -> Result<Molecule, AppError> {
    let conn = db.lock()?;
    create_reagent_conn(&conn, &name, &xyz, charge)
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
    const NA_XYZ: &str = "1\nNa+\nNa 0.0 0.0 0.0\n";

    /// A unique throwaway data dir (its DB kept for a manual reopen — NOT wiped by
    /// `test_db`, which removes-then-inits). Returned so the caller can clean up.
    fn fresh_dir(tag: &str) -> std::path::PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "orcastudio-reagent-{}-{}-{}",
            tag,
            std::process::id(),
            n
        ));
        std::fs::remove_dir_all(&dir).ok();
        dir
    }

    // (c3) The role flag separates reagents from library molecules over ONE table:
    // a reagent carries its (mandatory) charge, does NOT leak into `list_molecules`,
    // and an existing molecule (role 0) is untouched. The bite: if the v12 migration
    // dropped the column or `from_row` didn't read it, `create_reagent_conn` would
    // error (no such column) or `is_reagent` would read false → the asserts go red.
    #[test]
    fn reagent_role_separates_from_the_molecule_library() {
        let (conn, dir) = test_db();

        let mol =
            create_molecule_conn(&conn, "ethanol", "C2H6O", ETHANOL_XYZ, 0, 1, "").unwrap();
        assert!(!mol.is_reagent, "a create_molecule row is NOT a reagent");

        let na = create_reagent_conn(&conn, "Na+", NA_XYZ, 1).unwrap();
        assert!(na.is_reagent, "a create_reagent row IS a reagent");
        assert_eq!(na.charge, 1, "the mandatory charge is stored, not defaulted");
        assert_eq!(na.multiplicity, 1);

        // list_molecules shows the molecule, NOT the reagent.
        let mols = list_molecules_conn(&conn).unwrap();
        assert_eq!(mols.len(), 1);
        assert_eq!(mols[0].id, mol.id);

        // list_reagents shows the reagent, NOT the molecule.
        let reags = list_reagents_conn(&conn).unwrap();
        assert_eq!(reags.len(), 1);
        assert_eq!(reags[0].id, na.id);
        assert_eq!(reags[0].charge, 1);
        assert!(reags[0].is_reagent);

        std::fs::remove_dir_all(&dir).ok();
    }

    // (c3, persistence) A reagent survives a DB reopen with charge + role intact —
    // the migration is idempotent and the row is durable (the m4 manual gate's core).
    #[test]
    fn reagent_persists_across_reopen() {
        let dir = fresh_dir("reopen");
        {
            let conn = init_db(&dir).expect("init_db");
            create_reagent_conn(&conn, "Mg2+", "1\nMg2+\nMg 0.0 0.0 0.0\n", 2).unwrap();
        } // conn dropped — the file remains

        let conn2 = init_db(&dir).expect("reopen init_db (idempotent migration)");
        let reags = list_reagents_conn(&conn2).unwrap();
        assert_eq!(reags.len(), 1);
        assert_eq!(reags[0].name, "Mg2+");
        assert_eq!(reags[0].charge, 2, "charge survived the reopen");
        assert!(reags[0].is_reagent, "role survived the reopen");

        std::fs::remove_dir_all(&dir).ok();
    }

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
