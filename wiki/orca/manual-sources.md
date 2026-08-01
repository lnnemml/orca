# ORCA 6.1 manual — source format (measured)

**Measured 2026-07-31** by `scripts/fetch-manual.py`. Genre: like `parse-sources.md` — facts from a
real run, dated and versioned, not from docs or memory. Two gates fed this page: **Part A**
(`--manifest --sample 6`, 24 requests) established the format on a 6-file sample; **Part B** (`--all`,
137 requests) fetched the **full corpus of 126 leaves** and re-measured everything over all of it, so
the numbers below are corpus-wide unless a line says "sample". Where Part A gave an estimate, Part B's
exact figure replaces it (and the estimate is labelled as such).

**Update (unit 4.2):** the body **content analysis** — ATX sectioning, keyword-markup classification,
the anchor rule — moved out of this script into **Rust** (`src-tauri/src/manual/`, ADR-013 (3)). Those
numbers are now recomputed and verified over the whole corpus by the sectioner gate
(`cargo test manual_corpus -- --ignored`); see [`modules/manual-sections.md`](../modules/manual-sections.md)
and the "Sectioner gate" section below. The script keeps only fetch / manifest / toctree /
`objects.inv`.

## Where the sources live

- **Base:** `https://www.faccts.de/docs/orca/6.1/manual/`
- **Markdown sources:** `<base>_sources/<path>.md.txt` — the manual is **Sphinx + MyST** built with
  `html_copy_source`, so the original Markdown ships next to the HTML. Sectioning reads these, not
  the rendered HTML (ADR-013 (3)).
- **Rendered HTML (for anchors only):** `<base><path>.html`.
- **Local layout** this project writes (Part B): `resources/manual/<orca-version>/<path>.md.txt`,
  mirroring the manifest path. The remote `_sources/` prefix is a Sphinx artifact and is **dropped
  locally**; `manifest.json` sits at `resources/manual/manifest.json`. `resources/manual/*` is
  gitignored except its README — the copyrighted manual is never committed/redistributed (ADR-006).

## The manifest is a deterministic toctree walk, NOT a crawl

Start at `_sources/index.md.txt`, extract its `{toctree}` entries, recurse into container pages, and
build **every URL only from manifest paths**. Links found in document *bodies* are never followed —
that is the guardrail that keeps the walk from degenerating into a crawl of all of faccts.de.

**Measured graph (ORCA 6.1):**

| | count |
|---|---|
| total manifest paths | **140** |
| container pages (`index*`, carry a toctree) | **11** (all returned 200) |
| leaf source pages (`.md.txt`) | **126** |
| generated, **no `.md.txt`** (expected) | **3** — `bibliography`, `genindex`, `html_versions` |

The 3 no-source entries are Sphinx-generated pages; the script classifies them as
`no-source (expected)` and names them rather than silently dropping — a lost branch and a generated
page must not look alike.

### toctree parsing — the shapes that really occur

