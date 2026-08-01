# Module: keywords.json (`src/manual/keywords.json`)

**Status:** seeded + qualified + normalized (unit 4.4, Parts B–C). The keyword→section map that feeds
the Monaco **hover** provider (4.4 UI, not built yet). It is a **repo file, seeded programmatically
then curated by hand** (ADR-013 narrows ADR-006's "by hand") — Rust owns manual text-to-structure
(ADR-013), so a Rust `#[ignore]` generator emits it; the frontend consumes it. **Why a separate layer
at all:** the hover shows **one** section, confidently; at the FTS panel's hit@1 = 9/17 (~53 %,
ADR-013 amendment) a search-fed hover would be wrong half the time. So hover reads an **explicit** map,
not FTS.

## The core idea — a block-option is a qualified name

**A `block-option` is a qualified name, and the qualifier is part of the identity, not metadata.
`MaxIter` without a block is not a keyword — it is a string that occurs in fifteen different places.
The parallel to `AtomId` is exact: there a position in an array was mistaken for the identity of an
atom; here an option name is mistaken for the identity of an option. Both times the confusion shows up
not as a crash but as a surplus of candidates.** (In the generated file `MaxIter` is literally 11
records — `%scf MaxIter`, `%casscf MaxIter`, `%method MaxIter`, … — each a distinct hover target.)

So the lookup key for a block-option is **(block, option)**, never the bare option.

## Schema (`schema_version: 2`)

The file is **normalized**: one `sections` array (each distinct section once) and records that
**reference sections by integer index**. `schema_version` is at the root for the same reason
`results.parser_version` and the DB `SCHEMA_VERSION` exist — the record shape changed (`targets` went
from `[{…}]` to `[<int>]`), and an old reader on a new file would silently take integers for objects.
One line now, an ugly bug avoided later.

```jsonc
{
  "schema_version": 2,
  "orca_version": "6.1",
  "sections": [                          // index space; referenced by number
    { "file": "...", "breadcrumb": [...], "title": "...", "nth": 0 },   // = section 0
    ...
  ],
  "keywords": [
    { "keyword": "RIJCOSX", "type": "simple", "provenance": "seeded",
      "targets": [0, 3] },               // int refs into `sections`
    { "keyword": "MaxIter", "type": "block-option", "provenance": "seeded",
      "block": "%scf", "owner_source": "text", "section": 52 },
    { "keyword": "MaxIter", "type": "block-option", "provenance": "seeded",
      "block": null, "owner_source": null, "targets": [11, 88, 140] },
    { "keyword": "M06L", "type": "simple", "provenance": "curated",
      "aliases": ["M06-L"], "targets": [96, 105] }
  ]
}
```

- **`section` (one) or `targets` (many)** — never both. `targets[]` **reflects reality** (one option
  genuinely documented in several places — `%casscf MaxIter` lives in the CASSCF *and* the DMRG
  chapters), not our inability to choose. The hover **must not** collapse it to the first.
- **`block` / `owner_source`** — block-option only (see below).
- **`aliases[]`** — spelling variants (`M06L` ← `M06-L`), **not** hyphen-normalized (dashes are
  significant: `def2-SVP`, `NEB-TS`, `B3LYP-D4`).

## Owner derivation — union of two independent signals, with provenance

`block` is derived two independent ways, and `owner_source` records which one spoke:

- **`"text"`** — the option's home section carries **exactly one literal `%block` token** (`%scf
  MaxIter 200 end` in an annotated ` ```orca ` block, or the heading). Text, not inference. **Takes
  priority.**
- **`"structural"`** — text is silent, but the file has a unique `%block` (or a unique deepest
  `%block` ancestor by breadcrumb). Fills where text is silent.
- **`null`** — both are silent. `null` is a **value with meaning**, like `anchor_source =
  'undetermined'` in 4.3 — the section genuinely does not name a block, so the qualifier is *unknown*,
  not a hole to be filled by guessing.

