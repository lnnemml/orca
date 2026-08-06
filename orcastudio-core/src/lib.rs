//! `orcastudio-core` — the identity + `emit_input` core (Phase 4.2 Stage 1, ADR-016).
//!
//! ADR-016 moves the **order-bearing** emit out of TypeScript into Rust so the
//! ADR-010 `emit_input` / `parse_output` pair becomes same-language and
//! compiler-enforced (`parse_output` is immovable Rust — ADR-012). This crate is
//! the Rust home of that emit. In Stage 1 it carries **only** the two emit paths
//! whose output depends on atom order (ADR-016 "Stage 1 moves ONLY the emit that
//! carries atom order"):
//!
//!   * the coordinate block (`emit::emit_coordinate_block`), and
//!   * the `%geom Constraints` block (`emit::emit_constraints_block`).
//!
//! **Dead code beyond its own tests, on purpose.** Nothing wires this crate into
//! the app in 1c — 1d pairs the parsers, 1e mints the map at `create_job` and
//! brands the xtb serde boundary. Splicing the coordinate block into existing input
//! text does NOT move (it carries no order); it stays in TS (`injectSceneIntoInput`).
//!
//! std-only + serde; no tauri/tokio, so it stays WASM-ready for Stage 2.

pub mod emit;
pub mod ids;
pub mod mint;
pub mod scene;

pub use mint::mint_index_map;

/// Errors from deserialization / emit. Named and loud — never a silent default
/// (CLAUDE.md rule #9: every boundary checks the result in our own terms).
#[derive(Debug, thiserror::Error, PartialEq)]
pub enum CoreError {
    #[error("scene_json is version {found}, but this core reads only v2 — v1 is migrated in TypeScript at DB-read time (deserializeScene), never here; create_job always emits a fresh v2 (ADR-016 / unit 1c decision)")]
    UnsupportedSceneVersion { found: i64 },

    #[error("scene_json is malformed: {0}")]
    MalformedScene(String),

    #[error("atom id {id} is >= nextAtomId {next} — the counter must be ahead of every live id (v2 invariant)")]
    AtomIdOutOfRange { id: u32, next: u32 },

    #[error("atom id {0} appears more than once — ids must be unique within a scene")]
    DuplicateAtomId(u32),

    #[error("refusing to emit a programmatic constraint value with {digits} significant digits ({value:?}); a value this precise cannot round-trip byte-identically between JS String() and Rust (unit 1c Part A2, measured threshold 17) — it must arrive as value_text (from parsing) or be a short canonical number")]
    NonCanonicalConstraintValue { value: String, digits: usize },
}
