# server-profiles — remote execution targets (data layer + connection test)

**Status:** schema v19, save-time validation, the verification lifecycle, the run-target rule,
the connection-test script with its parser and verdict, the `test_server_profile` command that runs
it over ssh, and the Settings → Servers UI. The command has run live against uni (see "Live run").
The UI awaits Anton's live WebKitGTK check. Phase 5 unit 5.1, ADR-023,
[ADR-024](../architecture/adr-024-remote-execution-intermittent-connectivity.md) Decision n.

A **server profile** is the runtime configuration of one remote execution target. It is stored as
data, never as code: adding a server is a settings action, not a build (ADR-023). One `SshBackend`
(unit 5.3) serves any number of these rows.

## Table: `server_profiles` (schema v19)

v18 creates the table and v19 adds `slot_count` and `availability_window` (both in `db.rs`, each
ALTER guarded by `column_exists`). Every column is a discrete typed column; nothing is stored as a
JSON blob.

| Column | Type | Meaning |
|---|---|---|
| `id` | TEXT PK | UUID, generated on create. |
| `name` | TEXT | User-facing display name. |
| `host` | TEXT | The `~/.ssh/config` host alias, i.e. the transport handle (ADR-005). The app stores **no credentials**; auth stays with the user's SSH setup. |
| `remote_orca_path` | TEXT | Absolute path to the remote `orca` (rule #1). Checked on save. |
| `remote_scratch_dir` | TEXT | **The root** (ADR-024 n item 4). Job dirs, `tsp/` sockets and `bin/` all live under it. Checked on save (see below). |
| `core_mask` | TEXT? | `taskset` CPU list (rule #8). `NULL` until measured. A profile without a mask is not a run target. |
| `orca_version` | TEXT? | ORCA version from the last full pass. |
| `openmpi_version` | TEXT? | OpenMPI version reported by `ompi_info --version` at the last full pass. It is recorded, not matched against anything (n item 9). `NULL` if the host reported none. |
| `core_count` | INTEGER? | `nproc` from the last full pass, the ceiling for the mask (rule #8). |
| `verified_at` | TEXT? | Time of the last full-pass connection test. **`NULL` = not verified.** It gates **new submits only** (n item 6a); reconcile, cancel and fetch never read it. |
| `created_at` | TEXT | Insert timestamp. |
| `slot_count` | INTEGER | `NOT NULL DEFAULT 1 CHECK (slot_count = 1)`. A profile has one `core_mask`, and parallel slots need one disjoint mask each (Decision b). So until slots are measured, the database rejects any value other than 1. Rows from before v19 backfill to 1. |
| `availability_window` | TEXT? | `HH:MM-HH:MM` in the **laptop's** local time zone, or `NULL` for no window (n item 3). It is only an informational label. |

`ServerProfile::COLUMNS` and `from_row` (`models/server_profile.rs`) mirror the table one-to-one and
drive every `SELECT`.

## The nullable-FK `NULL = local` decision

`jobs.backend_id TEXT REFERENCES server_profiles(id) ON DELETE SET NULL` (v18). A local run has
`NULL`, and a remote run has the profile id. There is **no `'local'` sentinel row** and **no
backfill**: a `NULL` FK already means "local". This follows the v13 `pathway_id` precedent.

## Pure rules (`models/server_profile.rs`)

- **`ProfileTarget`** holds the fields a verification certifies: `host`, `remote_orca_path`,
  `remote_scratch_dir`, `core_mask`, `slot_count`. The name and the window are not part of it.
- **`validate_profile(target, window)`** runs before every create and update. An error becomes
  `AppError::Invalid`, and nothing is written. It checks that:
  - the host passes **`validate_host`**: not empty, at most 253 bytes, only `[A-Za-z0-9._@-]`,
    and **no leading `-`** (verifier F2). This refuses whitespace, control characters, shell
    metacharacters, `:` and `%`; an IPv6 literal or an ssh `%` token needs a `~/.ssh/config` alias
    instead. The host also reaches ssh only after `--` (see "The command"), so this check is the
    second layer;
  - `remote_orca_path` is absolute;
  - `slot_count` is 1;
  - the root passes **`validate_root`**:
    - it follows the one path rule of Decision l item 4 (absolute, `[A-Za-z0-9._-]` components,
      no empty, `.` or `..` component, no trailing `/`). This reuses `remote::classify::is_valid_path`,
      the Rust twin of the shell's `valid_path`;
    - every slot socket under it fits the 100-byte bound;
  - the core mask (if set) parses with **`parse_core_mask`**: it must match the wrapper's regex
    `^[0-9]+([,-][0-9]+)*$`, and each comma element must be `N` or an ascending `N-M`. The regex
    alone also admits `1-2-3` and `5-2`, and how `taskset` reads those is not measured, so they are
    refused rather than guessed;
  - the window (if set) passes **`validate_window`**: `HH:MM-HH:MM`, hours ≤ 23, minutes ≤ 59. A
    window may wrap past midnight (`22:00-08:00`), but equal ends are rejected.
- **Socket layout** is `<root>/tsp/slot<N>.sock` (`remote::slot_socket_path`, bound
  `remote::MAX_SOCKET_PATH_BYTES` = 100). For slot 0 the suffix is 15 bytes, so the longest root
  that fits is 85 bytes. 5.3's submit uses the same function.
- **`is_run_target(profile) -> Result<(), NotRunTarget>`** (n item 2) passes only for a profile
  that has `verified_at`, a mask, and a `core_count`, with every CPU of the mask within
  `0..core_count-1`. Otherwise it returns the reason: `NotVerified`, `NoCoreMask`, `BadCoreMask`,
  `NoCoreCount`, or `CpuOutOfRange { cpu, core_count }`. Its caller is 5.3's submit. The range rule
  is `cpu_out_of_range`, which the connection-test verdict uses too.

## CRUD and the verification lifecycle (`commands/server_profiles.rs`)

Each Tauri command locks the shared connection and calls a `*_conn(&Connection, …)` helper, so the
logic is testable without Tauri. Every command returns `Result<T, AppError>`.

- `create_server_profile(name, host, remote_orca_path, remote_scratch_dir, core_mask?,
  availability_window?)` validates and inserts. `slot_count` takes its default of 1, and the
  verified_* columns stay `NULL`.
- `list_server_profiles()` returns all profiles, newest first.
- `update_server_profile(id, name, host, remote_orca_path, remote_scratch_dir, core_mask?,
  availability_window?)` validates, then writes in one transaction. If the **value** of any
  `ProfileTarget` field differs from the stored one, `verified_at` **and** the three verified facts
  are set to `NULL` together (n item 5). A rename, a window edit, or a save that rewrites a field
  with its current value keeps the stamp. `slot_count` is not editable; it is carried over. A change
  of `host` or `remote_scratch_dir` is refused (`AppError::Conflict`, naming the jobs) while the
  profile has **live remote jobs** — `live_remote_jobs`: `backend_id` = the profile, coordinates
  set, status `queued`/`running` (ADR-024 n 6b): their coordinates name that host and root.
- `delete_server_profile(id)` is refused the same way while the profile has live remote jobs
  (ADR-024 o item 2). Otherwise it **nulls `backend_id` on every job of the profile first**, then
  deletes the row, so the jobs survive. This is the load-bearing invariant, the same as
  `delete_reaction`. A finished remote job keeps its coordinates, so it stays remote without a
  profile (`Job::is_remote`).
- `set_profile_verified_conn(id, tested, orca_version, openmpi_version?, core_count)` stamps a
  **full pass**. `tested` is the `ProfileTarget` the test ran against. The `UPDATE` matches on it,
  so if the profile was edited while the test ran, the write is refused with `AppError::Conflict`
  and nothing is stamped. A stamp therefore never certifies a target it did not test. It is **not
  an IPC command** (verifier F3): its only caller is `test_server_profile`, so every stamp comes from
  Rust's own verdict (rule #9). `the_stamp_is_not_an_ipc_command` pins this.
- `clear_profile_verified(id)` handles a re-test that was not a full pass (n item 6): it sets
  `verified_at` and the facts to `NULL`. It stays an IPC command, since clearing is the fail-closed
  direction.
- The commands that return a profile (`create`, `list`, `update`, `clear`) return a
  **`ServerProfileView`**: the row plus `run_target: { is_run_target, reason }`, computed by
  `is_run_target` in Rust. The UI shows that reason and never re-derives the rule.

Stamp and facts are always all set or all `NULL`; a stamp never outlives the facts it certified.

## The connection test (`connection_test.rs`, `remote/scripts/conntest.sh`)

### Transport (n item 11)

The test is one static script, `remote::scripts::CONNTEST` (the shared `head.sh` + `conntest.sh`,
embedded with `include_str!`). It is fed to `ssh -o BatchMode=yes -o ConnectTimeout=10 -- <host>
bash -s` on stdin, followed on the same stdin by the values as a NUL list: ORCA path, root, core
mask (empty when unset).
`conntest_stdin(args)` builds these bytes and refuses a value that contains a NUL. The shape rules
were measured in probe 5.1c:

- the script's **last line** is
  `args=(); while IFS= read -r -d '' a; do args+=("$a"); done; main "${args[@]}"; exit`;
- before it there are only definitions and commands that do not read stdin. Of the head, the script
  uses only `valid_path`;
- **every child command gets `</dev/null`**, and `export LC_ALL=C` is set.

### Output format

The output uses the 5.2 collector's record syntax and is read by the same strict
`remote::wire::Reader`:

```text
orcastudio-conntest 1
argc <n>                  the number of values received
arg <len>   (n times)     each value, verbatim
<check> <rc>|skipped      for each check, in this fixed order:
  out <len>                 mkdir realpath findmnt busctl id nproc orca_x orca ompi
  err <len>               (out/err follow only a check that ran)
end
```

- A record line is `<name>` or `<name> <arg>`.
- A byte record `<name> <len>` is followed by exactly `<len>` raw bytes and a newline.
- If `argc` is not 3, `end` follows right after the values.
- An `error <len>` record in place of any record line means the script itself failed (e.g.
  `mktemp`); it exits 3.

The checks the script runs:

| Check | Command (all `</dev/null`) | Notes |
|---|---|---|
| `mkdir` | `mkdir -p -- <root>` | Run only if `valid_path <root>`; otherwise `mkdir`, `realpath` and `findmnt` are `skipped` and the root is never touched. |
| `realpath` | `realpath -e -- <root>` | Measured on the laptop and on uni (live run): prints `<root>\n`, rc 0. |
| `findmnt` | `findmnt -no FSTYPE --target <root>` | Measured on the laptop and on uni (live run): one column, `ext4\n`, rc 0. |
| `busctl` | `busctl get-property org.freedesktop.login1 /org/freedesktop/login1 org.freedesktop.login1.Manager KillUserProcesses` | |
| `id` | `id -nG` | |
| `nproc` | `nproc` | |
| `orca_x` | `test -x <orca>` | `skipped` if the path is not absolute. |
| `orca` | `<orca> --version`, stderr merged into stdout, captured whole | Run only if `orca_x` is 0; its `err` record is empty. |
| `ompi` | `ompi_info --version` | |

### Parsing and verdict (rule #9: every decision is Rust's)

**`parse_output(stdout, sent)`** parses strictly. An unknown, missing, duplicated or out-of-order
record, a bad length, an rc that is not a canonical decimal 0–255, or bytes after `end` gives
`ConnTestError::Malformed`. **Post-condition:** before it reads any fact, the parser asserts that
the echoed values are byte for byte the ones sent. A wrong count gives `ArgCount`, a wrong value
`ArgMismatch`. Checking the values, not only the count, matters: a script line left after the read
loop is read into the first value, so the count stays 3 and only the value check catches it.

**`evaluate(facts, sent) -> Verdict`** (n item 8) returns a `FullPass { facts, warnings }` only if
every mandatory check passes. Otherwise it returns `NotPassed { failures: [CheckFailure { check,
reason }], warnings }`. "Undetermined" always counts as not passed. The mandatory checks are:

| Check | Passes when |
|---|---|
| `Orca` | `test -x` rc 0, then a `Program Version x.y.z` line in the output, **whatever the rc** (ORCA exits 2 on `--version`). rc 127 means not found and rc 126 not executable; both are named in the reason. |
| `Cores` | `nproc` rc 0 and a positive integer. |
| `CoreMask` | Only checked when a mask is set: it must parse and every CPU must lie within `0..nproc-1`. If `nproc` is undetermined, the mask cannot be checked and fails too. |
| `KillUserProcesses` | Exactly `b false\n` with rc 0. `b true` gets its own reason; anything else is "undetermined". |
| `Root` | The path is valid, `mkdir` rc 0, the `realpath` output equals `<root>\n`, and `findmnt` prints exactly one line whose first token is on `ROOT_FS_ALLOW_LIST` = `{ext4}`. |

Recorded but not gating (n item 9):
- the OpenMPI version, via `parse_openmpi_version`. If none is reported it becomes `None`, with a
  `Warning::OpenMpiNotReported`;
- the `sudo` token in `id -nG` gives `Warning::SudoGroup`, and a failed `id` gives
  `Warning::GroupsUndetermined`.

`run(stdout, sent)` combines parsing and evaluation. The version and `nproc` parsers
(`parse_orca_version`, anchored on `Program Version`; `parse_openmpi_version`; `parse_nproc`) are
the Part A parsers, reused here. `parse_presence` is superseded, because the script records
`test -x`'s rc itself.

## The command: `test_server_profile(id)` (`commands/server_profiles.rs`, `remote/ssh.rs`)

An `async` Tauri command; the work runs in `spawn_blocking`, off the GTK/WebKit main thread. The
logic is `test_server_profile_with(db, id, runner, timeout)`, which takes the runner as a
`&dyn CommandRunner` so the decisions are tested with a fake.

1. Lock the database, read the profile, **release the lock**. ssh never runs under the lock.
2. Re-validate the stored profile (`validate_profile`). A row that does not validate is never sent
   to ssh.
3. Build the argv with **`remote::ssh::ssh_bash_argv(host)`**:
   `ssh -o BatchMode=yes -o ConnectTimeout=10 -- <host> bash -s`.
   - The host is one argv element right after `--`. Measured (OpenSSH 9.6p1, `ssh -G`): without
     `--`, a host `-oProxyCommand=echo PWNED` becomes the option `proxycommand echo PWNED`; with
     `--`, ssh refuses it (`hostname contains invalid characters`).
   - The only words after the host are the static `bash -s`. Every profile value travels on stdin
     after the script, so no profile value is ever shell text on either side.
   - `BatchMode=yes`: ssh never prompts (password, host key); it fails instead.
4. stdin is `conntest_stdin(ConnTestArgs::for_target(target))`: the script, then the three values
   NUL-terminated, and nothing after them.
5. Run it with `SystemRunner` under **`CONNTEST_TIMEOUT` = 30 s**, the bound on the whole run.
   - The runner writes stdin from a thread and reads stdout and stderr from two more, so no pipe can
     fill and deadlock. Each stream is capped at 1 MiB (`MAX_OUTPUT_BYTES`). A stream past the cap
     kills the process group at once and is `OutputTooLarge`; the run does not wait for the deadline.
   - ssh runs in its own process group. On timeout the runner kills the group and reaps ssh, so
     nothing ssh started survives. The deadline also bounds a grandchild that keeps a pipe open
     after ssh exits.
   - Why 30 s: ssh gives up connecting after `ConnectTimeout` = 10 s; the live run measured 1.1 s
     cold and about 0.3 s warm (see "Live run").
6. Judge the result, in Rust only:
   - ssh exit **255** is ssh's own failure (connect, auth, host key). The reason carries ssh's
     stderr tail.
   - Otherwise `connection_test::run(stdout, sent)` parses and evaluates. A parse error is reported
     with the ssh exit and the stderr tail.
   - A complete output with any exit other than 0 is not trusted (the script ends `printf 'end\n'`
     and exits 0).
7. Write, under the lock again:
   - **`FullPass`** → `set_profile_verified_conn` with the tested target. `Conflict` (the profile was
     edited meanwhile) is reported as the `conflict` outcome; nothing is stamped, and the edit has
     already cleared the old stamp.
   - **`NotPassed`** → `clear_profile_verified_conn`.
   - **No verdict** (invalid profile, spawn failure, timeout, ssh 255, unreadable output, non-zero
     exit) → `clear_profile_verified_conn`. A clear is unconditional, so an old failing test also
     clears a newer stamp (fail closed); the UI runs one test per profile at a time.
   - **Known limit:** the stamp's CAS binds the tested *target*, not the test's start, so an older
     passing test that finishes after a newer failing one (same target) can still stamp.

It returns a **`ConnTestReport`**: `outcome` = `verified` (checks, facts, warnings) | `conflict`
(reason, checks, facts, warnings) | `not_passed` (checks, warnings) | `failed` (reason); plus the
profile after the write (as a `ServerProfileView`) and `elapsed_ms`. `checks` lists every mandatory
check in a fixed order (`orca`, `cores`, `core_mask` only when a mask is set, `kill_user_processes`,
`root`). Each is derived from the verdict only: it passed iff the verdict has no failure for it.

## Settings → Servers (`src/servers/`)

`ServersSection` is a card in the Settings screen.
- **List:** name, host, ORCA path, root, mask, slots (always 1) and window. The **headline** is the
  run-target status from Rust: "Run target", or "Not a run target: <Rust's reason>". It never reads
  "verified", because a verified profile without a mask is still not a run target. A second line
  says "Connection test passed <verified_at> UTC" with the recorded facts, or "Not verified".
- **Add / Edit form:** name, host alias, ORCA path, root, core mask, window. An empty optional field
  is sent as `null`. Slots are shown fixed at 1. Rust's validation error is shown inside the form as
  returned, and the form stays open. When editing, a note says that changing a target field clears
  the verification.
- **Delete:** two clicks (Delete → Confirm delete).
- **Test connection:** shows a spinner and disables the row's buttons while it runs. Then it shows
  the outcome line, each check as pass/FAIL with its reason, the measured facts, the warnings and
  the wall time. The row switches to the profile Rust returned. The component never calls a stamp
  command.
- `status.ts` holds the pure mapping (`profileStatus`, `checkLabel`, `warningText`,
  `outcomeSummary`); it only turns Rust's values into words.

## Tests (each invariant has a negative control)

- **`remote/ssh.rs`:**
  - `the_argv_is_the_fixed_shape_with_double_dash_right_before_the_host`. Negative control: drop
    `--` → red;
  - `the_argv_refuses_a_host_that_could_be_an_option_even_if_it_was_stored`;
  - `SystemRunner` on real processes: stdin beyond a pipe buffer comes back whole with both streams
    and the code; `exec sleep 30` is killed at a 300 ms timeout; output past the cap is refused; a
    missing program is a spawn error;
  - `a_timeout_kills_the_grandchildren_too`: a `sleep 30` started by the child, which writes its pid
    to a pidfile, is gone after the timeout, both while the child still runs and after it has
    exited holding stdout open. A guard kills that `sleep` if the test fails. Negative control:
    remove `libc::killpg` → red in each case;
  - `an_unbounded_stream_is_refused_promptly` (a writer that ignores SIGPIPE): `OutputTooLarge` in
    about a second, not a 20 s timeout. Negative control: treat `TooLarge` like ordinary bytes → red.
- **`commands/server_profiles.rs` (the command, with a `FakeRunner`):**
  - `a_full_pass_stamps_the_tested_target_with_exactly_the_measured_transport` (argv, and stdin
    byte for byte = `conntest_stdin`, ending in the NUL list);
  - `a_check_that_does_not_pass_clears_the_stamp_and_names_the_reason`. Negative control: stamp on
    `NotPassed` → red;
  - `no_verdict_clears_the_stamp`: timeout, spawn failure, ssh 255, empty output, truncated output,
    exit 1 after a complete output, killed by a signal. Negative control: skip the clear on that
    path → red (with `an_invalid_stored_host_never_reaches_ssh`);
  - `an_edit_during_the_test_is_a_conflict_not_a_stamp` (also shows the lock is free while ssh runs);
  - `an_invalid_stored_host_never_reaches_ssh`; `testing_a_missing_profile_is_not_found_and_runs_nothing`;
  - `the_stamp_is_not_an_ipc_command` (F3). Negative control: re-add the `set_profile_verified`
    command and its handler entry → red;
  - `the_run_target_status_carries_the_reason`, `the_report_serializes_flat_for_the_ui`,
    `warnings_and_outcomes_serialize_as_the_frontend_types_expect`;
  - `live_uni_connection_test` (`#[ignore]`, by hand: `cargo test live_uni -- --ignored
    --nocapture`): a throwaway DB, the `uni` alias only.
- **`models/server_profile.rs`:** `a_host_that_could_be_an_option_or_shell_text_is_refused` (F2) and
  `plausible_host_aliases_are_accepted`. Negative control: drop the leading-`-` rule → red.
- **vitest (`src/servers/`):** `status.test.ts` (headline from `run_target`, reasons, outcome
  wording) and `ServersSection.test.tsx` (jsdom: list states, spinner then report, a FAIL with its
  reason, a `failed` outcome, an inline validation error, edit, two-step delete, no stamp command).
  Negative control: map the headline from `verified_at` instead of `run_target` → two tests red.

- **`db.rs`:** `migrate_v18_to_v19_backfills_slot_count_and_keeps_the_row` (an existing row gets
  `slot_count` 1 and keeps its stamp); `migrate_v19_slot_count_check_rejects_anything_but_one` (2,
  0 and −1 are rejected on UPDATE and INSERT); `fresh_db_has_the_v19_columns_with_their_defaults`.
  - Negative control: drop the `CHECK`, and the last two go red.
- **`models/server_profile.rs`:** path, root, socket-bound (85 bytes pass, 86 fail), mask, window
  and run-target tests.
  - Negative control: `run_target_mask_range_is_zero_to_nproc_minus_one` goes red with an
    off-by-one `>` in `cpu_out_of_range`.
- **`commands/server_profiles.rs`:**
  - `invalid_fields_are_refused_and_nothing_is_written` (create and update);
  - `changing_a_target_field_clears_the_stamp_and_its_facts`. This is the inverted half of the
    old "update preserves the stamp" test. Negative control: an update that clears nothing goes red;
  - `set_profile_verified_stamps_and_a_non_target_update_preserves_it` (rename, window edit,
    same-value save). Negative control: clearing on every save goes red;
  - `a_stamp_for_a_target_the_profile_no_longer_has_is_refused`;
  - `clear_profile_verified_clears_stamp_and_facts`;
  - `delete_profile_nulls_children_and_jobs_survive`;
  - `a_profile_with_live_remote_jobs_cannot_be_deleted` (queued/running refused; finished remote and
    local jobs do not hold the profile). Negative control: drop the `refuse_while_live` call from
    `delete_server_profile_conn` and it goes red;
  - `a_profile_with_live_remote_jobs_keeps_its_host_and_root` (host and root refused; name, ORCA
    path and mask still editable; free again once the job is finished).
- **`connection_test.rs` (`conntest_tests`)** uses fixtures copied verbatim from
  `orca/remote-server-probe-commands.md` (rule #10): busctl `b false` and the bad-property stderr,
  `anton users`, `ext4   /dev/sdb4 /home`, ORCA line 54 with rc 2, the rc 127/126 shapes, the three
  `ompi_info` lines, and `48`.
  - **Strict parsing:** malformed output, the arg post-condition, the script's `error` record.
  - **Verdict:** each check, with negative controls: accepting `b true` →
    `kill_user_processes_must_be_exactly_b_false` red; accepting a non-ext4 type →
    `the_root_must_be_on_an_allow_listed_filesystem` red; the mask off-by-one →
    `the_mask_must_lie_within_zero_to_nproc_minus_one` red.
  - **The real script, run locally under `bash -s`** with stub `busctl`/`findmnt`/`id`/`nproc`/
    `ompi_info` on `PATH` and a stub ORCA at an absolute path. Every stub exits 99 unless its stdin
    is `/dev/null`. The tests cover: a full pass that creates the root; awkward values carried
    verbatim (a newline, `'`, `$HOME`, `;`); `b true` plus `tmpfs`; a symlinked root; an ORCA path
    that is a directory (rc 126); an invalid root that is never touched; the echoed count when only
    2 values are sent.
  - **Transport post-conditions, kept as permanent tests:**
    `a_script_line_after_the_read_loop_breaks_the_post_condition` (→ `ArgMismatch`, index 0) and
    `a_stdin_reading_child_before_the_loop_breaks_the_post_condition` (empty output → `Malformed`).
  - **Shape:** `the_read_loop_is_the_last_line_and_nothing_follows_it` and
    `every_child_command_in_the_script_reads_dev_null`.
  - Negative controls: a line appended after the loop turns the shape test and the real-script
    tests red; dropping `</dev/null` from ORCA's call turns the lint test red, and the real-script
    full pass too (the stub ORCA exits 99).

## Live run (2026-10-03, rule #10)

`live_uni_connection_test` ran the command path against the `uni` alias in a throwaway database:
root `/home/anton/.orcastudio`, ORCA `/opt/orca/orca`, mask `0-3`. The raw output and timings are
recorded in [remote-server-probe-commands.md](../orca/remote-server-probe-commands.md) ("Live
connection test on uni").
- **Full pass:** ORCA 6.1.1, OpenMPI 4.1.6, 48 CPUs, no warnings. The profile is stamped and is a
  run target.
- **Bogus ORCA path** (`/opt/orca/no-such-orca`): `not_passed`, with the ORCA check failing on
  "ORCA is missing or not executable (test -x failed)". The stamp is cleared. A missing path fails
  at `test -x`, so the script never runs it, and rc 127 is not reached for a missing path.
- **Times:** 1114 ms cold (no ControlMaster), 353 ms and 285 ms warm.

The live run used a throwaway database; no profile exists in the app's own database until Anton
creates one in Settings → Servers. The UI awaits his live WebKitGTK check.

## Cross-references

- `wiki/architecture/adr-024-remote-execution-intermittent-connectivity.md`: Decision n (this page),
  Decision l item 4 (the one path rule, the socket bound).
- `wiki/architecture/adr-023-server-agnostic-remote-execution.md`: the profile design and the
  nullable-FK amendment.
- `wiki/orca/remote-server-probe-commands.md`: the measured output formats and the transport probe
  (5.1c).
- `wiki/modules/remote-jobs.md`: the shared `head.sh`, the record `Reader`, `is_valid_path`.
- `src-tauri/src/remote/ssh.rs`: the ssh argv and the timeout runner, for reuse by 5.3's submit.
- `wiki/modules/reactions.md` / `commands/reactions.rs`: the jobs-survive delete pattern.
- `wiki/orca/performance.md`: the taskset mask probe (rule #8), bounded by `core_count`.