**Why trust the union — the agreement number.** Where both signals resolve (936 targets), they agree
**98.5 %** (14 disagreements, 8 of them one cross-reference section). Two independent derivations
confirming each other is the same construction as **`objects.inv` × `predict_anchor`** (4.2): each is
a guess alone, together they are a post-condition. Because agreement is near-total on the intersection,
text-priority is safe there (it settles the 14 sensibly).

**⚠ Scope of the 98.5 % (corrected — `orca/manual-sources.md` Part E).** That number is measured **only
on the intersection** where BOTH signals resolve (936 targets). It is **not** a validation of the
structural proxy **outside** it. The **855 structural-only** targets had nothing to check against — and
a later measurement found the error sits exactly there: **537 of 814 structural targets (66.8 %) name
an owner the section body never mentions**, ~500 of them **basis-set / simple-keyword tables whose
entries are simple `!` keywords, not block-options** (`aug-cc-pV5Z`, `MINI`, `TightOpt`, `PrintBasis`).
So the structural half **is** a liability off the intersection; fixing it needs a **third signal** (a
section-title / body-text veto) in the owner derivation — a generator change with its own gate, a
separate unit.

**Cross-reference sections are null by rule.** A section titled "List of related keywords" / "See
also" **lists other blocks'** keywords — it references, it does not document. Both derivations there
answer the wrong question, so its options get `block: null` **by rule**. Measured: **2 sections**
(`spectroscopyproperties/nocv`, `.../mcd`) — units, not a category, but named so the null is by design,
not by an accidental tie.

## Consumer contract (fixed here, for the hover unit — not to be reinvented there)

**25.3 % of block-option targets have `block: null`** — they are **unreachable by qualified lookup**
(`%scf` + `MaxIter`). The rule for the next unit is fixed **here**:

> The hover does **not** fall back to an unqualified, bare-option-name search when the qualified
> lookup misses. Unqualified lookup is a **separate, deliberate path**, and its answer is *"documented
> in N places"* — a list — **not one section.**

This is the same posture as *"hover does not fall back to FTS"* in the ADR-013 amendment: without
writing it down, it gets replayed. A qualified miss that silently degrades to "first place `MaxIter`
appears" is exactly the confident-wrong-answer the whole layer exists to prevent.

## The stable section key — `(file, breadcrumb, title, nth)`

Each `sections` element is keyed by `(file, breadcrumb, title, nth)` — **not** `manual_sections.id`
(synthetic, reassigned per ingest → would slide a curated entry). The other candidates were measured
out (unit 4.4, [orca/manual-sources.md](../orca/manual-sources.md)): `anchor` NULL in 518;
`(file, title)` collides 140×; **`(file, breadcrumb, title)` collides once** (mreom's two identical
`## Perturbative MR-EOM-CCPT` H2 siblings), so `nth` disambiguates only where the triple repeats.
`line_start` is **not** in the key (diff churn).

## The bridge to the DB — descriptor → row (unit 4.4 Part B)

