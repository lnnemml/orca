# MCP server — the job classifier, input assembly and grant core (`src-tauri/src/mcp/`)

[ADR-027](../architecture/adr-027-mcp-orchestration-server.md), units M1a and M1b. This module is the
guard that decides whether an AI model may spend compute. A `create_job` call ends in exactly one
of three dispositions: **REFUSED** (nothing is created), **PROPOSED** (a draft Anton runs by hand)
or **SUBMITTED** (charged to the active grant). The module also assembles the job's input, so the
model never writes a coordinate, a charge or a multiplicity (ADR-014 (1)).

The module has three layers. The **pure core** (M1a) is the classifier, the header grammar, the method
signature, the allowlist, the charge/multiplicity locator and the input assembly. The **DB-level
cores** (M1b) are schema v24, the grant core, steps A and C, the crash reconciliation, the
`submit_job` guard, the source resolver and the memory preflight; each is a `*_conn` function or
takes `&DbState`. The **app wiring** (M1b Part B) is the grant's Tauri commands (`commands/mcp.rs`),
the guard in the two commands that submit a job by hand, and `SidecarSmiles`, the resolver's SMILES
call over HTTP. There is no socket and no MCP tool yet. The module stays registered in `lib.rs` under a
scoped `#[allow(dead_code)]`, because steps A/C, the resolver, the reconciliation and `SidecarSmiles`
are called only from M2 and M3.

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
| `assemble.rs` | `assemble(header, maxcore_mib, atoms, charge_mult, source_label) -> Result<Assembled, AssemblyError>`; `comment_safe_label`; `AppSet`; `SERVER_DEFAULT_MAXCORE_MIB = 2000`; `COORD_TOLERANCE_ANG = 1e-6`; `read_coordinate_block`, the strict `* xyz` block reader shared with the resolver |
| `grant.rs` | `create_grant_conn`, `revoke_grant_conn`, `get_grant_conn`, `active_grant_conn`, `grant_snapshot_conn -> Option<CurrentGrant { id, snapshot }>`, `in_flight_conn`; `Grant` (with `status_at(now)`), `NewGrant`; `target_text`/`target_of` (`"local"` or a profile id); `begin_immediate`, `now_utc_conn`, `check_utc_text` |
| `steps.rs` | `charge_and_insert_conn -> StepA` (step A), `insert_proposed_conn`, `finish_submit_conn -> StepC` (step C), `reconcile_submitting_conn -> Reconciled`, `submit_refusal_for_mcp_state`, `refuse_while_mcp_submitting`; `McpReason { row, message }`; `SUBMITTING` |
| `resolve.rs` | `SourceRef::parse`, `resolve_source(&DbState, reference, &dyn SmilesTo3d)`, `resolve_db_conn`, `header_of_input`; the `SmilesTo3d` seam and `SmilesStructure` |
| `sidecar_smiles.rs` | `SidecarSmiles: SmilesTo3d` (`from_status(&SidecarStatus)`), `check_reply`; `SMILES_CONNECT_TIMEOUT` (5 s), `SMILES_TIMEOUT` (30 s) |
| `preflight.rs` | `mcp_maxcore -> McpMaxcore { mib, source }`, `memory_verdict_for -> MemoryVerdict`, the pure `verdict_of` |
| `*_tests.rs`, `test_db.rs` | one test file per submodule, plus `submit_guard_tests.rs` for the guard's wiring; `test_db.rs` builds a migrated scratch database per test |
| `../commands/mcp.rs` | the grant commands `mcp_create_grant`, `mcp_revoke_grant`, `mcp_get_grant`, `mcp_level_from_job` and their `*_core`/`*_conn` bodies; `GrantView`, `GrantState`. Tests in `commands/mcp_tests.rs` |

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
| R5 | `source_rows` | an element unknown after case normalisation; or, with electrons = Σ Z − charge, multiplicity < 1, electrons < 0, odd (electrons + multiplicity − 1), or multiplicity − 1 > electrons. The arithmetic is checked: a charge or multiplicity that overflows the count is R5 (`ChargeMultOutOfRange`), never a wrapped number |
| R6 | `source_rows` + `allowlist::constraint_entries` | an index token of **any** constraint entry (B/A/D/C) that `parse_index` cannot read as a `u32`, or that lies outside `[0, atom count)`. No index token is ever dropped: `{C 99999999999999999999 C}` is R6 |
| R7 | `has_scan` | a `%geom Scan` without an `OPT_FAMILY` token |
| R8 | `classify_mcp_job` | `open_proposals >= 50` |
| R9 | `header_rows` | a `!` token in `NEB_FAMILY` (`NEB`, `NEB-TS`, `NEB-CI`), any case |
| P1 | `classify_mcp_job` | a `Molecule` or `Smiles` source; the reason carries the charge, the multiplicity and where they came from (a SMILES always gives multiplicity 1) |
| P2 | `allowlist.rs` | see the allowlist below |
| P3 | `classify_mcp_job` | a `FREQ_TYPE` token with no `OPT_FAMILY` token, unless the source is a `Job` with `converged == Some(true)` and `opt_signature` equal to this job's signature |
| P4 | `classify_mcp_job` | `memory != Fits`. The `Unknown` message reads for either target: a server's facts come from its connection test, this machine's from `/proc/meminfo` and the data directory |
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

