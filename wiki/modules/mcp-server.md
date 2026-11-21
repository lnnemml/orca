# MCP server — the job classifier, input assembly, grant core, transport and read tools (`src-tauri/src/mcp/`)

[ADR-027](../architecture/adr-027-mcp-orchestration-server.md), units M1a, M1b, M2a and M2b. This module is the
guard that decides whether an AI model may spend compute. A `create_job` call ends in exactly one
of three dispositions: **REFUSED** (nothing is created), **PROPOSED** (a draft Anton runs by hand)
or **SUBMITTED** (charged to the active grant). The module also assembles the job's input, so the
model never writes a coordinate, a charge or a multiplicity (ADR-014 (1)).

The module has five layers. The **pure core** (M1a) is the classifier, the header grammar, the method
signature, the allowlist, the charge/multiplicity locator and the input assembly. The **DB-level
cores** (M1b) are schema v24, the grant core, steps A and C, the crash reconciliation, the
`submit_job` guard, the source resolver and the memory preflight; each is a `*_conn` function or
takes `&DbState`. The **app wiring** (M1b Part B) is the grant's Tauri commands (`commands/mcp.rs`),
the guard in the two commands that submit a job by hand, and `SidecarSmiles`, the resolver's SMILES
call over HTTP. The **transport** (M2a, `server/`) is the lock, the socket, the `rmcp` server with one
tool (`get_grant`), the admission gate, the lifecycle that switches it on and off, and the
`orcastudio --mcp` relay. The **read tools** (M2b) are the ten read tools of decision 7: each core in
`tools/` is a function over a `&Connection` (or `&DbState`) with its character cap, and
`server/read_tools.rs` registers it in the server with typed arguments, its lock and the mapping to
what the client receives. `server/wiring.rs` connects the transport to the app: `lib.rs` setup and
`ExitRequested`, the `main.rs` relay branch, and the switch's commands `mcp_status`/`mcp_set_enabled`,
which Settings → MCP calls. The module stays registered in `lib.rs` under a scoped
`#[allow(dead_code)]`, because steps A/C, the resolver and `SidecarSmiles` have no non-test caller
until M3.

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
| `tools/mod.rs` | the read tools' shared core: `ToolError` (`Refused`, `Measure`, `TooLarge`, `App`; `from_app` reads `NotFound` as a refusal), `ToolError::refused` (message ≤ 600 bytes), `echo` (echoed input ≤ 200 bytes), `CLIENT_SPILL_CHARS` (50,000), `RESULT_CAP_BYTES` (40,000), `size_of_text` (UTF-8 bytes), `result_text`, `within_cap` (the size post-condition), `clip` (serialised bytes, escapes counted), `page_limit` |
| `tools/jobs.rs` | `list_jobs(conn, &JobFilter, offset, limit) -> JobsPage`, `get_job(conn, id) -> JobView`; `JobSummary`, `ChargeMultView`, `ReasonView`; the field-level guard (`WITHHELD`, `withheld_fields`) and `tripwire`, the backstop on the serialised view |
| `tools/header_view.rs` | `reconstruct(input)`: `get_job`'s header built from `parse_header`, the allowlist and the display-only list, never raw text; `coordinate_row(line, Field)` (the tripwire's detector; `Field::Header`/`Text`); `GEOMETRY_BLOCKS`, `DISPLAY_INTEGER_KEYS`, `hides_bang_token` (`is_numeric_token` or `has_decimal_number`), `HIDDEN_TOKEN`, `is_identifier` |
| `tools/results.rs` | `get_results(conn, id) -> ResultsSummary`; `FrequencySummary`, `ScanSummary`/`ScanPointView`, `NebSummary`/`MepPointView` |
| `tools/geometry.rs` | `get_geometry(conn, reference) -> GeometryView`, `measure(conn, reference, indices) -> MeasureView`, the pure `measure_atoms -> Result<Measured, MeasureRefusal>`, `ase_dihedral`; `GEOMETRY_MAX_ATOMS` (600), `COINCIDENT_ANG`, `COLLINEAR_SIN` (both 1e-6), `SOURCE_MAX_BYTES` (200) |
| `tools/output.rs` | `output_path(conn, id)` (the DB read), `read_output_at(job_id, path, &OutputRequest) -> OutputView` (`Tail`/`Search`/`NoOutput`, no lock) |
| `tools/manual.rs` | `search_manual(conn, query, limit) -> ManualResults` over `manual::index::search_manual` |
| `tools/targets.rs` | `list_run_targets(conn)`, `list_run_targets_at(conn, local_time, now_min)`, the pure `window_state(window, now_min) -> WindowState`, `local_now` |
| `tools/memory.rs` | `memory_estimate(db, meminfo, data_dir, header, target) -> MemoryEstimateView` (takes no lock of its own) |
| `tools/groups.rs` | `list_groups(conn, offset, limit) -> GroupsPage` over `list_groups_conn` |
| `tools/*_tests.rs`, `tools/test_support.rs` | one test file per tool file plus `budget_tests.rs`; `test_support` (`pub(crate)`) has the unit-name walk (`unitless_numeric_keys`), `assert_fits`, row builders, and the fixtures that drive each core to its cap (`largest_get_job`, `largest_list_jobs`, `results_json`, `grid_molecule`, `output_job`, `largest_tail_lines`/`largest_search_lines`, `index_manual`/`largest_manual`, `largest_profiles`, `largest_groups`), shared with the end-to-end tests |
| `*_tests.rs`, `test_db.rs` | one test file per submodule, plus `submit_guard_tests.rs` for the guard's wiring; `test_db.rs` builds a migrated scratch database per test |
| `../commands/mcp.rs` | the grant commands `mcp_create_grant`, `mcp_revoke_grant`, `mcp_get_grant`, `mcp_level_from_job` and their `*_core`/`*_conn` bodies; `GrantView`, `GrantState`; the switch's `async` commands `mcp_status`, `mcp_set_enabled(on)` over `mcp_status_core`/`mcp_set_enabled_core`. Tests in `commands/mcp_tests.rs` |
| `switch.rs` | the switch's row: `MCP_ENABLED_KEY` (`mcp_enabled`), `read_enabled_conn`, `write_enabled_conn`, `is_reserved_setting` |
| `server/mod.rs` | the transport's submodule list; `CONNECTIONS = 4`, `REQUESTS_PER_CONNECTION = 4`; the `DbSource` seam (`impl DbSource for DbState`) |
| `server/paths.rs` | `resolve(runtime_dir, app_data_dir) -> Result<McpPaths, PathRefusal>`, `resolve_for_this_user`, `sun_path_max` (107); `McpPaths { dir, lock, socket, base }` |
| `server/lock.rs` | `try_acquire(path) -> Result<Acquire, LockError>` (`Acquired(McpLock)` / `HeldElsewhere`), `has_cloexec` |
| `server/socket.rs` | `prepare_dir`, `bind(paths, own_uid, after_bind) -> BoundSocket { listener, identity }`, `check_modes`, `unlink_if_same(identity) -> Unlinked`; `DIR_MODE = 0o700`, `SOCKET_MODE = 0o600` |
| `server/listener.rs` | `accept_loop`, the peer check (`PeerUid`, `kernel_peer_uid`, `own_uid`, `peer_allowed`), the per-start `Registry`, the `Transport` stream wrapper |
| `server/service.rs` | `McpService` (the `rmcp` `ServerHandler`: `get_grant`, the permit in `call_tool`, one router of `get_grant` and the read tools), `ConnState { closed, permits }`, `HostPaths { meminfo, data_dir }` (`production(data_dir)`); `bound_errors`/`bounded_error_text`, `ARGUMENT_ERROR_PREFIX`; `RECOVERY_SENTENCE`, `INSTRUCTIONS`, `BUSY` |
| `server/read_tools.rs` | the ten read tools registered (`#[tool_router(router = read_tools_router)]`): their descriptions, argument structs (`ListJobsArgs`, `JobIdArgs`, `GeometryArgs`, `MeasureArgs`, `ReadOutputArgs`, `SearchManualArgs`, `MemoryEstimateArgs`, `ListGroupsArgs`, `NoArgs`; all `deny_unknown_fields`), `output_request`; the mapping `respond` → `served_text` (the cap at the boundary) / `error_text`; `on_blocking_pool`, `McpService::with_db` |
| `server/gate.rs` | `AdmissionGate` (`open`, `close`, `spawn`, `task_count`, `wait_all`), `GateClosed` ("MCP is shutting down") |
| `server/lifecycle.rs` | `McpLifecycle` (`tick_step`, `set_enabled`, `status`, `on_exit_requested`, `gate`), the pure `decide` and `pass_through`; `LifecycleEnv` (`production(db, host)`, `production_at(db, paths, host)`; its `host` reaches every connection's service), `Hooks`, `Observed`, `Action`, `Why`, `McpStatus`, `ExitDecision`, `Executor` (`BlockingPool`), `ExitSteps`, `StopHandle`; `with_busy_timeout`, `RECONCILE_BUSY_TIMEOUT` (250 ms) |
| `server/wiring.rs` | the app's side: `ManagedDb` (the `DbSource` over the managed `DbState`), `wired_lifecycle(db, paths, host)`, `app_lifecycle(app, data_dir)`, `start_ticks`; `exit_requested`, `handle_exit_request`, the pure `prevents_exit`, `guarded_steps`, `backstop`/`run_backstop`/`backstop_steps` → `Backstop` (`Hung`/`Died`/`StillAlive`), `hard_end` (`libc::_exit(1)`), `BackstopTimings`, `EXIT_FLOW_DEADLINE` (60 s), `EXIT_GRACE` (10 s), `BACKSTOP_STEPS_DEADLINE` (20 s); `is_relay_invocation`, `relay_main` |
| `server/tick.rs` | `run_ticks(lifecycle, interval)`, `Interval`, `Every`, `TICK = 30 s` |
| `server/relay.rs` | `relay(stdin, stdout, stderr, socket_path) -> RelayEnd` (`From<RelayEnd> for ExitCode`), `NOT_RUNNING` |
| `server/*_tests.rs`, `server/test_support.rs` | one test file per submodule plus `server_tests.rs` (the server end to end over a real socket) and `read_tools_tests.rs` (the read tools end to end, over a recording `DbSource`); `test_support` has the scratch dir and DB, a counting environment and a raw NDJSON MCP client (`call`, `call_and_wait`) |
| `../../tests/mcp_relay_bin.rs` | the built `orcastudio --mcp` run as a real process (an integration test) |

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

## The reconciliation

`reconcile_submitting_conn(conn)` turns every `draft` with `mcp_state` set into PROPOSED: refunded,
with the reason `interrupted before submit`. Every other row with the flag only has it cleared. It
returns the counts (`Reconciled { refunded, cleared }`). Its one caller is the transport's lifecycle
(`server/lifecycle.rs`, injected as `LifecycleEnv::reconcile`): only the instance holding the
`mcp.lock` `flock` runs it, once per acquisition, before the first bind (below).

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

## The transport (`server/`, M2a)

The MCP server (ADR-027 decision 2 and the M2 amendment, items 1–6). The app reaches it through
`wiring.rs` (below, "The wiring"). Every async piece runs on Tauri's runtime
(`tauri::async_runtime`), never on a runtime of its own.

**Paths.** The directory is `$XDG_RUNTIME_DIR/orcastudio/`, holding `mcp.lock` and `mcp.sock`. When
`XDG_RUNTIME_DIR` is unset or not absolute it is `<app data dir>/mcp/`, with the same modes (a
subdirectory, so the data dir's own mode is never changed). A socket path over `sun_path_max()` (107
bytes, re-measured by the tests) is refused with `PathRefusal::SocketPathTooLong` before anything is
created; the status shows the reason.

**The lock** (`lock.rs`). An exclusive non-blocking `flock` on `mcp.lock`, opened `0600` and
`O_CLOEXEC`. Before the `flock` is taken, `FD_CLOEXEC` is read back with `fcntl(F_GETFD)`, and a fd
without it is refused (`LockError::NoCloexec`): a child that inherited the fd would keep the lock after
the app died (M0). std's `File::open` already sets `O_CLOEXEC` (measured by a run,
`mcp-transport-probe.md`, "M2a facts" 2), so the assertion is what guards. The lock is held while the
`McpLock` lives: from the first acquisition until the exit flow releases it, never on switch-off.

