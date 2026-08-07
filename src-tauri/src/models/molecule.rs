//! The `Molecule` model.
//!
//! A molecule is a persistent, reusable structure in the library: a name, an
//! xyz geometry, and the ORCA-relevant `charge`/`multiplicity`, plus free-text
//! `tags` for loose grouping. Molecules are standalone for now — not linked to
//! `jobs` (that association arrives with Phase 4.5, reaction modeling).

use rusqlite::Row;
use serde::Serialize;

/// A single library molecule. Mirrors the `molecules` table one-to-one.
#[derive(Debug, Clone, Serialize)]
pub struct Molecule {
    pub id: String,
    pub name: String,
    /// Molecular formula (e.g. `C2H6O`). May be empty when imported from a bare
    /// `.xyz` without a formula.
    pub formula: String,
    /// Full standard xyz string: `count`, comment, then `element x y z` rows.
    pub xyz: String,
    pub charge: i32,
    pub multiplicity: i32,
    /// Comma-separated free-text tags (simple LIKE search later).
    pub tags: String,
    pub created_at: String,
    /// Role flag (schema v12): `true` = a user-saved **reagent** (shown in the
    /// reagent catalog's "My reagents", not the molecule library). Existing rows
    /// and molecules saved through `create_molecule` are `false`.
    pub is_reagent: bool,
}

impl Molecule {
    /// Column list used by every `SELECT` that hydrates a [`Molecule`]. The
    /// order here is the contract [`Molecule::from_row`] relies on.
    pub const COLUMNS: &'static str =
        "id, name, formula, xyz, charge, multiplicity, tags, created_at, is_reagent";

    /// Build a [`Molecule`] from a row selected in [`Molecule::COLUMNS`] order.
    pub fn from_row(row: &Row) -> rusqlite::Result<Molecule> {
        Ok(Molecule {
            id: row.get(0)?,
            name: row.get(1)?,
            formula: row.get(2)?,
            xyz: row.get(3)?,
            charge: row.get(4)?,
            multiplicity: row.get(5)?,
            tags: row.get(6)?,
            created_at: row.get(7)?,
            // INTEGER 0/1 → bool (rusqlite's FromSql for bool).
            is_reagent: row.get(8)?,
        })
    }
}
