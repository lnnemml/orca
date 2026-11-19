# MCP server — the job classifier and input assembly (`src-tauri/src/mcp/`)

[ADR-027](../architecture/adr-027-mcp-orchestration-server.md), unit M1a. This module is the
guard that decides whether an AI model may spend compute. A `create_job` call ends in exactly one
of three dispositions: **REFUSED** (nothing is created), **PROPOSED** (a draft Anton runs by hand)
or **SUBMITTED** (charged to the active grant). The module also assembles the job's input, so the
model never writes a coordinate, a charge or a multiplicity (ADR-014 (1)).

The module is the **pure core only**: no DB, no Tauri command, no socket, no sidecar call. It is
registered in `lib.rs` under a scoped `#[allow(dead_code)]` until M1b and M3 call it.

**It fails closed.** A line outside the grammar is REFUSED (R1); a block, key or value outside the
allowlist is PROPOSED (P2); a source whose terminal state is unknown is REFUSED (R4); a memory
verdict that is not `Fits` is PROPOSED (P4); a grant that cannot be shown to cover the job is
PROPOSED (P5). Nothing reaches SUBMITTED by default.

## Files

| File | Contents |
|---|---|
| `mod.rs` | module docs and the submodule list |
| `header.rs` | `parse_header` → `ParsedHeader { keyword_lines, blocks, nested_openers, violations }`; `Block`, `SubBlock`, `BlockItem`, `Token` (`Word`/`Quoted`/`Braced`), `Violation` (`OutsideGrammar`, `Coordinates`) |
| `signature.rs` | `MethodSignature { keywords, solvation }` with `of_input` and `display`; `method_signature`; `NON_METHOD`; `is_pal_token` |
| `allowlist.rs` | `allowlist_findings` → `Vec<P2Finding { block, key, detail }>`; `constraint_entries` → `Vec<ConstraintEntry { text, index_tokens }>`; `parse_index` (ASCII digits within `u32`) |
| `charge_mult.rs` | `locate_charge_mult` → `Result<ChargeMult, ChargeMultError>` |
| `classify.rs` | `classify_mcp_job(&ClassifyInput) -> Disposition`; the input types and the typed reasons (below); `MAX_OPEN_PROPOSALS = 50`, `OPT_FAMILY`, `FREQ_TYPE`, `NEB_FAMILY` |
| `assemble.rs` | `assemble(header, maxcore_mib, atoms, charge_mult, source_label) -> Result<Assembled, AssemblyError>`; `comment_safe_label`; `AppSet`; `SERVER_DEFAULT_MAXCORE_MIB = 2000`; `COORD_TOLERANCE_ANG = 1e-6` |
| `*_tests.rs` | one test file per submodule |

## The classifier's input and output

`ClassifyInput` is plain data that M1b assembles before step A:

