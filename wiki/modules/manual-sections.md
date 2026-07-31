# Module: Manual sectioner (`src-tauri/src/manual/`)

**Status:** built (unit 4.2). Turns the fetched ORCA manual Markdown into `Section`s and reads the
authoritative `objects.inv` anchor map. **This module writes nothing to the database itself** — storage
(the v9 `manual_sections` + FTS5) and search are `manual/index.rs` (unit 4.3,
[manual-index.md](manual-index.md)), which consumes `sectionize` + `verify_against_inventory`. Rust per
[ADR-013](../architecture/adr-013-manual-indexing-ownership.md) (3): text-to-structure without a
chemistry library, over Markdown, no HTML parser.

Files: `sections.rs` (sectioner + line-conservation), `objects_inv.rs` (Sphinx inventory v2 reader +
anchor cross-check), `mod.rs`, `tests.rs` (the corpus gate). One dependency added: `flate2` (already
transitive in `Cargo.lock` 1.1.9 — declaration only).

## Section definition (explicit — it is not self-evident)

A **section is one ATX heading whose body runs to the NEXT heading of ANY level.** Bodies are **not
nested**: the body of a page-level `#` is only its preamble down to the first `##`. This is what FTS
needs (a parent's text must not be duplicated into every descendant) and it is exactly what makes
**line conservation** checkable.

```
struct Section { file, level: u8, title, breadcrumb: Vec<String>,
                 labels: Vec<String>, anchor: Option<String>,
                 body: String, line_start, line_end }
```

- `breadcrumb` = ancestor titles (nearest-enclosing higher levels), root first, self excluded.
- `line_start` = the heading's line index; the body is `line_start+1 ..= line_end` (empty when equal).
- Headings are found **only outside fenced blocks** — the Rust port of the deny-list fence rule
  (`prose_mask`, same open/close rule as the Python `iter_prose_lines`, now removed from the script).
  A naive scan miscounts ORCA `#`-comment lines inside ` ```orca ` examples as headings
  (`wiki/orca/manual-sources.md`).

## Label-binding rule (load-bearing — verified, not assumed)

A MyST `(name)=` line **directly above** a heading marks that heading. Implementation: the labels are
the consecutive `(...)=` lines immediately above the heading, **blank lines skipped**, stopping at the
first other line, returned in document order. `anchor` is the slug (`predict_anchor`) of the label
**closest** to the heading, or `None` when unlabelled (Sphinx then auto-generates one from the title
slug — see the collision finding below).

## Anchors: `objects.inv` authoritative, `predict_anchor` the independent check

`objects_inv.rs` parses Sphinx **inventory v2** (four `#` header lines, then a zlib stream of
`name domain:role priority uri dispname`; a `uri` ending `$` expands to the name, a `dispname` of `-`
means "same as name"). `predict_anchor(label)` (`lowercase; each run of non-alphanumerics → one '-'`)
is computed **independently** and asserted equal to the inventory fragment on **every** label — the
two derivations must agree (rule #9). `verify_against_inventory` is library code (not a test-only
block), so 4.3 reuses it.

## Three post-conditions (in code, rule #9 — not just tests)

1. **Line conservation** — inside `sectionize`: sections + preamble tile the file's lines exactly
   once. A gap = a lost paragraph, an overlap = a line in two sections; either returns
   `SectionError::LineConservation` naming the file and indices. This is the manual's analogue of
   Phase 3's atom-count-and-order round-trip.
2. **Anchors** — `predict_anchor` vs `objects.inv` fragment, asserted per label; mismatches named.
3. **Label binding** — a label bound to section S in file F must have an `objects.inv` uri pointing at
   F; mismatches named.

Post-conditions 2–3 need the corpus + `objects.inv`, so they run (and fail the gate) in the `#[ignore]`
corpus test; post-condition 1 runs on every `sectionize` call, corpus or fixture. The lookup keys are
case-folded on both sides by one `normalize_label` (Sphinx lowercases label names), and the gate
prints `N mismatches out of M checked; K unchecked` — so a check that silently examined nothing can
never masquerade as PASS.

## Running the gate

```bash
# unit tests (fixtures, no network/corpus) — part of the normal run:
cargo test
# the corpus gate (reads resources/manual/<version>/**.md.txt + objects.inv):
cargo test manual_corpus -- --ignored --nocapture
```

`objects.inv` is fetched once by `python scripts/fetch-manual.py --objects-inv` into
`resources/manual/<version>/objects.inv` (gitignored, ADR-006). If it is absent the gate still runs
sectioning + line conservation and names the two anchor checks as skipped.

## Measured on the ORCA 6.1 corpus (the gate report — inputs for 4.3's schema)

- **Sections:** 1586 (`#`=129, `##`=654, `###`=604, `####`=193, `#####`=6). Per file: **min 1 / median
  7 / max 162**. Deepest breadcrumb **4** (e.g. `orca_2json > Available information > Electron
  Integrals > 2-electron integrals > … in AO basis`). Matches the fence-aware ATX recount exactly.
- **Body size:** median **1330 B**, p95 **9074 B**, max **48 245 B**. **27** empty-body sections
  (1.7%) — navigational (heading immediately over heading); 4.3 decides whether FTS indexes them.
- **Labels:** **1069** sections have ≥1 label; **517** do not (their anchor is a title slug). **140**
  label-less sections **collide on title-slug within a single file** (many `## Keywords`) — so an
  unlabelled section's title slug is **not** a unique key; 4.3 needs a disambiguator.
- **`objects.inv`:** 1671 entries (1450 `std:label`); 603 not ours. Of our 1069 heading labels,
  **1068 found, 0 anchor mismatches, 0 binding mismatches** — the anchor and binding rules both hold
  **1068 / 1068 checked**. Label keys are **case-folded on both sides** (`normalize_label`): Sphinx
  lowercases std-domain label names before writing the inventory, so a mixed-case source label
  (`sec:…BohrToAngs`) must be looked up case-insensitively (measured: 124 of a former 125 "not found"
  were case alone — see `manual-sources.md`). The **1 still not found**
  (`sec:spectroscopyproperties.nocv.theory`) is a genuine section label Sphinx did not register — the
  lone real gap, flagged for 4.3. The gate prints denominators (`0 mismatches out of 1068 checked; 1
  unchecked`) so a post-condition can never read PASS while silently having checked nothing.
- **Line conservation:** **126 / 126 files PASS.**
- **Bytes:** prose **57.3 %** / fenced **42.7 %** of 3 985 404 line-bytes — nearly half the corpus is
  inside fenced blocks (code/tables). 4.3 decides: index the raw body, or a cleaned prose projection.
