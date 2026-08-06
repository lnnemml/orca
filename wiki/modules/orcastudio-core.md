# Module: orcastudio-core (`orcastudio-core/`)

**Status:** created in Phase 4.2 Stage 1 unit **1c** (ADR-016); **wired into the parsers in
1d** — `src-tauri` now depends on this crate and the ADR-012 readers take its
`IndexMap<OrcaIndex>` in `verify()` (see `modules/artifact-readers.md`). Holds the identity
types + the order-bearing `emit_input`. Unit 1e still owes: `create_job` mints the map and the
xtb serde boundary is branded.

A **separate crate** in the cargo workspace (root `Cargo.toml`, members `src-tauri` +
`orcastudio-core`) on purpose: Stage 2 compiles it to **WASM** for the renderer, so it
is kept **std-only + serde** — no `tauri`, no `tokio`, nothing that would block that
target (ADR-016).

## Why it exists

ADR-016: the ADR-010 `emit_input` / `parse_output` pair is a type-level invariant, but
`emit_input` was TS and `parse_output` is immovable Rust (ADR-012). A type invariant
across a language boundary is a convention with a compiler on one side, so the
**order-bearing** emit moves to Rust here. **Only** the emit that depends on atom order
moves (coordinate block + `%geom Constraints`); splicing a block into existing input
text does not (it carries no order) and stays in TS (`injectSceneIntoInput`).

## Files

- `src/ids.rs` — `AtomId(u32)` (identity, not a position), `OrcaIndex(u32)` /
  `AseIndex(u32)` (positions in two emitted orders), the `SpaceIndex` trait, and
  `IndexMap<T>` (`AtomId ↔ T`). **Mixing the newtypes does not compile** — two
  `compile_fail` doctests are the cheap proof. `IndexMap::from_emit_order(&[AtomId])`
  builds forward map + reverse vector from **one** source, so they cannot disagree
  (no constructor takes a separately-supplied, possibly-inconsistent pair).
- `src/scene.rs` — `deserialize_scene(&str) -> Result<Scene, CoreError>`: the **v2**
  `scene_json` with the SAME validation as the TS `deserializeScene` (ids unique,
  non-negative, `< nextAtomId`; malformed → `Err`, never panic). **v1 is NOT migrated
  here — a decision:** migration has one home, TS at DB-read time (mirrors ADR-010
  correction (ii)); `create_job` always hands this core fresh v2, so a v1 string is a
  **caller bug** and gets a loud, named `UnsupportedSceneVersion` (version + where
  migration lives), never a silent second migration.
- `src/emit.rs` — `emit_coordinate_block(&Scene) -> (String, IndexMap<OrcaIndex>)` and
  `emit_constraints_block(&[Constraint]) -> Result<String, CoreError>`, each
  **byte-identical** to its TS source. Plus the two measured number formatters
  (`fmt_coord`, `fmt_value`) and the `value_text_for` canonicality helper. Own
  `Constraint` type **with `value_text`** — deliberately NOT unified with
  `src-tauri/src/xtb.rs::Constraint` (that, and branding the xtb serde boundary, is
  unit 1e; a `TODO(1e)` marks it).
- `tests/golden.rs` — byte-identity vs the committed TS-emitted fixtures
  (`tests/fixtures/`), the IndexMap↔rows coupling, and the measured-value round-trip.
- `tests/roundtrip.rs` (unit 1d) — the **typed, in-process half** of the ADR-010 invariant:
  for 2000 seeded random scenes, `set(AtomId) → emit_coordinate_block → parse_coordinate_rows →
  re-key by AtomId through the map` is **identity** (element exact, coords to 1e-9) and the map is a
  bijection over every atom. Deterministic (a seeded splitmix64 — no `rand`/`Math.random`, which the
  runtime forbids and which would make a failure irreproducible); coords on the `k/8` lattice so
  formatting is lossless (float parity is the probe's job, not this test's). `emit::parse_coordinate_rows`
  is the crate-local inverse of the coordinate-block emit that closes this pair **inside the crate** —
  the compiler-visible half; the src-tauri readers are the *verified-at-the-persistence-boundary* half.
- `tests/parity_gate.rs` — the permanent `#[ignore]` `fmt_coord` gate over the full
  ~1M-double corpus (needs Node to regenerate the corpus; see below).
- `src/lib.rs` — `CoreError` (all named, loud; CLAUDE.md rule #9).

## The two number formatters (measured — see `architecture/float-formatting-parity.md`)

Byte-identity with TS requires reproducing two JS formatters exactly:

- **`fmt_coord`** = JS `toFixed(8).padStart(14)`. JS rounds half-**away**, Rust `{:.8}`
  half-to-**even**, so at the true 8th-decimal ties (`x = odd/512`) they diverge;
  `fmt_coord` renders those away-from-zero, and maps `-0.0 → +0.0` (JS drops the sign).
  0 divergences over the corpus; the bare formatter had 2025 (the negative control).
- **`fmt_value`** = JS `String(v)`. `format!("{}")` matches except on 17-significant-
  digit shortest-round-trip ties; those are **absorbed by construction** — the value
  model carries `value_text` (the user's exact token, judged canonical by each
  emitter's own render), and a programmatic ≥17-digit value is **refused loudly**.

## Build note (the workspace risk, named)

A cargo workspace shares one target dir at the repo root (`./target`), not
`src-tauri/target`. Verified headless: `cargo build --workspace`, `cargo test
--workspace` (src-tauri's 160 tests + core's, all green). **Not** verifiable headless:
`npm run tauri dev` / `npm run tauri build` (the desktop window + bundling) — Tauri
resolves the artifact via cargo metadata (workspace-aware), but that path is for the
author to confirm in the window.
