# Module: Manual index (Phase 4)

**Status:** built (fetch 4.1, sectioner 4.2, **schema + ingest + search 4.3**) · Strategy in
[ADR-006](../architecture/adr-006-manual-integration.md), ownership + source format narrowed by
[ADR-013](../architecture/adr-013-manual-indexing-ownership.md) (**Rust**, over raw Markdown, writing
the Rust-owned SQLite — not the sidecar). No UI yet — the panel + Monaco hover are 4.4.

## Pipeline (Rust, one-off per ORCA version)
Fetch is out-of-band (see below); indexing runs in Rust:
raw Markdown (local `resources/manual/<orca-version>/<path>.md.txt`) → **section splitter**
(`src-tauri/src/manual/sections.rs`, fence-aware, non-nested bodies, line-conservation; unit 4.2,
[manual-sections.md](manual-sections.md)) → **anchor resolve** against `objects.inv`
(`objects_inv.rs`) → **`manual_sections` + external-content `manual_fts`** (unit 4.3,
`manual/index.rs`, schema v9 in `db.rs::create_manual_tables`). The bundled SQLite (3.46.0) carries
FTS5 unconditionally — gated by `db.rs::fts5_is_available_with_ranking_and_snippet`.

## Schema (v9) — `manual_sections` + `manual_fts` + `manual_provenance`

`manual_sections`: `id` (**synthetic** INTEGER PK — neither `anchor` nor `(file, title_slug)` is
unique: 140 unlabelled sections collide on title-slug within one file), `orca_version`, `file`,
`level`, `title`, `breadcrumb` (JSON), `labels` (JSON), **`anchor` (NULLABLE)** + **`anchor_source`**,
`body_md`, `line_start`, `line_end`. Index on `(orca_version, file)`.

`manual_fts` is **external-content** FTS5 (`content='manual_sections'`, `content_rowid='id'`) over
`title, breadcrumb, body_md` — it stores the search index but reads column values from the base table,
so **the 4 MB of body is not duplicated**. Rebuilt wholesale on each ingest
(`INSERT INTO manual_fts(manual_fts) VALUES('rebuild')`). Ranking is `bm25` **ASC** (less = more
relevant), title-weighted 10/5/1.

`manual_provenance` (one row per `orca_version`): `base_url`, `corpus_collected_at`, `corpus_hash`
(deterministic content hash), `sectioner_version`, `section_count`, `anchors_verified`, `indexed_at` —
the same role as `results.parser_version`, so a 6.2 refresh is a decidable diff.

### Column choice — decided by the retrieval gate, not taste
The 4.2 gate flagged that 42.7 % of corpus bytes are inside fenced blocks, so the FTS column was an
open question: (A) raw `body_md` vs (B) a cleaned projection (`projection.rs`: strip MyST/LaTeX, keep
` ```orca ` blocks). The `retrieval_gate` (`cargo test retrieval_gate -- --ignored`) built both over
the real corpus and measured 17 queries (two are ROADMAP Phase-4 acceptance criteria). Result:
**A hit@5 15/17 (88 %), B hit@5 16/17 (94 %)** — B's only edge is GOAT, within the ±1-query noise band.
Per the tie-break, **A (raw `body_md`) is chosen** — simpler, and it is what makes external-content
(no duplication) possible. B's projection code stays for the gate but is not in the ingest path. See
[orca/manual-sources.md](../orca/manual-sources.md) "Retrieval gate".

### Two surfaces, two precision bars — why the hover provider does NOT read FTS
The same gate measured both ranks, and they diverge: **A hit@1 = 9/17 (~53 %), hit@5 = 15/17 (88 %)**
(both fractions from the gate, not rounded percentages). The right section is almost always *in* the
result set, but in about half the queries it is **not first**. **The tolerable imprecision depends on
how many results the surface shows.** The **search panel** renders five candidates, so hit@5 88 %
is fully workable there — the chemist recognises the one they wanted among the five. The **Monaco
hover** shows **one** section, and shows it confidently, with no visible sign it might be a guess —
at hit@1 ~53 % it would surface the wrong section on roughly half of all hovers.

**Therefore the hover provider is NOT fed by FTS search.** Its source is `keywords.json`, where a
keyword is mapped to a section **explicitly**; FTS stays for the panel. This is not taste — the number
(hit@1 9/17) is the reason `keywords.json` exists as a separate layer rather than a convenience.

### Three anchor populations — why `anchor` is NULLABLE (rule #11)
Of 1586 sections: **1068 have a verified anchor** (their closest label is in `objects.inv` with a
matching file + slug → `anchor_source='objects_inv'`). The rest are `NULL` / `undetermined`:
`objects.inv` carries only **explicit** labels, so **517 unlabelled** sections cannot be checked at all,
and Sphinx auto-generates their ids from the title slug with **traversal-state suffixes we cannot
recompute** — and ~140 of them collide on that slug within a file. A guessed anchor points at a
fragment that does not exist and reads as "the manual moved"; UNDETERMINED (NULL) is honest, and the
link lands on the page without a fragment. (`sec:spectroscopyproperties.nocv.theory` is the **1** named
label Sphinx did not register — the lone real gap.)

## Ingest + search (`manual/index.rs`)
- **`build_manual_index(version?)`** command (author-run; no UI) → `build_index`: sectionise the
  corpus, resolve anchors, write, **idempotently** (replace that version's rows). Content-preserving
  post-conditions run **inside the transaction** (rule #9), so a lossy ingest rolls back: row count ==
  sections; **every `body_md` reads back byte-for-byte** (subsumes a byte-sum check — catches silent
  truncation); byte total matches; FTS rows == table rows; NULL-anchor count == `section_count −
  verified`. Measured ingest: 1586 sections, 1068 verified, 518 NULL, 4 025 114 body bytes.
- **`search_manual(query, limit?)`** command → `Vec<ManualHit { id, file, breadcrumb, title, anchor,
  snippet, rank }>`. `snippet()` from FTS5, `ORDER BY bm25` ASC. **Empty query → empty result** (not an
  error — the `output_search` contract). The MATCH builder (`to_fts_match`) is the ONE shared with the
  gate, so the gate predicts production.

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
