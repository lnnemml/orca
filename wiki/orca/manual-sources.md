# ORCA 6.1 manual — source format (measured)

**Measured 2026-07-31** by `scripts/fetch-manual.py`. Genre: like `parse-sources.md` — facts from a
real run, dated and versioned, not from docs or memory. Two gates fed this page: **Part A**
(`--manifest --sample 6`, 24 requests) established the format on a 6-file sample; **Part B** (`--all`,
137 requests) fetched the **full corpus of 126 leaves** and re-measured everything over all of it, so
the numbers below are corpus-wide unless a line says "sample". Where Part A gave an estimate, Part B's
exact figure replaces it (and the estimate is labelled as such).

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

- **Why two fence scanners, and why it is not debt.** `parse_toctrees` is an **allow-list**: it acts
  only *inside* `{toctree}`/`{eval-rst}` directives, so a bare ` ```orca ` code fence can never inject a
  phantom entry into the manifest — it doesn't need to see code fences at all. `iter_prose_lines` is a
  **deny-list**: it must hide *every* fence, so it must see all of them. Opposite jobs; only the
  **close rule** (`_is_fence_close`) is shared. Collapsing them into one function would force one side
  to do the other's job badly — they stay separate on purpose.
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