**A grant level is read strictly.** `MethodSignature::of_level_text` refuses a level whose text has
any grammar violation (`LevelError::OutsideGrammar` or `Coordinates`, naming the first line), so a
refused line is never silently left out of the signature. It also refuses a level with no method token
(`NoMethodTokens`: `! Opt Freq`, a lone `%cpcm` block). The lenient `of_input` exists only in test
builds.

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

## Schema v24

`db.rs` migrates to v24 additively (full text in `modules/tauri-core.md`):

- `jobs.origin` (`'user'` or `'mcp'`, default `'user'`), `jobs.mcp_reasons` (a JSON array of
  `McpReason { row, message }`), `jobs.grant_id` (FK to `mcp_grants`, only on an MCP job) and
  `jobs.mcp_state` (only `'submitting'`, only on a charged job). `Job` carries all four;
  `JobOrigin` is `User` or `Mcp`.
- `mcp_grants`: `id`, `created_at`, `expires_at`, `revoked_at`, `targets` (JSON strings: `"local"` or a
  profile id), `levels` (JSON `MethodSignature`s), `max_jobs`, `max_in_flight` (both ≥ 1) and the
  stored `charged_jobs` (`0..=max_jobs`, a CHECK). The **expression** index
  `mcp_grants_one_unrevoked ON mcp_grants((revoked_at IS NULL)) WHERE revoked_at IS NULL` keeps at most
  one grant unrevoked.
- `mcp_audit`: `id`, `at`, `tool`, `args` (≤ 8192 characters, a CHECK), `disposition` (`refused`,
  `proposed`, `submitted` or NULL), `reasons`, `job_id`, `grant_id`. M3 writes it.

Times are `datetime('now')` text, UTC on the laptop's clock. `expires_at` is compared as text, so every
writer checks the exact `YYYY-MM-DD HH:MM:SS` form first (`check_utc_text`).

## The grant core (`grant.rs`)

Every write is a `BEGIN IMMEDIATE` transaction (`begin_immediate`), so two app instances on one file
are ordered by SQLite's write lock. A grant is created only by Anton, through the grant commands below;
no MCP tool can create one.

- **`create_grant_conn(conn, &NewGrant, now)`** checks everything before writing: each level through
  `of_level_text`, at least one level and one target, `max_jobs` and `max_in_flight` ≥ 1, `expires_at`
  in the exact form and after `now`, each server target an existing profile (and none spelled
  `local`). Then, in one transaction, it revokes whatever grant is unrevoked and inserts the new one.
  A refused create writes nothing, and the previous grant stays active.
- **`revoke_grant_conn(conn, id, now)`** is idempotent: the first `revoked_at` is kept.
- **`active_grant_conn(conn, now)`** is the unrevoked grant with `expires_at > now`.
- **`grant_snapshot_conn(conn, now)`** returns the newest grant (the unrevoked one, else the most recent
  revoked one) as `CurrentGrant { id, snapshot }`. The status is `Grant::status_at(now)`: `Revoked`,
  `Expired` (`expires_at <= now`) or `Active`. The Settings view reads the same function.
