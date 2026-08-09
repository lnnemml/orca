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

/// One reference job of a reaction (schema v14, Phase 4.5 Stage C2b-2a, ADR-018).
///
/// A reaction's **reactant reference** is a list of these — optimized-reactant jobs
/// whose parsed final energies SUM to `E(ref)` for absolute barriers. `title` names
/// the job in the UI; `final_energy_eh` is read from the authoritative `results` tier
/// (ADR-012) at read time and is **`None` when the job is unparsed / still running /
/// failed** — the "honest-or-absent" signal a caller uses to know the reference is
/// incomplete (see [`ReferenceEnergy`]). The energy is NEVER cached on the reaction
/// (ADR-018: one source of truth, no drift).
#[derive(Debug, Clone, Serialize)]
pub struct ReferenceJob {
    pub job_id: String,
    pub title: String,
    pub final_energy_eh: Option<f64>,
    /// The reference job's parsed Gibbs free energy G (Eh), read from the authoritative
    /// `results` tier (ADR-012) at read time. **`None` unless the job ran `Freq` and its
    /// thermochemistry parsed** — the honest-or-absent signal for ΔG‡ (Stage E1b): a
    /// reference without G means the reaction has no complete ΣG(ref), so ΔG‡ must be
    /// refused, never partially summed. Never cached (ADR-018: one source of truth).
    pub free_energy_g_eh: Option<f64>,
}

/// A reaction's summed reactant reference: the provenance (`jobs`) AND the honest
/// total (`energy_eh`). `energy_eh` is `Some(Σ final_energy_eh)` **only if the list is
/// non-empty and every job has a parsed final energy**; otherwise `None` (incomplete
/// or empty). A partial sum is never returned — a wrong `E(ref)` would silently poison
/// every absolute barrier built on it (ADR-018). The UI (C2b-2b) shows `jobs` so the
/// user can see which reference job is missing its energy.
#[derive(Debug, Clone, Serialize)]
pub struct ReferenceEnergy {
    pub jobs: Vec<ReferenceJob>,
    pub energy_eh: Option<f64>,
    /// Σ Gibbs free energy G (Eh) of the reference jobs, on the SAME honest-or-absent
    /// discipline as `energy_eh` but over `free_energy_g_eh`: `Some(ΣG)` **only if the list
    /// is non-empty and EVERY reference job has a parsed G** (i.e. every one ran Freq);
    /// otherwise `None`. A partial ΣG (some references without Freq) is NEVER returned — it
    /// would look like a valid ΔG‡ denominator while being wrong (Stage E1b, ADR-018).
    pub free_energy_g_eh: Option<f64>,
}