**The socket** (`socket.rs`). `prepare_dir` creates the directory `0700` or adopts an existing one,
which must be a real directory (not a symlink) owned by this uid, and `chmod`s it `0700`. `bind`
removes a stale socket (never a file of another type), binds, `chmod`s the socket `0600`, and then
`stat`s both as its post-condition (rule #9): a mode, owner or file type that differs refuses, and the
socket just bound is removed. Modes are set explicitly, never through the umask. A bound socket's
identity is its `(dev, ino)`; `unlink_if_same` removes the path only while it is still that inode, so
an old stop can never remove a newer socket.

**Accepting** (`listener.rs`). On every accept:
1. **the peer check** — the peer's uid (`SO_PEERCRED` through `UnixStream::peer_cred`, injected as
   `PeerUid` so a test can play a foreign uid) must be known and equal to ours, else the stream is
   dropped;
2. **a slot** — one of `CONNECTIONS` (4), held by the lifecycle and shared by every start; a fifth
   connection is closed at once;
3. **registration** in the start's `Registry`, which switch-off shuts.

Each connection then runs one `McpService` over a `Transport` wrapper of its stream. The wrapper sets
the connection's `closed` flag at the stream's read EOF: `rmcp`'s own end comes up to 5 s later, after
its response drain ("M2a facts" 4). When the service ends, the connection waits for its last request
permit (`ConnState::drain`), and only then returns its slot.

