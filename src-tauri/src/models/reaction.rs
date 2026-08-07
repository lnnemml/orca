//! The `Reaction` and `Pathway` models (schema v13, Phase 4.5 Stage C1).
//!
//! A **Reaction** is the central intellectual object of the reaction workstation
//! (ADR-007): a named transformation the researcher is studying. A **Pathway** is
//! one approach geometry explored end-to-end under a reaction; it groups the jobs
//! that make up that approach.
//!
//! These are **grouping metadata** over jobs, not the work itself. A job carries a
//! nullable `pathway_id` ONLY — its reaction is derived by joining `pathways`
//! (normalized: no `reaction_id` on jobs, the deliberate deviation from ADR-007's
//! both-FKs sketch — one source of truth). A pathway stays lean: `{ id, reaction_id,
//! label }`. It does NOT store the scan coordinate, method, or profile — those live
//! in the attached job's input/results and are read there by Stage C2.

use rusqlite::Row;
use serde::Serialize;

/// A reaction the researcher is studying. Mirrors the `reactions` table.
#[derive(Debug, Clone, Serialize)]
pub struct Reaction {
    pub id: String,
    pub name: String,
    /// Free-text notes. Nullable (a reaction may be created with only a name).
    pub description: Option<String>,
    pub created_at: String,
}

impl Reaction {
    /// Column list used by every `SELECT` that hydrates a [`Reaction`]. The order
    /// here is the contract [`Reaction::from_row`] relies on.
    pub const COLUMNS: &'static str = "id, name, description, created_at";

    /// Build a [`Reaction`] from a row selected in [`Reaction::COLUMNS`] order.
    pub fn from_row(row: &Row) -> rusqlite::Result<Reaction> {
        Ok(Reaction {
            id: row.get(0)?,
            name: row.get(1)?,
            description: row.get(2)?,
            created_at: row.get(3)?,
        })
    }
}

/// One approach geometry under a reaction. Mirrors the `pathways` table. Lean by
/// design (invariant: one source of truth) — the coordinate/method/profile live in
/// the attached job, not here.
#[derive(Debug, Clone, Serialize)]
pub struct Pathway {
    pub id: String,
    pub reaction_id: String,
    pub label: String,
    pub created_at: String,
}

impl Pathway {
    /// Column list used by every `SELECT` that hydrates a [`Pathway`].
    pub const COLUMNS: &'static str = "id, reaction_id, label, created_at";

    /// Build a [`Pathway`] from a row selected in [`Pathway::COLUMNS`] order.
    pub fn from_row(row: &Row) -> rusqlite::Result<Pathway> {
        Ok(Pathway {
            id: row.get(0)?,
            reaction_id: row.get(1)?,
            label: row.get(2)?,
            created_at: row.get(3)?,
        })
    }
}
