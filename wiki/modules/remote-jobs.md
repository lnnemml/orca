# Remote jobs — the pure classifier (`src-tauri/src/remote/`)

Phase 5 unit 5.2. Decides the state of a remote ORCA job from a **snapshot of raw facts** read on
the server, per [ADR-024](../architecture/adr-024-remote-execution-intermittent-connectivity.md)
Decision l. The server filesystem is the source of truth (ADR-024 c); the decision is made in Rust,
never accepted from the server (rule #9).

The module is **pure**: no ssh, no files, no processes. It is registered in `lib.rs` under a scoped
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
| `race_model.rs` | test-only model of the d′ race (`.started`/`.cancelled`) |

Rule #6 is shared with the local backend: `local_backend::has_normal_termination` (the one
`ORCA TERMINATED NORMALLY` test, also used by `detect_completion`) over the last
`local_backend::TAIL_BYTES` (5 KiB) of the snapshot's output tail.

## The `.started` format

The wrapper writes `.started` (temp file + `rename`) as six `key=value` lines, any order, each
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
else is a "bad exit code" → row 6.

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
   root that is not an absolute `[A-Za-z0-9._/-]` path without a trailing `/`; no socket facts; a
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

61 unit tests in the module: strict-parser garbage cases, the recorded probe fixtures (P2 cmdline,
P4 `tsp -l`, 5.2b `/proc/net/unix` line, 5.2c stat lines including `w q) x.sh` and the zombie), at
least one snapshot per table row, and the d′ race model over all 6 interleavings. The probe page
records stat and `/proc/net/unix` lines with `…` elisions; the fixtures keep every recorded field
verbatim and fill the elided ones from a laptop read, marked in comments.

Negative controls demonstrated (each broke the code, went red, was restored): rows 2/4 swapped →
`row2_late_cancel_keeps_a_clean_result`; no `state ≠ Z` → `zombie_wrapper_is_not_alive`; no starttime
equality → `forged_starttime_is_not_alive`; socket `Error` as no rows →
`row10_socket_error_is_indeterminate`; substring row match →
`foreign_row_sharing_a_prefix_is_not_matched`; wrapper checks `.cancelled` before writing `.started`
→ `orca_never_runs_unkilled_under_cancelled`; the SID guard removed from the job session →
`reused_sid_foreign_process_in_job_dir_does_not_hold_a_cancel` and
`reused_sid_foreign_process_in_job_dir_is_not_an_orphan`.

## Not built yet

- The wrapper and cancel scripts, the snapshot collector, and the shell-vs-Rust liveness parity
  tests on materialised processes — unit 5.2 Part B.
- Upload, submit, `.enqueued`, the slot check on submit — unit 5.3.
- Reconcile wiring, the DB re-enqueue counter (schema v19), `Lost`/`Cancelling` in `JobStatus`,
  sweep counting — unit 5.4.