- `header: &ParsedHeader`;
- `source: &Result<ResolvedSource, String>`. A `ResolvedSource` holds:
  - `kind` (`Job`, `ScanFrame`, `NebImage`, `GoatMember`, `Molecule`, `Smiles`) and `label`;
  - `atoms: Vec<SourceAtom>` (element and Å coordinates, rule #11);
  - `charge_mult: Result<ChargeMult, ChargeMultError>`;
  - `terminal_success: Option<bool>`, `converged: Option<bool>`;
  - `opt_signature: Option<MethodSignature>`;
- `target: &Target` (`Local` or `Server { profile_id }`);
- `memory: MemoryVerdict` (`Fits`, `Warns`, `Unknown`);
- `grant: Option<&GrantSnapshot>` (status, targets, levels, `charged_jobs`, `max_jobs`, `in_flight`,
  `max_in_flight`);
- `open_proposals: u32`.

`Disposition` is `Refused(Vec<RefusedReason>)`, `Proposed(Vec<ProposedReason>)` or `Submitted`.
Every reason has `row()` (`"R1"` … `"P5"`) and `message()`. The REFUSED rows are checked first and
every matching one is listed; only if none matches are the PROPOSED rows checked, again all listed.

## The rows in code

| Row | Where | Rule |
|---|---|---|
| R1 | `header.rs` `Violation::OutsideGrammar` → `RefusedReason::OutsideGrammar` | a line that is not `!`, a `%…end` block, a `#` comment or blank; text after a block's `end`; a block that never closes; a `%` block, `!` line or `$` line inside a block; an unterminated quote or brace, a `#` inside one, a control character or non-ASCII whitespace; **any line with two or more `#`**; **any non-ASCII character on a `!` line**. The first line is quoted and the rest counted |
| R2 | `Violation::Coordinates`, and a `%coords` block anywhere | any `*` line, inside a block or not |
| R3 | `classify.rs` `header_rows` | a `%pal` or `%maxcore` opener anywhere, or a `!` token matching `^PAL\d*$` in any case |
| R4 | `source_rows` | unresolved; `terminal_success == Some(false)`; a job-derived source with `terminal_success == None`; `converged == Some(false)`; no atoms; charge/multiplicity unreadable |
| R5 | `source_rows` | an element unknown after case normalisation; or, with electrons = Σ Z − charge, multiplicity < 1, electrons < 0, odd (electrons + multiplicity − 1), or multiplicity − 1 > electrons |
| R6 | `source_rows` + `allowlist::constraint_entries` | an index token of **any** constraint entry (B/A/D/C) that `parse_index` cannot read as a `u32`, or that lies outside `[0, atom count)`. No index token is ever dropped: `{C 99999999999999999999 C}` is R6 |
| R7 | `has_scan` | a `%geom Scan` without an `OPT_FAMILY` token |
| R8 | `classify_mcp_job` | `open_proposals >= 50` |
| R9 | `header_rows` | a `!` token in `NEB_FAMILY` (`NEB`, `NEB-TS`, `NEB-CI`), any case |
| P1 | `classify_mcp_job` | a `Molecule` or `Smiles` source; the reason carries the charge, the multiplicity and where they came from (a SMILES always gives multiplicity 1) |
| P2 | `allowlist.rs` | see the allowlist below |
| P3 | `classify_mcp_job` | a `FREQ_TYPE` token with no `OPT_FAMILY` token, unless the source is a `Job` with `converged == Some(true)` and `opt_signature` equal to this job's signature |
| P4 | `classify_mcp_job` | `memory != Fits` |
| P5 | `grant_rows` | an empty method signature ("no method level stated", e.g. `! Opt Freq`, whatever the grant holds); no grant; expired; revoked; target not granted; signature not a granted level; `charged_jobs >= max_jobs`; `in_flight >= max_in_flight` |

## The header grammar and the scanner

`parse_header` lexes each line on its own. A `#` starts a comment. A `"…"` string and a `{…}`
constraint entry are single tokens, so a quoted `"end"` and a `# end` comment never close a block.
Inside `%geom`, `Constraints` and `Scan` open a sub-block that its own `end` closes, the sub-block
set of `src/scene/geomBlock.ts`. So `%geom … Constraints … end … end` and the one-line
`%cpcm smd true SMDsolvent "methanol" end` both parse. Other blocks have no sub-blocks here: a
sub-block of an unknown block leaves a stray `end`, which is R1 (fail closed, never SUBMITTED).

The parser never fails. It returns what it recognised and every violation, so the classifier can list
all REFUSED reasons at once.

**Two `#` on one line are refused (R1), wherever the line is.** The ORCA manual says a comment "can
also be closed by a second `#`" (`resources/manual/6.1/contents/essentialelements/input.md.txt`, the
`TolE=1e-5; #Energy conv.# TolMaxP=1e-6;` example). That is a source, not a run, so how ORCA 6.1.1
reads such a line is unmeasured (rule #10). Reading only up to the first `#` would hide
`! … Opt Freq #x# PAL16` from R3. Every comment-stripping reader in the module refuses the line
instead of guessing (`header::has_unmeasured_comment`):

- the lexer (R1);
- the charge/multiplicity locator, for every line it reads up to and including the header (R4);
- the assembly post-condition.

**A `!` line must be ASCII (R1).** The signature folds ASCII case only, as `NON_METHOD` does, so a
Unicode fold (`ſp` → `SP`) cannot drop a token. ORCA's keyword is `wB97X-3c`; the spelling
`ωB97X-3c` is refused.

## The method signature

The signature is the ASCII case-folded (upper-cased), sorted, de-duplicated union of every `!` token,
minus `NON_METHOD` and minus `^PAL\d*$`. Each `%cpcm` block's normalised text is added: `%cpcm`, then
its tokens ASCII-lowercased and joined by one space (quotes kept), then `end`. So SMD(DMF) and SMD(THF)
differ. An inline `CPCM(DMF)` is a `!` token and is kept as written, case-folded.

`NON_METHOD` is a copy of the set in `src/reactions/compare.ts`. `signature_tests::non_method_matches_compare_ts`
extracts the TS literal by regex and requires the two to be equal. This is **not** the ΔΔ table's
`methodSignature`, which adds only `SMD` for a `%cpcm` block. A divergence between the two can only
send a job to Anton.

## The allowlist (ADR-027 decision 6)

| block | admitted | record |
|---|---|---|
| `%geom` | `Calc_Hess true` | `orca/optts.md` |
| `%geom` | `MaxIter <positive integer>` (no upper cap) | `orca/gotchas.md` |
| `%geom` | `Constraints` holding only `{C n C}` entries (upper-case `C`; an index `parse_index` reads, ASCII digits within `u32`, written `0` or without a leading zero) | `orca/constraints.md` |
| `%cpcm` | `smd true`; `SMDsolvent "<name>"` (quoted; letters, digits, space, `-`, `,`, `(`, `)`, `'`) | `orca/solvation.md` (form 2) |

Everything else is a P2 finding that names the block and the key: another block, key or sub-block
(any `Scan`), another value (`Calc_Hess false`, `MaxIter 0`, an unquoted or path-like solvent), a
constraint with a value, a B/A/D constraint, and a key or block given twice. Two layouts no run
recorded are P2 too:

- an index with a leading zero (`{C 007 C}`, `00`): "index form not recorded". R6 still checks its
  numeric value, so `{C 008 C}` on 8 atoms is REFUSED;
- a key whose value is not on the same line (`Calc_Hess` then `true` on the next line): "key and
  value on separate lines (not recorded)".

## Charge and multiplicity from a source job

`locate_charge_mult` is a **strict** port of the locator in `sceneFromOrcaInput`. `#` comments are
masked first. The first `*` line with a keyword must be exactly `* xyz <int> <int>` (`xyz` in any
case, an optional sign, digits only). `* xyzfile`, `* int`, `* gzmt`, no such line, a non-integer, a
third value, a value hidden by a comment, or a line with two or more `#` before the header are each an
error, and the classifier refuses (R4).

TypeScript falls back to (0, 1) or returns null on those inputs. That suits the human path, which
shows the values before a run. The divergence is deliberate. Both locators read the shared cases file
`src-tauri/tests/fixtures/mcp/charge_mult_cases.json`:

- `charge_mult_tests.rs` requires Rust to match on `wellFormed` and to refuse every `malformed` case;
- `src/scene/charge-mult-parity.test.ts` requires TS to match on `wellFormed` and pins what TS gives
  on `malformed`.

## Assembly and its post-condition

`assemble` writes, in order:

1. the header (CRLF normalised to LF);
2. `%maxcore <value>`, through `memory::apply_maxcore`, which inserts it after the first `!` line;
3. `# charge/mult from <source label>`. The label goes through `comment_safe_label` first: ASCII
   letters and digits, space and `:_-./()=[]@+` are kept, and anything else becomes `?`. So
   `smiles:C#N` is written `smiles:C?N`, and the line holds exactly one `#`. `AppSet` still names the
   source as given;
4. the coordinate block of `orcastudio_core::emit::emit_coordinate_block`.

The block is built from a one-fragment `Scene` with ids 0..n, the charge on the fragment and the
multiplicity on the scene. `assemble` refuses before emitting if:

- the header already sets `%maxcore`;
- there are no atoms;
- an element is unknown;
- a coordinate is not finite.

**The post-condition (rule #9)** re-parses the text with its own strict reader. No line may hold two
or more `#`, and there must be exactly two `*` lines. The header is read by the charge/multiplicity locator, and every row must be
an element and three finite numbers up to a closing `*`. The result must match the source's atom
count, element order, coordinates (within 1e-6 Å), charge and multiplicity, and `%maxcore` must
read back. The emitter's `parse_coordinate_rows` is not used, because it skips a row it cannot read.
A mismatch is `AssemblyError::PostCondition`, and every `AssemblyError` becomes
`RefusedReason::Internal` through `as_refusal`.

The emitter is a seam (`assemble_with`). The tests hand it faulty emitters: a reordered block, a
dropped row, a changed element, a NaN row, an extra column, Bohr-scaled coordinates, a changed
`* xyz` line, a doubled block and a line with two `#`. The post-condition refuses each one.

## Clarifications (orchestrator, within ADR-027)

- **C1** — the charge/multiplicity locator is strict, as above. Its parity with TS covers
  well-formed cases only; the malformed cases are a documented divergence.
- **C2** — `NON_METHOD` parity: the regex extraction from `compare.ts`, with no TS change.
- **C3** — the method signature as above. The `%cpcm` text is added for **every** `%cpcm` block, not
  only an admitted one. A non-admitted one is P2 anyway, and adding it can only make signatures
  differ.
- **C4** — the Opt family is exactly `RELAXED_SCAN_OPT_KEYWORDS` of `src/scene/scan.ts`; `NormalOpt`,
  `COpt` and `OptH` are not Opt.
- **C5** — R3 covers `%pal`, `%maxcore` and `! PAL\d*`, with bare `PAL` and `PAL0` included.
- **C6** — element symbols are case-normalised before `z_of`; an unknown symbol is REFUSED.
- **C7** — the depth-tracked scanner with comments and quotes masked.
- **C8** — the memory verdict is the three-state `MemoryVerdict`; `Warns` and `Unknown` are both P4.
  `SERVER_DEFAULT_MAXCORE_MIB = 2000` mirrors `REMOTE_DEFAULT_MAXCORE_MIB`.
- **C9** — the assembly post-condition as above.
- **C10** — R6 checks the indices of every constraint entry, including those P2 would propose.
- **C11** — R9 is the `NEB_FAMILY` set.
- **C12** — R1 is the closed grammar, including text after `end` on the same line.

Two choices go beyond the clarifications, and each can only move a job toward Anton:

- **P3 counts `AnFreq`** as a frequency token, alongside `Freq` and `NumFreq`.
- **A `GoatMember` source never satisfies P3**: only a `Job` source that is a converged optimization
  does.

## Open (for Anton)

- **`MaxIter` has no upper cap.** A large value is admitted and SUBMITTED under a grant. The grant
  bounds job count and concurrency, not iterations.
- **Unbraced constraint entries** (`C 0 C` inside `Constraints`) are P2 and are **not**
  range-checked by R6. A proposed draft could carry an out-of-range index to Anton's Run.
- **`Scan` atom indices** are P2 and are **not** range-checked by R6, with the same consequence.
- **How ORCA 6.1.1 reads a second `#`** on a line is unmeasured (a prober run). Until then, such a
  line is R1.

## Not yet wired

- **M1b:**
  - the source resolver over the DB and the readers, plus the SMILES sidecar call;
  - `%maxcore` and the `MemoryVerdict` before step A;
  - schema v24, the grant create and revoke commands;
  - the `submit_job` `mcp_state` guard and the reconciliation.
- **M2:** the socket server, the relay and the read tools.
- **M3:** `create_job` (steps A–C around `classify_mcp_job` and `assemble`), the audit log, and R8's
  live count.

## Tests

`cargo test -p orcastudio --lib mcp::` runs the module's tests, and `npx vitest run
src/scene/charge-mult-parity.test.ts` runs the TS half of the C1 parity. The negative controls, one
per row plus the ADR's M1a list, are recorded in `log.md` (2026-11-19, the M1a entry).
