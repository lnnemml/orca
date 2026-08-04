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

## Page vs section size (the "a section indexes, a page shows" split) — measured

`cargo test page_size_measure -- --ignored --nocapture` over the same 126 leaves, for the change from a
section-as-screen to a page-as-screen:

- **Display unit grew ~14×.** PAGE bytes (the whole file, the new display unit): **median 18 773 · p95
  119 545 · max 214 493** (≈209 KB). SECTION bytes (the index unit): **median 1330 · p95 9074 · max
  48 245** (cross-checks the 4.2 body-size row). So the reader's unit is ~14× the median section and
  ~13× at p95.
- **The max page is ~209 KB, not 48 KB** — the earlier 48 KB was the max *section*; the largest *page*
  (`contents/modelchemistries/CASSCF.md.txt`, 5247 lines) is 4× that. This is why the in-page ToC is not
  optional on big pages.
- **Sections per page:** median **7**, max **162**; **4 pages carry >50 sections** (where the ToC is
  mandatory).
- **Render-prep cost is negligible.** `parseManualBody` over the 209 KB page: **~0.48 ms/parse** (V8,
  200 iters) — the JS work of turning the file into blocks is sub-millisecond, so any perceptible cost on
  the real page is React reconciliation + WebKitGTK paint of ~5000 lines, not the parse. (WebKitGTK paint
  time itself is **not** measured here — see the session log's honesty note.)

## MyST construct census (unit 4.11) — the render plan by number

`cargo test myst_constructs_measure -- --ignored --nocapture` walks all 126 leaves
(**3 984 406 chars**) with the sectioner's own fence model (`fence_open_info` +
`is_fence_close`, so a `$`/backtick inside an opaque ` ```code``` ` fence is never
counted) and censuses every render construct by **occurrences AND characters** — the
first because it decides the category-3 hide-whitelist, the second because it scales the
category-1 transform effort. The reason it exists: the page render (4.11) made the manual
panel the main reading surface, and the hide-whitelist had to be built from numbers, not a
screenshot. The gate writes nothing; these are its numbers.

**Category 1 — MATH (→ KaTeX).** The whole corpus's math is **dollar-delimited, and only
dollar-delimited**:

| form | occ | chars | % corpus |
|---|---:|---:|---:|
| `$$…$$` block | 874 | 169 941 | 4.27 |
| `$…$` inline | 4746 | 65 165 | 1.64 |
| `\(…\)` | **0** | 0 | 0 |
| `{math}` role | **0** | 0 | 0 |
| `{math}` directive | **0** | 0 | 0 |
| `(eqn:N)` labels | 330 | 4 495 | — |

So KaTeX needs exactly two delimiters (`$$`, `$`) and **zero tail** — `\(…\)`, the `{math}`
role and the `{math}` directive do not occur at all. Combined math ≈ 5.9 % of corpus chars.

**Category 1 — INLINE CODE (→ `<code>`).** One construct, `` `…` `` — **9280 occ · 468 984
chars = 11.77 %**, the single largest transform surface. Measured role-aware: a role's
backtick arg (`` {ref}`sec:x` ``) is stripped before the count, so it is not double-counted
as code. Of the remainder **0 %** are bare `prefix:label` cross-refs written as code — they
are genuine code tokens (keywords, paths, commands).

**Category 1 — CROSS-REFERENCES (→ link).** Two syntaxes reach a page/table, **≈1710 nav
refs**: roles `{ref}` 685 + `{numref}` 502, and links `[text](sec:…)` 515 + `(tab:…)` 8.
Resolve via the existing anchor map (1068 verified + `predict_anchor`), unresolved → text.
A **tail stays verbatim** because its target is not an in-corpus page: `{cite}` 991
(citations — no bibliography in the corpus), `{eq}` 144 (equations), `{cspan}`/`{rspan}`/
`{sup}`/`{sub}`/`{ints}` (table-cell formatting), and links `(external url)` 82 /
`(relative path)` 118 / `(page fragment)` 138.

**Category 2 vs 3 — DIRECTIVES: the whitelist, decided here.** 32 distinct directive names;
**13.6 % of corpus chars sit under some directive fence** — but that is overwhelmingly
**visible content**, not metadata. Histogram by occurrence:

```
321 {index}          255 {literalinclude}  176 {raw}         161 {figure}
119 {note}           104 {flat-table}       95 {table}        76 {bibliography}
 76 {tabularcolumns}  72 {Note}             37 {warning}      37 {important}
 24 {glossary}        19 {list-table}       18 {tip}          14 {subfigure}
 12 {admonition}       8 {Tip}   (+ {NOTE}/{Warning}/{Caution}/{Hint}/{Attention}/
                                    {only}/{seealso}/{versionadded}/{deprecated}/{include})
```