**The service** (`service.rs`). The `rmcp` `ServerHandler` of one connection.
- **The request cap.** Every `tools/call` takes one of the connection's `REQUESTS_PER_CONNECTION` (4)
  permits with `try_acquire`. With none free it gets the tool error `BUSY` ("busy, retry: …") at once
  and never queues. The permit returns when the call's work ends. M3's `create_job` will move its
  permit into its tracked task.
- **Unknown methods.** `rmcp` answers them itself and keeps the connection: `server/discover` before
  `initialize` without the 2026-07-28 `_meta` gets −32602, with it a `DiscoverResult`; an unknown
  method after `initialize` gets −32601 ("M2a facts" 3).
- **The recovery sentence.** `RECOVERY_SENTENCE` ("After an app restart, start a new client
  session.") is one `macro_rules!` literal, `concat!`ed into `INSTRUCTIONS` (the `initialize` result)
  and into every tool description.
- **`get_grant`** returns `{"grant": <GrantView or null>}`, read by `commands::mcp::current_grant_core`
  in `spawn_blocking` with `DbState` locked for that one read. A failure is a tool error with its
  message.
- **The read tools** are registered by `read_tools.rs` (below, "The read tools"). `get_grant`'s router
  and theirs are one, which `#[tool_handler(router = self.tool_router)]` lists and calls (the macro's
  default, `Self::tool_router()`, would list `get_grant` alone).
- **Every tool error's text is bounded** on its way out (`bound_errors`, in `call_tool`): `rmcp`
  answers arguments that do not deserialise with `failed to deserialize parameters:` and
  `serde_json`'s message, which quotes the input (an unknown field's name, a string given for a
  number); that message keeps its prefix and at most 200 bytes after it, and any error text is cut to
  600 bytes. `BUSY` and a successful result pass unchanged.

**The admission gate** (`gate.rs`). `TaskTracker::close` does not stop spawns ("M2a facts" 7), so the
refusal is the gate's: one mutex holds `open` across both the check and the tracked spawn, and `close`
takes the same mutex. Once `close` has returned, `spawn` refuses with `GateClosed` ("MCP is shutting
down"). Opening after a close starts a fresh tracker generation, and every closed generation is kept
until its tasks end; `wait_all` waits on all of them. A test hook sits between the check and the
spawn. No task is spawned through it until M3.

**The lifecycle** (`lifecycle.rs`). One owner, `McpLifecycle`, and **one lifecycle mutex** around
every transition: a tick step, `set_enabled` (its row write included), and the exit flow's
switch-off. A transition reads the row, asks the pure `decide`, and acts, all under one hold:

| observed | action |
|---|---|
| `exiting` | nothing |
| the row read failed | nothing, not even the status (retried on the next tick) |
| "on", serving | nothing |
| "on", a stop still running | nothing; the status says `stopping …` |
| "on", holder | `Start`: reconcile if this acquisition has not, then bind and serve |
| "on", not holder | `Acquire`: try the lock; taken → `Start`; held elsewhere → "another OrcaStudio instance holds the MCP lock" |
| "off", serving | `Stop` (the `flock` is kept) |
| "off", not serving | nothing |

- **The lock passes only on the holder's exit.** `mcp_enabled` is one shared row, so switching MCP off in
  either instance turns it off for both: the holder stops serving but keeps the `flock`, and a second
  instance stays a non-holder. A second instance takes the lock, and then the socket, only after the
  holder exits (measured live: 27 s after the first exited, one tick).
- **Acquisition.** Taking the lock begins one, and `reconcile_submitting_conn` runs once per
  acquisition, before the first bind. The acquisition counts as done only when it commits: a failure
  (`SQLITE_BUSY` from another instance) binds nothing, shows the reason, and the next tick retries.
  The reconciliation runs under `with_busy_timeout(RECONCILE_BUSY_TIMEOUT)`: it waits at most
  **250 ms** on another instance's write transaction (not `rusqlite`'s 5 s, "M2a facts" 6), because it
  holds `DbState` meanwhile and every synchronous UI command waits with it (decided by Anton). The
  connection's own timeout, read with `PRAGMA busy_timeout`, is restored by a drop guard on every
  path: success, error and panic.
- **Switch-on**: bind, `stat`, open the gate, start the accept loop.
- **Switch-off**, under the mutex: close the gate, shut the registry (every `closed` flag set), cancel
  the start's token (the accept loop and every service). Then a task on Tauri's runtime, outside the
  mutex: wait for the accept loop and every tracked task of every generation, unlink the socket this
  start bound (by inode), and mark the `StopHandle` complete. While a stop runs, "on" is a no-op.
