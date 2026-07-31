# ORCA 6.1 manual — source format (measured)

**Measured 2026-07-31** by `scripts/fetch-manual.py --manifest --sample 6` (ADR-013 Part-A gate).
Genre: like `parse-sources.md` — facts from a real run, dated and versioned, not from docs or memory.
Nothing here is a guess: every number is from the walk + a 6-file representative sample (24 HTTP
requests, cap 250). The full-corpus fetch is a later unit (Part B, `--all`).

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

Sections come straight from ATX (`#`/`##`/`###`) heading levels — no HTML parser needed.

- **Sample (6 files):** 44 headings — `#`=7, `##`=20, `###`=17; **deepest in the sample = `###`
  (level 3)**. The corpus certainly goes deeper on the big detailed pages; the full level
  distribution is a Part-B measurement over all 126 leaves.
- **Size:** sample mean **≈ 23.5 KB/file** → corpus **≈ 2.8 MiB** of Markdown (rough extrapolation
  over 126 leaves). Comfortably read-whole territory.

## "Keywords" sections are HETEROGENEOUS — seedable, but from two forms

This is the fact ADR-013's "seed `keywords.json` from the manual's Keywords sections" paragraph
stands on. Reality (measured, not assumed): the machine-readable keyword lists come in **two distinct
markups**, so a seeder needs **two extractors**, not one:

1. **Labeled `:::{table}` directive wrapping a GFM pipe table** — e.g. RI's `## Keywords` has
   `(tab:essentialelements.ri.keywords.simple)=` / `(tab:...block)=` tables: **column 1 = keyword**
   (backtick-wrapped, sometimes comma-separated aliases), **column 2/3 = options / description**.
   Cleanly parseable.
2. **Annotated ` ```orca ` code block** — e.g. `## Complete Keyword List for the %cpcm Block` is NOT
   a table but a fenced `%cpcm … end` block of `name  value  # description` lines. Also seedable, but
   by a code-block line parser, not a table parser.

Some sections ("Surface Scan Keywords", "OpenCOSMO-RS Keywords") read as prose in the coarse
classifier and likely mix prose with one of the two forms below their heading. **Consequence for
ADR-013:** the keywords paragraph survives — the information IS structured and seedable — but "seed
from the Keywords sections" means **table extractor + `%block` code-block extractor**, curated on top;
not one uniform table format. (This concerns the *seeder*, not sectioning, so it does not touch the
(3) MyST-parser review condition.)

## Anchors: `(label)=` → `#slug`, and there is an authoritative map

- **Rule (verified 46/46 labels in the sample, across `sec:`/`tab:`/`fig:`/`table:` prefixes):** a MyST
  target `(sec:a.b.c)=` in the source becomes the HTML id `#sec-a-b-c`. The transform is: **lowercase,
  then every run of non-alphanumeric characters → a single `-`** (so `:` and `.` and `_` all fold to
  `-`; `CPCM-features` → `cpcm-features`). Not just `sec:` — every label prefix follows it.
- **`objects.inv` EXISTS** (`<base>objects.inv`, **46 257 bytes**). This is Sphinx's authoritative
  label→anchor (and label→title) map. So the slugify rule above does **not** have to be trusted as a
  guess — the inventory is the ground truth. **Not parsed in this unit** (ADR-013 scope); flagged so
  the sectioner/keyword units use it instead of re-deriving anchors.

## MyST-parser review condition (ADR-013 (3)) — NOT triggered by the sample

The only thing that would force a *true* MyST parser for **sectioning** is structural `{eval-rst}` in
document bodies. Measured: **root** `index` has **1** `{eval-rst}` block (expected — the back-matter
toctree); **body eval-rst in the 6-file sample = 0**. So ATX-only sectioning holds so far. This is a
sample, not a proof over all 126 — the definitive count is a Part-B sweep, and if bodies turn out to
carry structural eval-rst, ADR-013 (3) reopens (decisions (1)/(2) do not depend on it).

## Network hygiene (this is someone else's server)

`scripts/fetch-manual.py` uses: a descriptive User-Agent naming the project + a contact link; a
~0.7 s pause between requests; retry with exponential backoff on 5xx/timeout (max 3); and a **hard
cap of 250 requests** so a walk bug cannot become a hammer. `--sample N` fetches only N leaves. The
Part-A run used **24/250** requests.