- **The counters.** `charged_jobs` is stored: step A increments it and only a refund decrements it, so
  deleting a job never re-opens the budget. `in_flight` (`in_flight_conn`) is derived: every job with a
  non-null `grant_id` whose status is not terminal, under **any** grant, a draft in step B included.
  The terminal list comes from `JobStatus::is_terminal` (completed, parsed, failed, cancelled, lost).

## Steps A and C (`steps.rs`)

Only `charge_and_insert_conn` and `insert_proposed_conn` write `origin = 'mcp'`. Both insert through
the app's own `commands::jobs::create_job_conn`, so an MCP job mints its index map like any job, and
stamp the MCP columns in the same transaction.

- **Step A, `charge_and_insert_conn(conn, McpJobDraft { title, input_content, target, signature })`.**
  In one `BEGIN IMMEDIATE` transaction it reads the time (`datetime('now')`, after the write lock is
  granted, never from the caller, so a grant that expired while step A waited on the lock is
  PROPOSED), re-reads the grant snapshot and re-runs the P5 rows
  (`classify::grant_problems`, the classifier's own code). If the grant still covers the job, the job is
  inserted as a `draft` with `origin = 'mcp'`, `grant_id` and `mcp_state = 'submitting'`, and
  `charged_jobs` is incremented (`StepA::Charged`). Otherwise it is inserted as PROPOSED with the
  reasons and no grant (`StepA::Proposed`). Its post-condition: exactly one grant row was charged and
  `in_flight` is still within `max_in_flight`; a breach rolls the whole step back.
- **`insert_proposed_conn(conn, title, input, reasons)`** inserts a PROPOSED draft. It refuses an empty
  reason list.
- **Step C, `finish_submit_conn(conn, job_id, grant_id, outcome_message)`.** In one transaction it
  re-reads the row and always clears `mcp_state`. A row still in `draft` and charged to `grant_id` is
  refunded: `grant_id` set to NULL, `charged_jobs` decremented, and the reason `submit refused: …`
  appended (`StepC::Refunded`). Any other row keeps its charge (`StepC::Kept { status }`); a missing row
  is `StepC::Gone`. Step C is idempotent: a refund clears the row's `grant_id`, so a second call on the
  same job finds it no longer charged to the grant and refunds nothing.

## The reconciliation (not yet called)

`reconcile_submitting_conn(conn)` turns every `draft` with `mcp_state` set into PROPOSED: refunded,
with the reason `interrupted before submit`. Every other row with the flag only has it cleared. It
returns the counts (`Reconciled { refunded, cleared }`). Nothing calls it yet: only the instance
holding the `mcp.lock` `flock` may run it, right after taking the lock and before serving (M2).

## The grant commands (`commands/mcp.rs`)

Settings → MCP (M4) calls these, and they are the **only** way a grant is created. Each is a
synchronous command over a short `BEGIN IMMEDIATE` transaction, like the other short database
commands. Each body is a `*_core`/`*_conn` function that reads `now` from SQLite (`now_utc_conn`) and
wraps the grant core.

| command | returns | what it does |
|---|---|---|
| `mcp_create_grant(targets: Vec<String>, levels: Vec<String>, max_jobs: u32, max_in_flight: u32, expires_at: String)` | `GrantView` | `create_grant_conn`; a target is `"local"` or a profile id (`target_of`); each level is pasted text |
| `mcp_revoke_grant(id)` | `GrantView` | `revoke_grant_conn`; idempotent, `NotFound` for an unknown id |
| `mcp_get_grant()` | `Option<GrantView>` | the grant `grant_snapshot_conn` reads (the unrevoked one, else the newest revoked one); `null` before any grant |
| `mcp_level_from_job(job_id)` | `String` | "this job's level": the job header's signature (`header_of_input` + `of_level_text`), written back with `MethodSignature::level_text` |

`GrantView` holds `id`, `state` (`"active"`, `"expired"`, `"revoked"`, from `Grant::status_at`),
`created_at`, `expires_at`, `revoked_at`, `targets` (`"local"` or a profile id), `levels` (each
`MethodSignature::display`), `max_jobs`, `max_in_flight`, `charged_jobs` and `in_flight`. Times are UTC
on the laptop's clock in the `YYYY-MM-DD HH:MM:SS` form; `expires_at` must be in that form (M4
converts).