- **Lock order.** lifecycle → gate, lifecycle → `DbState`, lifecycle → the status facts. Nothing takes
  the lifecycle mutex while holding one of those; handlers never take it.
- **`set_enabled(on)`** writes the row (its only writer: the generic `set_setting` refuses
  `mcp_enabled`) and runs one step at once. Once exit has begun it writes nothing and the status says
  `exiting`.
- **`McpStatus`** (`enabled`, `holds_lock`, `serving`, `stopping_tasks`, `connections`,
  `socket_path`, `reason`) is advisory: `status()` reads a leaf snapshot and the row, never the
  lifecycle mutex. `enabled` is `None` when the row cannot be read; `stopping_tasks` is `None` when no
  stop runs.

**The exit flow.** `on_exit_requested(executor, steps)` is what `ExitRequested` calls on the main
thread (through `wiring::exit_requested`), and it takes no mutex:
1. `exit_ready` set → `PassThrough` (the pure `pass_through`; a programmatic `app.exit()` re-enters
   `ExitRequested` with `prevent_exit` honoured, "M2a facts" 1);
2. else one `compare_exchange` on the terminal `exiting` flag: the winner hands the flow to the
   executor (`Started`), and any later request is `Absorbed`. The caller calls `prevent_exit` unless
   `PassThrough`.

The flow, off the caller's thread (`BlockingPool`: Tauri's blocking pool):
0. run `pause_queue` (the local queue starts nothing from here on);
1. take the lifecycle mutex, switch off (or adopt a stop that began before exit), release it;
2. wait for **stop complete** (every generation ended, the unlink done);
3. run `terminate`, then `sidecar_stop`;
4. release the lock, set `exit_ready`, call `exit` — `exit_ready` **before** `exit`, so the
   `ExitRequested` that `exit` raises passes through (`lifecycle_tests::the_flows_own_exit_passes_through`).

Every step is injected (`ExitSteps`), so the tests drive it without a `RunEvent`.

**The tick** (`tick.rs`). `run_ticks` runs one step at once and then one per `Interval` (`Every(TICK)`,
30 s) until exit begins. A step that changes nothing is retried on the next tick: the driver never
gives up while the app runs. Setup starts it on the `mcp-tick` thread.

**The relay** (`relay.rs`). `relay(stdin, stdout, stderr, socket_path)` copies stdin → socket and
socket → stdout, flushing every chunk, until either side ends: stdin EOF closes the socket
(`ClientClosed`, exit 0), socket EOF ends it (`ServerClosed`, exit 1). A failed connect writes exactly
`NOT_RUNNING` ("OrcaStudio is not running, or its MCP server is off") and a newline to stderr
(`NotRunning`, exit 2). stdout carries only the socket's bytes; every diagnostic goes to stderr.

### The wiring (`wiring.rs`)

**Setup** (`lib.rs`, right after `DbState` is managed): `app_lifecycle(app, &data_dir)` builds the one
`McpLifecycle` over `LifecycleEnv::production` — this user's paths, the real reconciliation,
`SO_PEERCRED`, and `HostPaths::production(data_dir)` (the real `/proc/meminfo` and the data dir
`JobRunner` is built over, for `memory_estimate`) — with `ManagedDb`, a `DbSource` that reaches the
managed `DbState` through the app handle. It is managed as
`Arc<McpLifecycle>`, and `start_ticks` starts the `mcp-tick` thread, whose first step runs at once, so
a switch left on serves from app start. A path refusal (`sun_path`) or no base dir never fails setup:
it stays in the environment and becomes the status reason once the switch is on.

**The commands.** `mcp_status()` and `mcp_set_enabled(on)` are `async` and run their work through
`spawn_blocking` (as `test_server_profile`), so the GTK main thread never takes the lifecycle mutex or
waits on a reconciliation, a bind or the row write. Both return `McpStatus`; an unmanaged lifecycle is
`AppError::Internal`.

**`ExitRequested`** is the hand-off and nothing else: `wiring::exit_requested(app)` builds the four
steps (the queue pause, `terminate_on_exit`, the sidecar stop, `app.exit(0)`), asks
`on_exit_requested(&BlockingPool, steps)`, and returns `prevents_exit(decision)` — true exactly when
the decision is not `PassThrough` — and `lib.rs` calls `api.prevent_exit()` on it. So the order is the
flow's: queue paused → MCP wait → `terminate_on_exit` → sidecar stop → lock release → `exit_ready` →
`app.exit(0)`, all off the main thread, each step once; the flow's own `app.exit(0)` comes back as an
`ExitRequested` that passes through. With MCP never switched on (no lock, no start) the same flow
runs: its stop has nothing to wait for, so the kill, the sidecar stop and `exit` follow at once.

**The queue pause** (`JobRunner::pause_for_exit`, decided by Anton) comes first because the job
`terminate_on_exit` kills frees its slot, and its drive thread's `try_start_next` would otherwise start
the next queued ORCA job during the kill's 2 s grace; that mpirun would outlive the app, since the
SIGKILL sweep matches only the old job dir. The claim (`JobRunner::claim_next`) checks the flag under
the `running` lock. `terminate_on_exit` is safe off the
main thread: it touches managed state, the `JobRunner`'s `running` mutex, signals, `/proc` and sleeps,
and no window, GTK or event.