- **Genuinely INVISIBLE in real Sphinx → category 3 (hide), a NAMED whitelist:** `{index}`
  (321 occ, 0.31 % — invisible index markers, `` ```{index} Installation ``; the noise
  *per occurrence*), `{tabularcolumns}` (76 occ — LaTeX-only column spec, invisible in HTML),
  and `{raw} latex` (176 occ, 0.14 % — raw LaTeX passthrough, invisible in HTML; the arg is
  always `latex` here, whereas `{raw} html` would be visible).
- **Everything else is VISIBLE content → category 2 (verbatim):** the 29 remaining names —
  tables (`{table}`/`{flat-table}`/`{list-table}`, already monospaced), admonitions
  (`{note}`/`{warning}`/`{important}`/`{tip}`/`{admonition}`/…), `{figure}`/`{subfigure}`,
  `{bibliography}`, `{glossary}`. Separately, **`{literalinclude}` (255)** references an
  EXTERNAL `.inp` not present in the corpus — a broken-content-reference, not metadata; it
  stays verbatim and is flagged, not hidden.
- **Case matters:** `{index}` is single-case, but admonitions arrive as `{Note}`/`{note}`/
  `{NOTE}`, `{Tip}`/`{tip}` — a whitelist that ever hides one must fold case. Not needed for
  `{index}`/`{tabularcolumns}`.

**The reading (answers the gate's question — do 2–3 constructs carry most of it?).** Yes,
per category: math = **2** delimiters (no tail); inline code = **1** construct (11.77 %);
cross-refs = **2** nav syntaxes (citation/equation tail verbatim); hide = **`{index}`** by
occurrence, plus `{tabularcolumns}` and `{raw} latex` — a **2–3-name** whitelist. The
load-bearing correction to the screenshot model: the noise is **not** the whole `{directive}`
class (13.6 % is mostly visible tables/admonitions) — the invisible-metadata subset is only
≈0.3–0.6 %. This is exactly why the invariant's category (3) must be a **named** whitelist: a
blanket "looks like a directive → hide" would eat 13 % of visible content.

**Cross-reference resolution (`cargo test xref_resolution_measure -- --ignored`).** How many of
the navigational cross-references reach a section through the anchor map (`predict_anchor(label)` ∈
the section anchors — the SAME slugify that built the stored anchor, reused; scanned over the same
population the runtime collects, prose + directive bodies):

| form | total | resolved | % |
|---|---:|---:|---:|
| `{ref}` | 688 | 680 | 98.8 |
| `{numref}` | 509 | 167 | 32.8 |
| `[..](sec:/tab:)` links | 525 | 517 | 98.5 |
| **ALL** | **1722** | **1364** | **79.2** |

The 358 unresolved stay **verbatim text** (never a dead click). The section-targeting forms (`{ref}`,
`sec:`/`tab:` links) resolve ~98.7 %; the total is pulled to 79 % by `{numref}`, which points at
numbered **tables/figures/equations** — most of those labels are not section anchors, so those refs
correctly render as plain text. The **342** unresolved `{numref}` are partly downstream of the
absent-content gap below (see "External content the fetch skipped").

**KaTeX bundle cost — the first visual dependency, before/after `npm run build`.** main JS 6 254 →
6 557 KB (**+303 KB raw / +82 KB gz**), CSS 187 → 217 KB (**+30 KB**), whole `dist/` 16.5 → 17.9 MB
(**+1.41 MB**, the 59 bundled font files). **0 external `url()` in the built CSS** (verified) — fully
offline, no CDN. **Provenance of these deltas is laborious, not absent:** the "after" is a fresh
`npm run build`; the "before" is recoverable by rebuilding at **`d9a6492^`** (the commit before KaTeX
landed), not a committed gate.

**KaTeX render cost — an AD-HOC NODE PROXY, not a gate, and NOT the answer to the perf question.**
An ad-hoc `node`/`katex.renderToString` run (not committed, not reproducible from the repo) over the
corpus's math-heaviest page `modelchemistries/mdci` (**364 formulas**) rendered all of them in
**21.5 ms**; the largest-*bytes* page `modelchemistries/CASSCF` (214 KB, 153 formulas) in **6.6 ms**.
This measures **string generation in Node V8** — an *adjacent* quantity. The actual question ("does the
page hang?") is **DOM insertion of KaTeX's nested spans + WebKitGTK paint of a ~5000-line page**, which
this does NOT measure and which is left to the **author's interactive check in a real window**.
Committing a proxy gate and calling it the number's provenance would manufacture a fresh false number
with a look of legitimacy (see "the adjacent-measurement trap" under Part F) — so it stays labelled a
proxy, deliberately un-gated.

## External content the fetch skipped — one gap, one cause, one cure (a named future unit)

The render (4.11) surfaced several directives that reference content **not in our corpus**, because the
manifest fetch (`scripts/fetch-manual.py`) took only `_sources/*.md.txt` — not the linked `.inp`
examples, not the `_images/`. These are **ONE gap** (one cause, one cure — fetch the referenced files),
so they are collected here rather than scattered:

| directive | count | referenced content |
|---|---:|---|
| `{literalinclude}` | 255 | external ORCA `.inp` input examples |
| `{figure}` + `{subfigure}` | 175 | `_images/` figures |
| **total** | **430** | **the absent-content gap** |

Today `{literalinclude}` renders as a visible **absence marker** ("input example not loaded (`<path>`)")
and figures are untouched; both are recorded, not handled. The cure is a manifest-fetch extension (a
future unit) that also pulls the referenced `.inp` and `_images/` — the input examples first, since
worked input is this manual's most valuable content (mission).

**Two more components of the SAME gap, kept separate (different mechanism — GENERATED, not a skipped
file):** `bibliography` was one of the three no-source pages (Sphinx generates it; there is no
`.md.txt`), so the real author-year records are unreachable — the reason `{cite}` renders as the **keys**
(`[barone1998]`) and not a number (4.15, Task 2). And **equation numbers** (`{eq}`, 146×) are likewise a
*generated numbering* absent from the source, which is why `{eq}` stays category 2 (no compact form
found). All five components share **one root: we took the sources, not the render** — the sources omit
everything Sphinx generates or links (input files, images, the bibliography, equation numbers). This is
one sentence worth keeping, because from it grows the question "should we have fetched the HTML instead?"
— and the 4.15 `--html-sample` gate is the first, cheap, sampled answer to it. The five are **not one
number**: 430 (input examples + images, skipped files) · 342 unresolved `{numref}` (mixed targets) ·
`bibliography` (1 generated page) · `{eq}` 146 (generated numbers).

**Dependent forecast (a gate for that unit before it starts):** `{numref}` resolves at only **32.8 %**
(167/509), and **342** are unresolved. A share of those point at the very figures/tables the fetch
skipped — so **if `_images/` (and the numbered objects) are ingested, `{numref}` resolution must rise
by a measurable amount.** That predicted rise is the future unit's built-in post-condition: if fetching
the images does *not* move the `{numref}` number, the ingest did not do what it claimed. (This is a
**distinct** debt from the 430: the 342 mix absent-figure targets with non-section objects — do NOT
merge them into one number.)

## Source vs published HTML — asking the render, not our list (unit 4.15 gate)

The 4.12 census enumerated construct TYPES (directives / math / code / xrefs) and built the hide-whitelist
from that. But a MyST **anchor label** line `(name)=` is none of those types, so the census never listed
it, and we rendered `(sec:…solvationmodels)=` before a heading — visible junk that is INVISIBLE in the
real manual. The census's inventory was OUR list of types, not the subject's answer to "what does Sphinx
not show" — **Pattern 1 (a check that measures us), 5th instance** — and it was found not by any gate but
by the **author's eye in the window**. That is the lesson: a manual window-check catches a class no gate
built from our own inventory can, because the gate inherits our blind spots.

The cure is to ask the subject: the published HTML. `scripts/fetch-manual.py --html-sample N` (author-run,
out-of-band — ADR-013 (2) intact) fetches a **diverse 12-page sample** (8 chapters + the shortest leaf +
the largest + a container), extracts the main content (Furo `<article id="furo-main-content">`, stdlib
`html.parser`, no dependency), and tests which SOURCE constructs are absent from the rendered text. The
test is only decisive for **synthetic** payloads (a label/key/spec never coincides with prose); for
natural-word payloads (index terms, code tokens) it over-reports "visible" and is marked NOT decisive —
those are settled by Sphinx builder semantics, and saying so is worth more than a false checkmark
(**Pattern 2**: the measurement's scope is smaller than the claim; name it).

| class (synthetic payload) | visible in rendered (sample) | corpus | verdict |
|---|---|---:|---|
| `(name)=` anchor label | **2/186** (the 2 = substring noise; RI+mdci hand-checked 0/60) | **1438** | **INVISIBLE → category 3** |
| `{cite}` keys | 2/165 | 1002 | keys invisible, but the CITATION is visible → **category 1** |
| `{numref}` keys | 0/64 | 509 | keys invisible, renders a number → already a cross-ref |
| `{tabularcolumns}` | 0/3 | 76 | INVISIBLE (confirms the existing whitelist item) |

*Not decisive by substring (Sphinx semantics decide):* `{index}` 38/56 (index terms = prose words), `{raw}
latex` (HTML build excludes latex-only raw), inline code 85/347 (visible; sanity). **Reverse check — of
the three existing whitelist items, none is actually visible** (no defect); the one distortion found was
`{cite}` shown verbatim (fixed → category 1, Task 2).

**`(name)=` boundary (checkable, measured).** The render hides EXACTLY a whole trimmed prose line matching
`^\([^()]+\)=$` — the sectioner's own `parse_label` rule (`sections.rs`, rule #9: hide precisely what the
index calls a label). Measured over the corpus with render.ts's fence model: **1438 hidden**; the
look-alikes that are correctly KEPT — **67** mid-line `(x)= …` (not a whole line) and **10** `(x)=` that
ride inside a ` ```orca ` block whose fence over-extends past a non-standard `` ``` --> `` close (our
strict "close = fence-chars only" rule; a source quirk, the safe side of the boundary — never hide code).
The 1448 of unit 4.1 vs 1438 here is exactly this boundary: 4.1 counted every `(…)=` target; 1438 is the
ones that are a whole prose line and would actually hide.

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
- **The agreement number — and its SCOPE (corrected, see Part E).** Where both signals resolve (936
  targets) they **agree 98.5 %** (14 disagreements). This validates the two derivations **on their
  intersection** — but **not** the structural proxy where it resolves ALONE (855 targets), which was
  measured only in Part E and is where the error turned out to sit.
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

### Part D — coverage over an explicit, named inventory (the population, not just the form)

The coverage gate improved twice by the number falling as the *form* of the question got honest
(46/46 → 44/46, string → type). Part D fixes the **population**: the expectation set is now
`src/manual/keyword-inventory.json` (one home, both gates), each word carrying a **source** — `builder`
/ `template` / `domain` (ADR-014 guards) / `workflow` (ADR-007 reaction chain). **53 words; 45 resolve
(type- and block-aware).** The 8 gaps, classified by closer: **(a) `{numref}` layer — 1** (`%maxcore`);
**(b) curated prose — 3** (`IRC`, `ScanTS`, `NEB-CI`); **(c) second/right form of a concept in the map
— 4** (`CPCM`, `XTB`, `TightOpt`, `Constraints`-under-`%geom`); **(d) not in corpus — 0**. Corpus
checks behind the classification: "List of Input Blocks" contains `%maxcore`/`%cpcm`/`%xtb`/`%irc`/
`%neb` (but not `%scan`); `IRC`/`ScanTS`/`NEB-CI` appear in the corpus only in prose (0 backtick
entries). **`{numref}` closes 1 of 8** — so the block index is not the high-value next step for the
words the project is built around; curation + simple-form records close 7.

### Part E — the structural owner is wrong off the intersection (scale: hundreds, not units)

Before any curation, a measurement of the population the 98.5 % agreement never covered. That number is
the **intersection** (936 targets where text AND structural both resolve). Structural resolves **alone**
on 855 targets — nothing to check it against — and the error is there. Of the **802 structural
block-option records (814 targets)**:

- **537 targets (66.8 %) name an owner the section body never mentions** — the `%block` is not in the
  body; the owner was inferred purely from a `%block` higher up the breadcrumb. **529 distinct records.**
- **515 (64 %) sit in a section body with NO `%`-token at all.** (The reverse `!`-line-in-` ```orca `
  signal is weak — only **36 (4.5 %)** — so absence of the owner is the discriminator, not presence of
  `!`.)
- **By section title (rule derived from the real corpus headings):** **475 targets / 473 records** live
  in sections whose **heading is about `!`-line keywords, not a `%`-block** — **384** in `… Basis Sets`
  tables (basis names are simple `!` keywords), **54** `… Optimization Keywords`, **21** `Convergence
  Tolerances`, **16** `Simple Input Keywords`. (Full title histogram: 69 distinct titles; top ones are
  `Keywords` 105, `… Basis Sets` variants, `Geometry Optimization Keywords` 54.)
- **Manual check of 10 (spread across the list): 7 are misqualified** — 5 basis-set names owned by
  `%basis` (`aug-cc-pwCV5Z/C`, `cc-pV5Z-DK`, `HGBSP1-7`, `MINI`, `Partridge-3` — simple `!` keywords),
  2 geometry keywords owned by `%method` instead of `%geom` (`ConnectFragments`, `TolMaxG`, the same
  wrong-owner class as `Constraints`). 3 are genuine (`%mm`, `%md`/`Minimize` options).

Two failure modes, both from the structural proxy assigning an owner the text does not support:
**(i)** simple keywords (basis sets, run-type keyword tables) qualified as block-options; **(ii)**
right-kind-wrong-block (geom keywords → `%method`). Scale is **~500 records — hundreds, not units** —
so this is **not** a curate-a-few fix like `IRC`/`ScanTS`. The owner derivation needs a **third signal**
(a section-title / body-text veto: do not accept a structural owner the body never names), which is a
**generator change with its own gate** — a separate unit. Nothing is changed here; this is the scale.

### Part F — the ROOT of the mis-typing, and why merging won't fix it

**The root (found in the generator, not a symptom).** `type_of` in `generate_keywords_json` is:

```
if tok.starts_with('%')                               -> "block"
else if app_simple.contains(tok) || title_home(tok)   -> "simple"
else                                                  -> "block-option"   // ← a dumpster
```

`simple` is granted ONLY to words OUR input-builder emits or that match a section title; **everything
else defaults to `block-option` with no manual signal at all.** The `else` branch does not classify —
it *collects the unknown*. The structural proxy then dutifully hands each such record a breadcrumb
owner, because it was handed a thing already declared a block-option. So Part E's two symptoms —
simple-word-as-block-option and right-kind-wrong-block — are **one defect: the TYPE was inferred from
OUR application, not from the manual.**

**Pattern 1 — a check that measures US instead of the subject (collected, all five).** Our knowledge is
not forbidden; it must not **masquerade as a measurement of the manual** — it gets its own channel with
provenance. The same defect appeared **five times** (this collection is the canonical one; the module
[modules/manual-keywords.md](../modules/manual-keywords.md) carries the same list):
1. **`%maxcore` "covered"** — the gate measured in *our* notation (string-normalised), so a `%maxcore`
   directive matched a `MAXCORE` block-option.
2. **`46/46` held** — the expectation set was assembled from *our* builder's output, so it could only
   confirm what we emit.
3. **`app_simple` in `type_of`** — the seed's *type* was inferred from what our app emits (the `else`
   above is a dumpster: everything unknown → block-option). *(Fixed in Part G: type from the manual.)*
4. **the demand corpus looked independent but was us** — the author's inputs (A) barely diverge from our
   fixtures+templates (B∪C), because he generates inputs *with* the input-builder (Part H). "Measure
   what the author types" was "measure what we emit" once more; caught by measuring A against the
   manual's own `!` vocabulary (15 vs 424, 10 %). *(Found later, in Part H — collected here.)*
5. **the render hide-whitelist listed OUR construct types** — the 4.12 census enumerated
   directives/math/code/xrefs and built category 3 from that, so the `(name)=` anchor label (none of
   those types) was never a candidate and rendered as visible junk. The inventory was our type-list, not
   the subject's "what does Sphinx not show". *(Fixed 4.15: the `--html-sample` gate asks the published
   HTML; `(name)=` surfaced at once.)* **Found not by a gate but by the author's EYE in the window** — a
   manual window-check catches a class no gate built from our inventory can, because the gate inherits
   our blind spots. That is itself the lesson, and the reason the interactive check is not optional.
Each fix was the same shape: give our knowledge its own attributed channel and let the check ask the
manual.

**Pattern 2 — the adjacent-measurement trap (a DISTINCT pattern; three instances).** Not "we measured
ourselves" but "a number measured an **adjacent** quantity and was read as the answer to the actual
question." Named separately because its tell and its cure differ (the cure is to re-scope the claim to
what was measured, or measure the real thing — not to re-channel our knowledge):
1. **`98.5 %` owner agreement** — read as validating the structural proxy; measured **only the
   intersection** (936 targets), never the 855 structural-only ones where the error sat (Part E).
2. **"render preservation: 0 loss on 126 leaves"** — read as the render being loss-free; the corpus
   check that existed measured **Rust storage** (`body_md` byte-for-byte through SQLite, `index.rs`),
   **not** the render. It was never true of the render, four units, until `render.corpus.test.ts` (4.12).
3. **KaTeX `21.5 / 6.6 ms`** — read as "does the page hang?"; measures **Node V8 string generation**,
   not WebKitGTK DOM-insert + paint (labelled a proxy above, deliberately un-gated).
The tell: a real, correct number sitting next to the question it is cited for. The cure is scope, not
provenance — a gate that measures the proxy would only launder the trap.

**Does merging fix it? Measured — no** (`cargo test structural_overlap_measure -- --ignored`). Of the
structural block-option targets whose owner is **not** named in the section body — **522** (the exact
figure with the real sectioner; Part E's 537 was an approximate probe):

| | count | meaning |
|---|---|---|
| **1a** already documented elsewhere as `type=simple` | **0** | merge is impossible — the dumpster made **no** simple record for any of them |
| **1b** already a **confirmed** block-option elsewhere | **14** | genuine block-option, this target a duplicate from a foreign section |
| **1c** true orphans (no other record) | **508** | **500 distinct words** |

So the merge hypothesis is **refuted by 1a = 0**: because the root typed *everything* unknown as
block-option, none of these words got a simple record to merge into. And the direct signal is weak
**corpus-wide too**: of the 500 orphan words, only **44 (8.8 %)** appear on a `!` line anywhere in a
` ```orca ` block. The orphan population is overwhelmingly **basis-set tables** — top source sections:
`Jensen Basis Sets` (56), `Correlation-consistent Basis Sets` (53), `Auxiliary basis sets … (AuxC)`
(47), `Hydrogenic Gaussian Basis Sets` (24), `Relativistic Correlation-Consistent …` (22), … — i.e.
basis names, which are simple `!` keywords, with **no per-word `!`-example** to lean on.

**What this means for the fix (numbers, not a guess).** The owner **veto** (accept a structural owner
only when the body names it) removes the wrong owner from the 522 and keeps the 14+ confirmed. But
veto alone leaves a `block-option` with `block: null`, and for a basis name that is **still the wrong
type** — a `!`-line hover looks for `simple` and misses, silently. Since 1a = 0 (nothing to merge) and
the `!` signal is weak (8.8 %), the words cannot be confidently retyped `simple` either. So the shape
is a **third type value `scope: "undetermined"`** — a value with meaning (like `anchor = NULL`,
`owner_source = null`), not a false second type — for the ~508 orphans, decided by these numbers.
(Nothing is changed here; this is the scale and the shape.)

### Part G — the fix (owner veto + type from the manual)

Two independent signals, in the generator (`type` is a value **`undetermined`**, not a separate
`scope` field — one field per decision).

- **Owner veto (B1):** a **structural** owner is accepted only when the section body NAMES it
  (correlating two sources, like `objects.inv` × `predict_anchor`). The 522 unconfirmed structural
  targets lose their owner; **291 remain** structural (confirmed in body), 1224 text. Block-option
  records with a null owner drop from the file: **`owner_source = null` block-options → 0**.
- **Type from the manual (B2):** `type_of` no longer reads `app_simple`. Seed types: `%…`→block,
  title-is-keyword→simple, else `undetermined` (block-option needs a positive, veto-confirmed owner).
  Result: **1183 `undetermined` records** where the dumpster + proxy used to force block-option/null.
  A basis table is now homogeneous in the seed (`def2-QZVPP` and `ma-def2-SVP` both `undetermined`).
- **Curated channel (B2 correction):** the `app_simple` knowledge moves to `provenance:"curated"`, not
  deleted. Of the 42 builder simple keywords: **5 stay seeded** (title is the keyword), **22 flipped**
  from `undetermined`, **11 added** beside a block-option, **2 via alias**; **0 hard words lost**.
- **Coverage, by channel:** **46 of 53 resolve — 9 via SEED (manual), 37 via CURATION (ours,
  attributed)** — the split is now visible so the two channels can't merge again. (`CPCM` moved from a
  gap to curated-covered; gaps: `%maxcore` a; `IRC`/`ScanTS`/`NEB-CI` b; `XTB`/`TightOpt`/`Constraints`
  c.) File **576 KB → 515 KB**; regeneration byte-identical; descriptors **317/317** resolve; 0
  dangling refs.

(B2 arithmetic reconciled: the buckets 5 title + 22 flip + 11 add + 2 alias = 40; the **remaining 2 of
42 are `TightSCF`/`VeryTightSCF`**, covered separately via `PROSE_CURATED` — not a discrepancy, a fourth
curation path.)

### Part H — DEMAND on the `!` line, from the author's own inputs (`demand_measure`)

The inventory of 46 is OUR builder's vocabulary — the pattern's third face was measuring it. This
measures **demand**: the `!`-line tokens of inputs the author actually ran, kept in groups that are
**never merged**. Group **(A)** = the user DB (`~/.local/share/orcastudio/orcastudio.db`, outside the
repo); **(B)** = our test fixtures; **(C)** = the Phase-1 templates. Tokenised by the **real
`wordPattern`** read from `orca-language.ts` (not a second tokenizer).

- **Sizes:** (A) **16 jobs, 15 distinct** `!`-tokens; (B) 3 fixtures, 6 distinct; (C) 8 templates, 9
  distinct; **(B∪C) 11 distinct**. `CPCM(water)` → `wordPattern` yields `["CPCM", "water"]` (the `(…)`
  are separators, so a solvent name becomes its own token).
- **A barely diverges from (B∪C)** — `A \ (B∪C) = {cpcm, def2-tzvp, dmso, ethanol}`, and of those
  `dmso`/`ethanol` are solvent VALUES, `cpcm`/`def2-tzvp` are builder words merely absent from our
  fixtures. **The demand IS the builder's vocabulary** — because the author generates inputs *with* the
  input-builder. So the earlier "measure of 46" was, by accident, already a measure of demand; the
  independent signal (hand-typed `%`-block options) does not exist yet. This is the pattern once more,
  now visible from the demand side: at this stage there is no daylight between "what we emit" and "what
  the author types".
- **(A) `!`-line coverage (consumer form): 12 of 15 resolve** (2 via SEED, **10 via CURATION**). The 3
  misses: `dmso`, `ethanol` (solvent values, not keywords — a future "solvent" class, correctly silent
  now) and **`XTB`** — a real gap `c` **hit by real demand** (`! XTB GOAT` is the author's ibuprofen
  conformer run and a mandatory domain guard; `GOAT` resolves, `XTB` does not). Top A tokens: `TightSCF`
  13, `Opt` 12, `Freq` 11, `B3LYP`/`D4`/`def2/J`/`RIJCOSX` 8, `def2-SVP` 7, `r2SCAN-3c` 5.
- **Reverse — the value of the seed's big half: 1 of 1363 distinct block-option keywords is touched by
  demand** (`nprocs`, from `%pal`). The 1515 block-option records (≈ 53 % of the map) — the part the
  manual documents structurally and the seed captured well — serve **essentially no current demand**,
  because the author types `!` lines, not `%`-block bodies. That is a fact about where seed effort paid
  off (structured capture) versus where demand actually is (the `!` line, documented in flat-tables and
  prose the seed can't read).

Conclusion: the next curation step is not "the seven gaps" broadly — it is **`XTB`** (the one
real-demand keyword miss) and a **solvent** class; the block-option half needs no curation for demand.
And the demand corpus should be **re-measured** once the author hand-edits `%`-blocks — only then will
independent demand for that half appear.

### Part I — the DENOMINATOR: the manual's own `!` vocabulary (`manual_bang_vocabulary`)

Demand ≈ builder (Part H), so demand is not an independent signal. The available proxy that is **not
us**: the `!`-lines in the manual's own ` ```orca ` blocks — how ORCA's authors use the simple-input
line. This is the denominator that decides whether the curated layer is a seven-item list or a plan.

- **417 `!`-lines, 424 distinct `!`-tokens** in the corpus's ` ```orca ` blocks. Our map resolves
  **42 / 424 = 10 %** as simple (13 seed, 29 curation); **293 absent, 89 undetermined-only**. Our
  demand (A, 15 tokens) covers **3.1 %** of it.
- **The manual's `!` vocabulary is ~28× wider than our 15** and only 10 % covered. Top absent tokens
  are real methods/bases, not noise: `MOReAd` (27), `DLPNO-CCSD` (20), `NEB` (15), `NoIter` (13),
  `ALPB` (10), `RHF` (10), the `/C` aux bases (`def2-TZVP/C`…, `undetermined`). So the **curated layer
  is OPEN — it needs a plan, not seven entries** — and Phase 4's keyword coverage is **not** "done".
- **Argument forms measured** (`KEYWORD(arg)` on `!`-lines): `CPCM` (11), `DLPNO-CCSD` (11), `SV` (9),
  `ALPB` (8), `GCP` (3), `DEF2-TZVP` (3), `SMD` (3), `DDCOSMO`/`CPCMX`/`CCSD`/`RI-CCSD`/`PAL8` (2 each).
  So the argument rule is **general** (solvents, `SV(P)` basis, `CCSD(T)` triples, `PAL8(n)`), not a
  `CPCM`/`SMD` whitelist.

Acted on this number (not on convenience): **`XTB` curated** (the one gap real demand hit — `! XTB
GOAT`), pointing at `semiempirical › Extended Tight-Binding: GFN0-xTB, …` (found, not guessed; its
`%xtb` block-option left intact). The other six inventory gaps (`IRC`/`ScanTS`/`NEB-CI` b,
`TightOpt`/`Constraints`-in-%geom c, `%maxcore` a) stay **declared gaps** — not curated one-by-one,
because 10 % coverage means the layer needs a plan and none of the six is demand-hit. Inventory
**47 / 53** (9 seed, **38 curation**). keywords.json diff = **one curated record + one appended section**
(the section array appends curated-only sections after the stable seed sections, so a curation add is
reviewable, not a renumber cascade); regeneration byte-identical; descriptors **318/318**.

The only thing that would force a *true* MyST parser for **sectioning** is structural `{eval-rst}` in
document bodies. Measured over the **full corpus (Part B, all 126 leaves): body `{eval-rst}` = 0**
(the root `index` has 1, expected — the back-matter toctree). So this is no longer "holds so far" — it
is settled: **ATX-only sectioning is sufficient, and ADR-013 (3) stays closed.** (Were a future ORCA
version to introduce structural eval-rst in bodies, this same measurement would reopen it; decisions
(1)/(2) do not depend on it.)

## Selection normalization for the manual lookup (unit 4.13)

The editor lookup's trigger moved from **hover** (one `wordPattern` token) to a user **SELECTION**
(anything — half a word, three words, an xyz line). Hover gave a grammar-bounded token; a selection
gives arbitrary text, so the normalization had to be MEASURED, not invented — a plausible answer to an
imprecise selection is worse than none (the NULL-anchor / hover-silence posture). Measured by
`selection-lookup.corpus.test.ts` over the corpus's **1475 ` ```orca ` blocks** and the map (**2528
distinct keys** incl aliases):

- **Key forms decide what a selection can be:** **space 0** (a selection with an internal space can
  never be a single key → silence — so `Opt Freq`, `%geom Constraints` are silence), **paren 0** (`(…)`
  is an argument delimiter, not part of a key), slash **78** (`def2/J`, `aug-cc-…/C`), dot **47**
  (`basename.xyz`, `D.Print`). Slash/dot/`-`/`%` are token-continuation; parens and space are not.
- **Two premises REFUTED by the measurement — the same shape, on both sides.** (a) "`Tight` gives
  silence because it is not in the map" — **`tight` IS in the map** (`Tight[block-option/%scf]`); the
  silence in a `!`-line's *simple* context comes from the **type qualifier** (wrong type → miss), not
  absence. (b) "`SV(P)` is a map key, so stripping its `(P)` would lose it" — **`sv(p)` is NOT a key**
  (`sv` is); **0 keys contain a paren**, so `SV(P)` → `SV` is correct, not a loss.
- **The real risk is a same-type partial: 16 of 121 simple keys are a substring of a longer SIMPLE
  key** — `opt`⊂`optts`, `freq`⊂`numfreq`, `d4`⊂`wb97x-d4`, `ri`⊂`ri-jk`, plus `hf`/`pbe`/`b3lyp`/
  `tightscf`/`def2-svp`/`r2scan`. These are the *most common tokens of real inputs*; selecting `Opt`
  inside `OptTS` would give a confident answer about the neighbour, and the type qualifier is powerless
  (both simple). This is the `MaxIter`-has-15-owners defect in another plane: **the entity boundary does
  not coincide with the selection boundary.** So the **boundary guard IS the rule** (not a UX add-on):
  a selection is rejected unless its source neighbours are outside the token-continuation class
  `[\w%/.-]` (parens excluded, so `CPCM`-before-`(` is not a cut and `water`-inside-`()` is reached by
  the argument rule, not the guard).
- **Guard-vs-muffler — the decisive number: FALSE REJECTS = 0 / 2877.** Of every whitespace-delimited
  corpus token that resolves, **zero** would be rejected by the guard (their neighbours are whitespace /
  line edges). The guard rejects only mid-token cuts, never a natural selection — so it is a guard, not
  a muffler.
- **Argument order — whole-first, then strip (0 loss):** try the WHOLE selection exactly, and only on a
  miss strip one trailing balanced `(…)` and retry. Of **69** `KEYWORD(arg)` corpus tokens, **0** are
  themselves keys, so all **12** distinct that resolve do so **stripped** (`SV(P)`→`SV`,
  `CPCM(Water)`→`CPCM`, `DLPNO-CCSD(T)`→`DLPNO-CCSD`) with **no loss** — but the whole-first order is the
  correct invariant the day a paren-bearing key is added.
- **Resolve rate (whole-then-strip, exact):** `!`-line tokens **229/550 distinct (42 %)**, `%`-block-
  option first tokens **1045/1852 (56 %)** — the map's coverage fraction; the rest correctly → silence.

**The rule, and its FOUR silences (three silent, one VISIBLE).** A selection resolves iff: (1) single
line, (2) no internal whitespace, (3) boundaries not mid-token, (4) — after whole-then-strip and the
position qualifier — an exact type/block-aware hit. A miss can be four things, and the display treats
them in two classes: **malformed** (internal space or a mid-token cut) → the panel appears with a
**format hint** ("select one keyword whole"), a correctable user action; **qualified miss** (well-formed
but not in the map, or wrong type/position like an argument token) → **no panel** (silence), the boundary
of our data. The distinction is the learning-instrument's job: the author must be able to tell "select
differently" from "we don't document this."

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
