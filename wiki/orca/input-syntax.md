# ORCA input block syntax — measured facts for the hover qualifier (unit 4.4 Part A)

**Measured 2026-08-01** over the **1477 ` ```orca ` code blocks of the ORCA 6.1 manual itself**
(`resources/manual/`) — a real, large validation corpus, not hand-picked examples. These are ORCA
facts, established by a run (rule #10), that the editor's `enclosingBlock` scanner
(`src/editor/enclosing-block.ts`) is built around. Genre: like `parse-sources.md`.

## Why a scanner at all — the Monarch tokenizer is stateless

`src/editor/orca-language.ts` has **one `root` state, no `@push`/`@pop`**. It colours `%scf` and
`end` but has **no notion of "cursor inside `%scf`"**, so Monaco cannot hand a block qualifier to the
hover provider. The qualifier — which `%block` encloses the cursor — must be derived by a **pure
function over the input text** (`enclosingBlock`), tested without Monaco. Do **not** add state to the
Monarch grammar: highlighting works, and the state we need is a different shape (a scan, not colouring).

**Why it must be conservative.** A scanner that thinks the cursor is in `%scf` when it is in `%casscf`
makes the hover show the **wrong** section **confidently** — a qualified lookup finds a `(block,
option)` record either way, and a wrong answer about `MaxIter` in CASSCF looks exactly like the right
one. The whole `keywords.json` design (a block-option is a *qualified name*) rests on the qualifier
being correct. So `enclosingBlock` returns **`null` on any ambiguity** rather than guess.

## Block openers: three forms, and the trap

An ORCA `%name` at line start is **not always a block that closes with `end`.** Measured over 755
openers in the corpus ` ```orca ` blocks:

| form | count | example |
|---|---|---|
| **multi-line block** (`… end` later) | **682** | `%scf` ⋯ `end` |
| **single-line block** (opens + `end` same line) | **27** | `%pal nprocs 4 end` |
| **no-`end` directive** (one line, never an `end`) | **46 (6.1 %)** | `%maxcore 3000`, `%moinp "x.gbw"`, `%base "…"`, `%pointcharges "…"` |

**The trap (this is the number that shapes the scanner).** A naive "every `%name` opens a block"
scanner leaves everything after a `%maxcore 3000` **forever "inside %maxcore"**. Measured: such a
naive scanner ends with a **non-empty stack on 42 of 1477 blocks (2.8 %)** — and every one is a
no-`end` directive dirtying the stack. So the scanner must **not** treat `%name` as an opener by
shape; it classifies a `%name` as a block **only when a matching `end` follows** (a forward scan
counting nested `%`-opens). Built this way, the scanner opens only what closes, so it can never run
away: **0 of 1477 blocks leave an open stack** — no-`end` directives correctly open nothing, and a
genuinely truncated example fragment is treated as a directive too (conservative, never a wrong block).

## Nesting is real (verified, not assumed)

ORCA blocks nest, and **sub-blocks open with a bare word (no `%`)**, closed by `end`:

```orca
%geom
  Constraints
    {B 0 1 C}
  end          # closes Constraints
  MaxStep 0.1
end            # closes %geom
```

`%geom … Constraints … end end` — two `end`s, the first for the bare `Constraints` sub-block (this is
exactly what `scene/constraints.ts::locateGeom` already tracks). Since bare sub-block openers aren't
`%`-prefixed, the scanner does not track them; a sub-block's `end` pops the enclosing `%block`
**early**, so a token after the sub-block resolves to **`null`** (conservative) rather than a wrong
block. A token *inside* the sub-block still resolves to the enclosing `%block` (`%geom`), which is
correct — that is where its options are documented.

## Other structure the scanner must not misread (each a test case)

- **Coordinate block** `* xyz 0 1` … closes with `*`, **not `end`**; its interior is not in any
  `%block`. External forms (`* xyzfile 0 1 mol.xyz`) are self-contained (no toggle).
- **`#` comments** can contain `%` and `end` — masked out before scanning.
- **Quoted strings** (`%moinp "prev_%scf_end.gbw"`) can contain `%`/`end` — blanked before scanning.

## Word boundaries — Monaco splits ORCA tokens (needs a `wordPattern`)

Monaco's **default** word definition (reconstructed verbatim from
`node_modules/monaco-editor/.../wordHelper.js` `USUAL_WORD_SEPARATORS`, not from memory) treats
`` ` ~ ! @ # $ % ^ & * ( ) - = + [ { ] } \ | ; : ' " , . < > / ? `` as separators. So
`getWordAtPosition` **splits ORCA keywords** — measured:

| token | default → | fixed `wordPattern` → |
|---|---|---|
| `def2-SVP` | `def2`, `SVP` | `def2-SVP` |
| `NEB-TS` | `NEB`, `TS` | `NEB-TS` |
| `M06-2X` | `M06`, `2X` | `M06-2X` |
| `%maxcore` | `maxcore` (drops `%`) | `%maxcore` |
| `def2/J` | `def2`, `J` | `def2/J` |
| `RIJCOSX` | `RIJCOSX` (only survivor) | `RIJCOSX` |

`def2` handed to the lookup instead of `def2-SVP` is a **miss that looks exactly like "not in the
map"** — the silence-masking-an-error the consumer contract forbids. **Applied (4.4 Part B):** the
language configuration now sets `orcaWordPattern =
/(%?[A-Za-z][A-Za-z0-9]*(?:[-_./][A-Za-z0-9]+)*)|(-?\d*\.\d\w*)/`, keeping `- _ . / %` inside words;
tested on all six tokens (`src/editor/orca-language.test.ts`).

## Lookup coverage on OUR text (the Phase-1 template library)

First check of `keywords.json` against text **we** wrote, not the manual. Every simple `!` keyword the
templates emit resolves: `B3LYP` (3 targets), `RIJCOSX` (2), `r2SCAN-3c` (2), `def2-SVP` (1), `D4` (1),
`TightSCF` (1), `def2/J` (1). Of the `%`-blocks: **`%pal` ✓, `%geom` ✓, but `%maxcore` is ABSENT** —
it is a no-`end` directive that lived in the `{numref}`-deferred "List of Input Blocks" layer and was
never seeded as a home record. A hover on `%maxcore` therefore **misses → stays silent** ("not in the
map"), which is the *correct* behaviour under the consumer contract (silence, not a wrong section), but
it names a real coverage hole for a very common directive — a curation target, recorded here.

## Rendering piggyback — pipe tables outside ` ``` ` fences

`SectionView` fences only ` ``` ` blocks; a MyST pipe table (often inside a `:::{table}` colon
directive) therefore renders as **proportional-font prose with `word-break`**, so its columns
misalign — characters are all present (the preservation test still holds) but it is hard to read.
Measured: **110 of 1558 sections (7.1 %)** carry a pipe-table line outside a ` ``` ` fence, **26 of
them Keyword-titled**. Substantial, not units — so the follow-up routes a pipe-table line (`^\s*\|`)
into the same monospace `<pre>` as a fence: **the same linear line check, no parser, the preservation
test unchanged.**
