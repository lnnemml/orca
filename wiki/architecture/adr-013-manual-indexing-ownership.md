# ADR-013: Manual indexing ownership

**Status:** accepted · 2026-07-31
**Narrows:** [ADR-006](adr-006-manual-integration.md) (the *what* — a local FTS5 index of the ORCA
manual — is unchanged; this ADR fixes the *who* and the *how*). ADR-006 is **not edited** — the same
precedent as ADR-012 narrowing ADR-002: history is superseded by a new ADR, never rewritten.

## Context

ADR-006 sketched Phase 4 as "a one-off **sidecar** pipeline: fetch HTML docs → split into sections →
SQLite FTS5", plus a hand-curated `keywords.json` and an optional "Explain with Claude" layer. Three
of those specifics were written before ADR-009 (external-process spawning is Rust's), ADR-012 (the
authoritative text-to-structure tier moved out of the sidecar), and before the ORCA 6.1 manual's
actual publishing format was measured. Settling them now — *before* Phase 4 starts — keeps the phase
from re-deciding them ad hoc, possibly differently.

## Decision

### (1) Only Rust writes `orcastudio.db`; the sidecar does not index

The manual index tables and their FTS5 virtual table are created and populated by **Rust**, over the
same `Connection` that owns every other table. The sidecar gets no indexing endpoint and no write
path to the database.

**Why — the sidecar's own invariant forbids it.** `modules/sidecar.md` states the sidecar contract as
"**Stateless. All persistence lives in SQLite owned by Rust**". An indexer living in the sidecar would
have to open and write `orcastudio.db`, breaking the sidecar's *own* stated boundary — this is not an
appeal to ADR-004 in the abstract, it is the sidecar contradicting itself.

**A second, independent argument — two different SQLite builds must not write one file.** The sidecar
links the **system** `libsqlite3` (whatever the OS ships); Rust links its **own** amalgamation
(SQLite **3.46.0**, compiled with `-DSQLITE_ENABLE_FTS5` unconditionally — measured, see
`db.rs::fts5_is_available_with_ranking_and_snippet`). Two distinct SQLite builds writing the same file
give two independent answers to "is FTS5 present here?" and "which tokenizers exist?" — the system
build may lack FTS5 entirely, or carry a different `unicode61`/ICU configuration. Whoever writes the
FTS5 table must be the build whose FTS5 guarantees we have tested. That is Rust.

### (2) The app never fetches the manual over the network

Fetching the ORCA documentation is a **standalone script**, run **by the author once per ORCA
version**, that deposits files into `resources/manual/` (RAW, immutable — see its README). It is
**not** a Tauri command and **not** a sidecar endpoint. The running application only ever reads the
already-present raw files.

**Why.** `overview.md` §"Security / privacy posture" already commits the app to **no network calls
except** (a) user-configured SSH and (b) the optional Anthropic "explain" feature with the user's own
key. An in-app manual fetcher would be a third network path, silently violating that posture. Keeping
fetch in an out-of-band author script preserves it: the shipped app is local-first, and the
copyrighted manual is never redistributed (it lives in `.gitignore`, ADR-006 consequence).

### (3) Sectioning is done in Rust (ADR-012's rule), over Markdown, not HTML

Splitting the fetched docs into indexable sections — `(id, version, breadcrumb, title, body_md,
input_examples, source_anchor)` rows — is **text-to-structure without a chemistry library**, which by
ADR-012 belongs in **Rust**, not the sidecar. The measurement that pins *what* to parse:

The ORCA 6.1 manual is **Sphinx + MyST** built with `html_copy_source`, so the original Markdown
sources are published alongside the HTML at `_sources/<path>.md.txt`. Sectioning therefore reads
**Markdown with ATX (`#`/`##`/`###`) headings**, not rendered HTML. **No HTML parser is needed in any
language** — the heading tree comes straight from the ATX levels.

**Review condition (named so it is not silently overridden).** If the Phase-4.1 gate finds that the
real `_sources/*.md.txt` files require a *true* MyST parser (roles, directives, cross-references that
carry section-structural meaning ATX headings alone miss), then decision **(3)** is reopened and
reconsidered. Decisions **(1)** and **(2)** do **not** depend on it and stand regardless.

### keywords.json — seeded from the manual, curated on top (narrows ADR-006)

ADR-006 says `keywords.json` is "curated by hand". The manual itself contains **dozens of native
"Keywords" sections** plus a `genindex`, which are a far better starting corpus than a blank file.
ADR-013 narrows this: `keywords.json` is **seeded programmatically from those manual sections and the
index**, then **curated on top** (summaries written by hand, entries pruned/merged). It is still our
own file living in the repo — but written by *seed-then-curate*, not typed from scratch.