- **Fence styles both occur:** backtick (```` ```{toctree} ````) **and** colon (`:::{toctree}`),
  and colon fences come in varying lengths (`:::`, `::::`). The parser accepts any fence char run ≥3.
- **Option lines start with `:`** (`:maxdepth:`, `:hidden:`, `:caption:`, `:numbered:`) — skipped.
- **Entries** are bare paths or `Title <path>` — both handled.
- **The root also uses RST inside `{eval-rst}`:** a ```` ```{eval-rst} ```` block with `.. toctree::`
  holds `bibliography`/`genindex`. Miss this and you lose the back-matter — so the parser reads
  `.. toctree::` directives too.
- **Path normalization is load-bearing, not cosmetic.** The root `index.md.txt` has one toctree entry
  written with a **double slash**: `contents/structurereactivity//index_structurereactivity`. Naïve
  concatenation yields a `//` URL and can drop the entire **Structure and Reactivity** branch — the
  one carrying the project's whole research program (Opt/Scan/TS/IRC/NEB/GOAT). `posixpath.normpath`
  collapses it to `contents/structurereactivity/index_structurereactivity`. Confirmed: this is the
  only path the walk had to normalize, and it is exactly that branch.
- **Container detection:** container (toctree-bearing) pages are named `index*` in this manual; the
  walk recurses into those and treats everything else as a leaf. (A sample leaf carrying an unexpected
  toctree would be flagged — none did.)

## ATX headings (the sectioning unit)

Sections come straight from ATX (`#`/`##`/`###`…) heading levels — no HTML parser needed. **These
counts are fence-aware: only lines OUTSIDE fenced blocks are headings** (see the correction below).

- **Full corpus (all 126 leaves, fence-aware):** **1586 headings** — `#`=129, `##`=654, `###`=604,
  `####`=193, `#####`=6. **Deepest ATX level = `#####` (level 5)** (e.g.
  `contents/utilitiesvisualization/orca_2json`). No discrepancy with the TOC: it implied level-4
  (`####`) sections exist (2.6.7.1.1 scf.md, 2.7.2.13.1 basisset.md, 3.3.4.1.1 DFT, …) and the recount
  confirms `####` (193) — and one level deeper still (`#####`). Deep subsections **are** ATX, so
  ATX-only sectioning reaches them.
- **Correction (unit-4.1 recount).** An earlier count reported **2055** headings (`#`=593) — that was
  **polluted**: `analyze_atx` matched `#` on *every* line, including inside ` ```orca ` code blocks,
  and **ORCA input comments with `#`**, so comment lines were counted as headings. **464 of the 593
  level-1 "headings" were comments** (469 false across all levels; `##`/`###` lost 3/2, `####`/`#####`
  none — the deep levels were real). Fixed by counting only lines outside fenced blocks (one shared
  `iter_prose_lines` tracker; regression test `--selftest`); recount is offline (`--analyze-only`, no
  refetch).
- **One H1 per page — mostly.** 3 of 126 leaves carry **two** level-1 headings
  (`essentialelements/numericalintegration`, `preface/foreword`, `spectroscopyproperties/magnx`) —
  genuine double-top-heading pages (e.g. foreword has a 6.1.1 and a 6.1.0 foreword). A sectioner must
  not assume exactly one H1 per file.
- **Sample-only (Part A, superseded):** the 6-file sample showed a max of `###` — a sample artefact.
- **Exact size:** **4 084 799 bytes = 3.90 MiB** of Markdown over 126 leaves (mean ≈ 32.4 KB). Unchanged
  by the recount — the bug was heading-*counting* only, not the bytes. (The Part-A ~2.8 MiB was a
  low sample estimate: the three tiny `preface/*` leaves pulled the mean below corpus.)

> **Sectioner requirements (direct spec for the next unit, not an observation).** The ATX sectioner
> **MUST** support at least **5 heading levels** (`#`…`#####`), and **MUST** ignore any `#` line inside
> a fenced block — ` ``` ` code fences (ORCA examples comment with `#`) **and** `:::` directives. A
> naive line scan invents **~460 phantom sections** (measured). Track fences in **one** place, as
> `iter_prose_lines` does; do not re-derive the rule per call site.

## "Keywords" sections are HETEROGENEOUS — three structured forms plus a prose tail

This is the fact ADR-013's "seed `keywords.json` from the manual's Keywords sections" paragraph stands
on. Part A's 6-file sample suggested **two** forms; the full-corpus reclassification (`--analyze-only`,
all 126 leaves, **79 "Keyword" headings**) shows **more** — three machine-seedable markups *and* a
substantial prose remainder:

| form | count | seedable? |
|---|---|---|
| **annotated ` ```orca ` code block** (`name value # description`) | **33** | yes — richest |
| **GFM pipe table** (often wrapped in a `:::{table}` directive) | **27** | yes |
| **`{list-table}` directive** | **1** | yes |
| **prose** (unstructured text) | **21** | **no** — needs curation, not a parser |

*(counts sum to 82 over 79 headings — a few sections carry two forms.)*

- The ` ```orca ` code block is the **richest** source: each line yields keyword **+ default value +
  description** at once (e.g. `%cpcm` "Complete Keyword List"), where a pipe table gives only keyword +
  description (e.g. RI's `## Keywords`). A seeder should prefer it where both exist.
- **`{list-table}` is a third structured form** (1 occurrence) — a table variant, but a distinct MyST
  directive a pipe-table parser would miss.
- **21 of 79 keyword headings are prose** — no table, no code block. These are **not** mechanically
  seedable; they are curation targets, not extractor input.