**Nothing can leave the app un-exitable.** Each step runs under `catch_unwind` (`guarded_steps`), so a
panicking step is reported on stderr and the steps after it still run. When the flow starts, a
backstop thread (`backstop` over `run_backstop`, `BackstopTimings::PRODUCTION`) waits for the `exit`
step and ends the process in every case where it is still running:
- `exit` not reached within `EXIT_FLOW_DEADLINE` (60 s; the M2a flow takes the kill's ≤14 s) →
  `Hung`, or the flow ended without reaching it (its steps dropped) → `Died`: the backstop first runs
  its own steps (`backstop_steps`: the queue pause — idempotent, and in `Died` the flow's may never
  have run — then the kill, then the sidecar stop) for at most `BACKSTOP_STEPS_DEADLINE` (20 s);
- `exit` reached but the process still alive `EXIT_GRACE` (10 s) later → `StillAlive` (the flow has
  run the kill and the sidecar stop already).

The end is `hard_end`: one stderr line, then `libc::_exit(1)`, which skips atexit handlers that could
deadlock against the GTK/WebKit loop. It is injected, so the tests record it instead. A hard end is a
crash in the reconciliation's terms: a job left in step B is refunded by the next holder's
`reconcile_submitting_conn`. M3 revisits the 60 s.

**`orcastudio --mcp`.** `main.rs` asks `is_mcp_relay(args_os().nth(1))` before `run()` — so the relay
opens no window, initialises no GTK, and prints nothing — and exits with `relay_main()`'s code: the
`RelayEnd` code over the process's stdin/stdout/stderr and `resolve_for_this_user()`'s socket, or 2
with one stderr line (`orcastudio --mcp: <the refusal>`) when the path is refused. The argument is
compared as an `OsStr`, so a non-UTF-8 argument cannot panic `args()`.

**Settings → MCP** (`src/mcp/McpSection.tsx`, `modules/frontend.md`): the switch, decision 9's data
line, and the status line from `McpStatus`, re-read on mount, after a toggle and every 5 s.

## The read tools (`tools/`, M2b)

The ten read tools of decision 7 (M2 amendment, items 1, 7 and 8). Each core in `tools/` is a function
over a `&Connection`; `memory_estimate` takes `&DbState` because the cores it reuses lock it
themselves, and `read_output` splits into a DB read (`output_path`) and a file read without the lock
(`read_output_at`). A refusal is `ToolError::Refused` (or `Measure`) with the reason in words; an
unknown id is a refusal, not an app error. `server/read_tools.rs` registers each core in the server
(below, "Registration").

**Units (rule #11).** Every number names its unit in its field: `_eh`, `_ang`, `_deg`, `_cm1`,
`_km_mol`, `_k`, `_s`, `_mib`, `_kib`, `_bytes`; a count ends in `_count` and an index in `_index` or
`_indices`. SQLite's `datetime('now')` times are UTC on the laptop's clock and end in `_utc`.
`test_support::unitless_numeric_keys` walks a serialised result and lists every numeric key without
a suffix; the `get_results`, job and `measure` tests require it empty.

**Size (item 7).** The client spills a result over 50,000 characters to a file (M0, measured on
ASCII text only). Each core's caps (the table below) are sized so that its largest tested result
stays under **40,000 UTF-8 bytes**, and the result says when it pages or cuts. The guarantee is
the post-condition, not the caps: every core checks its serialised result (`within_cap`), and a
result over 40,000 bytes is `TooLarge`, never served. **The unit is the byte** because what the client counts for
non-ASCII text is unmeasured; a UTF-8 byte count is at least every candidate (characters, UTF-16
units), so it is the conservative bound. Every field budget below is in serialised bytes too:
`clip` counts each character as it serialises (`\u00XX` is 6 bytes), so a field full of control
characters is cut, not refused. A Cyrillic title therefore keeps about half as many characters as an
ASCII one. **Field budgets stay in UTF-8 bytes** (decided by Anton, 2026-11-20).

**Refusals** quote caller input cut to 200 bytes (`echo`), and every refusal message is cut to 600
bytes (`ToolError::refused`), including a reused reader's error that quotes the input. A source
reference (`get_geometry`, `measure`) longer than 200 bytes is refused before it is read, because the
resolver's messages quote it whole; so no refusal quotes more than 200 bytes of one input.

| tool | caps (all stated in the result) |
|---|---|
| `list_jobs` | page 20 by default, at most 40; title cut to 120 bytes, error excerpt to 240; `total_count`, `next_offset` |
| `get_job` | header cut to 6,000, error to 4,000; at most 20 MCP reasons of 400 (`mcp_reasons_note` counts the rest) |
| `get_results` | at most 20 imaginary modes, 200 scan points, 100 MEP points, 10 warnings of 300; each list beside its full count |
| `get_geometry` | refused above 600 atoms, with a pointer to `measure` |
| `read_output` | tail: last 256 KiB of the file, 100 lines by default, at most 400, each cut to 400 bytes, stopped at 36,000 bytes of line text; search: 20 matches by default, at most 50, context at most 3 lines, stopped at 36,000 bytes, `next_offset`; query at most 200 characters |
| `search_manual` | 10 hits by default, at most 20; snippet 400, title/file/anchor 160, at most 6 breadcrumb entries of 80; query at most 500 |
| `list_run_targets` | at most 50 profiles (newest first), names cut to 120 |
| `list_groups` | 50 by default, at most 100; names cut to 120 |
| `memory_estimate` | fixed shape; header at most 20,000 characters |

**`list_jobs`** is a filtered, paged query over `Job::COLUMNS` / `Job::from_row` (`status`, `origin`,
`group_id`; all given must match), newest first (`created_at DESC, id DESC`). A filter value outside
its set is refused with the set. Paging is by offset: a job created between two calls can repeat a
row, and a deleted one can skip one. `energy_eh` is `jobs.energy` (Eh: the parsed
`final_energy_eh`, or the tail regex's estimate while unparsed). Each row's `title` and
`error_excerpt` pass the field-level tripwire below, checked on the whole text before the excerpt is
cut, and `withheld_fields` names any replaced.

**`get_job`** builds its projection field by field; no field carries `input_content` or the scene
(ADR-014 invariant 1). **Its header is built, never filtered** (decided by Anton after two verifier
FAILs on filtering raw text, M2b; ADR-027's M2 amendment, item 8). `header_view::reconstruct` takes
the input with its main `* xyz` block and the app's `%maxcore` blanked (`header_of_input_in_place`,
so line numbers are the input's), parses it with M1a's `parse_header`, and emits only parsed parts:

| construct | shown as |
|---|---|
| a `!` line | `! ` and its parsed keyword tokens; each **numeric-looking** token becomes `[value not shown]`: a token that starts with a digit, sign or `.` and holds only digits, signs, `.`, `e`/`E`/`d`/`D`, `,`, `;` (`is_numeric_token`), or a token that holds a decimal number anywhere, `\d*\.\d+` (`has_decimal_number`: `(0.000000,0.000000,0.117790)`, `x=0.117790`). Method, basis and job type stay (`D3`, `def2-SVP`, `6-31G*`, `PAL8` hold no decimal number) |
| the app's `%maxcore N` | from the strict reader (`memory::read_maxcore`), at its line |
| an allowlisted key with an admitted value (`allowlist::value_rule`: `%geom Calc_Hess`, `MaxIter`; `%cpcm smd`, `SMDsolvent`) | the key and the value |
| an allowlisted key with another value | the key, `# value not shown` |
| a key on the **display-only list** (`DISPLAY_INTEGER_KEYS`: `%pal nprocs`, `%scf MaxIter`, `%neb NImages`, `%geom Recalc_Hess`) with exactly one value of 1–9 digits | the key and the value; any other value: `# value not shown` |
| a `%geom Constraints` entry `{T i … [v] [C]}` (T = B/A/D/C with its index count, ASCII-digit indices, an optional plain number, an optional `C`) | re-emitted from its parsed parts, the value as written |
| any other key, in any block | its name only (an identifier), `# values not shown` |
| any other entry or sub-block (`Scan`, …) | `# … not shown` with its line |
| `%coords`, `%pointcharges`, a block holding a nested `%` opener, a `*` line | a marker naming the kind and the line |
| a `$…` line (a second job) | a marker; nothing after it is read |
| a line the parser refuses | a marker with the parser's reason kind and the line (a run of lines with one reason is one marker), never the line |

The display-only list is **separate from M1a's SUBMITTED allowlist**, which is unchanged: showing a
value admits nothing (Anton, round 4). The constraint entries are shown although M1a's allowlist
admits only `{C n C}`: they are indices, a type letter, an optional target and `C`, never a
coordinate (Anton). An app-built constrained Opt (`! B3LYP D3BJ def2-SVP Opt`, `%maxcore 2000`,
`%pal nprocs 8 end`, a `%geom Constraints` block) gives:

```
! B3LYP D3BJ def2-SVP Opt
%maxcore 2000
%pal
  nprocs 8
end
%geom
  Constraints
    {B 0 1 1.234 C}
    {A 0 1 2 C}
    {D 0 1 2 3 90.0 C}
  end
end
```

**Known limitation.** The shared grammar knows only `Constraints` and `Scan` as `%geom` sub-blocks,
so `TS_Mode {B 0 1} end` closes `%geom` at that `end`: `TS_Mode` is named without its value and the
later `end` becomes a refused-line marker. Safe, but the rest of that `%geom` is shown as markers.
M1a's grammar is not changed for display.

The main block is never shown: `atom_count` is its rows, and charge and multiplicity come from the
strict locator with their source ("the `* xyz` line of job <id>'s input"). **Every error the view
builds is a kind and a line, never source text** — the header's, the charge and multiplicity's, and
a coordinate row's — fixed at the source: `read_coordinate_block` returns
`CoordinateBlockError` (`line N: a coordinate row is not an element and three numbers`, …),
`header_of_input` returns `HeaderOfInputError`, and `charge_mult.error` is
`ChargeMultError::kind_and_line` (its `Display` quotes the line and is not used here); each is cut
to 200 bytes. The job's `error` field is not built by the view: it is ORCA's stored text, served
under the tripwire below.

**The rule-#9 tripwire works per field** (Anton, round 4). Every text field (`title`, `header`,
`header_withheld`, `charge_mult.source`/`error`, `notices`, each MCP reason's `row` and `message`,
`mcp_reasons_note`, `error`, `remote_host`) is checked whole, before any cut. A field holding a
coordinate-like row is replaced by `"withheld: coordinate-like content (rule #9)"`, named in
`withheld_fields`, and the rest of the view is served. A failed job's stored `error` is a line with
the exit code plus the tail of its stderr when stderr is non-empty, else the last 20 lines of
`output.out` (`local_backend::detect_completion`). It is served unless it holds a
coordinate-like row, for example an echoed `|  4> * xyz 0 1`; ORCA's `***` banners alone do not
withhold it (Anton, round 5). A **coordinate-like row** (`header_view::coordinate_row`) is either:
- a geometry line. In text fields (`Field::Text`) this is only a real geometry opener: `* xyz`,
  `* int`, `* gzmt` or `* xyzfile`, in any case and with any spacing (`*xyz`), also after ORCA's
  input-echo prefix (`|  4> * xyz 0 1`). In the built header (`Field::Header`) it is any line
  starting with `*`;
- or an element-or-label token (a **label**: a token with a letter that is not a number: `O`, `O(1)`,
  `H:`, `geometry:`) followed by at least three finite **numbers** (tokens that parse as finite
  floats) **at any position on the line**, a lone `:` token skipped.
ORCA's input echo `|  6> C     4.10220628   -0.56008055    0.58413357` (measured in a real
`output.out`) matches; SCF iteration rows (numbers only) do not. A line starting with `{` (a
constraint entry) or holding `=` (a scan or parameter definition) is skipped **in the built header
only** (`Field::Header`); titles, errors and reasons skip nothing (`Field::Text`). Behind the
guards, `jobs::tripwire` walks the serialised view with the same detector and refuses it if a row
escaped every guard. ID fields (`id`, `group_id`, `target_profile_id`, `grant_id`) and timestamps
are not guarded individually: in `get_job` a coordinate-like ID field trips the backstop, which
refuses the whole view. `list_jobs` guards each row's `title` and `error_excerpt` and has **no**
backstop; its ID fields (`id`, `group_id`, `target_profile_id`) are v4 UUIDs the app mints
(`Uuid::new_v4` in `create_job_conn`, the group and profile commands), and its timestamps are
SQLite's.

**Notices** are the job's pending states in words: a cancel requested or sent, a submit waiting for
the server's slot, Claude submitting it.

**`get_results`** summarises `read_job_results` with no per-atom array: `final_energy_eh`,
`converged`, `atom_count`, the imaginary modes (`imaginary_freqs_cm1` most negative first, their
`imaginary_ir_km_mol`, the parser's `imaginary_count`, a warning if the two disagree), the
thermochemistry (`ThermoJson`, every field already suffixed), the scan profile and the NEB summary
(`relative_energy_eh` along `distance_ang`). A scan coordinate is `coordinate_ang` or
`coordinate_deg` by the stored `coordinate_unit` literal (`Å`/`°`); any other unit gives no
coordinate and a warning. No results: `parsed: false` with the reason.

**`get_geometry` and `measure`** read a DB source through `resolve_db_conn` (`job:`, its `scan:`,
`neb:`, `goat:` frames, `molecule:`). A `smiles:` reference is refused (the read tools do not call
the sidecar), and so is a job-derived source whose job has not ended in success. `get_geometry` gives
xyz text (`xyz_ang`, 8 decimals) with charge, multiplicity and their source; its post-condition reads
the text back with the app's xyz reader (same elements in order, within 1e-8 Å).

`measure` takes 2, 3 or 4 0-based indices (distance, angle with the middle index as the vertex,
dihedral) and answers `distance_ang`, `angle_deg` (`[0, 180]`) or `dihedral_deg` (`[0, 360)`), in
ASE's convention. Distance and angle are `xtb::distance` and `xtb::angle`. **The dihedral is not
`xtb::dihedral`**, which gives 360° − ASE's value; it is `ase_dihedral`, a line-for-line port of ASE's
`get_dihedrals` (as `measure.ts` has). The ASE cross-check uses the butane fixtures of
`sidecar/tests/test_geometry.py`, whose values are recorded in
[mcp-transport-probe.md](../architecture/mcp-transport-probe.md), "M2b facts". It refuses, never
panics and never returns a number, for:
- a count other than 2–4, an index out of range, a repeated index, a non-finite coordinate;
- coincident atoms in a distance (closer than `COINCIDENT_ANG` = 1e-6 Å, the app's coordinate
  round-trip tolerance);
- a zero-length arm in an angle (an end atom within 1e-6 Å of the vertex);
- a zero axis in a dihedral (within 1e-6 Å), or a collinear inner triple: `|sin θ|` at either middle
  atom below `COLLINEAR_SIN` = 1e-6 (θ within about 5.7e-5° of 0° or 180°), or an arm under 1e-6 Å.
  Stricter than `measure.ts` (1e-9) and ASE (exact zero): at 1e-6 the projection that sets the
  dihedral's direction is about the size of the app's coordinate tolerance. Both tolerances are
  accepted by Anton (2026-11-20). So in a narrow band (`|sin θ|` between 1e-9 and 1e-6, or a pair
  between 1e-12 and 1e-6 Å apart) the scene's panel shows a number where MCP refuses.

A value outside its range after these checks is `OutOfRange`, an internal error, never served.

**Lock span.** For a `scan:`, `neb:` or `goat:` source, `resolve_db_conn` reads files in the job
directory (`read_scan_geometries`, `read_neb_geometries`, `read_ensemble_text`) on the caller's
connection, so `get_geometry` and `measure` hold `DbState` across those bounded, size-capped reads,
as the existing commands that read them do. That is allowed (only an `.await` under the lock is
forbidden), and it stays inside the tool's one blocking task.

**`read_output`** never reads the whole `output.out` (rule #5). The tail reads at most the last
256 KiB through `local_backend::read_tail_window`, the core `read_tail_lines` (the log console's
reader) also goes through, and reports `file_bytes`, `window_start_byte` and `read_bytes`. The
rule-#5 gate checks `read_bytes` (what was actually read), not only the reported offset. The search streams the
file through `output_search::search_output`, which keeps the first 500 matches and counts all.

**`search_manual`** keeps each hit's section id (`section_id`, the `manual_sections` row) and turns
the index's private-use match marks into `«` `»`. `bm25_score` is dimensionless; lower is better.

**`list_run_targets`** lists this machine (`"local"`) and each profile with `is_run_target` and its
reason (`RunTargetStatus::of`), the window and `window_state`: `no_window`, `inside`, `outside` or
`unreadable`. The pure `window_state(window, now_min)` is inside from the start minute **included**
to the end minute **excluded**, wrapping midnight when the end is before the start (`22:00-08:00`
holds 23:00 and 07:59, not 08:00). Production reads `now` from SQLite's
`datetime('now','localtime')`, the measured route to the laptop's local time. `reachability` is
`"unknown"` for every profile (no per-profile contact exists in Stage 1) and `"this_machine"` for
local.

**`memory_estimate`** estimates the header **with the app's `%maxcore`** (`mcp_maxcore`, inserted by
`apply_maxcore`, the assembly's route), checks it with `memory_estimate_with`, and gives the
classifier's verdict (`verdict_of`). A header with its own `%maxcore` is refused: the app sets it.

**`list_groups`** pages `list_groups_conn` (`created_at, id`).

### Registration (`server/read_tools.rs`)

Each tool is a thin shell: typed arguments, the core's call with its lock, and the mapping.

| tool | arguments (schema bounds) |
|---|---|
| `list_jobs` | `status`, `origin`, `group_id` (each optional), `offset`, `limit` (1–40) |
| `get_job`, `get_results` | `job_id` |
| `get_geometry` | `source` (≤ 200 bytes) |
| `measure` | `source`, `atom_indices` (2–4, 0-based) |
| `read_output` | `job_id`; the tail: `lines` (1–400); a search: `query` (1–200), `regex`, `case_sensitive` (both default false), `context_lines` (≤ 3), `offset`, `limit` (1–50) |
| `search_manual` | `query` (≤ 500), `limit` (1–20) |
| `list_run_targets` | none |
| `memory_estimate` | `header` (≤ 20,000), `target` (`"local"` or a profile id) |
| `list_groups` | `offset`, `limit` (1–100) |

- **Arguments.** Every argument struct has `deny_unknown_fields`, so its schema has
  `additionalProperties: false` and an unknown argument is a tool error (`unknown field …`), never
  ignored; `list_run_targets` takes the empty `NoArgs`. The schema's bounds are advice to the
  client; the cores clamp or refuse on their own. `read_output` without `query` is the tail and
  refuses a search-only argument; with `query` it is a search and refuses `lines`.
- **Descriptions.** Each carries `RECOVERY_SENTENCE` (pinned for every listed tool), names the units
  it returns, and stays under 700 bytes. `get_geometry` and `measure` say indices are 0-based;
  `get_job` says its header is rebuilt from the parsed input and values outside the display
  allowlist are not shown; `read_output` says it is a bounded tail or search.
- **The mapping** (`respond`). A result is the core's value as compact JSON (`result_text`), every
  field kept, `withheld_fields` and the truncation flags included, and `served_text` checks it
  against `RESULT_CAP_BYTES` once more as it leaves: over it, a tool error, never served. A
  `ToolError` is a tool error whose text is the error's own message (an app fault as
  `<tool> failed: …`), cut to 600 bytes (`error_text`); the mapping never adds the arguments.
- **Threads and the lock.** Every core runs on Tauri's blocking pool (`on_blocking_pool`), never on an
  async worker. `with_db` locks `DbState` once for the read, inside that blocking task, so no guard
  lives across an `.await`. `memory_estimate` takes no lock in the service (its core locks itself).
  `read_output` locks only for `output_path` and drops the guard before `read_output_at` reads the
  file. Each call takes one of the connection's 4 permits in `call_tool`, like `get_grant`.

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
- **`measure` keeps the `ase_dihedral` port** (2026-11-20). It does not reuse `xtb::dihedral`, because
  `xtb::dihedral` returns 360° − ASE (the M2 amendment, item 8, records it).
- **The read tools refuse `smiles:` sources** (2026-11-20): they do not call the sidecar.
- **`get_job`'s header is built from the parsed structure, never filtered from raw text** (2026-11-20,
  after two verifier FAILs on filtering). See the read tools, `get_job`.
- **Round 4** (2026-11-20, after the verifier's PASS WITH FINDINGS): numeric-looking `!` tokens are
  hidden; the tripwire withholds a field, not the view, and also guards `list_jobs`' title and error
  excerpt; the detector looks at every token position; the `{`/`=` skips hold for the header only; a
  display-only list of integer keys and `%maxcore` are shown, separate from the SUBMITTED allowlist.
- **Round 5** (2026-11-20): in text fields `*` means only a geometry opener (`* xyz`/`int`/`gzmt`/
  `xyzfile`, also echoed), so a failed job's `***` banners no longer withhold its error.
- **Field budgets stay in UTF-8 bytes** (2026-11-20).
- **A `!` token holding a decimal number is hidden** (2026-11-21, the M2b Part A verifier's F1):
  `\d*\.\d+` anywhere in the token, so `(0.000000,0.000000,0.117790)` and `x=0.117790` are hidden,
  while `D3`, `def2-SVP` and `6-31G*` stay.
- **`COINCIDENT_ANG` and `COLLINEAR_SIN` stay at 1e-6** (2026-11-20), knowing `measure.ts` uses 1e-9,
  so in a narrow band the panel shows a number where MCP refuses.

## Open (for Anton)

- **Unbraced constraint entries** (`C 0 C` inside `Constraints`) are P2 and are **not**
  range-checked by R6. A proposed draft could carry an out-of-range index to Anton's Run.
- **`Scan` atom indices** are P2 and are **not** range-checked by R6, with the same consequence.
- **How ORCA 6.1.1 reads a second `#`** on a line is unmeasured (a prober run). Until then, such a
  line is R1.

- **Atomic-number rows** (the M2b Part A verifier's F2, INFO). The tripwire's detector needs a
  label (a token with a letter), so a row written with an atomic number for the element
  (`8 0.0 0.0 0.117790`) is not caught in a text field. Whether ORCA 6.1.1 accepts atomic numbers in
  a `* xyz` block is unmeasured (rule #10: a prober run). The detector is unchanged until then.

- **Source headers with other one-line directives** (`%moinp "x.gbw"`, `%base`) give no
  `opt_signature`, so a Freq from such a job is PROPOSED. Only `%maxcore` is set aside.

- **`xtb::dihedral`'s sign outside MCP.** It returns 360° − ASE's value on the butane fixtures (its
  doc comment now says so; until M2b it claimed ASE parity). It sets xtb's default dihedral target ("freeze as is") and
  `check_held`'s dihedral check. Whether xtb's own `$constrain` dihedral uses the same sign is
  unmeasured (rule #10: a prober run). If xtb's sign is ASE's, an explicit target other than 0° or
  180° is reported as not held; if it is `xtb::dihedral`'s, an explicit target taken in ASE's
  convention (the scene's `measure.ts`) is held at its mirror image. Not changed here.

## Not yet wired

- **M3 (from the M2b verifier, INFO):** the classifier's source charge reason quotes the source job's
  `* xyz` line through `ChargeMultError::Display` (`classify.rs`, the R4 source reason). `create_job`
  returns reasons to the model, so M3 maps it to `ChargeMultError::kind_and_line`, as `get_job` does.
- **M3:** `create_job` as one tracked task (the preflight, step A, step B, step C around
  `classify_mcp_job` and `assemble`), `SidecarSmiles` handed to the resolver, the audit log, and R8's
  live count.
- **M4:** the Settings → MCP UI over the grant commands, and "Claude is submitting" in place of Run.

## Tests

`cargo test -p orcastudio --lib mcp::` runs the module's tests (the transport's are
`mcp::server::`, the wiring's `mcp::server::wiring_tests`, the read tools' cores `mcp::tools::` and
their registration end to end `mcp::server::read_tools_tests`), `cargo test -p orcastudio --lib commands::mcp`
the grant and switch commands' tests, `cargo test -p orcastudio --test mcp_relay_bin` the built
binary's relay, `npx vitest run src/screens/SettingsScreen.mcp.test.tsx` the switch row, and
`npx vitest run src/scene/charge-mult-parity.test.ts` the TS half of the C1 parity. The schema tests are
`cargo test -p orcastudio --lib db::tests::` (v24 names in `modules/tauri-core.md`). The negative
controls are recorded in `log.md`: one per row plus the ADR's M1a list in the M1a entry, and the M1b
list in the M1b Part A entry, the Part B list in the M1b Part B entry, and the M2a Part A and Part B
lists in their entries (all 2026-11-19), the M2b Part A list in its entry (2026-11-20), and the M2b
Part B list in its entry (2026-11-21). The ASE
pin is `pytest sidecar/tests/test_geometry.py` (in the sidecar venv).
