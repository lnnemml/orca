# Remote jobs — scripts and classifier (`src-tauri/src/remote/`)

Phase 5 unit 5.2. Three static server-side scripts run and observe a remote ORCA job; the app
decides the job's state from a **snapshot of raw facts** they collect, per
[ADR-024](../architecture/adr-024-remote-execution-intermittent-connectivity.md) Decision l. The
server filesystem is the source of truth (ADR-024 c); the decision is made in Rust, never accepted
from the server (rule #9).

The Rust side does no I/O itself: no ssh, no file reads, no processes. The scripts are embedded
bytes; nothing uploads or runs them yet. The module is registered in `lib.rs` under a scoped
`#[allow(dead_code)]` until units 5.3/5.4 call it.

## What exists

| File | Contents |
|---|---|
| `mod.rs` | module docs; `FactError` — the one error type of every raw-fact parser |
| `markers.rs` | `parse_started` → `Started`, `parse_exit_code` → `u8`, `BootId::parse` |
| `procfs.rs` | `parse_stat` → `ProcStat`, `split_cmdline`, `unix_socket_listed` |
| `tsp.rs` | `match_job_row` → `Option<TspRow { id, state }>` |
| `snapshot.rs` | `Snapshot`, `JobIdentity`, `Attempt`, `SessionMember`, `SocketFact`, `SocketState` |
| `classify.rs` | `classify`, `Outcome`, `FailReason`, `Classification`, `SnapshotError`; predicates `is_alive`, `is_our_wrapper`, `job_session`, `sid_reused` |
| `scripts.rs` + `scripts/` | `WRAPPER`, `CANCEL`, `COLLECT` (embedded scripts) and `sha256_hex` |
| `wire.rs` | `parse_snapshot` — the collector's output → `Snapshot`; `WireError` |
| `race_model.rs` | test-only model of the d′ race (`.started`/`.cancelled`) |
| `script_tests.rs` | test-only: the real scripts run on this machine (see Tests) |

Rule #6 is shared with the local backend: `local_backend::has_normal_termination` (the one
`ORCA TERMINATED NORMALLY` test, also used by `detect_completion`) over the last
`local_backend::TAIL_BYTES` (5 KiB) of the snapshot's output tail.

## The `.started` format

The wrapper publishes `.started` from a temp file by a no-clobber hard link (`ln -T`, one
`linkat(2)`) as six `key=value` lines, any order, each
ending in `\n`:

```text
pid=376681
pgid=376681
sid=376681
boot_id=0f6e3c1a-5b7d-4c2e-9a8b-1d2e3f4a5b6c
starttime=319395485
started_at=1759490000
```

- `pid`, `pgid`, `sid` — positive decimal ids (the wrapper is PID = PGID = SID under tsp);
- `boot_id` — `/proc/sys/kernel/random/boot_id`, lowercase 8-4-4-4-12 hex;
- `starttime` — field 22 of the wrapper's own `/proc/$$/stat`, read with the builtin
  `read -r l </proc/$$/stat`;
- `started_at` — Unix seconds; informational, compared with nothing (the server clock is not
  NTP-synchronised).

Every key is required exactly once. An unknown or duplicate key, a non-decimal value, a blank line,
`\r`, spaces around `=`, a missing final `\n`, or an empty file is a parse error → row 1.

`.exit_code` is the wrapper's `$?`: decimal 0–255, no leading zeros, an optional single `\n`. Anything
else is a "bad exit code" → row 6. The wrapper writes it as `<n>\n`.