**Consequence:** the "two extractors" figure from the sample was an **undercount**. A seeder needs
**three** structured extractors (` ```orca ` block, pipe table, `{list-table}`), and even then ~27% of
keyword sections (prose) fall to hand-curation. This still **refines, not cancels**, ADR-013's
seed-then-curate paragraph — the bulk is seedable — but "seed from the Keywords sections" is a
three-form job with a real curation tail, not a two-form one. (A *seeder* concern, separate from
sectioning — it does not touch the (3) MyST-parser review condition.) The `prose` bucket is measured,
not a hedge: `_keyword_forms` adds `prose` **only when no structured form is present** in the body, so
these 21 sections carry no in-body table or code block at all.

## Fence tracking: two scanners, one close-rule — and the H1 post-condition

- **Why two fence scanners, and why it is not debt.** `parse_toctrees` (in the fetch script) is an
  **allow-list**: it acts only *inside* `{toctree}`/`{eval-rst}` directives, so a bare ` ```orca `
  code fence can never inject a phantom entry into the manifest — it doesn't need to see code fences at
  all. The **deny-list** scanner — which must hide *every* fence so heading detection skips ORCA
  `#`-comments — is `sections::prose_mask` in **Rust** (unit 4.2; the Python `iter_prose_lines` was
  removed when body analysis moved to Rust). Opposite jobs, deliberately separate; the two live in two
  languages now, and that is correct — the toctree walk is the fetch script's, the sectioning is Rust's.
- **H1 identity as a post-condition (asserted on every `--analyze-only`).** Level-1 headings total
  **129 = 126 leaves + 3** pages that legitimately carry two top-headings
  (`essentialelements/numericalintegration` = sections 2.9 + 2.10; `preface/foreword` = 6.1.1 + 6.1.0;
  `spectroscopyproperties/magnx` = 5.27 + 5.28 — the pairs confirmed independently from the TOC). Every
  leaf contributes **≥ 1** H1, so the recount asserts **H1 ≥ 126** (0 leaves with zero H1, measured):
  if it ever drops below the leaf count, some file lost its top heading to an **unclosed fence** — the
  one way `iter_prose_lines` can silently swallow content. `--analyze-only` exits non-zero if it fails.

## Anchors: build the label→anchor map as a POST-CONDITION, not a guess

The section unit will need a `label → #anchor` map (every cross-reference in the body is a `(label)`).
How that map is built is **fixed here as a construction, not left as a later choice** (rule #9 — two
independent derivations that must agree):

- **Authoritative source: `objects.inv`.** `<base>objects.inv` **EXISTS** (**46 257 bytes**) — Sphinx's
  authoritative label→anchor (and label→title) inventory. It is the ground truth for anchors.
- **Independent check: `predict_anchor`, asserted on EVERY label.** The rule `(label)=` → `#slug` where
  `slug` = **lowercase, then every run of non-alphanumeric characters → a single `-`** (so `:`/`.`/`_`
  fold to `-`; `CPCM-features` → `cpcm-features`; holds for every prefix — `sec:`/`tab:`/`fig:`/`table:`,
  not just `sec:`). This is computed **independently** of `objects.inv` and asserted equal to it for
  **each** label; a mismatch fails loudly rather than silently emitting a dead anchor.
- **The two derivations are intentional (rule #9), not debt.** `predict_anchor` alone is a guess;
  `objects.inv` alone is an opaque binary; each guards the other. Verified so far: `predict_anchor`
  matched real HTML ids **46/46** in the Part-A sample, and **all 1448 labels across the full corpus
  are ASCII** (Part B) — so the `[^a-z0-9]+` → `-` transform loses nothing here.
- **`objects.inv` is NOT parsed in this unit** (ADR-013 scope is fetch + measure). Only the *intent* is
  recorded, so the next unit builds the map as `objects.inv` **cross-checked by** `predict_anchor`, and
  does not quietly pick just one.
- **Sphinx lowercases std-domain label names (measured, unit 4.2).** The names written into
  `objects.inv` are **case-folded**, while a source label keeps its case — and the ORCA manual is full
  of acronyms (RI, DFT, MP2, CASSCF, NEVPT2) and camelCase command labels (`BohrToAngs`, `closeFile`).
  A case-sensitive lookup therefore *missed* **125 of 1069** heading labels; folding case on both sides
  (one `normalize_label`, called for the map and every lookup) brought that to **1** — and that lone
  remainder (`sec:spectroscopyproperties.nocv.theory`) is a genuinely unregistered label, not a
  case difference. **Before/after: 944 → 1068 labels actually cross-checked** against the inventory
  (still 0 anchor and 0 binding mismatches). The 125 were never "missing data": the corpus has 1448
  `(…)=` targets and the inventory 1450 `std:label` entries — equal cardinality ruled out absence and
  pointed at key spelling, which the run confirmed.

## Sectioner gate (Rust, unit 4.2) — measured over all 126 leaves

`cargo test manual_corpus -- --ignored` runs the ATX sectioner + `objects.inv` cross-check over the
corpus and prints these (full detail + method in [`modules/manual-sections.md`](../modules/manual-sections.md)):

- **Sections: 1586** (`#`=129, `##`=654, `###`=604, `####`=193, `#####`=6 — identical to the fence-aware
  ATX recount, i.e. the Rust sectioner and the earlier Python count agree). Per file: **min 1 / median
  7 / max 162**; deepest breadcrumb **4**.
- **Body size:** median **1330 B** / p95 **9074 B** / max **48 245 B**; **27** empty-body (navigational)
  sections (1.7 %).
- **Labels:** **1069** sections labelled, **517** not; **140** unlabelled sections **collide on
  title-slug within one file** (many `## Keywords`) — an unlabelled title-slug is not a unique key.
- **Anchors/binding (post-conditions):** of 1069 heading labels, **1068 found in `objects.inv`, 0 anchor
  mismatches, 0 binding mismatches** — the rule holds **1068/1068 checked** (the gate prints the
  denominator, `0 out of 1068 checked; 1 unchecked`, so a check can't read PASS having examined
  nothing). The **1 not found** (`sec:spectroscopyproperties.nocv.theory`) is a genuine unregistered
  section label. See the Sphinx label-normalization note below — it is why this is 1068, not 944.
- **Line conservation: 126/126 files PASS** — every line owned by exactly one section or preamble.
- **Bytes: prose 57.3 % / fenced 42.7 %** — ~half the corpus is inside code/table fences; 4.3 decides
  raw-body vs cleaned-prose indexing.

## Retrieval gate (unit 4.3) — the FTS column chosen by number

`cargo test retrieval_gate -- --ignored` builds two FTS5 indexes over the sectioned corpus and measures
both with **17 fixed queries whose target sections were registered before measuring** (two are ROADMAP
Phase-4 acceptance criteria: "RIJCOSX explains what it is", "how do I set up CPCM for water"). The
variants: **(A)** raw `body_md`; **(B)** a cleaned projection (strip MyST directive/target syntax and
LaTeX, **keep ` ```orca ` code blocks** — an input line is often a better target than the prose; 42.7 %
of corpus bytes are inside fences). Same title-weighted `bm25` for both.

| variant | hit@1 | hit@5 |
|---|---|---|
| A — raw `body_md` | 9/17 | **15/17 (88 %)** |
| B — cleaned projection | 9/17 | **16/17 (94 %)** |

Both clear the 80 % hit@5 exit bar. B's *only* advantage is **GOAT** (A ranks the Compound-scripting
`goat` commands above the GOAT page) — a 1-query difference, within the noise band. Per the tie-break,
**A (raw `body_md`) is chosen**: simpler, and it is what lets the FTS be *external-content* (no 4 MB
duplication). Two honest notes: the `imaginary frequency` "miss" actually returned a relevant section
(`troubleshooting#Imaginary Frequencies after Optimization`) that was not in the pre-registered target
list — the goalpost was not moved; and hit@1 is only ~53 %, so the future exact-keyword layer
(`keywords.json` / hover) still has a job.

## Keyword-seed measurement (unit 4.4) — the stable key, and coverage of what the app emits

`cargo test keyword_seed_measure -- --ignored --nocapture` measures the three things that decide
whether `keywords.json` can be a *confident single-answer* hover (ADR-013's hit@1 = 9/17 finding): a
**stable key**, **coverage of what the app itself emits**, and a cheap **precision proxy** per entry.
It does **not** count "keywords extracted" — a map that points at the wrong section yet looks complete
is the hazard, so the numbers are about correctness, not volume.

**Stable key — `(file, breadcrumb, title)` is NOT unique (measured).** `manual_sections.id` is a
synthetic PK reassigned on every ingest, so a curated file cannot key on it (it would silently slide to
another section after re-indexing). Of the candidate keys, `anchor` is out (NULL in 518) and
`(file, title)` is out (140 within-file slug collisions, unit 4.2). The next candidate,
**`(file, breadcrumb, title)`, collides on exactly 1 pair**: `contents/modelchemistries/mreom` carries
**two identical `## Perturbative MR-EOM-CCPT` H2 headings** (lines 374 and 1572), same parent → same
breadcrumb → same triple. So the **chosen key is `(file, breadcrumb, title)` plus an optional `nth`
ordinal used only where the triple is ambiguous** (today: that one mreom pair). `line_start` is **not**
written into the curated file (it would churn the diff on every reflow). The **loader post-condition
(rule #9): every key must resolve to exactly one section; 0 or ≥2 is an error naming the key — never
pick-first.**

**Candidate sources (heuristic token extraction).**

| source | tokens | distinct |
|---|---|---|
| the 79 "Keyword…"-titled sections — ` ```orca ` blocks | 1052 | 847 |
| the 79 "Keyword…"-titled sections — pipe / list / flat tables | 863 | 747 |
| ↳ their union | | **1471** |
| **"List of Input Blocks"** flat-table (the single richest `%`-block source) | 64 | **64** |
| corpus-wide structured markup (maximal seed pool) | 11134 | **4247** |

Two hypotheses tested, both answered by number:
- **"List of Input Blocks" IS the richest single `%`-block source** — 60 rows → 64 names (4 aliases),
  each with a `{numref}` to its documentation section (105 refs). It carries `pal`, `geom`, `maxcore`
  — all three `%`-blocks the app emits.
- **"Simple Keyword Lines" is NOT a list of `!` keywords** — it is a flat-table *index* (25 topic
  rows, 35 `{numref}` to *other* tables like `tab:…dft.gga`). The individual simple keywords live in
  those referenced per-topic tables, not here.
- Incidentally `:::{flat-table}` is a **fourth** structured markup (36 files corpus-wide), beyond the
  three the 4.1 count found in the 79 keyword sections (` ```orca ` / `:::{table}` pipe / `{list-table}`).

**App coverage (the number that matters) — 42 of 46.** Every keyword the app itself writes into an
input (read from `src/input-builder/orca-options.ts`, `build-input.ts`, `templates/orca-templates.ts`,
`scene/constraints.ts` — not from memory) checked against the corpus pool. **4 are missing, and they
split into two distinct causes:**
- **`TightSCF`, `VeryTightSCF`** — present and backtick-wrapped (×5, ×2) but **only in prose**; there
  is no SCF-convergence keyword *table*. → curation targets (ADR-013's prose tail), not a corpus gap.
- **`M06-L`, `M06-2X`** — the manual spells them **`M06L` / `M062X` (no hyphen)** in the functional
  table, while the app emits the hyphenated form. → a **spelling divergence**, handled by an
  `aliases[]` field on the record, **not** by hyphen-normalization (dashes are significant elsewhere:
  `def2-SVP`, `NEB-TS`, `B3LYP-D4`).

One extractor lesson for the seeder: the DFT functional table (`DensityFunctionalTheory.md`) holds the
input token in its **second** column (`| M06-L {cite} | \`M06L\` | … |`), not the first cell — the
seeder must read the token column, not assume column 1.

**Precision proxy — 7574 / 7574 = 100 %.** Every token extracted from a section occurs literally in
that section's title or body (0 suspicious) — the extractor invents nothing. This validates only the
*home* mapping; the real risk is the `{numref}`-referenced target (target ≠ home), whose precision the
seeding unit measures directly.

### Part C — qualifying block-options and normalizing sections (before/after)

The first-cut `keywords.json` keyed block-options on the **bare option name**, which conflated two
faults into one file: **size** (each of 3173 target objects carried a full copy of its section
breadcrumb) and **"ambiguity"** (`MaxIter` looked 17-fold ambiguous when it is really 15 different
options of 15 different blocks). Both have one cause — the owner block, known at extraction, was
thrown away. Measured on the generated file and fixed:

- **Normalization.** 3173 target objects → **317 distinct sections** (10× duplication). Records now
  reference a `sections` array by integer. **1.00 MB → 0.56 MB** (dedup alone would reach ~0.25 MB;
  qualification trades part back — see below).
- **Owner, two independent signals.** `owner_source ∈ {text, structural, null}`. **Text** (a single
  literal `%block` token in the option's home section) takes priority; **structural** (unique `%block`
  of the file / unique deepest ancestor) fills; else **null**. Union coverage **74.7 %** (text 45.2 %
  + structural 29.5 %), null **25.3 %** — up from either signal alone (text 45 %, structural 62 %).
- **The load-bearing number — agreement.** Where both signals resolve (936 targets) they **agree
  98.5 %** (14 disagreements, 8 in one cross-reference section). So the structural 62 % recorded above
  is **independently validated**, not merely plausible — the same `objects.inv` × `predict_anchor`
  construction (two derivations that must agree), and it held again.
- **Ambiguity is now real.** Keyed on `(block, option)`, `MaxIter`'s 17 targets become **11 records**
  (one per owner block + one `null`); 320 word-level block-option "ambiguities" fall to ~201 **genuine
  multi-doc** cases (`%casscf MaxIter` truly in CASSCF and DMRG). The residual is not measured error —
  it is one option documented in several places, which `targets[]` states honestly.
- **Cross-reference sections → null by rule.** "List of related keywords" / "See also" sections list
  *other* blocks' keywords; both derivations there answer the wrong question. Measured: **2 sections**
  (`nocv`, `mcd`) — closed by rule, not by an accidental tie.

The "several `%`-tokens" refinement (a third rule for sections naming multiple blocks) was **dropped by
number**: it would rescue only 145 of 726 such targets (4.7 %) on a heuristic with nothing to check it
against — cost without a post-condition.

## MyST-parser review condition (ADR-013 (3)) — NOT triggered by the sample

The only thing that would force a *true* MyST parser for **sectioning** is structural `{eval-rst}` in
document bodies. Measured over the **full corpus (Part B, all 126 leaves): body `{eval-rst}` = 0**
(the root `index` has 1, expected — the back-matter toctree). So this is no longer "holds so far" — it
is settled: **ATX-only sectioning is sufficient, and ADR-013 (3) stays closed.** (Were a future ORCA
version to introduce structural eval-rst in bodies, this same measurement would reopen it; decisions
(1)/(2) do not depend on it.)

## `manifest.json` and idempotent refresh

`--all` writes every leaf to `resources/manual/<version>/<path>.md.txt` and a `manifest.json` at
`resources/manual/manifest.json` recording, per file: path, source URL, HTTP status, size, **sha256**,
**ETag / Last-Modified**, fetch time — plus the ORCA version and run date. The point of the manifest is
that a **6.2 refresh is a diff, not a blind re-download**.

- **Idempotency (measured):** a re-run does not re-download a file whose server copy is unchanged.
  **This server sends `Last-Modified` on all 126 files but NO `ETag` (0/126)** — so the conditional
  request that actually yields `304 Not Modified` here is **`If-Modified-Since`** (from the stored
  `Last-Modified`), not `If-None-Match`. The script prefers an ETag when present and falls back to
  Last-Modified; verified — a second `--all` reports **downloaded=0, reused=126**. `--force` ignores
  the cache and refetches.
- **Post-conditions in our terms (rule #9)** run after the fetch: every stored file is text (not an
  `<!DOCTYPE`/`<html>` error page), non-empty, and the count of OK files **equals** the number of
  200-status leaves in the manifest. Any mismatch → non-zero exit + a **named** list; never a silent
  "done". Part B: all three PASS (126 == 126).

## Network hygiene (this is someone else's server)

`scripts/fetch-manual.py` uses: a descriptive User-Agent naming the project + a contact link; a
~0.7 s pause between requests; retry with exponential backoff on 5xx/timeout (max 3); and a **hard
cap of 250 requests** so a walk bug cannot become a hammer. `--sample N` fetches only N leaves. The
Part-A run used **24/250**; the full Part-B `--all` used **137/250** (11 containers + 126 leaves), 0
failures.

## License

`resources/manual/*` (the fetched Markdown **and** `manifest.json`) is gitignored except the README —
the copyrighted ORCA manual is indexed locally for personal use and **never committed/redistributed**
(ADR-006). Verified after the Part-B run: `git status` shows nothing under `resources/manual/`.
