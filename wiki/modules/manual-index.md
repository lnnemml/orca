# Module: Manual index (Phase 4)

**Status:** not started · Strategy in [ADR-006](../architecture/adr-006-manual-integration.md),
ownership + source format narrowed by [ADR-013](../architecture/adr-013-manual-indexing-ownership.md)
(**Rust**, over raw Markdown, writing the Rust-owned SQLite — not the sidecar).

## Pipeline (Rust, one-off per ORCA version)
Fetch is out-of-band (see below); indexing runs in Rust:
raw Markdown (local `resources/manual/<orca-version>/<path>.md.txt`, mirroring the manifest path —
the remote `_sources/` URL prefix is a Sphinx artifact, dropped locally; ATX headings) → section
splitter (heading tree, no HTML parser) → rows
`(id, version, breadcrumb, title, body_md, input_examples, source_anchor)` → SQLite FTS5 table on the
**Rust-owned `Connection`**. The bundled SQLite (3.46.0) carries FTS5 unconditionally — gated by
`db.rs::fts5_is_available_with_ranking_and_snippet`, so a future rusqlite bump that drops FTS5 fails
in that test, not here.

**Why Rust, not the sidecar:** ADR-012's rule (text-to-structure without a chemistry library → Rust)
plus the sidecar's own "stateless, all persistence is Rust-owned SQLite" invariant, plus the two-
SQLite-builds hazard (system libsqlite3 vs Rust's amalgamation). Full argument in ADR-013.

**Source format (measured — see [orca/manual-sources.md](../orca/manual-sources.md)):** the ORCA 6.1
manual is Sphinx + MyST built with `html_copy_source`, so the original Markdown ships at
`_sources/<path>.md.txt`. Sectioning reads ATX (`#`/`##`/`###`) headings — **no HTML parser in any
language**; measured 2026-07-31, body `eval-rst` = 0 in the sample. Review condition (ADR-013 (3)): if
the Phase-4.1 gate shows the real files need a true MyST parser, that decision reopens; ownership
(Rust) and the no-network rule do not. Note (also measured): the "Keywords" markup is **heterogeneous**
(`:::{table}` pipe tables vs annotated ` ```orca ` blocks), so the seeder needs two extractors; and
`objects.inv` exists as the authoritative label→anchor map, so anchors need not be re-slugified.

## Fetch — out-of-band, not in the app
The manual is fetched by `scripts/fetch-manual.py` — a **standalone author script, run once per ORCA
version** (`--manifest` builds a deterministic toctree walk, never a crawl; `--all` fetches). It writes
`resources/manual/<orca-version>/<path>.md.txt` (RAW, immutable; the remote `_sources/` prefix is
dropped locally) plus `resources/manual/manifest.json` (path/URL/status/size/sha256/ETag per file, so a
6.2 refresh is a diff, not a blind re-download). The running app **never** pulls the manual over the
network (ADR-013 (2), preserving `overview.md`'s no-extra-network posture). Not a Tauri command, not a
sidecar endpoint. Standard library only — no new dependency.

## keywords.json
Curated map: `{ "RIJCOSX": { "summary": "...", "section_id": ... }, ... }`
**Seeded then curated** (ADR-013 narrows ADR-006's "curated by hand"): the manual has dozens of native
"Keywords" sections plus a `genindex` — seed `keywords.json` programmatically from those, then curate
on top (hand-written summaries, pruned/merged entries). Seed order: everything used by the Phase 1
template library → solvation → job types → %blocks. Lives in repo (our own writing, not manual
content).

## UI integration
- Search panel: FTS query, snippet highlighting, section rendering (markdown).
- Monaco hover provider: tokenize `!` line and `%block` names → keywords.json lookup.
- "Explain with Claude" (optional): POST keyword + current .inp + manual excerpt to
  Anthropic API with the user's key from settings (the one sanctioned extra network path).