`.enqueued` (written by 5.3's submit, by temp file + `rename`) is two `key=value` lines, any order,
each once: `socket=<absolute TS_SOCKET path>` and `id=<decimal tsp id>`. `.cancelled` is an empty
file. Every marker is published atomically from a temp file, never written in place: `.exit_code`,
`.cancelled` and `.enqueued` by `rename` (`mv -fT`); `.started` by a no-clobber hard link
(`ln -T`, one `linkat(2)`), so it is created only if no `.started` exists in any form.

## The scripts

Each script is `scripts/head.sh` (shebang, `set -u`, the shared parsers and readers) concatenated at
compile time with its body (`scripts/<name>.sh`), so all three ship the same parser code and the
tests run exactly the uploaded bytes. Every per-job value is a positional argument. 5.3 uploads each
as `<root>/bin/<name>-<sha256>.sh` (content-addressed; `sha256_hex` gives the sha).

**Shared head.** A strict `key=value` loader (each key once, nothing else, every line
newline-terminated, byte count checked so a NUL cannot hide); `started_parse_file` with the rules of
`parse_started` (leading zeros normalised away, ids ≤ u32, times ≤ u64, compared as strings — no
shell arithmetic); `parse_stat_line` (after the last `) `, field N = token N−2); `valid_path`
(absolute, `[A-Za-z0-9._-]` components, no empty/`.`/`..` component, no trailing `/` — **one
rule** with Rust `classify::is_valid_path`, checked against each other by a test); and the raw
readers `read_raw` (cat), `read_link` (`readlink -n -v`), `path_exists` (stat), `session_pids` (`ps -o pid= -s`). The readers return **absent only for ENOENT
or ESRCH**, matched as the exact coreutils 9.4 message in the C locale; any other error, or a message
that differs, is an error (fails closed). `ps` exits 1 both for "no such session" and for errors, so
only rc 1 with empty stdout and stderr is an empty session.

**`wrapper.sh <job_dir> <mask> <orca_path>`** — launched by tsp as `bash <root>/bin/wrapper-<sha>.sh
…` (tsp makes it PID = PGID = SID). Refuses (exit 2, nothing written) a job dir that fails
`valid_path`, a core mask that does not match `^[0-9]+([,-][0-9]+)*$` (a leading `-` would be a
taskset option), or a relative ORCA path (rule #1). Then the ADR's start sequence, in this order:
- **0** `cd` into the job dir; failure → exit 1 before `.started`, so ORCA never runs elsewhere;
- **1** publish `.started` (own pid/pgid/sid/starttime from the builtin `read -r l
  </proc/$$/stat`, `boot_id`, `started_at` from `printf '%(%s)T'`) with `ln -T`. If a `.started`
  already exists the wrapper **refuses**: exit 1, no ORCA, `.started` and `.exit_code` untouched —
  whatever failed (the temp write or the link), as long as a `.started` exists in any form. Test and
  creation are one atomic step, so two wrappers never run in one job dir;
- **1a** self-check: `.started` must parse and hold exactly what was meant; otherwise `.exit_code`
  = 97, exit 97, no ORCA;
- **2** `.cancelled` exists → exit 0: no ORCA, no `.exit_code`;
- **3** `mkdir -p .tmp` — if that fails, `.exit_code` = 96 and exit 96, no ORCA (a `TMPDIR` outside
  the job dir would let OpenMPI litter escape it, rule #3); then `TMPDIR=<job>/.tmp
  HWLOC_COMPONENTS=-gl OMPI_MCA_hwloc_base_binding_policy=none taskset -c <mask> <orca> input.inp
  >output.out 2>stderr.log` (file names as `local_backend`);
- **4** publish `.exit_code` = ORCA's status, exit with it.

A TERM to the wrapper's group kills the wrapper (default disposition, measured) before it can write
`.exit_code`, so a cancelled running job has none.

**`cancel.sh cancel|check <job_dir> <root>`** — starts with `cd /` and never enters the job dir.
`cancel`: publish `.cancelled` (failure → exit 3, nothing signalled) → **queued path**: read
`.enqueued`; only if `/proc/net/unix` lists its socket exactly, run `tsp -l`; `tsp -r <id>` only if
exactly one row has that id, state `queued` and the job dir as a whole token → **running path**,
only for a parsing `.started` from the current boot and never for its own session or process group:
TERM `-<pgid>` iff the wrapper is alive, ours, and leads its group (live pgrp = `.started` pgid =
pid); then, unless the SID is reused, the sweep iff (alive and ours) or the job session is
non-empty — TERM+CONT each job-session PID, re-evaluate up to 50 × 0.1 s, KILL what is left → at
the end of every cancel (queued, running, nothing found), remove `<job>/.tmp` unconditionally (a
wrapper starting later sees `.cancelled` before it creates `.tmp`). The cwd comparison is
byte-exact (`read -r -d ''`, no trailing-newline stripping), like Rust's. Prints one line per
decision (`queued: …`, `group: …`, `sweep: …`, `tmp: …`) and `done`. A fact
it cannot read stops it with `error …` and exit 3 before any further signal. No `tsp -k`, no kill
by tsp id or by name. `check` prints `started=`, `boot=`, `alive=`, `ours=`, `leader=`,
`sid_reused=`, `own_session=`, `session=` from the same functions and writes or signals nothing — the
shell side of the parity tests. The race window between `tsp -l` and `tsp -r` (a row starting) is
accepted: `.cancelled` already stops the wrapper.

**`collect.sh <job_dir> [<socket>...]`** — starts with `cd /`. A missing job dir is an error (it
would otherwise read as NeverStarted). It reads `/proc` only for a `.started` that parses and is from
this boot, runs `tsp -l` only on a socket `/proc/net/unix` lists, adds the `.enqueued` socket after
the slot sockets (an unparsable `.enqueued` becomes an `Error` fact with the marker's path), and on
any read error other than ENOENT/ESRCH prints an `error` record and exits 3.

## The snapshot wire format

`collect.sh` prints records in a fixed order. A record is a line `<name>` or `<name> <arg>`; a
**byte record** has a decimal length (no leading zeros) or `-` (absent) as its arg, and a present one
is followed by exactly that many raw bytes and `\n` (a cmdline holds NULs, so nothing is
line-delimited).

```text
orcastudio-snapshot 1
boot_id <len>               required
started <len>|-             first read
proc collected|skipped      collected iff .started parses and is from this boot
  wrapper_stat <len>|-      ┐
  wrapper_cmdline <len>|-   │ only when collected
  sid_stat <len>|-          │
  members <n>               │ then n × (member <pid>, cwd <len>|-)
sockets <n>                 then n × (socket <len>, then socket_error <len>
                              | net_unix <len> + (nodaemon | rows <n> + n × row <len> | tsp_error <len>))
exit_code <len>|-
cancelled yes|no
tail <len>|-                last 5 KiB of output.out
started <len>|-             re-read last
end
```

`error <len>` may replace any record line. `wire::parse_snapshot(bytes, identity, attempt,
slot_sockets)` is strict: an `error` record → `WireError::Collector`; an unknown, missing, duplicate
or out-of-order record, a bad length, a missing `\n` after the bytes or anything after `end` →
`Malformed`. It also re-checks the collector's own decisions (rule #9) → `Inconsistent`: `proc
collected/skipped` must match Rust's parse of `.started` and `boot_id` (a corrupt `.started` accepts
either — row 1 does not use `/proc`); `nodaemon` vs `rows`/`tsp_error` must match `unix_socket_listed`
over the emitted `/proc/net/unix` evidence (header + the lines naming the socket); the slot sockets
come first, in order, with at most one extra; every row mentions the job dir; the tail is ≤
`TAIL_BYTES`. `WireError` converts into `SnapshotError::Collection`, so a broken collection means no
action this pass, like `Indeterminate`.

## Parsers

- **`/proc/<pid>/stat`** — fields come from the text after the **last** `) ` (comm may hold spaces
  and parens); field N is token N−2. Extracts pid (1), state (3), pgrp (5), session (6),
  starttime (22). A truncated line, a non-numeric field or a multi-letter state is an error.
- **cmdline** — NUL-separated argv, final NUL dropped; an empty cmdline (a zombie) is an empty argv.
- **`/proc/net/unix`** — `unix_socket_listed(text, path)`: true iff some line's path column equals
  `path` exactly. The header must be `Num RefCount Protocol Flags Type St Inode Path` and every line
  must have the seven fixed columns, so a garbled read is an error, never "no daemon".
- **`tsp -l` rows** — a row is this job's iff the job dir is a **whole whitespace-separated token**
  (`/jobs/j1` does not claim `/jobs/j10`). Its id and state (`queued`/`running`/`finished`) must
  then parse; any other state word is an error.

## Predicates

- **Alive** — `.started`'s `boot_id` is the current one, the stat line exists, state ≠ `Z`, and
  field 22 equals `.started`'s `starttime`.
- **Ours** — argv[0] = `bash`, argv[1] = `<root>/bin/wrapper-<lowercase hex>.sh`, argv[2] = the job
  dir exactly. The probe-recorded cmdline (`bin/wrapper.sh`, no sha) is therefore *not* ours; the
  tests derive the uploaded shape from it.
- **SID reused** — a stat line exists at the SID with a **different** starttime. Absent or same
  starttime (alive, or a zombie not yet reaped) → not reused. The cancel sweep and the classifier
  apply the same guard.
- **Job session** — empty if the SID is reused; otherwise the `ps -s <sid>` members whose cwd is
  exactly the job dir, a member with no readable cwd (ENOENT) excluded. This is the set row 3
  triggers on and row 8 reports as orphans, so a foreign process in the job dir under a reused SID
  neither holds a cancelled job in `Cancelling` nor appears as an orphan.

## `classify(snapshot, reenqueue_count) -> Result<Classification, SnapshotError>`

1. **Snapshot checks** → `SnapshotError` (no action this pass, like `Indeterminate`): a job dir or
   root that fails `is_valid_path` (the shell's `valid_path` rule; 5.3's submit also asserts the job
   dir equals its `realpath`, since the cwd filter compares canonical paths); no socket facts; a
   current `boot_id` that does not parse; and — only when a row needs it — a wrapper stat line
   (rows 3/7) or a `/proc/<sid>/stat` line (the job session, rows 3/8) that does not parse or names
   another pid. A garbled stat is never read as "dead": that would make
   a computing job `Lost`.
2. **Bracketing `.started` reads.** If the first and last reads differ (in presence or bytes), a
   first snapshot returns `Classification::Retake`: the caller collects again with
   `Attempt::Retake`. A retake whose reads still differ is `Indeterminate`. So at most one retake,
   and the retake decision stays inside the pure function.
3. **The precedence table**, ADR-024 l, rows 1–11 in order, first match wins:
   corrupt `.started` → `Failed`; cancelled + clean finish → `Completed { late_cancel: true }`;
   cancelled + (alive and ours, or non-empty job session), this boot → `Cancelling`; cancelled →
   `Cancelled`; clean finish → `Completed`; any `.exit_code` → `Failed`; alive and ours, this boot →
   `Running`; started → `Lost { orphans }` (job session, this boot only); a queued/running row →
   `Queued`; any socket `Error` (or an unreadable row of ours) → `Indeterminate`; else `ReEnqueue`
   when `reenqueue_count` = 0, `Failed` ("wrapper never started") otherwise. `NoDaemon` counts as no
   rows.

`Outcome` is the classifier's own enum; `JobStatus` is unchanged (`Lost`/`Cancelling` join it in
5.4).

## Tests

108 tests in the module.
- **Pure (classifier, parsers, wire):** strict-parser garbage cases, the recorded probe fixtures (P2
  cmdline, P4 `tsp -l`, 5.2b `/proc/net/unix` line, 5.2c stat lines including `w q) x.sh` and the
  zombie), at least one snapshot per table row, the d′ race model over all 6 interleavings, and the
  wire parser's malformed/inconsistent cases. The probe page records stat and `/proc/net/unix` lines
  with `…` elisions; the fixtures keep every recorded field verbatim and fill the elided ones from a
  laptop read, marked in comments.
- **Real scripts (`script_tests.rs`)**, on this machine: a stub `tsp` on `PATH` (logs its argv;
  `-l` prints the P4 header and rows, `-r` succeeds) and a stub ORCA (records cwd, `TMPDIR`, env,
  args and affinity; can sleep, fail, or start extra session members: an escaped "rank" with its own
  PGID, a foreign-cwd member, a zombie member, a non-dumpable member whose cwd is EACCES). Wrappers
  start in their own session like under tsp; a perl `IO::Socket::UNIX` listener stands in for a
  live daemon's socket, and a killed one leaves a stale socket file. Covered: the wrapper's happy path
  (inotify sees `.started` as one `IN_CREATE` and `.exit_code` as one `IN_MOVED_TO`, nothing else),
  cd failure, bad arguments, `.cancelled` preset, forced self-check failure (97, via an `ln` on `PATH`
  that publishes an empty `.started`), an existing `.started` (refused, both markers byte-identical),
  two wrappers launched together (exactly one runs ORCA), `.tmp` not creatable (96), ORCA's non-zero
  code, a temp-write failure next to an existing `.started` (refused, never 97), bad core masks
  (`-p`, `0-3x`, … → exit 2, nothing written); one path rule (shell `valid_path` = Rust
  `is_valid_path` on every case); shell-vs-Rust parity on fixtures (a)–(e) and on which `.started`
  files are corrupt (duplicate/unknown/missing key, NUL, CRLF, …); a byte-exact cwd compare (a
  `<job>\n` twin dir is foreign); the cancel paths (group TERM + escaped member swept, foreign cwd
  untouched, reused SID, own session, a `.started` from another boot, a live leader that is not our
  wrapper, our wrapper that does not lead its group, verified-queued `tsp -r`, no daemon → no tsp
  call, `.tmp` removed for a never-started job); both d′ orders; the collector round trip through
  `classify` (Running, Completed, Cancelled, Queued, ReEnqueue, Indeterminate, `Lost` for a
  `.started` from another boot with `proc skipped`) and its read errors. Each test signals only processes it started, by recorded PID and start time, and cleans up
  in `Drop`.

Negative controls (each guard broken, the named tests red, restored): listed per unit in
[log.md](../log.md) (Part A 2026-10-03, Part B 2026-10-03).

## Not built yet

- Upload of the scripts, the NUL-separated stdin argument transport, submit and `.enqueued`, the
  slot check on submit, the socket-path post-condition, the job-dir `realpath` assertion — unit 5.3.
- **5.3 probe list (rule #10, measured on the laptop only):** on uni, the exact C-locale messages of
  coreutils (`cat`, `tail`, `stat`, `readlink -v`) and procps (`ps -s` exit codes) that the readers
  match — a different message fails closed, as a snapshot error; and `ln -T` as one `linkat` that
  refuses an existing file, directory or dangling symlink.
- Reconcile wiring, the DB re-enqueue counter (schema v19), `Lost`/`Cancelling` in `JobStatus`,
  sweep counting — unit 5.4.
