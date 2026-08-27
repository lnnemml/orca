# server-profiles — remote execution targets (data layer)

**Status:** Part A landed (schema + model + CRUD + pure connection-test parsers); Part B
(settings UI + real SSH connection-test) is the next unit. Phase 5 unit 5.1, ADR-023.

A **server profile** is the runtime configuration of one remote execution target, stored as
data (never code): "add a server" is a settings action, not a build (ADR-023). One
`SshBackend` (Part B) is parameterized by any number of these rows.

## Table: `server_profiles` (schema v18)

Created by the v18 migration (`db.rs`), guarded by `column_exists`. Discrete typed columns,
never a JSON blob:

| Column | Type | Meaning |
|---|---|---|
| `id` | TEXT PK | UUID (generated on create). |
| `name` | TEXT | User-facing display name. |
| `host` | TEXT | `~/.ssh/config` host alias — the transport handle (ADR-005). The app stores **no credentials**; auth stays with the user's SSH setup. |
| `remote_orca_path` | TEXT | Absolute path to the remote `orca` binary (rule #1 — always invoked by full path or OpenMPI silently fails). |
| `remote_scratch_dir` | TEXT | Remote dir under which each calculation gets its own isolated job dir (rule #3). |
| `core_mask` | TEXT? | `taskset` mask (rule #8). `None` until the performance probe measures it; the connection-test only establishes the `core_count` ceiling. |
| `orca_version` | TEXT? | Verified remote ORCA version (connection-test, rule #10). |
| `openmpi_version` | TEXT? | Verified remote OpenMPI version (rule #2 — must match the ORCA build). |
| `core_count` | INTEGER? | Verified logical CPU count (`nproc` ceiling, rule #8). |
| `verified_at` | TEXT? | Timestamp of the last successful connection-test. **`NULL` = not verified = not a run target.** The usability gate. |
| `created_at` | TEXT | Insert timestamp. |

The four `verified_*` fields are `NULL` until a real connection-test measures them
(honest-or-absent — never a guessed stand-in). `ServerProfile::COLUMNS` + `from_row`
(`models/server_profile.rs`) mirror the table one-to-one and drive every `SELECT`.

## The nullable-FK `NULL = local` decision

`jobs.backend_id TEXT REFERENCES server_profiles(id) ON DELETE SET NULL` (v18). A job's
`backend_id` is `NULL` for a **local** run and the profile id for a remote one. There is **no
`'local'` sentinel row** and **no backfill** — a `NULL` FK is exactly the "no remote target =
local" meaning, following the v13 `pathway_id` precedent (a nullable grouping FK, no default
row). ADR-023's Consequences originally sketched `backend_id (default 'local')`; unit 5.1
refined it to this nullable form (see the ADR's dated amendment).

## CRUD surface (`commands/server_profiles.rs`)

Same shape as `commands::reactions`: each Tauri command locks the shared connection and
delegates to a `*_conn(&Connection, …)` helper, so the logic is unit-testable without a
running Tauri app. Every command returns `Result<T, AppError>`.

- `create_server_profile(name, host, remote_orca_path, remote_scratch_dir, core_mask?)` —
  generates the id, leaves the verified_* columns `NULL`, returns the new `ServerProfile`.
- `list_server_profiles()` → `Vec<ServerProfile>`, newest first.
- `update_server_profile(id, …user fields…)` — edits the **user-owned** fields only. It does
  **not** touch the verified_* columns: a manual edit of a measured spec would forge a
  rule-#10 fact, and it must not silently clear a verification stamp.
- `delete_server_profile(id)` — **nulls the `backend_id` of every job that ran on the profile
  first** (they revert to `NULL = local`), then deletes the row. The jobs survive as
  standalone jobs — the load-bearing invariant, mirroring `delete_reaction`. The explicit null
  holds even if the FK's `ON DELETE SET NULL` were not enforced.
- `set_profile_verified(id, orca_version, openmpi_version, core_count)` — stamps the four
  verified_* columns + `verified_at = datetime('now')`. This is the **pure DB-write half** of
  the connection-test; Part B's real SSH session measures the specs and calls this to persist
  them. Flipping `verified_at` from `NULL` to a timestamp is what admits the profile through
  the usability gate.

### The `verified_at` usability gate

`verified_at IS NOT NULL` is the query that answers "is this profile a run target?". A profile
that has not passed the connection-test is not offered for a run (ADR-023). Part B's run-target
selector filters on this.

## Pure connection-test parsers (`connection_test.rs`)

Parsers only — **no `std::process::Command`, no ssh** (that execution is Part B). Each takes a
captured `stdout` string and targets the prober's **verbatim** measured format
(`wiki/orca/remote-server-probe-commands.md`, rule #10):

- `parse_orca_version(stdout) -> Option<String>` — the indented banner line
  `Program Version 6.1.0  -  RELEASE   -` from `<path> --version 2>&1`. Regex anchored on the
  literal `Program Version`, so unrelated digits (a file size, a build tag) cannot be scraped
  as a version.
- `parse_openmpi_version(stdout) -> Option<String>` — `Open MPI v4.1.6` (line 1 of
  `ompi_info --version`), falling back to the `mpirun (Open MPI) 4.1.6` shape.
- `parse_nproc(stdout) -> Result<u32, AppError>` — a bare trimmed integer from `nproc`;
  `AppError::Backend` on empty/garbage, never a guessed core count (rule #9 post-condition —
  a bad `nproc` blocks the profile rather than defaulting a ceiling).
- `parse_presence(stdout) -> bool` — the `test -x <path> && echo ok` gate is fundamentally an
  **exit-code** concern (Part B's shell responsibility); this helper only recognises the `ok`
  convention on stdout.

The version parsers are honest-or-absent: an absent/malformed line yields `None`, never a
bogus version. `regex` is already a project dependency (result-extraction), so these three
patterns reuse it rather than hand-rolling.

## Tests (biting)

- `server_profiles.rs`: create→list round-trips all fields; a new profile is honestly
  unverified (verified_* all `NULL`); update mutates user fields only and preserves a
  verification stamp; **`delete_profile_nulls_children_and_jobs_survive`** — a job pointed at a
  deleted profile survives with `backend_id` nulled (a naive `DELETE`-the-job cascade fails
  the "job survives" assert — demonstrated red); `set_profile_verified` flips the usability
  gate `verified_at IS NOT NULL` from 0 to 1 profiles.
- `connection_test.rs`: real-format fixtures (the indented ORCA banner, the 3-line
  `ompi_info --version`, `mpirun --version` fallback, `16\n`); not-found/empty/garbage edge
  cases → `None`/`Err`, never a panic; **`orca_version_does_not_scrape_unrelated_digits`** —
  the negative control that goes red if the ORCA regex is loosened to grab any `x.y.z`
  (demonstrated red), proving the `Program Version` anchor is load-bearing.

## Live gate (Part B)

The university server is simply the **first** profile — its ORCA/OpenMPI/`nproc` specs remain
`UNDETERMINED` until Part B's real SSH connection-test runs (Anton authorizes the session),
measures them (rule #10), and calls `set_profile_verified` to stamp `verified_at`. Until then
no profile is a run target.

## Cross-references

- `wiki/architecture/adr-023-server-agnostic-remote-execution.md` — the profile design + the
  nullable-FK amendment.
- `wiki/orca/remote-server-probe-commands.md` — the measured stdout formats the parsers target.
- `wiki/modules/reactions.md` / `commands/reactions.rs` — the jobs-survive delete pattern this
  reuses.
- `wiki/orca/performance.md` — the taskset mask probe (rule #8) that uses the `core_count`
  ceiling.