`keywords.json` references sections **by index into its own `sections` array** (a descriptor
`(file, breadcrumb, title, nth)`); `get_manual_section` takes a **DB synthetic id** — different spaces.
`index.rs::resolve_descriptor` bridges them: it matches `(orca_version, file, breadcrumb, title)`
ordered by `line_start` and returns the `nth`. **Post-condition (rule #9), specified in 4.4 and only
checkable now that there is a consumer — verified:** every one of the **317 descriptors resolves to
EXACTLY one row**, injectively (gate `cargo test keywords_bridge -- --ignored`: 317 → 317 distinct
rows, 0 failures). 0 matches (the manual moved) or an out-of-range `nth` is a `NotFound` error, never a
pick-first. The command `resolve_manual_section` also checks **`keywords.json.orca_version` == the
built index's version** — a stale map is reported, not silently resolved against a different corpus.

## Coverage — an explicit, named INVENTORY, and the honest number

The coverage gate improved twice by the number **falling**: 46/46 → 44/46 when it asked the consumer's
question (type, not string). The deepest flaw was left: we fixed the *form* of the question, not the
*population*. The 46 were what `input-builder` **emits**; the hover fires on what the author **types** —
domain guards (`! XTB GOAT`, `TightOpt` before `Freq`), the reaction chain (NEB-TS → OptTS → IRC).
Different sets.

**One home, named per word.** The expectation set lives in **`src/manual/keyword-inventory.json`**,
read by **both** gates (the Rust `generate_keywords_json` and the TS `coverage.test.ts`) — no second
list. Every entry carries a **`source`** (why it is here), so the set is arguable word-by-word, not
arbitrary: `builder` (`input-builder/` + `goatInputForFragment`), `template`, `domain` (ADR-014
guards), `workflow` (ADR-007 reaction chain). Populated from those named sources, **not** memory —
`MORead`/`PrintBasis` are deliberately **absent** (block-option-only in the map, but no named source),
so the inventory stays named, not merely longer.

**Honest number: 46 of 53 resolve** (type- and block-aware; Rust and TS agree), **split by CHANNEL so
the two never merge: 9 via SEED (the manual), 37 via CURATION (ours, attributed)**. A word without a
`gap` tag is a **hard** post-condition (must resolve); a `gap` word is a declared, classified hole —
**reported, never a panic**. The seven gaps, by closer:

| closer | count | words |
|---|---|---|
| **(a) `{numref}` layer** | 1 | `%maxcore` (block; it is in "List of Input Blocks") |
| **(b) curated (prose only)** | 3 | `IRC`, `ScanTS`, `NEB-CI` — run-types documented in chapter prose, no keyword-table entry (like `TightSCF`) |
| **(c) second/right form of a concept in the map** | 3 | `XTB`, `TightOpt` (exist as `%xtb`/`%method`, need the **simple** form) and `Constraints` (seeded under `%method`, the app emits it under **`%geom`**) |
| **(d) not in corpus at all** | 0 | — |

(`CPCM` was a gap; the curated overlay now covers it — a `!`-simple record pointing at the `%cpcm`
documentation — so its `gap` tag was removed.)

**Type comes from the manual, not from our builder (the fix).** `type_of` no longer consults
`app_simple`; the seed types a token `%…`→block, title-is-the-keyword→simple, else **`undetermined`**
resolved by the owner (a block-option needs a positive, body-confirmed owner). So a basis name and its
table-mate — `def2-QZVPP` and `ma-def2-SVP` — are **both `undetermined` in the SEED** (homogeneous; the
builder accident removed). The **curated channel** then asserts, WITH `provenance: "curated"`, that the
words OUR builder emits are simple: 22 flipped from `undetermined`, 11 added beside a block-option, 2
via alias, 5 stay seeded (their section title IS the keyword). Same bit as before — but attributed,
visible in the diff, arguable per word.

**`{numref}` still closes only 1 of the gaps** (`%maxcore`); the rest are curation (b, ×3) + second-form
(c, ×3). It is not the high-value next step for the words the project is built around.

### The recurring pattern — a check that measures US instead of the subject

State it precisely, because stated loosely it does harm: **not** "our knowledge is forbidden" (that
would reject entering what we know). The rule is: **our knowledge must not MASQUERADE as a measurement
of the subject matter — it has its own channel, with provenance.** The same defect has now appeared
**three times**, each a *mixing of channels*, not the presence of a second one:

1. **`%maxcore` "covered"** — the coverage check measured in *our* notation (string-normalised), so a
   `%maxcore` directive matched a `MAXCORE` block-option. (Fixed: ask the type/entity, not the string.)
2. **`46/46` held** — the expectation set was assembled from *our* builder's output, so it could only
   confirm what we emit. (Fixed: an explicit, sourced inventory — builder / domain / workflow.)
3. **`app_simple` inside `type_of`** — the seed's *type* was inferred from what our app emits, so a
   basis name was `simple` only if our builder happened to list it. (Fixed here: type from the manual;
   what we know moves to the `curated` channel with `provenance`.)

Each time the tell was the same — a number that looked like a fact about ORCA but was a reflection of
OrcaStudio — and each fix was the same shape: give our knowledge its own attributed channel and let the
check ask the manual. Named here so it is recognised a **fourth** time before it ships.

## The hover provider + drawer (the consumers)

`src/editor/orca-hover.ts` registers a Monaco hover provider and an `orca.openManualSection` command.
Three cases, kept apart (`keyword-lookup.ts::hoverContext`): `!`-line → simple; `%name` → block; inside
a block (`enclosingBlock`, plus a same-line `%pal nprocs …` check) → block-option of that block.
`aliases[]` are consulted (`M06-L`↔`M06L`). **Contract, enforced:** a qualified **miss returns `null`
→ no hover at all** (silence), never a bare-name or FTS fall-back — that is the panel's separate path.
A record whose type is **`undetermined`** answers **neither** a qualified block lookup **nor** a simple
one — it is invisible to the hover (silence), reachable only by the panel's unqualified path
(*"documented in N places"*), already a separate, deliberate channel.
An empty `summary` is not a reason to suppress (seeded records have none): the hover shows keyword,
type, owning block (+ `owner_source`), and the target's breadcrumb › title as an **Open** command-link;
several targets → *"documented in N places"* with a list, **not** a picked first. Clicking Open fires
the command → `ManualDrawer` resolves the descriptor via `resolve_manual_section` and shows the section
in a **side drawer that reuses the SAME `SectionView`** as `ManualScreen` — the author is not pulled
out of the editor. Word boundaries come whole via the language `wordPattern` (`def2-SVP`, `%maxcore`;
`wiki/orca/input-syntax.md`).

## What was seeded / deliberately deferred

Seeded from the **broad structured pool, home mappings only** — keyword tables (`:::{table}` pipe /
`{list-table}` / `:::{flat-table}`), annotated ` ```orca ` blocks, and keyword-titled sections
(`## RI-JK`, `## GOAT`). Functional-table token is the **2nd** column; MyST role backticks
(`{cite}\`…\``) are stripped; the appendix (change log / glossary) is excluded. **Deferred** (measured,
not lost): `{numref}`-target records (60 `%`-blocks in "List of Input Blocks", precision unmeasured)
and the ~21 prose "Keywords" sections (curation, not extractor input).

## Size and shape (measured)

**~0.56 MB, 2836 records, 317 sections.** Normalization collapsed **3173 target objects → 317
distinct** (10× duplication removed); on its own that would have reached ~0.25 MB, but qualification
**splits block-options by owner** (one `MaxIter` → 11 records, each with `block`/`owner_source`), which
trades part of that back **for correctness** — the surplus is now *qualified* identities, not
duplicated sections. `owner_source` over block-option records: **text 1204 / structural 802 / null
669**. 240 records are ambiguous (`targets[]`). Bundled with the frontend, parsed once into a lookup.

## Regenerating

```bash
cargo test generate_keywords_json -- --ignored --nocapture   # emits src/manual/keywords.json
# the measures behind it:
cargo test keyword_seed_measure   -- --ignored --nocapture   # A1 key, A2 sources, A3 coverage, A4 precision
cargo test keyword_seed_ambiguity -- --ignored --nocapture   # ambiguity % + 30% exit gate
cargo test owner_signal_measure   -- --ignored --nocapture   # literal %-token owner signal
cargo test owner_union_measure    -- --ignored --nocapture   # union coverage + the 98.5% agreement
```

Deterministic (byte-identical re-run; sections sorted by key, records by keyword then block). Curation
(hand summaries, aliases, prose homes) is layered on top of the seed. No DB schema, no migration: the
file bundles with the frontend and the hover does a map lookup, not a query.
