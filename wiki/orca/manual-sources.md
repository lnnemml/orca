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

Sections come straight from ATX (`#`/`##`/`###`…) heading levels — no HTML parser needed.

- **Full corpus (all 126 leaves, Part B):** **2055 headings** — `#`=593, `##`=657, `###`=606,
  `####`=193, `#####`=6. **Deepest ATX level = `#####` (level 5)** (e.g.
  `contents/utilitiesvisualization/orca_2json`). No discrepancy with the TOC: it implied level-4
  (`####`) sections exist (2.6.7.1.1 scf.md, 2.7.2.13.1 basisset.md, 3.3.4.1.1 DFT, …) and Part B
  confirms `####` (193) — and one level deeper still (`#####`). Deep subsections **are** ATX, so
  ATX-only sectioning reaches them.
- **Sample-only (Part A, superseded):** the 6-file sample showed a max of `###` and a mean of
  ≈ 23.5 KB/file — both **sample artefacts**, kept here only to show why a sample under-measures.
- **Exact size (Part B):** **4 084 799 bytes = 3.90 MiB** of Markdown over 126 leaves (mean ≈ 32.4 KB).
  The Part-A **estimate of ~2.8 MiB was low**, not high: the three tiny `preface/*` leaves in the
  sample pulled the mean down more than the three hand-picked keyword-heavy files pushed it up.
  Comfortably read-whole territory either way.

## "Keywords" sections are HETEROGENEOUS — seedable, but from two forms

This is the fact ADR-013's "seed `keywords.json` from the manual's Keywords sections" paragraph
stands on. Reality (measured, not assumed): the machine-readable keyword lists come in **two distinct
markups**, so a seeder needs **two extractors**, not one:

1. **Labeled `:::{table}` directive wrapping a GFM pipe table** — e.g. RI's `## Keywords` has
   `(tab:essentialelements.ri.keywords.simple)=` / `(tab:...block)=` tables: **column 1 = keyword**
   (backtick-wrapped, sometimes comma-separated aliases), **column 2/3 = options / description**.
   Cleanly parseable.
2. **Annotated ` ```orca ` code block** — e.g. `## Complete Keyword List for the %cpcm Block` is NOT
   a table but a fenced `%cpcm … end` block of `name  value  # description` lines. Seedable by a
   code-block line parser, not a table parser — and it is the **richer** source: each line yields
   keyword **+ default value + description** at once, whereas the pipe table gives only two fields
   (keyword + description). A seeder should prefer form 2 where both exist.

Some sections ("Surface Scan Keywords", "OpenCOSMO-RS Keywords") read as prose in the coarse
classifier and likely mix prose with one of the two forms below their heading. **Consequence for
ADR-013:** the keywords paragraph survives — the information IS structured and seedable — but "seed
from the Keywords sections" means **table extractor + `%block` code-block extractor**, curated on top;
not one uniform table format. (This concerns the *seeder*, not sectioning, so it does not touch the
(3) MyST-parser review condition.)

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
