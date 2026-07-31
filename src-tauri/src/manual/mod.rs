//! ORCA manual sectioning + anchor map (Phase 4.2, ADR-013 (3): Rust, over the raw
//! Markdown `_sources/*.md.txt`, no HTML parser). This unit produces [`Section`]s
//! and reads the authoritative [`objects_inv`] map; it writes **nothing** to the
//! database — the FTS schema and storage are unit 4.3.
//!
//! Three post-conditions carry it (rule #9), all as code, not just tests:
//!   1. **line conservation** — inside [`sections::sectionize`]: every line of every
//!      file belongs to exactly one section (or the preamble). This is the manual's
//!      analogue of Phase 3's atom-count-and-order round-trip invariant.
//!   2. **anchors** — `objects.inv` is authoritative, `predict_anchor` an independent
//!      check asserted on every label; mismatches are named, not silently resolved.
//!   3. **label binding** — a label we bind to section S in file F must have an
//!      `objects.inv` uri pointing at F and an anchor matching S.
//!
//! Post-conditions 2–3 need the real corpus + `objects.inv`; they run in the
//! `#[ignore]` corpus gate below (precedent: `orca_plot.rs`):
//! `cargo test manual_corpus -- --ignored --nocapture`.
#![allow(dead_code)] // scaffolding consumed by unit 4.3 (storage) — not wired to a command yet.

pub mod index;
pub mod objects_inv;
pub mod projection;
pub mod sections;

#[cfg(test)]
mod tests;