**Errors.** Every refusal is the grant core's `AppError`, and nothing is written:
- a level outside the grammar or with no method token is `Invalid("level <n>: …")`;
- a count below 1 or an `expires_at` not after now, or not in the exact form, is `Invalid`;
- a server target with no profile is `NotFound`.

**"This job's level".** The header is the input outside its coordinate block, with the app's
`%maxcore` line set aside. It is read strictly, so a header with an unrecorded directive (`%moinp`) or
no method token is `Invalid`, and so is a job whose input has no `* xyz` block. The text is one `!` line
of the method tokens (upper-cased, sorted), then each normalised `%cpcm` block on its own line. **Its
post-condition (rule #9):** the text read back through `of_level_text` must give the same signature,
else `Internal`. Run-type, SCF and print tokens are not part of a level (`NON_METHOD`).

## The `submit_job` guard

`submit_refusal_for_mcp_state(&Job)` returns `AppError::Conflict` for a job whose `mcp_state` is set
(any value; the CHECK admits only `submitting`), and `None` otherwise. `refuse_while_mcp_submitting`
is the same check as a `Result`.

**Where it sits: in the two commands that submit a job by hand, not in the backends.** Step B calls
the backends on the very job the guard refuses, so a guard there would refuse Claude's own submit
(ADR-027 decision 5).

| entry point | how it reaches a submit | guard |
|---|---|---|
| `submit_job` (Run, Create & Run, every "run all" in the views: `ConnectivityPanel`, `ScanProfilePanel`, `ScanSurface2dPanel`, `NebBandPanel`, `JobDetailScreen`) | local arm: `LocalBackend::submit` → `local_backend::submit` (`draft` → `queued`); remote arm: `submit_to_server` → `SshBackend::submit_attempt` → `ssh_backend::submit_remote` → `persist_submit` | the job is read through `commands::jobs::submittable_job_conn` **before** `Backend::for_submit`, so both arms are covered |
| `retry_remote_submit` (Retry) | `SshBackend::retry` → `ssh_backend::resubmit_remote` | `refuse_while_mcp_submitting(job)` on the row `run_guarded` read under the job's in-flight guard, before the backend is built; no ssh call runs and the record is left as it was |
| the poller's pending-submit re-check (ADR-024 o23, `poller/pending.rs` → `pending_plan`) | not an entry point: it continues the one submit step B started (step B's slot refusal made the job pending) | none needed: it never moves a `draft`, and step C judges the row, which stays out of `draft` |
| the re-enqueue (ADR-024 o25, `poller/reenqueue.rs` → `reenqueue_plan`) | not an entry point: it re-sends a job already enqueued on the server | none needed, as above |
| `try_start_next` (the local queue) | not an entry point: it starts a job already `queued` | none needed |

Every frontend path that submits calls one of the two commands; no Rust code outside them moves a
`draft` to `queued` except `local_backend::submit` and `persist_submit`, which only these commands and
step B reach.

**Why one read outside the job's lock is enough.** `mcp_state` is set only by step A, in the
transaction that inserts the row, and only ever cleared after that. A job read with `mcp_state` NULL can
never become `submitting` later, so the check stays true for the rest of the command. The remote arm's
row is read again under the in-flight guard by `submit_to_server`, unchanged.