## Consequences

- `modules/sidecar.md` — the planned `POST /manual/build-index` and `GET /manual/search` endpoints are
  marked **REJECTED (ADR-013)**, in the same style already used for `POST /parse` (ADR-012). The
  sidecar is **not involved in Phase 4** at all.
- `modules/manual-index.md` — the pipeline is rewritten as a **Rust** pipeline over `resources/manual/`
  Markdown sources; fetch is an out-of-band per-version author script.
- `resources/manual/README.md` — the "Refreshed per ORCA version via `sidecar` indexing pipeline" line
  is corrected: refresh is the author-run fetch **script**, and indexing is **Rust**.
- ADR-006 is unchanged (superseded-by-narrowing, not edited).
- No new dependency in `Cargo.toml` or `requirements.txt`: FTS5 is already in the bundled SQLite
  (measured), ATX-heading parsing needs no library.

## Not decided here (Phase 4.1+ owns them)

The concrete `manual_sections` schema, the FTS5 table shape, the fetch script itself, and the Monaco
hover-provider wiring are Phase-4 work with their own gate. ADR-013 fixes only **ownership** and the
**source format**, so Phase 4 does not re-litigate them.

## Amendment (2026-07-31, unit 4.1) — keyword markup is heterogeneous; the seeder needs two extractors

The decision text above **stands unchanged**; this amendment records a measurement made after it
(precedent: ADR-012's unit-3.3 amendment). Unit 4.1 built `scripts/fetch-manual.py` and fetched the
full ORCA 6.1 manual (126 leaf pages; full numbers in [`orca/manual-sources.md`](../orca/manual-sources.md)).
It confirmed the format decisions — ATX-only sectioning is sufficient (**body `{eval-rst}` = 0 across
all 126**, so the (3) review condition is **closed**, not merely "not yet triggered") — and refined
the **`keywords.json` seed paragraph** with one measured fact:

**The manual's keyword lists are not one uniform format. They come in two markups, so seeding needs
two extractors, not one:**

- **(a) A `:::{table}` MyST directive over a GFM pipe table** — e.g. `RI.md` `## Keywords`: column 1 =
  keyword (backtick-wrapped, sometimes comma-separated aliases), column 2 = description. Two fields.
- **(b) An annotated ` ```orca ` code block** in `name value # description` form — e.g.
  `solvationmodels.md` "Complete Keyword List for the `%cpcm` Block". **Form (b) is the richer
  source**: it yields keyword **+ default value + description** in one line, where the table gives only
  keyword + description. A seeder should prefer (b) where both exist.

This **refines, does not cancel**, the ADR-013 seed-then-curate paragraph: `keywords.json` is still
seeded programmatically from the manual and curated on top — but the seeder is `{table}` extractor
**plus** `%block` code-block extractor, not a single table reader. This is a *seeder* concern, separate
from sectioning, so it does not touch decisions (1)/(2)/(3).

One more construction fixed by the run (rule #9): the label→anchor map is `objects.inv` (authoritative,
exists — 46 257 B) **cross-checked by** an independent `predict_anchor` slug rule asserted on every
label (all 1448 corpus labels ASCII; 46/46 matched real HTML ids in the sample). Two derivations that
must agree — the next unit builds it that way, not "one or the other". `objects.inv` is **not parsed**
in this unit.

### Correction to the amendment (2026-07-31, unit-4.1 recount)

Two numbers in the amendment above came from the **6-file sample** and from a heading count that was
later found **polluted** (`analyze_atx` matched `#` on lines inside ` ```orca ` code blocks, and ORCA
input comments with `#` — 464 phantom level-1 "headings"; fixed to count only outside fenced blocks).
A fence-aware recount over **all 126 leaves** revises the keyword finding:

- **Not two forms — three, plus a prose tail.** Over 79 "Keyword" headings: ` ```orca ` code block
  **33**, GFM pipe table **27**, `{list-table}` **1** (a third structured markup), **prose 21**. So the
  seeder needs **three** structured extractors, and ~27% of keyword sections (prose) fall to
  hand-curation. The seed-then-curate decision still **holds** — the majority is seedable — but the
  "two extractors" figure was a sample undercount; full table + method in
  [`orca/manual-sources.md`](../orca/manual-sources.md).

Decisions (1)/(2)/(3) are untouched: this is a *seeder* refinement. **Body `{eval-rst}` = 0** still
stands (it is counted via `parse_toctrees`, which always tracked fences correctly — it was never the
buggy path), so (3) stays closed.
