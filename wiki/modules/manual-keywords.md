# Module: keywords.json (`src/manual/keywords.json`)

**Status:** seeded (unit 4.4, Part B). The keyword→section map that feeds the Monaco **hover**
provider (4.4 UI, not built yet). It is a **repo file, seeded programmatically then curated by hand**
(ADR-013 narrows ADR-006's "by hand") — Rust owns manual text-to-structure (ADR-013), so a Rust
`#[ignore]` generator emits it; the frontend consumes it. **Why a separate layer at all:** the hover
shows **one** section, confidently; at the FTS panel's hit@1 = 9/17 (~53 %, ADR-013 amendment) a
search-fed hover would be wrong half the time. So hover reads an **explicit** map, not FTS.

## Why this file exists as data, not FTS

Two surfaces, two precision bars ([manual-index.md](manual-index.md)): the **panel** shows 5
candidates (FTS hit@5 88 % works), the **hover** shows 1 (needs an explicit map). This file is that
map. FTS stays for the panel.

## Schema (one record per keyword)

```jsonc
{
  "keyword": "RIJCOSX",             // the token as it appears (case preserved)
  "type": "simple" | "block" | "block-option",   // seed heuristic; curation refines
  "provenance": "seeded" | "curated",
  "aliases": ["M06-L"],             // optional — spelling variants (NOT hyphen-normalized)
  "summary": "…",                   // optional — empty on seed; curation fills
  "section": { "file": "...", "breadcrumb": [...], "title": "...", "nth": 0 }
  //  ── OR, when the keyword documents in more than one place ──
  "targets": [ { "file": ..., "breadcrumb": [...], "title": ..., "nth": 0 }, ... ]
}
```

A record has **either `section` (unambiguous) or `targets[]` (ambiguous)** — never both.

## The stable key — `(file, breadcrumb, title, nth)`, and why

`keywords.json` must survive **re-ingest of the same corpus**, so it **cannot** key on
`manual_sections.id` — the synthetic PK is reassigned on every ingest and would silently slide a
curated entry onto another section. The other candidates were measured out (unit 4.4,
[orca/manual-sources.md](../orca/manual-sources.md)): `anchor` is NULL in 518 sections;
`(file, title)` collides 140× (label-less `## Keywords`); and even **`(file, breadcrumb, title)`
collides once** — `modelchemistries/mreom` has **two identical `## Perturbative MR-EOM-CCPT` H2
siblings** under the same parent. So the key adds an **`nth` ordinal, used only where the triple
repeats** (document order; `0` for all but that one pair). `line_start` is deliberately **not** in the
key — it would churn the diff on every manual reflow, and the file is human-reviewed.

**Loader post-condition (rule #9), when the hover layer is built:** every key must resolve to
**exactly one** section; **0 or ≥2 is an error naming the key — never pick-first.** A key that resolves
to nothing (manual moved) or to two sections (bad key) must fail loudly, not silently point somewhere.

## Coverage — the number that gates the generator: 46/46

The seed is broad (the whole structured pool), but its **post-condition is narrow**: every keyword the
**app itself emits** into an input must resolve, or the generator **panics naming the misses**. The 46
come from the code, not memory (`src/input-builder/orca-options.ts`, `build-input.ts`,
`templates/orca-templates.ts`, `scene/constraints.ts`). Four needed help, and they are the curation
seed:
- **`M06-L` / `M06-2X`** — the manual spells them **`M06L` / `M062X`** (no hyphen), so the seeded
  `M06L`/`M062X` records carry `aliases: ["M06-L"]` / `["M06-2X"]`. **No hyphen normalization** — the
  dash is significant in `def2-SVP`, `NEB-TS`, `B3LYP-D4`.
- **`TightSCF` / `VeryTightSCF`** — documented only in **prose** (no keyword table), so they are
  **curated** entries pointing at `essentialelements/scf › Convergence Tolerances`.

## What was seeded, and what was deliberately left

Seeded from the **broad structured pool, home mappings only** — the token's documentation home is a
keyword table (`:::{table}` pipe / `{list-table}` / `:::{flat-table}`), an **annotated** ` ```orca `
keyword-list block (`name value # desc`), or a **section title that is itself a keyword** (`## RI-JK`,
`## RIJCOSX`, `## GOAT`). The functional table's input token lives in its **second** column
(`| M06-L {cite}\`m06l\` | \`M06L\` | … |`) — the extractor reads that column, and strips MyST role
backticks (`{cite}\`…\``) so it takes the keyword, not the citation key. The appendix (change log /
glossary) is excluded — its `## GOAT`-style change entries are not documentation.

Deliberately **not** seeded here (measured, deferred — not lost):
- **`{numref}`-target records** — a keyword whose only home is a *reference* to another section (the
  60 `%`-blocks in "List of Input Blocks", each pointing at its doc section via `{numref}`). Their
  precision was **not measured** (A4 = 100 % was for *home* mappings only), so resolving them is the
  next unit's measure, not a guess now.
- **prose keyword sections** — the ~21 prose "Keywords" sections (unit 4.1) are curation targets by
  definition, not extractor input.

## Ambiguity — measured 14.2 %, and why `targets[]` instead of a guess

One keyword string can document in more than one section (the mirror of the mreom key collision: there
one section had two keys, here one key has several sections). Measured: **370 of 2606 home-seed tokens
(14.2 %) map to ≥2 sections** — common block options like `MaxIter` (17), `%method` (16), `PrintLevel`
(15). Under the **30 % exit bar** (above which the record shape itself would be wrong), so generation
proceeded — but an ambiguous keyword gets **`targets[]` (all homes), not a single `section`**, and the
future hover **must not pick the first**. How to disambiguate (block context under the cursor, the
current selection, or showing several) is the **next unit's** decision, made on a number — this unit
only refuses to fake a resolution it has not measured.

## Size (measured)

**2608 records, ~1.05 MB** (390 ambiguous → `targets[]`; types: 2447 block-option, 121 simple, 40
block). Larger than the "small map" the strategy first imagined — the bulk is `block-option` seed with
empty summaries, i.e. **curation material**, not finished entries. Bundled with the frontend, parsed
once into a lookup; a map lookup, no FTS. If it needs trimming, that is a curation decision, recorded
here so it is a choice and not a surprise.

## Regenerating

```bash
# Rust generator (author-run, ADR-013) — rewrites src/manual/keywords.json from the corpus:
cargo test generate_keywords_json -- --ignored --nocapture
# the two measures behind it:
cargo test keyword_seed_measure   -- --ignored --nocapture   # A1 stable key, A2 sources, A3 coverage, A4 precision
cargo test keyword_seed_ambiguity -- --ignored --nocapture   # ambiguity % + the 30% exit gate
```

Deterministic (sorted by keyword, case-insensitive) so a re-seed is a readable diff. Curated entries
(hand summaries, aliases, prose homes) are preserved by re-running curation on top — the generator
writes the seed; curation is layered after. No DB schema, no migration: the file is small enough to
bundle and the hover does a map lookup, not a query.
