//! Canonical frontend physical constants — the single source of truth.
//!
//! Add future frontend physical constants here (energy/length/etc. conversion
//! factors) so a value is defined exactly once and every consumer imports it,
//! rather than each module carrying its own copy that can silently drift.
//! (The Rust readers keep their own named factors at the parse boundary — this
//! home is frontend-only, by design.)

/** 1 Hartree = 627.509… kcal/mol — the named factor (CODATA). */
export const HARTREE_TO_KCAL = 627.5094740631;