Tests (`mcp/submit_guard_tests.rs`): the guarded read refuses a `submitting` job and passes a user
draft; the job submits by hand once step C has cleared the flag; the retry's guard; the place of each
guard in `submit_job` and `retry_remote_submit`, pinned in the source (these commands need an
`AppHandle`, as `cancel_job`'s test does); and `the_backends_carry_no_mcp_guard`. The existing submit
tests (`a_submit_with_no_target_never_reaches_the_ssh_path` and the remote-command pins in
`commands::remote_jobs`) are unchanged.

## The source resolver (`resolve.rs`)

`resolve_source(db, reference, sidecar)` parses the reference (`SourceRef::parse`) and resolves it. It
locks the database only for a DB-backed reference, never for a SMILES. `Err` carries why the
reference did not resolve, and the classifier turns it into R4.

| reference | geometry | charge / multiplicity |
|---|---|---|
| `job:<id>` | `results.final_geometry` | the job's `* xyz` line, strict (`locate_charge_mult`) |
| `job:<id>/scan:<k>` | scan point k (`read_scan_geometries`) | the same |
| `job:<id>/neb:<k>` | MEP image k (`read_neb_geometries`) | the same |
| `job:<id>/neb:ts` | the converged TS (`results.neb.ts_geometry`) | the same |
| `job:<id>/goat:<k>` | ensemble member k (`commands::jobs::read_ensemble_text`, shared with `read_job_ensemble`) | the same |
| `molecule:<id>` | the molecule's xyz | its `charge` and `multiplicity` columns |
| `smiles:<s>` | the `SmilesTo3d` seam (the sidecar's `/smiles-to-3d`) | the sidecar's |

**Every `k` is 0-based**, like the atom indices: `scan:0` is ORCA's `input.001.xyz`, `neb:0` the
reactant end of the MEP, and `goat:0` the ensemble's first member (the `source_conformer_index`
convention). An id is printable ASCII with no `/`. A SMILES is 1 to 1000 printable ASCII characters
with no space; it may contain `/`.

**Per-source decisions:**

- **`terminal_success`** is the parent job's status for every job-derived source: `Some(true)` for
  `completed` or `parsed`, `Some(false)` for `failed`, `cancelled` or `lost`, and `None` while the job is
  not terminal. Both of the last two are R4. No geometry is read from a job that did not end in success.
- **`converged`**: `job:<id>` takes `ParsedResults.converged`, so a non-converged optimization is R4. A
  scan frame, a NEB image or TS, and a GOAT member are `None`: none is a converged optimization for P3,
  so a Freq from one is PROPOSED.
- **`opt_signature`**: for `job:<id>` whose header has an Opt-family token, the signature of the header
  (the input outside its coordinate block), read strictly by `of_level_text`. The app's own
  `%maxcore <n>` directive, which has no `end`, is set aside first by the memory preflight's locator,
  and only when it reads cleanly. A header the grammar refuses gives `None`, so P3 fails closed. Every
  frame source is `None`.
- **A constrained optimization is not a stationary point** (Anton, 2026-11-19). A `job:<id>` whose
  `%geom` holds a `Constraints` or `Scan` sub-block, or any key beyond `Calc_Hess`, `Recalc_Hess` and
  `MaxIter` (with `true`/`false`/integer values), has `opt_signature = None`, so a Freq from it is P3
  PROPOSED (`geom_leaves_a_stationary_point`). Any doubt about the `%geom` block makes the source
  ineligible.
- A molecule and a SMILES have `terminal_success`, `converged` and `opt_signature` all `None`; P1 sends
  them to Anton.

**Post-conditions (rule #9).** A job-derived geometry's element sequence must equal the source job's own
`* xyz` block, element for element; otherwise the source does not resolve. Every reader keeps the
input's atom order (measured for GOAT in `orca/goat.md`; asserted for NEB in `parse/neb.rs`; the scan
points and the final geometry are verified at parse time). The check is skipped only when the charge line
itself is unreadable, which R4 refuses anyway. A SMILES reply's atom count must equal its `num_atoms`.

The SMILES seam is a trait, so the resolver's tests use a stub and need no sidecar. The app's
implementation is `SidecarSmiles` (below).

## The SMILES call (`sidecar_smiles.rs`)

`SidecarSmiles` is the first Rust → sidecar call. It posts `{"smiles": "<s>"}` to
`http://127.0.0.1:<port>/smiles-to-3d` (`sidecar/app/smiles.py`, `SmilesToXyzRequest`) with `ureq`, 5 s
to connect and 30 s for the whole request. It is built from the sidecar's status
(`SidecarSmiles::from_status(&SidecarManager::get_status()?)`). A sidecar never started (no port) or
`down` is an `Err` with no request made; any other health is called.

An HTTP error status, a 2xx other than 200, a transport failure or a timeout is an `Err` naming the
cause. The call blocks, so its caller (M3) runs it off the main thread; the resolver never holds the
database lock across it. No command calls it yet.

**The post-condition (rule #9), `check_reply`.** A reply that is "200 OK" with nonsense is an `Err`,
never a geometry:
- the body is JSON with `xyz` text and `charge`, `multiplicity`, `num_atoms` as JSON integers (a `1.0`,
  a string or a `null` is refused); unknown fields are ignored;
- `num_atoms > 0`;
- `multiplicity == 1`. The sidecar's contract says so, and so does P1's message ("the SMILES sidecar,
  which always gives multiplicity 1"); any other value would make that message false;
- `xyz` is exactly one frame: a count line equal to `num_atoms`, a comment line, `num_atoms` rows of
  exactly `element x y z`, then only blank lines (a fragment suffix like `O(1)` is refused);
- every element is known to `z_of` after case normalisation (C6);
- every coordinate is finite (Rust's `f64` parser accepts `NaN` and `inf`).

The coordinates are Å (the xyz format's unit) and pass on as text; `resolve_smiles` reads them with
the app's xyz reader and checks the count again. The tests serve canned HTTP responses from a
throwaway `TcpListener` on `127.0.0.1:0`, so no sidecar runs.

## The memory preflight (`preflight.rs`)

Both functions are computed before step A and take **no lock of their own**: `memory_estimate_with`
and `local_fitted_maxcore_with` lock `DbState` themselves.

- **`mcp_maxcore(db, meminfo, data_dir, header, target)`.** For Local it is `local_fitted_maxcore_with`'s
  default (`LocalFitted`). For a server it is the 5.5 proposal P from the stored facts and the core mask
  (`ServerProposal`), else `SERVER_DEFAULT_MAXCORE_MIB` (`ServerDefault`: unknown memory, no mask, or
  P below 300).
- **`memory_verdict_for(db, meminfo, data_dir, input, target)`**, on the assembled input. A RAM or disk
  warning gives `Warns`; a missing RAM fact, disk fact or estimate gives `Unknown`; only all known and
  quiet gives `Fits`.

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

## Decided by Anton

- **P3's same-level rule** is confirmed (2026-11-19). A deliberate exception, such as a cheaper
  Hessian for an OptTS, goes through PROPOSED.
- **A constrained optimization is not a stationary point** (2026-11-19). A source optimization with
  `%geom` constraints or a scan never satisfies P3 (see the resolver).
- **The `MaxIter` cap** is deferred to M5 and the core-hour budget (2026-11-19). Until then a large
  value is admitted and SUBMITTED under a grant; the grant bounds job count and concurrency, not
  iterations.

## Open (for Anton)

- **Unbraced constraint entries** (`C 0 C` inside `Constraints`) are P2 and are **not**
  range-checked by R6. A proposed draft could carry an out-of-range index to Anton's Run.
- **`Scan` atom indices** are P2 and are **not** range-checked by R6, with the same consequence.
- **How ORCA 6.1.1 reads a second `#`** on a line is unmeasured (a prober run). Until then, such a
  line is R1.

- **Source headers with other one-line directives** (`%moinp "x.gbw"`, `%base`) give no
  `opt_signature`, so a Freq from such a job is PROPOSED. Only `%maxcore` is set aside.

## Not yet wired

- **M2:** the socket server, the relay, the read tools, and the lock holder calling
  `reconcile_submitting_conn`.
- **M3:** `create_job` as one tracked task (the preflight, step A, step B, step C around
  `classify_mcp_job` and `assemble`), `SidecarSmiles` handed to the resolver, the audit log, and R8's
  live count.
- **M4:** the Settings → MCP UI over the grant commands, and "Claude is submitting" in place of Run.

## Tests

`cargo test -p orcastudio --lib mcp::` runs the module's tests, `cargo test -p orcastudio --lib
commands::mcp` the grant commands' tests, and `npx vitest run src/scene/charge-mult-parity.test.ts`
the TS half of the C1 parity. The schema tests are
`cargo test -p orcastudio --lib db::tests::` (v24 names in `modules/tauri-core.md`). The negative
controls are recorded in `log.md`: one per row plus the ADR's M1a list in the M1a entry, and the M1b
list in the M1b Part A entry and the Part B list in the M1b Part B entry (all 2026-11-19).
