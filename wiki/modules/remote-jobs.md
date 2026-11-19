# Remote jobs — scripts and classifier (`src-tauri/src/remote/`)

Phase 5 unit 5.2. Three static server-side scripts run and observe a remote ORCA job; the app
decides the job's state from a **snapshot of raw facts** they collect, per
[ADR-024](../architecture/adr-024-remote-execution-intermittent-connectivity.md) Decision l. The
server filesystem is the source of truth (ADR-024 c); the decision is made in Rust, never accepted
from the server (rule #9).

The module itself does no remote I/O: no ssh, no processes. The scripts are embedded bytes, and
the pure parts here build each call's values and parse its reply. The one exception is `sync.rs`,
which lists and hashes the **local** job dir for the transfer post-conditions. The calls are run by
the remote backend's core, `src-tauri/src/ssh_backend.rs` (submit, retry, label over an injected
`CommandRunner`), and by the poller and a cancel's delivery (`crate::poller`; [execution-backends.md](execution-backends.md#sshbackend-ssh_backendrs)),
which `submit_job` and the remote job commands call. The module is registered in `lib.rs` under a
scoped `#[allow(dead_code)]` until units 5.3/5.4 call all of it (the poller, reconcile).

## What exists

| File | Contents |
|---|---|
| `mod.rs` | module docs; `FactError` — the one error type of every raw-fact parser |
| `markers.rs` | `parse_started` → `Started`, `parse_exit_code` → `u8`, `BootId::parse` |
| `procfs.rs` | `parse_stat` → `ProcStat`, `split_cmdline`, `unix_socket_listed` |
| `tsp.rs` | `match_job_row` → `Option<TspRow { id, state }>` |
| `snapshot.rs` | `Snapshot`, `JobIdentity`, `Attempt`, `SessionMember`, `SocketFact`, `SocketState` |
| `classify.rs` | `classify`, `Outcome`, `FailReason`, `Classification`, `SnapshotError`; predicates `is_alive`, `is_our_wrapper`, `job_session`, `sid_reused` |
| `scripts.rs` + `scripts/` | the uploaded job scripts `WRAPPER`, `CANCEL`, `COLLECT`; the stdin-fed calls `CONNTEST`, `SUBMIT`, `LABEL`, `POLL_LOG`, `LIST`, `PREPARE`, `INSTALL`, `RUN` (the trampoline), `MKJOB` (`STDIN_SCRIPTS`, each ending in `READ_LOOP`); `stdin_with_values` (script + NUL list), `upload_path` (`<root>/bin/<name>-<sha256>.sh`), `sha256_hex` |
| `wire.rs` | `parse_snapshot` — the collector's output → `Snapshot`; `WireError` |
| `ssh.rs` | `ssh_bash_argv` (`ssh -o BatchMode=yes -o ConnectTimeout=10 -o ServerAliveInterval=30 -o ServerAliveCountMax=4 -- <host> bash -s`, host re-validated), `ssh_options` (the four `-o` options, shared with rsync's `-e`; the keepalive — a probe every 30 s, given up after 4 — is ADR-024 o24.5: a silently dropped link ends an ssh or rsync in ~2 min whatever `~/.ssh/config` says, except a call multiplexed over a control master, which uses the master's keepalive) and `CommandRunner`/`SystemRunner` (stdin and both streams on threads, 1 MiB cap, process group killed on timeout). `SystemRunner::run` — the one production spawn site of every ssh and rsync: poller steps, commands, the connection test — is `run_gated(&REMOTE_CALLS, …)`: it takes a permit of the argv's host from the process-global gate **before** the timeout's deadline is computed, then runs with the full timeout; the permit is released after the child is reaped, on every path |
| `gate.rs` | the per-host cap on concurrent ssh/rsync processes (ADR-024 o24.2–o24.4): `Gate::new(n)` — per host key a FIFO ticket queue, a caller with ticket `t` is admitted iff `t < released + n`, `released` moves only on a release and every release wakes all waiters; `acquire` → RAII `Permit`; a key's entry is dropped once `next_ticket == released`; mutex poisoning recovered (`into_inner`). `MAX_CONCURRENT_REMOTE_CALLS = 8` and the process-global `REMOTE_CALLS: LazyLock<Gate>` that `SystemRunner::run` uses: at most 8 ssh/rsync per host, one queue shared by the poller, the commands and the connection test, no reserved permit; in-process (empty on every launch, blind to ssh/rsync orphaned by a previous run). Two profiles naming one server differently (`uni` and `anton@uni`) get 8 each. `host_key(program, argv)` → `HostKey::Host` (ssh: the argument after `--`; rsync: the `host:` prefix of the one remote end among the last two) or the shared `HostKey::Fallback` (gated, never ungated) |
| `sync.rs` | rsync argv (`upload_argv`, `download_argv`), the download filter `download_filter_args`, its leaf patterns `download_patterns` and its Rust mirror `download_selects`, `list_dir`/`upload_expected`/`expected_values` (local file lists with sha256), `ListArgs` + `parse_list_reply` (the server's listing), `compare_download` (the download post-condition) |
| `poll.rs` | `PollLogArgs`, `parse_poll_reply` → `LogChunk` with the length post-condition; `check_echo` (the n-6d echo check every 5.3 reply shares) |
| `submit.rs` | `remote_job_dir`, `SubmitArgs` (its `SubmitMode`: a submit, or the re-enqueue form, ADR-024 o25.4), `parse_submit_reply` → `SubmitReply` with the typed `Refusal` (`Blocked { blockers: Vec<Blocker> }`, `SlotCheckFailed`, `LockBusy`, `UploadMismatch`, `Other`, ADR-024 o23.7); `LabelArgs`, `parse_label_reply` → `LabelFacts`, `label` → `Label` (the label rules) |
| `prepare.rs` | the calls that ready the server before an upload, a cancel's delivery or a sweep: `UPLOADED` (wrapper, cancel, collect), `PrepareArgs`, `parse_prepare_reply` → `PrepareFacts` (with the host facts, `PrepareFacts::host`), `check_prepare` → `Prepared` or `ShapeRefusal`; `InstallArgs`, `parse_install_reply` → `[Installed; 3]` |
| `run.rs` | the trampoline's `RunArgs` (`JobScript::{Cancel, Collect}`) and `parse_run_reply` → `RunReply`; a not-on-server cancel's `MkjobArgs` and `parse_mkjob_reply` (o 14.1, o22.4.3) |
| `race_model.rs` | test-only model of the d′ race (`.started`/`.cancelled`) |
| `script_tests.rs` | test-only: the real job scripts run on this machine; the shared `Lab` harness (see Tests) |
| `call_script_tests.rs` | test-only: the real 5.3 calls through `bash -s`, read by their Rust parsers (see Tests) |
| `backend_e2e_tests.rs` | test-only: `ssh_backend`'s submit, retry and label, the cancel core's delivery and the poller end to end against the `Lab` (see Tests) |
| `reenqueue_script_tests.rs` | test-only: `submit.sh reenqueue` against the real script after a dropped queue (see Tests) |

Rule #6 is shared with the local backend: `local_backend::has_normal_termination` (the one
`ORCA TERMINATED NORMALLY` test, also used by `detect_completion`) over the last
`local_backend::TAIL_BYTES` (5 KiB) of the snapshot's output tail.

## A remote job in the database (schemas v20–v22)

(Schema v23 is the server profile's host facts, not a job column — [server-profiles.md](server-profiles.md).)

`jobs` carries the job's **coordinates**: `remote_host`, `remote_job_dir` (absolute,
`<root>/jobs/<job_id>`) and `remote_socket`, nullable TEXT, written once at submit before any ssh and
never rewritten from the profile (ADR-024 o item 1, n 6b). A CHECK on the last column keeps the three
all-NULL or all-set. **A job is remote iff its coordinates are non-NULL**: `Job::is_remote()` is
`remote_host.is_some()`, the same column every local-only query filters on (`remote_host IS NULL`) —
today `local_backend::next_local_queued_job` (behind `try_start_next`) and `reconcile_on_startup`.
`backend_id` never decides it. A remote job keeps `queued`/`running` in `jobs.status`, and
`cancelling` once a cancel has reached the server while processes of the job still run there
(`JobStatus::Cancelling`, ADR-024 o22.5): transient, not terminal, remote only. A remote job that
started on the server and whose wrapper is gone without an `.exit_code` ends **`lost`**
(`JobStatus::Lost`, ADR-024 o25.1): **terminal**, remote only, written only by the poller's evidence
path (`finalize_job_conn`, which stamps `completed_at` and NULLs the pending columns; so does
`update_job_status_conn`'s terminal arm). It has no CHECK and no schema version: the status column is
unconstrained, and the v21/v22 CHECKs already refuse the pending columns on it. Nothing restarts a
`lost` job automatically; the user starts a new one (restart-from-last-geometry stays Decision e's).
`submit::remote_job_dir(root, job_id)` builds the dir and checks it with `is_valid_path`; the job id
must be one component.

**Remote-live** = a remote job in `queued`, `running` or `cancelling`. Every predicate that keeps a
live remote job uses this one set: `poller::plan::candidates` (SQL), `poller::is_live` (behind
`live_row`), `poller::set_message` (SQL), the poller's `mark_cancelled_conn` `from` list,
`ssh_backend::is_remote_live` (behind `refuse_if_remote_live` — itself behind `delete_job_conn`,
`local_backend::cancel` and `cancel_remote` — and the cancel core's routing), the delivery's mark and
clears in `poller/cancel.rs`, the evidence fetch's terminal `cancelled` (`poller::LIVE`), and
`server_profiles::live_remote_jobs` (behind `refuse_while_live`).
The one deliberate exception is the cancel **record** write (`commands::jobs::RECORD_CANCEL_SQL`):
`queued`/`running` only (ADR-024 o22.12.1). The local paths
(`next_local_queued_job`, `reconcile_on_startup`) select explicit statuses with `remote_host IS NULL`
and never see `cancelling`. Every one of these is a positive list, so **`lost` is live for none of
them** (pinned by `poller::lost::tests::a_lost_row_is_live_for_no_predicate`): a `lost` row is
deletable and refuses a cancel like any terminal row. `JobStatus::from_db` has a wildcard arm, so its
explicit `"cancelling"` and `"lost"` arms are pinned by a test, not by the compiler; every other
`match` on `JobStatus` lists its variants (no wildcard).

**The pending cancel (schema v21, ADR-024 o22.2).** `jobs.cancel_requested_at` (the user asked) and
`jobs.cancel_sent_at` (a delivery is about to run `cancel.sh`), nullable TEXT in **UTC, laptop
clock** (SQLite's `datetime('now')`, measured for the app's bundled SQLite 3.46.0 under a non-UTC `TZ`
by `db::tests::sqlite_datetime_now_is_utc_under_a_non_utc_tz`, ADR-024 o22.14.6), only ever displayed
(the view converts it, `formatLaptopUtc`), so a cancel survives a quit and a closed laptop. Three CHECKs, in the v20 ADD-COLUMN
form (a CHECK naming earlier columns, tested by SQLite against every existing row):
`cancel_requested_at` only on a remote `queued`/`running`/`cancelling` job; `cancel_sent_at` only
where `cancel_requested_at` is set; `cancelling` only on a remote job. **Every terminal write of a
remote row clears both columns in the same statement** — `finalize_job_conn` (always; a no-op on a
local row; every poller terminal `completed`/`failed` write, the evidence fetch's row 1 included) and
`ssh_backend::mark_cancelled_conn` (every poller terminal `cancelled` write, the evidence fetch's and
its last strike's included) — so a forgotten clear is a failed write, never a
silent breach. **Every write of `cancelling` clears both too** (ADR-024 o22.12.1): delivery's and the
ordinary status step's row-3 write (`Poller::record_cancelling`); the CHECK admits a request on
`cancelling`, so that clear is pinned by a test, not by the schema. `record_shown`'s `queued` → `running` is not terminal and keeps them. The arm is
guarded on `jobs.status` existing: SQLite refuses an added CHECK over a missing column. Their writers
are the remote-cancel cores ([execution-backends.md](execution-backends.md#remote-cancel-record-deliver-undo-adr-024-o223o224)):
the cancel core records `cancel_requested_at` on a `queued`/`running` job only (a `cancelling` job's
cancel is already on the server, so it never holds a request — ADR-024 o22.12.1); a delivery marks `cancel_sent_at` before it spawns
`cancel.sh`, and clears both when the cancel is delivered (rc 0, or `.cancelled` already on the
server when it classifies first) or the job ended first — its write of `cancelling` clears both in
the same statement, as every write of `cancelling` must; Undo clears `cancel_requested_at` while the
send is not marked and the job is `queued`/`running`; the terminal writes above clear both. The
poller's status step reads the request with the row in its one locked read and, while it is set,
runs the delivery instead of the ordinary step (ADR-024 o22.13.1). `cancel_job` runs the cancel core
and `undo_cancel` the Undo core. The `Job` struct carries both columns (`Job::COLUMNS` 19–20, the TS
`Job` too, o22.14.5), so the list and the detail show a pending cancel; the cores read them under the
lock of the read that acts (`commands::jobs::pending_cancel_conn`). **Every write of either column
emits `job:status`** for the job once its lock is released (o22.14.5).

**The pending submit (schema v22, ADR-024 o23.5).** `jobs.submit_pending_since`, nullable TEXT in
UTC, laptop clock: when a submit the user started (Create & Run, Retry submit) was first refused **for
the slot** — `submit.sh` answered `blocked` (slot busy) or `slot-check-error`. One CHECK in the v21
ADD-COLUMN form: only on a remote `queued` job. It is set by the attempt's own write
(`ssh_backend::record_attempt`, `COALESCE(submit_pending_since, now)`: a later refusal keeps the first
time) and NULLed by every write that ends the wait: the enqueue, a refusal that ends it (the
re-check's `record_attempt`, or its own guarded write for a local refusal of the plan),
`record_delivered` (both branches) and `record_ended_first` (the two `queued`-keeping writes, pinned by
tests — the CHECK cannot catch them), and every terminal or `cancelling` write (`finalize_job_conn`,
`mark_cancelled_conn`, `record_cancelling`, `record_delivered`'s `cancelling` branch — the CHECK fails
one that keeps it). An Undo leaves it: the pending submit resumes. `Job` carries it
(`Job::COLUMNS` 21, the TS `Job` too); every write of it emits `job:status` after its lock. While it
is set the poller **re-checks** the row on every status step instead of labelling it, and submits it
once the slot frees ([execution-backends.md](execution-backends.md#the-pending-submit-pollerpendingrs-adr-024-o23)).
`retry_remote_submit` refuses a pending row ("a submit is pending; the slot is re-checked every
period", o23.8). The view keys on the **row's flag**, not the record (the record is empty after a
restart): the "Waiting for the slot since <time>" badge, the notice with its blockers once
`notified`, no Retry submit, the first Cancel offered, the interim `error_message` not shown as an
error ([frontend.md](frontend.md), o23.15).

## Transfers (`sync.rs`, ADR-024 o items 3.2, 3.3.4, 6)

- **Upload:** `rsync -a --checksum --mkpath -e 'ssh <the four ssh_options>' <local>/
  <host>:<remote>/`. **Download:** `rsync -a --checksum <filter> -e '…' <host>:<remote>/ <local>/` —
  `--checksum` both ways, so a retry never trusts size+mtime over a wrong file (on the download, a
  local file corrupted with its size and mtime intact would otherwise fail the hash post-condition on
  every retry). Never `--partial`, never `--delete` (a test asserts it). The host is validated as for ssh, the remote dir by
  `is_valid_path`, the local dir must be absolute.
- **The download filter** comes from the shared artifact list (`crate::artifacts`), see
  [execution-backends.md](execution-backends.md#the-shared-artifact-list-src-taurisrcartifactsrs-adr-024-o-item-6).
- **Upload expected list:** `upload_expected(dir)` walks the local job dir and returns every file with its
  sha256, sorted; it refuses more than 1000 files (`MAX_UPLOAD_FILES`), a name outside the path rule's
  characters, or anything that is not a regular file. `expected_values` turns it into the submit call's
  two NUL values per file (name, sha256).
- **Download post-condition:** `compare_download(local, server)` — both lists of `FileDigest { name,
  digest }`, `.tsp-out/` left out on both sides — names the **missing**, **extra** and **differing**
  files. The local side is `list_dir(dir, download_selects)`, the filter-selected subset. A symlink is
  listed by its target (`Digest::Symlink`), since the `.submitting` claim is a dangling symlink with
  nothing to hash.
- **The server side** is the `LIST` call (values: the job dir, then `download_patterns(policy)` — the
  filter's leaf patterns, sent, so the list is never restated in shell). For every top-level entry
  except `.tsp-out` it reports the name and, if a pattern selects it, a regular file's sha256 (one
  `sha256sum` per file on stdin, so no name reaches its output), a symlink's **target** (`readlink`,
  never followed), `dir` or `other`; an unselected entry is `unselected` and not read. Nothing below
  the top level is read (the download's `--exclude=*` keeps rsync out of every directory but
  `.tsp-out/`). The reply:

  ```text
  orcastudio-list 1
  argc <n>, arg <len> × n     the values, verbatim
  entries <n>                 then n × (entry <len> + sha256 <hex> | link <len> | dir | other | unselected)
  end
  ```

  `parse_list_reply` returns the selected entries, sorted. It **re-derives the selection** (rule #9):
  every verdict must equal `download_selects(name, policy)`, in both directions; a selected `other`,
  a duplicate, a name with `/`, or `.tsp-out` is `SyncError::Listing`.

## `poll_log` over ssh (`poll.rs`, ADR-024 o item 7)

The values sent (NUL list): the job dir, the offset, the cap (`POLL_LOG_MAX_BYTES`, 256 KiB — a
compile-time assert keeps it plus a 64 KiB framing margin under the runner's 1 MiB output cap). The
reply, in the 5.2 record format:

```text
orcastudio-log 1
argc 3
arg <len>      × 3, each value verbatim
size <n>|-     output.out's size, or - when it does not exist
bytes <len>    then the raw bytes and \n
end
```

`parse_poll_reply` checks the echo, then the post-condition through `plan_log_read`: with a size,
`len == min(cap, size − offset)` when `size ≥ offset`, no bytes and a `reset` when `size < offset`;
with no file, no bytes and the offset unchanged. Anything else is `PollError::PostCondition`, never a
plausible chunk.

The `POLL_LOG` script reads the size first (`stat -c %s`; only the exact ENOENT message is "no file",
any other failure an `error` record), prints `size`, and — only when `size > offset` — takes `head -c
<size> | tail -c +<offset+1> | head -c <cap>` into a temp file whose length is the `bytes` record. A
shrunken file (`size < offset`) gets no bytes, which the parser reads as a reset. The first two stages
may end by SIGPIPE (141) when the last `head` stops early; any other status is an `error` record.

## Submit (`submit.rs`, ADR-024 o item 3)

`SubmitArgs::new(root, job_id, socket, mask, orca_path, files)` checks every value before anything
leaves the laptop (the derived job dir, the socket's path rule and ≤ 100-byte bound, the mask syntax,
an absolute ORCA path, files with a sha256) and takes the wrapper's sha256 from the embedded
`WRAPPER` (`wrapper_sha`). Values (6 + 2n): job dir, root, socket, mask, ORCA path, **the wrapper's
sha256** (never a path: the script builds `<root>/bin/wrapper-<sha>.sh` itself, so the enqueued argv
cannot point outside `<root>/bin`; ADR-024 o item 13.1), then name + sha256 per file. Upload names
carry the full 64-hex sha (`scripts::upload_path`, o item 13.2). The reply:

```text
orcastudio-submit 1
argc <n>
arg <len>                  × n, each value verbatim
refused <len>              exactly one outcome — nothing claimed: any other refusal, reason bytes
refused-kup <len>          nothing claimed: KillUserProcesses is not `b false`; the evidence
blocked <n>                nothing claimed: the slot check's n ≥ 1 blocker records (below)
slot-check-error <len>     nothing claimed: the slot could not be checked, why
lock-busy <len>            nothing claimed: the account lock's openers
upload-mismatch            nothing claimed: missing <n>, extra <n>, differing <n>, each + n `name <len>`
enqueued <id>              decimal tsp id (.enqueued published)
failed-after-claim <len>   reason bytes (.submitting stays)
end
```

`parse_submit_reply` → `SubmitReply::{Refused(Refusal), RefusedKup, Enqueued, FailedAfterClaim}`.
**The refusals a caller acts on are their own reply kinds** (ADR-024 o23.7), parsed into the typed
`Refusal`: `Blocked { blockers }`, `SlotCheckFailed(why)`, `LockBusy { openers }`, `UploadMismatch {
missing, extra, differing }`, and `Other(why)` for every other refusal — for people; no caller decides
on reason text. Each blocker record is `process <pid>` + `kind wrapper|pinned` + `cores <len>` + `job
<len>` (or `job -`: the cwd was not readable), or `row <id>` + `socket <len>` + `job <len>` + `mask
<len>` (a queued row of another own slot daemon) — every path, list and name a length-prefixed byte
record, never `%q` text, since a cwd or a file name can hold a newline. `Blocker::fallback()` is how
the pending notice shows a blocker that is no local job: "pid <pid> (cores <cores>)" or "tsp row <id>
on <socket>". A broken reply, a count that is not the records', `blocked 0`, an unknown blocker kind or
an `error` record is an error (an unreadable answer), never an outcome.

**`refused-kup` evidence** (o item 13.3) — the record's bytes are themselves records:

```text
rc <n>              busctl's exit status, decimal 0–255 (124: `timeout` ended it)
stdout <len>        its stdout, verbatim
stderr <len>        its first stderr line (at most 200 bytes)
```

so the reply reads `refused-kup <len>\n<evidence>\nend\n`. `SubmitReply::RefusedKup(KupEvidence { rc,
stdout, stderr })`. Evidence of rc 0 with stdout exactly `b false\n` — the one passing result — is a
protocol error (`SubmitError::Kup`), never the variant. **Only this variant** clears the profile's
`verified_at` (n item 7); no caller decides on a `refused` reason's text.

**The `SUBMIT` script** (one `bash -s` call; every refusal stops before any later step and wrote no
claim):
- **0** the values' form, before the lock: count 6 + 2n (n ≤ 1000), the root and socket by the path
  rule, the job dir exactly `<root>/jobs/<one component>`, socket ≤ 100 bytes, the mask
  `^[0-9]+([,-][0-9]+)*$`, an absolute ORCA path, the wrapper sha exactly 64 lowercase hex digits (a
  path or anything else refuses here), each file name by the path rule with a 64-hex sha256, no name
  twice;
- **1** `exec 9>"$HOME/.orcastudio-submit.lock"; flock -w 20 9` — one lock per account. A timeout
  is `lock-busy` with `<pid> <cmdline>; …` — every process (other than itself) that has the lock file
  open, from one `find /proc/[0-9]*/fd` (only own processes' fds are readable): the holder, any other
  waiting submit, a daemon that leaked fd 9. It does not say which one holds the lock (o item 13.4).
  With none found: `no process found (the holder may have exited)`;
- **2** `busctl … KillUserProcesses` exits 0 printing exactly the 8 bytes `b false\n` (content and
  size both checked: `read -d ''` alone stops at a NUL) — anything else (`b true`, other output, a
  NUL, rc ≠ 0, a `timeout` 124) is `refused-kup` with the evidence (n item 7, o item 13.3);
  `realpath -e <job> <job>/..` prints exactly `<job>` and `<root>/jobs` (a job dir reached through a
  symlink refuses); `<root>/bin/wrapper-<sha>.sh` is a regular file, not a symlink, whose `realpath`
  is itself (so `<root>/bin` is not a symlink either) and whose `sha256sum` is `<sha>` — an absent
  or mismatched wrapper is `refused`, never `failed-after-claim`;
- **3** no `.started`, `.enqueued`, `.exit_code`, `.cancelled`, `.submitting` in any form
  (`path_exists`, a dangling symlink counts), and — only if `/proc/net/unix` lists the slot socket —
  no `tsp -l` row of the slot holding the job dir as a whole token. The slot socket's own state
  unreadable — `/proc/net/unix` absent or unreadable, its header not the expected one, or the slot's
  `tsp -l` failed — is `slot-check-error` (l's socket `Error`, o23.1);
- **4** the upload post-condition: `find <job> -path <job>/.tsp-out -prune` lists every entry; the
  regular files must be exactly the expected names, and one `sha256sum` over them must give every
  expected hash. A difference is `upload-mismatch` naming `missing`, `extra` (an rsync temp, a
  non-file) and `differing` (a changed byte, or an expected name that is not a regular file); a
  failed listing or an unexpected `sha256sum` line stays a plain `refused`;
- **5** the slot check (o item 9), below: blockers → `blocked`; a scan that fails, times out or prints
  anything but its status line → `slot-check-error`. **A refusal at steps 3–5 writes nothing in the
  job dir** (no `.tsp-out`, no claim; o23.3) — it touches only the account lock file, the `mktemp -d`
  dir (removed by the EXIT trap) and `tsp -l` on listed sockets;
- **6** `mkdir -p <job>/.tsp-out`, which must then be a directory and not a symlink;
- **7** the claim `ln -sT x <job>/.submitting` — no-clobber; it fails if anything took the name
  since step 3;
- **8** `TMPDIR=<job>/.tsp-out TS_SOCKET=<sock> tsp bash <wrapper> <job> <mask> <orca>`; its stdout
  must be one decimal id (an optional final newline). Then `.enqueued` (`socket=…\nid=…\n`, temp
  file + `mv -fT`), then the post-condition: the socket is listed verbatim in `/proc/net/unix`. A
  failure from here on is `failed-after-claim` (the claim stays).

Every plain `refused` reason starts with its step (`values:`, `lock:`, `realpath:`, `wrapper:`,
`marker:`, `row:`, `upload:`, `tsp-out:`, `claim:`), for people reading it.

**The re-enqueue form** (`submit.sh reenqueue`, ADR-024 o25.3–4 as o25.14 settles them): the same
call with the literal value `reenqueue` **first** (`SubmitArgs::reenqueue()`, `SubmitMode::ReEnqueue`;
a job dir starts with `/`, so the forms cannot be confused) — 7 + 2n values, the same reply kinds, the
same steps under the same lock, except:
- **3** the markers are `.started`, `.exit_code`, `.cancelled` and the claim `.reenqueue-1`
  (`.enqueued` and `.submitting` are a submitted job's own); a row holding the job dir as a whole
  token refuses unless it is our wrapper's row for this job in state **`finished`** (a wrapper that
  crashed before `.started`, row 11) — any other state, or a row it cannot read as ours, refuses. The
  rows are read on the slot socket and, **only when it differs**, on `.enqueued`'s socket (an
  unparsable `.enqueued` refuses as a marker). Step 3 runs before step 4, so a `.started` is the
  marker refusal, never an upload mismatch (o25.14.1);
- **4** the file check leaves out `.submitting`, `.enqueued` and top-level `.reenqueue-*` (and
  `.tsp-out/`, as always);
- **7** the claim is `ln -sT x <job>/.reenqueue-1`, **after** the slot check and before the enqueue:
  a refused re-enqueue claims nothing, and a second one finds the claim and refuses. The claim is
  the server's re-enqueue counter (o25.3) — no DB column;
- **8** the enqueue is step 8's (the current mask, the current wrapper sha); `.enqueued` is replaced by
  `mv -fT`; `.submitting` and a stale `.tsp-out/` stay.

The two-wrapper guarantee rests on the wrapper's no-clobber `.started` (`ln -T`), not on the row
check alone: a second wrapper in one job dir cannot publish `.started` and exits.

**The lock fd and the bounds.** Every `tsp` call (`-l` too), every child under `timeout` and the
claim/publish commands run with `</dev/null` and `9>&-`, so no daemon tsp starts can inherit the
account lock (probe 5.3c); the head's short readers (`stat`, `cat`) still inherit it and exit at once.
Every child that can block while the lock is held runs under `timeout -k 1 N` (TERM after N s, KILL
1 s later): busctl 2 s, realpath 2, the wrapper's realpath 1, the wrapper's sha256 2, the slot's
`tsp -l` 2, the upload `find` 2, the upload `sha256sum` 6, the slot scan 8 (its own `tsp -l` calls
each under `timeout -k 1 2` inside it), the enqueue 3 — 28 s, and 37 s worst case with the nine
kill-afters, under the 40 s of o item 3.3.1. A re-enqueue whose `.enqueued` socket differs from the
slot's runs one more `tsp -l` (2 s) and gives the upload's `sha256sum` 5 s instead of 6
(`T_SHA_REENQUEUE`): 29 s + 10 kill-afters = **39 s**, still under 40 (o25.14.5); with the sockets
equal (every job today: one slot, the socket never changes) it is the submit's 37 s. A timeout (rc 124) of a bounded child **before the
claim** — realpath, the wrapper's realpath and sha256sum, `find`, the upload's `sha256sum`, the scan,
the slot's `tsp -l` — is `slot-check-error` (transient, o23.13.2); busctl's timeout stays
`refused-kup` (fail closed); after the claim a timeout is `failed-after-claim`. A child killed by the
kill-after exits 137 (not measured here): for realpath, the wrapper's realpath and sha256sum, `find`
and the upload's `sha256sum` that is an ordinary refusal; for the slot's `tsp -l` and the scan **any**
non-zero rc, 137 included, is `slot-check-error` (ADR-024 o23.14.6). The short file operations — `stat`, `cat`, `mkdir`, `ln`,
`mv` on the job dir and its markers — are deliberately **not** wrapped: they touch only the root,
which is local ext4, measured on uni (`findmnt` in the connection test, ADR-024 n item 8 allow-list
`{ext4}`; [uni-server.md](../infrastructure/uni-server.md), 2026-10-03), not a network mount that
could hang.

**The slot check** runs as one `bash -c` child under `timeout` (its functions passed by `declare
-f`, its values as arguments). It reads `/proc` with builtins and forks only `readlink` for a pinned
process and `tsp -l` for a qualifying socket:
- the full set is the scanning shell's own `Cpus_allowed_list` (unreadable → refuse);
- **candidates**, own uid only (`Uid:` of `/proc/<pid>/status` = `$UID`; a failed read is "gone"):
  every **wrapper** — argv element by element `bash`, `*/bin/wrapper-<hex>.sh` of any root, job dir,
  mask — with its argv[3] mask; and every process whose `Cpus_allowed_list` differs from the full set,
  with that list and its cwd (byte-exact; none for a zombie or an unreadable cwd);
- **queued work**: every socket in `/proc/net/unix` with the layout `<dir>/tsp/slot<N>.sock`, a valid
  path and `[[ -O ]]`, other than the slot's own, is read with `tsp -l`; each `queued` row whose
  command holds `…/bin/wrapper-<hex>.sh` contributes the mask two tokens after it. A failed `tsp -l`
  there refuses;
- **accounted for**: a candidate whose job dir (a wrapper's argv[2], a pinned process's cwd) is the
  job dir of a `running` row of the slot's own daemon; the slot's own queued rows never block. With no
  daemon on the slot nothing is accounted for;
- **blocks**: any other candidate or queued row whose CPUs meet the mask (an unreadable CPU list meets
  every mask: fail closed). The scan appends one blocker record per blocker to a file in the temp dir
  (the byte-exact channel to `main`, o23.13.8) and prints only `done <n>`; `main` then answers
  `blocked <n>` followed by that file verbatim.

**The `LABEL` call** (read-only; values: the recorded job dir and socket) reports the facts in the
order of o item 3.4:

```text
orcastudio-label 1
argc 2, arg <len> × 2          the values, verbatim
dir yes|no                     no → end; only ENOENT is "no"
started|exit_code|cancelled|enqueued|submitting yes|no     in this order, any form
net_unix <len>                 /proc/net/unix header + the lines naming the socket, then
  nodaemon | rows <n> + n × row <len> | tsp_error <len>
end
```

`tsp -l` runs only on a socket `/proc/net/unix` lists, under `timeout -k 1 3`: a hung or failed
client is `tsp_error`, so the job goes to the classifier as a socket Error. `parse_label_reply` → `LabelFacts` re-checks
`nodaemon`/`rows`/`tsp_error` against `unix_socket_listed` over the evidence, requires every row to
mention the job dir, and sets `row_holds_job` only for a **whole-token** match; `tsp_error` sets
`socket_error`. Any `error` record or a contradiction is an error, never facts.

**Labels** (o item 3.4) — `label(&LabelFacts) -> Label` over the read-only label call's facts (dir
exists, markers present, a row holding the job dir on the recorded socket, a socket `Error`), checked
in this order: no dir → `NotOnServer`; any of `.started`, `.exit_code`, `.cancelled`, `.enqueued`, or a
row → `Classifier`; a socket `Error` → `Classifier`; `.submitting` → `SubmitInterrupted`; otherwise →
`NotOnServer`.

## Readying the server, and running the uploaded scripts (`prepare.rs`, `run.rs`, ADR-024 o items 3.2, 13.1, 14)

Three scripts are uploaded to `<root>/bin/<name>-<sha256>.sh` (`prepare::UPLOADED`): the **wrapper**
(tsp runs it), **`cancel`** and **`collect`** (the trampoline runs them). Before an upload, a cancel's
delivery or a sweep, two calls make sure nothing is written through a symlink and that all three exist with the
right bytes. This section is the wire formats' spec of record (like the label call's, o item 13.5).

**The `PREPARE` call** — read-only; values: the root, the job dir (`<root>/jobs/<id>` exactly), and
the sha256 of the wrapper, cancel and collect scripts (5 values). For `<root>`, `<root>/jobs`, the job
dir, `<root>/bin`, `<root>/tsp` and the three scripts, in that order, it reports whether the path
exists (only ENOENT is `absent`; any other `stat` failure is an `error` record and exit 3), its kind
(`stat -c %F`, which does not follow a symlink) and its `realpath -e` (`-` when it does not resolve).
For each script it adds the sha256, computed only for a regular file (a fifo would block
`sha256sum`), under `timeout -k 1 5`. Last come the two **host facts** (ADR-024 o26.6): `head -5
/proc/meminfo` and `df --output=avail,target -B1 <root>` (never `-P` with `--output` — they conflict
on coreutils 9.4, [orca/memory.md](../orca/memory.md)), each under `timeout -k 1 3`, each its stdout
byte-exact or `-` when the read failed or timed out.

```text
orcastudio-prepare 1
argc 5, arg <len> × 5          the values, verbatim
<name> absent|present          root, jobs, job, bin, tsp, wrapper, cancel, collect — in this order
  kind <len>                   ┐ only for present
  realpath <len>|-             │
  sha256 <hex>|-               ┘ scripts only: a regular file's sha256, else -
meminfo <len>|-                head -5 /proc/meminfo, or - (a failed read)
df <len>|-                     df --output=avail,target -B1 <root>, or - (a failed read)
end
```

**The host facts are recorded, never gating.** The prepare call also readies the server for a
cancel's delivery, a sweep, `Lost`, the re-enqueue, the pending re-check and `collect_readying`, none
of which may break on a df failure. So a failed read is a `-` fact — the shell's `fact` helper never
reaches `die`, never changes the exit status — and `parse_prepare_reply` accepts `-`, a payload in any
other format (`None`) and the records' **absence** alike; only a broken frame (a wrong length, the two
out of order, a duplicate, an unknown record) is an error. The facts land in `PrepareFacts::host`
(`memory::SubmitHostFacts { mem_total_kib, mem_available_kib, root_avail_bytes }`, KiB / KiB / bytes)
through the strict parsers of `crate::memory`; `check_prepare` never looks at them, so `ready()` never
depends on a df. The header stays `orcastudio-prepare 1` (stdin-fed, the records additive; o26.14.5).
`ssh_backend::ready_server` returns the facts of its **last** successful prepare; only
the submit the user started reads them (its `memory_notice`,
[execution-backends.md](execution-backends.md#the-memory-and-disk-preflight-adr-024-o26)).

`parse_prepare_reply` → `PrepareFacts` re-checks what it can (rule #9): a sha256 appears exactly for a
regular file (`regular file` or `regular empty file`), and no path exists below a component reported
absent (`jobs`/`bin`/`tsp` under `root`, the job dir under `jobs`, a script under `bin`).
`check_prepare` decides:
- every component that **exists** — `<root>/tsp` included (o 14.3) — must have kind `directory` and a
  realpath equal to its own path; the first that does not is a `ShapeRefusal` naming it (a symlinked
  `<root>/bin` refuses even when the scripts inside it have the right bytes, o item 13.1). A component
  that does not exist is fine: rsync `--mkpath` and the install call create them;
- a script **hashes right** iff it is a regular file whose realpath is its own path and whose sha256 is
  the one its name carries. Anything else (absent, other bytes, a symlink) is not a refusal: the
  install call replaces it. `Prepared::missing()` names those that do not.
- `Prepared::ready()` = all three hash right and `bin/` and `tsp/` exist.

**The `INSTALL` call** — values: the root, then for the wrapper, cancel and collect in that order its
sha256 and its bytes (7 values; the bytes are echoed too, n item 6d). `mkdir -p <root>/bin <root>/tsp`
— `tsp/` holds the slot sockets and nothing else creates it; what real tsp does when the socket's
directory is missing is **not measured** (rule #10; probe 5.2b had the parent present; the B4 live run
records it) — then the root, `bin/` and `tsp/` must each be a directory, not a symlink, equal to its
realpath. Per script: one already a regular file with the right sha256 is left as it is (`kept`);
otherwise the bytes go to a unique `mktemp` name `.<name>-<sha>.XXXXXX` in `<root>/bin/` (two profiles
aliasing one host cannot clobber each other), the temp's sha256 must be `<sha>`, and `mv -fT` renames
it over the name (`installed`) — a running script keeps its own inode (probe P3); a temp is removed
on any failure.

```text
orcastudio-install 1
argc 7, arg <len> × 7          the values, verbatim
wrapper installed|kept
cancel installed|kept
collect installed|kept
end
```

Any failure is an `error` record and exit 3. The answer is not the post-condition: the caller runs the
prepare call again and requires `ready()`.

**The `RUN` trampoline** (o item 14.1) — the one way an uploaded `cancel.sh`/`collect.sh` runs, so no
per-job value is re-parsed by the remote login shell (`ssh host cmd args` joins and re-parses argv,
(l)): it is fed through `bash -s` with the NUL list `<root> <name> <sha> <args…>` and hands the args on
as argv. In order:
1. `<root>` by the one path rule; `<name>` in the **closed allow-list** `BUDGET` — exactly `cancel`
   and `collect` (never `wrapper`: run here it would start ORCA outside tsp); `<sha>` exactly 64
   lowercase hex. Otherwise `refused`.
2. `<root>/bin/<name>-<sha>.sh`, built by the script: absent (ENOENT) → `not-installed`; otherwise it
   must be a `regular file` (not a symlink) whose `realpath` (under `timeout -k 1 5`) is itself — so
   `<root>/bin` is not a symlink either — and whose `sha256sum` (same bound) is `<sha>`, else `refused`.
3. `timeout -k 1 <N> bash <path> <args…> </dev/null` — **N per script: cancel 20 s** (its sweep waits
   up to 5 s after the TERM, plus `tsp -l`/`-r`), **collect 15 s** (`/proc` reads and a few `tsp -l`);
   the script's stdin is at EOF; stdout and stderr go to temp files, each capped at 400 000 bytes (two
   of them plus the echo stay under the laptop's 1 MiB `MAX_OUTPUT_BYTES`; over the cap → an `error`
   record).
4. The reply carries the script's rc and both streams; the trampoline exits 0 whenever the reply is
   complete, whatever the script's rc.

```text
orcastudio-run 1
argc <n>, arg <len> × n        the values, verbatim
refused <len> | not-installed | ran
rc <n>                         ┐
stdout <len>                   │ only after `ran`: the script's exit status (124: timed out)
stderr <len>                   ┘ and its streams, verbatim
end
```

`run::RunArgs::new(root, JobScript::{Cancel, Collect}, args)` takes the sha from the embedded bytes;
`parse_run_reply` → `RunReply::{Refused, NotInstalled, Ran { rc, stdout, stderr }}`. The collector's
snapshot is the `stdout` payload, unwrapped before `wire::parse_snapshot`. A test-only variant of the
trampoline (its allow-list extended by one `probe` entry, in the test only; a test pins the shipped
line `declare -A BUDGET=([cancel]=20 [collect]=15)`) shows an argument with `'`, `$`, a space and a
newline arriving byte-identical and a non-zero rc in a complete reply. Stdin at EOF is an
**observation, not a control**: the `bash -s` read loop drains stdin before the script runs, so
removing `</dev/null` cannot turn a test red; `</dev/null` stays as defence in depth against a future
framing change (ADR-024 o 14.1).

**Laptop-side bounds** (`ssh_backend`), derived from the scripts' own bounds with margin, never
below them: `CALL_TIMEOUT` 60 s for prepare, install, label and mkjob (ssh's 10 s connect + at worst
the install's six `sha256sum`s at `timeout -k 1 5`, 36 s = 46 s; prepare's three `sha256sum`s, 18 s,
and its two host-fact reads at `timeout -k 1 3`, 8 s, = 36 s); `RUN_TIMEOUT` 60 s for the
trampoline (10 s connect + `realpath` 6 s + `sha256sum` 6 s + cancel's 20 s budget + 1 s kill-after =
43 s); `SUBMIT_TIMEOUT` 60 s (o 3.3.1).

**The `MKJOB` call** (o items 2, 14.1) — values: the root, the job dir. `mkdir -p <job dir>`, then,
**after** the mkdir, the realpaths of the job dir and of its parent as facts:

```text
orcastudio-mkjob 1
argc 2, arg <len> × 2          the values, verbatim
job <len>                      realpath -e <job>
parent <len>                   realpath -e <job>/..
end
```

`parse_mkjob_reply` requires exactly `<job>` and `<root>/jobs` (o item 1), so a cancel of a job not
on the server never publishes `.cancelled` through a symlinked component.

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
as `<root>/bin/<name>-<sha256>.sh` (content-addressed; `sha256_hex` gives the sha). The same head
also fronts the profile connection test (`CONNTEST`), which is not uploaded but fed through
`bash -s` stdin, and which reuses the collector's record `wire::Reader` for its output
([server-profiles.md](server-profiles.md)). That is why the head holds only definitions and
stdin-free commands.

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
- **4** the **session wait** (ADR-024 o24.7, below);
- **5** publish `.exit_code` = ORCA's status (TERM ignored for the publish itself, default after
  it), exit with it.

While ORCA runs there is no trap: a TERM to the wrapper's group kills the wrapper (default
disposition, measured) before it can write `.exit_code`, so a job cancelled while ORCA runs has none.

**The session wait.** A killed ORCA driver leaves its MPI ranks, `mpirun` and a shell running in the
session, pinned to the slot's cores and still writing `output.out` (B4 M7), and tsp starts the next
job as soon as the wrapper exits. So `.exit_code` is published only once the wrapper's session is
empty:
- **Guard first.** Only if the wrapper leads its session and group (`$$` = the SID = the PGID that
  `.started` recorded) does it list or signal anything; otherwise it publishes at once.
- **Members** = one snapshot `ps -o pid=,ppid=,stat=,comm= -s <SID>`, minus the wrapper, its own
  descendants within that snapshot (its `ps`, `sleep`s, subshells) and zombies. Every other member
  counts, whatever its cwd. The listing must contain the wrapper itself; any `ps` failure or an
  unparsable line publishes at once with a note.
- **Sequence.** None left → publish. Otherwise wait up to **10 s** for them to end on their own
  (re-listing every 0.2 s), then re-list and TERM+CONT each member **by PID**, wait up to 5 s,
  re-list and KILL each, wait up to 1 s, and publish — a member still listed then (D state) is named
  and does not block the publish. Bound ≈ 16 s after ORCA's return (10 + 5 + 1: the 1 s after KILL
  keeps a member that is still exiting from being named a false survivor; ADR-024 o24.14).
  `ORCASTUDIO_TEST_SESSION_GRACE` shortens the 10 s for the tests: an integer 0–9 is used, anything
  else (absent, 10 or more, not an integer) means 10; production never sets it. The normal publish runs under `trap '' TERM`, so it cannot be doubled by the trap.
- **TERM during the wait** (a trap installed right after `rc=$?`, cleared at the normal publish):
  single-shot (`trap '' TERM` first — a cancel sends TERM twice), prints its one line, publishes
  ORCA's rc — so a late cancel keeps a clean result, row 2 `Completed { late_cancel }` — and then
  holds the tsp slot by the **same bounded sequence** on what remains (grace, TERM+CONT, 5 s, KILL,
  1 s), silently, and exits with ORCA's rc: at most ≈ 16 s after the TERM. A cancel ends it sooner
  (`cancel.sh` KILLs what remains after its own 5 s, the wrapper included). Any TERM in the wait now
  publishes ORCA's rc (rows 5/6, not row 8).
- **Lines** on stdout (tsp's `.tsp-out/`, fetched with the job), every one **before** `.exit_code`:

  ```
  wrapper: session empty at ORCA's return                       (the healthy path: the only line)
  wrapper: ORCA returned <rc>; session members left: <pid> <stat> <comm>, …
  wrapper: waiting up to <s> s for them to end
  wrapper: session empty after <s> s
  wrapper: TERM+CONT <pid> <stat> <comm>, …
  wrapper: session empty <s> s after TERM+CONT
  wrapper: KILL <pid> <stat> <comm>, …
  wrapper: session empty <s> s after KILL
  wrapper: session empty before TERM+CONT|KILL
  wrapper: survivors after KILL, publishing anyway: <pid> <stat> <comm>, …
  wrapper: session wait skipped: the wrapper (pid <p>) does not lead its session (sid <s>, pgid <g>); publishing now
  wrapper: session wait skipped: ps -s <sid> failed: rc=<n> <message>; publishing now
  wrapper: ps -s <sid> …; publishing now                         (a ps error later in the sequence)
  wrapper: TERM during the session wait: publishing ORCA's rc <rc>, holding the slot until the session is empty (at most <s> s more)
  ```

  The `<s>` of "empty after" counts the 0.2 s sleeps, not wall time.
- **Old wrappers.** A changed wrapper has a new sha and a new `bin/wrapper-<sha>.sh`, installed by the
  next prepare; a job already queued keeps the old path and runs the old behaviour. No code deletes
  an old wrapper file.

**`cancel.sh cancel|check|sweep <job_dir> <root>`** — starts with `cd /` and never enters the job
dir; any other mode is a usage error (exit 2).
`cancel`: publish `.cancelled` (failure → exit 3, nothing signalled) → **queued path**: read
`.enqueued`; only if `/proc/net/unix` lists its socket exactly, run `tsp -l`; `tsp -r <id>` only if
exactly one row has that id, state `queued` and the job dir as a whole token → **running path**,
only for a parsing `.started` from the current boot and never for its own session or process group:
TERM `-<pgid>` iff the wrapper is alive, ours, and leads its group (live pgrp = `.started` pgid =
pid); then, unless the SID is reused, the sweep iff (alive and ours) or the job session is
non-empty — TERM+CONT each job-session PID, re-evaluate up to 50 × 0.1 s, KILL what is left → at
the end of every cancel that completes (queued, running, nothing found; a cancel that hits a read
error fails closed with exit 3 first), remove `<job>/.tmp` unconditionally — after the sweep, so a
process that writes into `TMPDIR` while it dies cannot leave it behind (a wrapper starting later
sees `.cancelled` before it creates `.tmp`). The cwd comparison is
byte-exact (`read -r -d ''`, no trailing-newline stripping), like Rust's. Prints one line per
decision (`queued: …`, `group: …`, `sweep: …`, `tmp: …`) and `done`. A fact
it cannot read stops it with `error …` and exit 3 before any further signal. No `tsp -k`, no kill
by tsp id or by name. `check` prints `started=`, `boot=`, `alive=`, `ours=`, `leader=`,
`sid_reused=`, `own_session=`, `session=` from the same functions and writes or signals nothing — the
shell side of the parity tests. The race window between `tsp -l` and `tsp -r` (a row starting) is
accepted: `.cancelled` already stops the wrapper.

`sweep` (ADR-024 o25.2) kills the orphans of a job the classifier read as row 8 `Lost` in this boot.
It **re-checks row 8 first** (rule #9 at the actor), from the same `assess`: no parsable `.started`,
or `.exit_code` or `.cancelled` there → `sweep: skip not row 8: …`, **exit 5**; the wrapper alive and
ours in this boot (row 7) → `sweep: skip wrapper alive`, **exit 4** — both signal nothing and keep
`.tmp`. Otherwise it runs the running cancel's sweep (`sweep_session`, shared with `cancel`: the
SID-reuse guard, own-session exclusion, the cwd filter, TERM+CONT → up to 5 s → KILL) — with no group
TERM (the wrapper is not alive and ours) — and removes `.tmp`. With a `.started` from another boot
there is nothing of ours left to signal (start times count from boot): it only removes `.tmp`. It
**never publishes `.cancelled`** and never runs `tsp`. Its budget is the `cancel` script's (`run.sh`
`BUDGET[cancel]` = 20 s).

**`collect.sh <job_dir> [<socket>...]`** — starts with `cd /`. A missing job dir is an error (it
would otherwise read as NeverStarted). It reads `/proc` only for a `.started` that parses and is from
this boot, reports whether the re-enqueue claim `.reenqueue-1` exists (`path_exists`, so the
dangling symlink counts; read **before** the sockets, ADR-024 o25.3), runs `tsp -l` only on a socket
`/proc/net/unix` lists, adds the `.enqueued` socket after the slot sockets (an unparsable `.enqueued`
becomes an `Error` fact with the marker's path), and on any read error other than ENOENT/ESRCH prints
an `error` record and exits 3.

## The snapshot wire format

`collect.sh` prints records in a fixed order. A record is a line `<name>` or `<name> <arg>`; a
**byte record** has a decimal length (no leading zeros) or `-` (absent) as its arg, and a present one
is followed by exactly that many raw bytes and `\n` (a cmdline holds NULs, so nothing is
line-delimited).

```text
orcastudio-snapshot 2
boot_id <len>               required
started <len>|-             first read
proc collected|skipped      collected iff .started parses and is from this boot
  wrapper_stat <len>|-      ┐
  wrapper_cmdline <len>|-   │ only when collected
  sid_stat <len>|-          │
  members <n>               │ then n × (member <pid>, cwd <len>|-)
reenqueue_claim yes|no      <job>/.reenqueue-1 exists, in any form (ADR-024 o25.3)
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

**Version 2** (ADR-024 o25.3) added the `reenqueue_claim` record; a version-1 output (no claim
record) fails the header check (`Malformed`), never "no claim" — a belt, since the trampoline only
ever runs the app's own `collect-<sha>.sh`. `Snapshot::reenqueue_claims` is 1 for `yes`, 0 for `no`:
the only claim the app makes is `.reenqueue-1` (the re-enqueue refuses on it), so this is the count.
The claim precedes the enqueue under the account lock, and the collector does not take that lock: a
collect whose claim read lands between a still-running re-enqueue's claim and its enqueue (possible
only when the previous step's reply was lost and the server's call still runs) reads no row and the
claim. So the poller **confirms** a never-start with the claim before it is terminal (ADR-024
o25.15, below): two consecutive passes at least 45 s apart, longer than any `submit.sh` call can hold
the lock.

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
   `Attempt::Retake`. A retake whose reads still differ is `Indeterminate { why: "the .started read
   disagreed twice" }` (`STARTED_DISAGREED`). So at most one retake, and the retake decision stays
   inside the pure function.
3. **The precedence table**, ADR-024 l, rows 1–11 in order, first match wins:
   corrupt `.started` → `Failed`; cancelled + clean finish → `Completed { late_cancel: true }`;
   cancelled + (alive and ours, or non-empty job session), this boot → `Cancelling`; cancelled →
   `Cancelled`; clean finish → `Completed`; any `.exit_code` → `Failed`; alive and ours, this boot →
   `Running`; started → `Lost { orphans, stale_boot }` (job session, this boot only); a
   queued/running row → `Queued`; any socket `Error` (or an unreadable row of ours) →
   `Indeterminate { why }`; else `ReEnqueue` when `reenqueue_count` = 0, `Failed` ("wrapper never
   started") otherwise. `NoDaemon` counts as no rows. **`reenqueue_count` is the snapshot's own
   `reenqueue_claims`** (`ssh_backend::collect_and_classify`, the Lab's `classify` likewise; ADR-024
   o25.3), never a constant.

`Outcome` is the classifier's own enum. These payloads are computed from the snapshot so later
steps decide from the outcome alone (ADR-024 o22.4.5, o22.5, o22.6, o25.1, o25.6):
- `Lost { orphans, stale_boot }` — the job-session PIDs still in the job dir (this boot only), and
  whether `.started` was written in another boot (the server restarted under the job) or not (the
  wrapper died in this boot);
- `Indeterminate { why }` — row 10's socket errors (`"<socket>: <error>"`, joined by `; `) or the
  retake's `STARTED_DISAGREED`;
- `Cancelling { session }` — the job-session size (the cwd-filtered `ps -s` members after the
  SID-reuse guard; 0 when only the wrapper's own liveness holds row 3), for row 3's progress rule;
- `Cancelled { started }` — the snapshot holds a `.started`: the job ran, and its files are evidence;
- `FailReason::CorruptStarted { detail, cancelled }` — the snapshot also holds `.cancelled`; the text
  then adds that processes of the job may survive on the server.

Serialized (serde `tag = "outcome"` / `"reason"`) as `{ "outcome": "cancelling", "session": n }`,
`{ "outcome": "cancelled", "started": b }`, `{ "reason": "corrupt_started", "detail": …,
"cancelled": b }`, `{ "outcome": "lost", "orphans": [pid, …], "stale_boot": b }` and `{ "outcome":
"indeterminate", "why": "…" }`; `src/types.ts` mirrors all five (`lost` with `orphans` and
`stale_boot`, `indeterminate` with `why`). `Cancelling` and `Lost` are also `JobStatus` values (above).

## `Lost`, the re-enqueue and the `Indeterminate` count in the poller (ADR-024 o25, unit 5.4b)

Every classifier verdict now has its action in the poller's ordinary status step
(`crate::poller`; the cancel paths are unchanged: a delivery still "goes on" from `Lost` and
`ReEnqueue`, and a delivered `.cancelled` makes the job `cancelled`, never `lost` — d′).

- **`Lost`** (`poller/lost.rs`, o25.2). Orphans first: while the job session is non-empty the step
  readies the server, runs `cancel.sh sweep` (`ssh_backend::sweep_call`), then collects and
  classifies again. An empty session → the evidence path in the same step. Orphans left → counted by
  o22.13.4's rule over the `Lost` job's own counter and baseline (`PollerMemory::count_lost_sweep`,
  seeded from the first observed orphan count; a `cancelling` job's count is separate, so a Cancel
  starts fresh); the result is `StatusStep::LostSwept` and the row says which sweep this was. After
  `MAX_SWEEPS_WITHOUT_PROGRESS` (3) the job is written `lost` anyway, naming the survivors and saying
  they may have written after the files were listed. A sweep exit 4/5 ("wrapper alive" / "not row
  8") signals nothing and is **not counted**: the next step classifies again. A ready with no answer
  is `CheckFailed`, counting nothing. **Evidence** (`poller/evidence.rs`, `Evidence::Lost`): the
  fetching flag, rsync down + the LIST check (never `.exit_code`), then the terminal write through
  `finalize_job_conn` — "Lost: the server restarted while the job ran; …" (stale boot) or "Lost: the
  job's wrapper died without an exit code; …" — then the drain. A failed fetch counts the strikes; the
  3rd writes `lost` with "evidence not downloaded: <why>; it stays on the server". A cancel recorded
  during the fetch (after the step's read) is dropped by the terminal write, which NULLs its columns
  and appends "A pending cancel was dropped: the job had already stopped." (`DROPPED_CANCEL_NOTE`).
- **Row 1 without `.cancelled`** (o25.8) takes the same evidence path and ends `failed`, saying ORCA
  is unlikely to have run and nothing can be swept.
- **The second never-start** `Failed { WrapperNeverStarted }` (the claim present, o25.5) is
  **confirmed first** (o25.15, `Poller::never_started_again`): the first pass of a run, and any
  consecutive pass sooner than `NEVER_STARTED_CONFIRM` (45 s, by the poller's injected `Clock`) after
  it, is `Shown` with a neutral row text (`NEVER_STARTED_UNCONFIRMED`) and the row stays `queued`; a
  consecutive pass at least 45 s after the first takes the evidence path and ends `failed`. The 45 s
  separate the two **server reads**: the first pass is remembered when its collect **ended**, and a
  confirming pass counts only if its collect **started** (the step's clock before
  `collect_readying`) at least 45 s after that — "the job
  never started on the server after one re-enqueue (or the re-enqueue did not reach the queue); see
  .tsp-out" (`.tsp-out/` and the markers, `.reenqueue-1` included, come down with it; `sync::MARKERS`
  lists the claim). The run (`JobMemory::never_started_at`) is per launch; any other verdict ends it,
  a step that decided nothing keeps it.
- **`ReEnqueue`** (`poller/reenqueue.rs`, o25.4, o25.14): **the plan is decided locally, without
  writing** (`ssh_backend::reenqueue_plan`): the profile must be a run target — if not, the job
  **waits** with no call and is not counted ("re-test the server profile to continue", o25.14.3); the
  input is derived in memory with the profile's **current** mask (`local_backend::job_dir_files`, the
  pure half of `prepare_job_dir`) and compared, digest for digest, with the local job dir — a
  difference fails the job at once with "Never started on the server, and the core count of the
  profile changed since it was submitted (the input's %pal no longer matches); start it again as a new
  job." (any other difference is named as it is), **no call**, the local dir untouched; a mask edit
  that keeps the CPU count re-enqueues on the new cores (o25.14.2). Then `ready_server`, then one
  `submit.sh reenqueue` carrying the local list. Its outcome (`reenqueue::decide`): enqueued →
  `StatusStep::ReEnqueued { tsp_id }`, the row stays `queued`; slot busy, slot check error, lock busy
  → `StatusStep::ReEnqueueWaiting` (nothing claimed; counted toward the notice only, at
  `REENQUEUE_NOTICE_AT` = 3); a reply that never came (the call's own contact `Failed`) →
  `CheckFailed`, not counted; any other answered refusal — an upload mismatch (the server's copy, not
  "profile changed"), `refused-kup` (which clears the profile's stamp, so the next step waits), a
  marker or a live row, an install refusal, failed after the claim, an unreadable reply →
  `StatusStep::ReEnqueueRefused { refusals, why }`, and the `MAX_REENQUEUE_REFUSALS`-th (3rd) fails
  the job with that refusal's text. The 5.4c pending flag is never set.
- **The runs a later step ends** (`poller::step_effect`, one exhaustive `match`; applied to
  `PollerMemory::note_step` before the record reads it). The `Indeterminate` run counts a step whose
  verdict is `Indeterminate` (an ordinary step's, a delivered or stopped cancel's), is reset by a
  decided verdict and kept by a step that decided nothing (no answer, a label, a pending submit's
  re-check). The re-enqueue's waiting state and counted refusals end on its success and on a decided
  verdict other than `ReEnqueue` — which is how "a refusal the next classify re-decides is not
  counted" is kept (o25.14.4) — and are kept across a failed check, a cancel not sent and a stopped
  delivery. The never-start confirmation (o25.15) is kept by the never-start again or a step that
  decided nothing, and ended by any other verdict.
- **Ready on a not-installed collect** (o25.7): the ordinary step's collect
  (`ssh_backend::collect_readying`) readies the server once and collects again when the collector
  answers `not-installed` (an app upgrade changed its sha, as 5.4b's own change does); a second
  `not-installed`, or a ready that fails, is the step's `CheckFailed`. `run_call`'s `RunError` tells
  the two apart.
- **Texts.** No row text defers to a later unit any more; the Rust text guard
  (`text_guard_tests.rs`) forbids "unit 5.4b" in user-facing strings.

**The record's new fields and steps (wire shapes for the view; `src/types.ts` mirrors them):**

```text
StatusStep  { "step": "lost_swept", "detail": { "outcome": <Outcome>|null, "error": { "text", "answered" }|null } }
            { "step": "re_enqueued", "detail": { "tsp_id": n } }
            { "step": "re_enqueue_waiting", "detail": { "kind": "slot", "detail": <Wait> }
                                                    | { "kind": "unverified", "detail": "<why>" } }
            { "step": "re_enqueue_refused", "detail": { "refusals": n, "why": "…" } }
RemoteState + "undecided": { "passes": n, "notice": "…" } | null          (from the 3rd pass)
            + "re_enqueue_waiting": { "refusals": n, "notified": b, "notice": "…"|null,
                                      "last": <ReEnqueueWait> } | null
```

**Named residuals** (ADR-024 o25.15.7):
- a stray `.started.tmp.<pid>` — a wrapper killed between writing its temp `.started` and linking it
  — is an extra file to the re-enqueue's file check: `upload-mismatch`, a counted refusal, so the job
  ends `failed` after 3 (its text names the file);
- a sweep's exit 4/5 is never counted, so a job whose shell predicates (`cancel.sh`) and Rust's
  (`classify`) disagreed on "alive and ours" would be swept, and re-classified `Lost`, forever. Their
  agreement is what the parity tests pin, through `cancel.sh check` against the Rust predicates on
  real processes (fixtures (a)–(e)).

`known` is `Shown(re_enqueue)` while a re-enqueue waits or is refused (the waiting view's gate,
o25.14.6), `None` after `re_enqueued`, `Shown(lost)` after a `lost_swept` that classified. The
terminal steps are the existing `finalised` (`status` `lost` or `failed`, `outcome` the verdict) and
`fetch_failed`.

## Tests

298 tests in the module (`cargo test --lib -- --list remote::`, 2026-11-18).
- **Pure (classifier, parsers, wire):** strict-parser garbage cases, the recorded probe fixtures (P2
  cmdline, P4 `tsp -l`, 5.2b `/proc/net/unix` line, 5.2c stat lines including `w q) x.sh` and the
  zombie), at least one snapshot per table row, the d′ race model over all 6 interleavings, and the
  wire parser's malformed/inconsistent cases. The probe page records stat and `/proc/net/unix` lines
  with `…` elisions; the fixtures keep every recorded field verbatim and fill the elided ones from a
  laptop read, marked in comments.
- **Real scripts (`script_tests.rs`)**, on this machine: a stub `tsp` on `PATH` (logs its argv;
  `-l` prints the P4 header and rows, `-r` succeeds) and a stub ORCA (records cwd, `TMPDIR`, env,
  args and affinity; can sleep, fail, trap TERM and write into `TMPDIR` ~0.5 s later (recreating it
  if gone), or start extra session members: an escaped "rank" with its own
  PGID, a foreign-cwd member, a zombie member, a non-dumpable member whose cwd is EACCES). Wrappers
  start in their own session like under tsp; a perl `IO::Socket::UNIX` listener stands in for a
  live daemon's socket, and a killed one leaves a stale socket file. Covered: the wrapper's happy path
  (inotify sees `.started` as one `IN_CREATE` and `.exit_code` as one `IN_MOVED_TO`, nothing else),
  cd failure, bad arguments, `.cancelled` preset, forced self-check failure (97, via an `ln` on `PATH`
  that publishes an empty `.started`), an existing `.started` (refused, both markers byte-identical), a dangling-symlink or directory
  `.started` (refused with 1, never 97: left as it was, no `.exit_code`, no ORCA),
  two wrappers launched together (exactly one runs ORCA), `.tmp` not creatable (96), ORCA's non-zero
  code, a temp-write failure next to an existing `.started` (refused, never 97), bad core masks
  (`-p`, `0-3x`, … → exit 2, nothing written); one path rule (shell `valid_path` = Rust
  `is_valid_path` on every case); shell-vs-Rust parity on fixtures (a)–(e) and on which `.started`
  files are corrupt (duplicate/unknown/missing key, NUL, CRLF, …); a byte-exact cwd compare (a
  `<job>\n` twin dir is foreign); the cancel paths (group TERM + escaped member swept, foreign cwd
  untouched, reused SID, own session, a `.started` from another boot, a live leader that is not our
  wrapper, our wrapper that does not lead its group, verified-queued `tsp -r`, no daemon → no tsp
  call, `.tmp` removed for a never-started job and removed after the sweep when a dying ORCA writes
  into it late); both d′ orders; the collector round trip through
  `classify` (Running, Completed, Cancelled, Queued, ReEnqueue, Indeterminate, `Lost` for a
  `.started` from another boot with `proc skipped`) and its read errors; a cancelling job whose
  `Cancelling { session }` equals the shell's own job-session count. **Cancel idempotence** (ADR-024
  o22.4.6, the premise of re-sending a cancel): a second `cancel.sh cancel` on a dir that already
  holds `.cancelled` — queued and removed, started and killed, never started, and a late cancel
  (row 2) — gives rc 0, keeps `.cancelled`, leaves no `.tmp`, changes nothing else in the dir (every
  file's bytes, recursively), runs no `tsp -r`, does not signal a same-session member whose cwd is
  elsewhere (the started fixtures; the witness a cwd-unfiltered sweep would reach), and the
  classifier's verdict on a fresh collect is unchanged. Two clauses are weaker than they read: the
  **bystander** process each test also checks sits in its own session with its cwd outside every job
  dir, so no path of `cancel.sh` could signal it — it witnesses nothing; and in the positive fixtures
  `.tmp` is already absent before the second run (the first cancel removed it, or the job never
  started), so "no `.tmp`" cannot fail there — only the kept-`.tmp` control below, which plants a
  `.tmp` first, shows that clause can fail. Two
  concurrent runs (a running job ×3, a queued job) both end rc 0 with one verdict and no temp marker
  left; a missing or non-directory job dir is rc 3 and nothing is created. Five negative controls run
  the same check on a copy of `cancel.sh` broken in one place (a toggling publish, a no-clobber
  publish, an unverified `tsp -r`, a sweep without the cwd filter, a cancel that keeps `.tmp`) and
  require the named clause to fail. Each test signals only processes it started, by recorded PID and start
  time (a wrapper that is not the test's own child is tracked from its `.started`), and cleans up in
  `Drop`: SIGSTOP to every live process it knows of, then SIGKILL, then a bounded retry of the dir
  removal, so a tracked wrapper is stopped before its ORCA is killed. Two cancel tests also
  assert, after the drop, that no process has a cwd in or an argument under the lab root.

- **The session wait (`script_tests.rs`, ADR-024 o24.10)**, with the wrapper's stdout captured in
  `<job>/wrapper.out` and three more stub extras: `writer` (own group, keeps writing to
  `output.out` for ~2 s after the stub exits, ending with `writer done`), `forever` (own group,
  ignores TERM, never stops) and `grouped` (in the wrapper's own group, never stops). Covered: the
  healthy path publishes at once with exactly the "session empty" line; a writer left after a 143
  exit — `.exit_code` = 143 only after its last write, writer gone; `forever` TERMed then KILLed
  within 1 + 5 + 1 s; every line before `.exit_code` (inotify order in the job dir); a member of
  the wrapper's own group is signalled by its PID and the wrapper, never signalled, publishes
  normally; a per-PID TERM in the wait runs the trap within 1 s; a stray per-PID TERM with a
  `forever` member ends within the bound (the handler KILLs it, no line after the publish); two
  TERMs give one trap line and one `.exit_code`; a group TERM in the wait (with `.cancelled`)
  publishes rc 0 → row 2; a group TERM while ORCA runs publishes nothing; the real `cancel.sh
  cancel` against a wrapper in its wait (rc 0, `.cancelled`, one trap line, row 2); the guard —
  a wrapper run without setsid in a test-made session, and one that leads its own group but not
  its session, each publish at once and a sentinel `sleep` there survives; the grace override 60,
  `2.5` and `-1` all mean 10; and, through a `ps` stub on `PATH`, a `ps` error (publish at once,
  note), a listing without the wrapper's own line (publish at once, note — rule #9), a zombie-only
  session (no wait) and a member still listed after KILL (named, publish within the bound). Each
  has a negative control that mutates `wrapper.sh` in one place (no wait, no kill, a print after
  the publish, no descendant exclusion, one long sleep, no `trap ''`, no trap, a trap while ORCA
  runs, no guard, a PGID-only guard, `ps` rc 1 as empty, no self-listed check, zombies counted, a
  group kill by the member's pid, a kill of the member's group, an unclamped grace, an unbounded
  handler hold, an unbounded KILL round) and turns it red; the controls are run by hand, not kept
  as tests.
- **Unit 5.4b (ADR-024 o25.11, o25.14.7).** Lab, real `bash`: `cancel.sh sweep` on a wrapper killed
  in this boot ends ORCA, its child and an escaped rank, spares a foreign-cwd member, publishes no
  `.cancelled`, removes `.tmp`, and the classifier then reads `Lost` with no orphans; with the wrapper
  alive and ours it exits 4 and signals nothing; `.exit_code`, `.cancelled`, no or a corrupt
  `.started` exit 5; an unknown mode exits 2 (`script_tests.rs`). `submit.sh reenqueue` after a
  dropped queue (the stub daemon's rows emptied): enqueued with the claim, the new `.enqueued` and a
  new row, `.submitting`/`.enqueued` left out of the file check, then row 9, then — the queue dropped
  again — row 11 `Failed { WrapperNeverStarted }`, and a second re-enqueue refused on the claim; a
  refusal with no claim on each of `.started`, `.exit_code`, `.cancelled` (the marker kind, not an
  upload mismatch), the claim, a queued and a running row and a sha mismatch; a finished row
  accepted; `.enqueued`'s own socket read when it differs; no claim when the scan blocks or the slot
  cannot be checked (`reenqueue_script_tests.rs`); the collector's claim under the v2 header
  (`script_tests.rs`) and `.reenqueue-1` in the server's listing. End to end against the real
  scripts (`backend_e2e_tests.rs`): a dropped queue, with and without a finished row → re-enqueued →
  `Queued` → dropped again → confirmed by a pass 45 s later → `failed` with the evidence; a changed core count → `failed` with no call;
  a stale `boot_id` → `lost`; a live non-wrapper session leader in the job dir → `cancel.sh sweep` →
  `lost`. Fake server (`poller/lost/tests.rs`, `poller/reenqueue/tests.rs`,
  `poller/undecided_tests.rs`, over `poller/fake_tests.rs`): every gate of o25.11 and o25.14.7 —
  `lost` written with its texts and NULLed columns and live for no predicate, sweep before finalise,
  the bound and the survivors, progress, a "wrapper alive" sweep not counted, the evidence strikes,
  the dropped-cancel note, row 1 without `.cancelled`; one re-enqueue per pass with the current mask
  and the local list, the claim count reaching `classify`, the never-start confirmation (one pass not
  terminal, passes under 45 s apart not terminal, another verdict between them restarting it, the 45 s
  measured between the two server reads with a slow collect), a label ending the waiting state, an
  `Unknown` with no call made counted, a claim read error ending the snapshot (Lab), a changed core count failing with no call
  and the local dir byte-identical, the waiting notice at the 3rd transient refusal and its reset, 3
  counted refusals → `failed`, the reset of a refusal the next classify re-decides, the unverified
  wait with no call, a lost reply not counted, `refused-kup` clearing the stamp, a server upload
  mismatch counted; the `Indeterminate` notice at the 3rd pass, kept by a check without an answer and
  reset by a decided verdict; ready + one more collect on a not-installed collector. Every gate has a
  negative control (a source mutation that turns it red), kept as a re-runnable script with the unit
  (54 controls; the sweep's four and the re-enqueue's claim-order, finished-row and exclusion controls
  are also permanent tests over mutated script copies).
- **The gate (`gate.rs`, `ssh.rs`, ADR-024 o24.10)**, each on a local `Gate::new(n)` unless named:
  with n = 8, 12 callers parked on a latch — exactly 8 hold, 4 wait, max overlap 8, all 12 finish,
  the key's entry dropped; FIFO under a forced interleaving (a releaser that asks again queues
  behind the waiter, 500 fresh gates); two releases admit two waiters with no third (500 fresh
  gates); a release wakes its own key's waiter when another key's waiter sleeps first on the shared
  condvar; two hosts do not block each other; the host key from all three production argv shapes,
  anything else the fallback key; a call queued longer than its 300 ms timeout still runs to
  completion; a spawn error and a timeout kill release the permit; `run_gated` holds the permit for
  the child's whole life (on one permit the second child starts after the first ends). **Wiring:**
  nine real `SystemRunner` rsync calls to one unique host (a fake `-e` transport that sleeps 1.5 s)
  are seen in the global `REMOTE_CALLS` as 8 holders and 1 waiter, then all released; every ssh argv
  and both rsync `-e` transports carry the two keepalive options. The `ssh.rs` timing tests
  (`elapsed < 5 s`) run ungated (`run_with_timeout`), so the other tests' `SystemRunner` calls,
  which share the global gate's fallback key, cannot skew them. Controls (run by hand): n = 9 and
  n = 1, a plain counting condvar, a head-of-line `serving` counter with no notify on admission,
  `notify_one`, one shared key, the argument before `--` / the local rsync end as the key, the
  deadline taken before the permit, a permit kept on an error, a permit dropped at once, a
  `SystemRunner` that bypasses the gate, the wrong rsync key under the wiring test, each keepalive
  option dropped.

- **The pending submit's reply kinds (`call_script_tests.rs`, ADR-024 o23)**: a blocked attempt —
  a pinned process on the mask (step 5), and the slot daemon's `tsp -l` failing (step 3) — is refused
  with its kind and leaves the job dir **byte-identical** (every entry, recursively; no `.tsp-out`, no
  claim) with nothing enqueued; step 3's three branches (the slot's `tsp -l` failed; `/proc/net/unix`
  absent; a wrong header — the last two through a test copy whose step-3 read takes `$FAKE_NET_UNIX`)
  are each `SlotCheckFailed`; a hung `realpath`, wrapper `sha256sum`, `find`, upload `sha256sum` (PATH
  stubs that sleep) and the scan (its budget shrunk to 10 ms in a test copy) are each `SlotCheckFailed`
  naming the child; the upload mismatch names arrive as lists, an extra name holding a space and a
  newline byte for byte. Controls run on mutated copies: `.tsp-out` made before the scan, step 3's
  failed `tsp -l` refused as text, the realpath timeout branch removed.

- **Real 5.3 calls (`call_script_tests.rs`)**: each script runs as `bash -s` with its NUL list (the
  ssh call's exact stdin, `stdin_with_values`) and its stdout goes to its **Rust parser** — never
  compared with a hand-written string. The `Lab` adds a stub `busctl` and a stub `tsp` that enqueues
  (a `queued` row in the recorded shape, the id on stdout, `TMPDIR` recorded), starts a setsid'd
  "daemon" listener that inherits its fds, and logs every call that inherited fd 9; `HOME` is the
  lab root, so each lab has its own account lock. Covered: the happy submit (claim, `.enqueued`,
  `.tsp-out` as `TMPDIR`, argv verbatim, lock free after, collector → `Queued`); a second submit of
  the job; two concurrent submits of one job (one enqueue); a claim taken between the marker check
  and the claim (fault-injected `mkdir`); a busy lock (refused after the 20 s wait, the opener
  listed, nothing enqueued); a corrupted byte (top level and in a subdirectory), a missing and an
  extra file; `refused-kup` for `b true`, rc 1, other output, `b false\n` + NUL + bytes, and a
  busctl sleeping past its timeout (124); the wrapper (bytes ≠ name, a symlinked wrapper, a symlinked `<root>/bin` holding the right
  bytes, an absent wrapper, a path or an uppercase sha as value 5); a symlinked job dir, a job dir
  outside `<root>/jobs`, every marker as a dangling symlink, a row already holding the job; a failed enqueue → `failed-after-claim`
  → label `SubmitInterrupted`; the slot check (a running job of the same slot accounted; the same
  without its row blocked, wrapper and pinned ORCA named, no claim — the test first waits until the
  running job's session holds no zombie, since a pinned zombie is never accounted for (o item 13); no daemon → blocked; a queued row
  on another own slot socket; a failed `tsp -l` there; a pinned stray on vs off the mask); every
  label rule and precedence boundary; a read error in the label call; a hung `tsp -l` in the label
  call (bounded, a socket Error); poll (absent file, absent dir,
  full, capped, no growth, shrunken, reassembled from 7-byte chunks with NUL and non-UTF-8 bytes, an
  unreadable log); the listing against `list_dir` of the same dir under both policies; `bash -n` on
  every stdin script. **The slot check scans this machine's real own-uid processes**, so these tests
  hold one mutex and use the mask `8-11`, clear of every CPU the 5.2 tests pin.

- **The backend core end to end (`backend_e2e_tests.rs`)**: `ssh_backend`'s functions with a runner
  that runs each call locally against the `Lab` — `env PATH=<stubs> HOME=<lab> bash -s` for the
  exact production ssh argv (asserted), and the production rsync argv with `-e` dropped and
  `<host>:` stripped; `cancel.sh` and `collect.sh` run through the real trampoline. Covered: a submit
  to a root with no wrapper and no `tsp/` (prepare → install → prepare → rsync → submit;
  `Enqueued(0)`, the coordinates, `%pal` inserted at the mask's CPU count and announced, the upload
  byte for byte, no install temp left, label `Classifier`, the 5.2 collector `Queued`, and the queued
  job **cancelled** through `cancel_job_core` — delivery (prepare, label, collect `Queued`, mark,
  `cancel.sh`, collect) → `CancelDelivered(Cancelled)` with the status unchanged, then **one ordinary
  status step** (label, collect) → `cancelled`); a failed enqueue → `FailedAfterClaim` → label
  `SubmitInterrupted` → retry refused → cancelled (delivery: prepare, label, **mkjob**, `cancel.sh`,
  collect; then one status step) → `cancelled`; `b true` from busctl → `KillUserProcesses`, stamp
  cleared, label `NotOnServer`, no ssh for an unverified profile, retry after re-stamping →
  `Enqueued`, a second such job, not on the server, cancelled with the profile unverified (mkjob first;
  no `verified_at` gate) → `cancelled`. These ports of the former withdraw tests are the only
  real-script coverage of label → mkjob → `cancel.sh` → collect for a job not on the server (ADR-024
  o22.14.7); without the mkjob, the real `cancel.sh` exits 3 on the missing dir and they go red; `%pal nprocs 48` uploaded as the 4-CPU mask's 4 (the server's hash
  check accepts it) and a retry after the mask narrows to 1 CPU uploading `nprocs 1`, the database
  keeping the original; a symlinked `<root>/bin` → refused at the prepare step, nothing uploaded. The
  poller (`crate::poller`, unit 5.3 B2) against the same lab: the real `poll_log` streams the first
  lines of a finished job's `output.out` into a watching view; the status step's real label call hands
  it to the classifier, the real collector says `Completed`, the real rsync brings the files down (the
  `.submitting` claim as a symlink) and the real `list` hashes match the copy; the row is finalised
  from the copy (`completed`, energy and wall time) and the view gets the rest of the log, the
  unterminated last line included, before the terminal `job:status`; a copy corrupted right after the
  rsync fails the real listing → a strike, the row stays `queued`. **The pending submit** (o23) against
  the same lab: a Create & Run whose slot is held by a pinned `sleep` on the mask goes pending (the
  real `blocked` reply naming the sleep's pid); the poller's re-check while it is held runs only
  prepare + `submit.sh` and leaves the job dir's names unchanged; once the sleep is reaped, the next
  re-check enqueues it (`Enqueued(0)`, the flag cleared, `.enqueued` published). **The host facts**
  (o26.6): with a `df` that fails on the server (`a_failing_df_never_breaks_ready_or_a_cancel_delivery`)
  the real prepare still answers ready — the submit enqueues, its `memory_notice` says the free disk
  was not checked — and a cancel's delivery through the same prepare is `CancelDelivered`, then
  `cancelled`; the in-suite negative control swaps in a prepare that `die`s on the df failure and the
  delivery is not made, the job stays `queued`, no `.cancelled`. `the_real_prepare_reads_this_machines_meminfo_and_df`:
  the real prepare's `MemTotal` equals the one Rust reads from `/proc/meminfo`, and its `df` free bytes
  are within 256 MiB of `statvfs`. `the_prepare_host_fact_reads_are_bounded`: a `df` and a `head -5
  /proc/meminfo` that sleep 30 s → the real prepare answers rc 0 with both facts `-` within seconds,
  under a 20 s bound of its own (negative control: the `timeout` dropped from `fact` → red). The
  prepare parser on its own
  (`the_host_facts_parse_and_their_absence_is_never_an_error`): KiB/bytes from the recorded formats;
  `-`, garbage and absence → `None` with `ready()` unchanged; a wrong length, the two out of order, a
  duplicate or an unknown record → an error (negative control: `ready()` requiring the df fact → red).
  The scripts on their own: install readies a fresh root (all three), `kept` on a second install, wrong
  bytes under the cancel script's name replaced by rename (new inode, the others kept), bytes that do
  not hash to the name never published (no temp left), a symlinked `bin/` or `tsp/` refused with
  nothing written through it; mkjob makes the dir and refuses, after the mkdir, a job dir linked outside `jobs/` and a sibling link `jobs/w3 -> jobs/w1` (only the job dir's own realpath catches that one; a permanent mutant that reports the job dir unresolved must miss it); the
  trampoline refuses `wrapper`, a path or upper-case name, a bad sha, forged bytes, a symlinked script
  and a symlinked `bin/`, answers `not-installed` for a missing script, runs the real cancel and
  collect, and — through its test-only `probe` entry — passes a hostile argument byte-identical with
  stdin at EOF and rc 7 in a complete reply. (Stdin at EOF is an observation, not a control; it cannot bite
  here: the read loop has already consumed `bash -s`'s stdin before the script runs.)

Negative controls (each guard broken, the named tests red, restored): listed per unit in
[log.md](../log.md) (Part A 2026-10-03, Part B 2026-10-03, 5.3 A2 2026-10-05, 5.3 B1 Part A
2026-10-05, 5.3 B2 Part A 2026-10-06, 5.4a Part B1 2026-10-18, 5.4c Part A 2026-10-19). The five of 5.3 A2 are
also permanent tests: each runs its guard's check on a mutated copy of the script (exactly one
occurrence replaced) and requires it to fail.

## The view

The screens over the remote jobs (B3 Part B; details in
[frontend.md](frontend.md#remote-job-view-logic-srcjobsremotets--adr-024-o19)): New Job's "Run on"
select (o5) sends `submit_job`'s `target` and is remembered app-wide in the `last_run_target` setting
(o20.1: written on Create & Run, a gone or disabled profile falls back to Local on load, the load
never writes); the job view calls `watch_job_log` only after its
`job:log`, `job:convergence` and `job:log-reset` listeners resolve, closes it only if it opened it,
and skips the backfill for a live remote job (o16.7); the job view and the Jobs table attach their
`job:remote-state` listener, then read `remote_job_state`, keeping the greatest `seq` (o19.6), and
show `remoteView`'s badges and actions: retry submit / retry fetch (hidden while a cancel is pending,
disabled while the last contact failed), and the cancel family — Cancel for a `queued`/`running`
job, after a plugin-dialog confirm and a re-check — the row and record read again from the backend —
that it is still offered (o20.3, o22.14.2, o22.14.14.1);
Undo cancel / Retry cancel for a pending one by o22.14.1's table; Retry cancel, no confirm, for a
`cancelling` job. Every click of the family goes through one entry (`clickCancelFamily`: `sendCancel` →
`confirmThenCancel`, the one `invoke("cancel_job")`, which derives its offer from the row itself; or
`sendUndo`, the one `invoke("undo_cancel")`), then the row and the record are reloaded and the answer
is worded (`cancelNotice` / `undoNotice`). The Jobs table, the detail's panel and the stored times
(`formatLaptopUtc`) show all of it. What they bind to:
each non-terminal remote job's **remote record** (`RemoteStates`: the label, the last outcome or
"fetching", a failed check, the fetch bound) set by every status step under the job's guard and
pushed as `job:remote-state { job_id, seq, state }`, and the `remote_job_state(id)` read
([execution-backends.md](execution-backends.md), "The remote record"). The poller runs: one loop
thread started at launch monitors every non-terminal remote job (status every 15 s; the log every
2 s while a view watches it), emitting the local `job:log` / `job:convergence` / `job:status`
events and `job:log-reset { job_id }`; `watch_job_log(id, open)` counts a job's open views and
`retry_remote_fetch(id)` is the manual retry after 3 failed fetches
([execution-backends.md](execution-backends.md#the-remote-pollers-core-pollerrs-poller-adr-024-o-item-4-o15-o16)).

## Live-checked on uni

Recorded in [remote-live-run-b4.md](../architecture/remote-live-run-b4.md):
- **B4 fixes** (ADR-024 o24.10, 2026-11-18 and 2026-11-19): the session wait's healthy path
  (`.tsp-out` reads "session empty at ORCA's return"); M6 in the app (at most 8 concurrent ssh
  observed in 454 samples; no "Server unreachable" seen by eye on a cold burst).
- **The pending submit** (o23.15 item 6, 2026-11-19): the "slot busy" answer, the badge, the blockers'
  notice, no Retry submit, and the enqueue 5–10 s after the blocker exited.
- **Retry cancel** with the link down (o24.9's hand check, 2026-11-19): the cancel recorded pending,
  delivered after the link came back.

## Not built yet

- **Not measured:** that the keepalive ends a call on a dead link in ~2 min is inferred from the
  options (M2 waived, o24.9). G1b (the overnight suspend) is still open.

- **A run target is chosen only through Create & Run** (ADR-024 o20.2): a draft made with "Create
  Job" and later run from the Jobs table's "Run locally" button runs locally, and every fan-out
  submits locally (`target: null`) and says so — the DFT re-opt, the OptTS refines of the 1D scan,
  the 2D surface and the NEB band, and the connectivity check. Choosing a target there is not built
  in 5.3.
- **Known limitation — `started_at` of a remote job** is stamped with the time of the status check
  that first sees the classifier's `Running` (`queued` → `running`, `crate::poller`), not the time the
  job started on the server. It can be late by up to a status period (15 s), or by hours when the
  laptop was closed while the job started. The collector's snapshot carries the server's `.started`
  (its own fields), so a later unit can stamp the real start from it.
- **Not measured** (B4's live run): the download and listing timeouts (300 s, 1800 s with the
  `.gbw`), rsync down over a real link and its exit codes under a drop.
- **Measured on uni** (probe 5.3 B0, 2026-10-05, [remote-sync-probe.md](../orca/remote-sync-probe.md#probe-53-b0-2026-10-05)):
  real `tsp <command>` prints `<id>\n` (accepted by the submit's regex); with `9>&-` neither a fresh
  nor an existing daemon nor its jobs hold the lock; `tsp -l` on a stale socket starts a daemon;
  `timeout -k 1 2` ends a hung client with rc 124; `busctl` prints exactly `b false\n` (8 bytes); the
  scan takes 0.57–0.79 s; a full `submit.sh` run returned `enqueued 0`. **Still unmeasured:** an
  other-uid socket with the slot layout (`-O` false; needs a second account).
- The Full-mode export skip of rsync temp names (`.*.??????`, ADR-024 o6 residual) — 5.3 Part B,
  with the first real download.
- **The uni measurements behind these scripts are done** (probe 5.3, 2026-10-03,
  `architecture/task-spooler-uni-probe.md`):
  - every ENOENT message the readers match is **identical** on uni (bash 5.2.21, coreutils 9.4,
    procps-ng 4.0.4), and EACCES/EISDIR/EINVAL read as errors; the ESRCH alternatives are not
    exercised (they fail closed if different);
  - `ln -T` is one `linkat` there too;
  - the shipped scripts ran end to end, and their wire output classified correctly with the Rust
    parser.
- The cancel and undo commands, the cores and the poller's run order around them — delivery instead
  of the ordinary step while a cancel is pending, the `cancelling` sweep with its bound, the evidence
  fetch of a cancelled job that ran — and the view's pure logic exist
  ([execution-backends.md](execution-backends.md#remote-cancel-record-deliver-undo-adr-024-o223o224),
  [frontend.md](frontend.md)); the screens draw them (5.4a Part B2: Undo and Retry cancel buttons in
  the row and the panel, the notices, the `cancelling` chip and count, the laptop-time display).
  Awaiting Anton's live gate: how the badges, buttons and notices look and behave in the real window.
  The sweep and the
  evidence fetch have run only against the fake server; the B4-style live run on uni is still to come.
- **Unit 5.4b's view and live checks** (ADR-024 o25.9–10): the view is built — the `lost` chip and
  badge, the TS mirrors of the new shapes above, the waiting and `undecided` notices
  ([frontend.md](frontend.md)). Both o25.10 checks ran on uni with throwaway jobs (2026-11-18 and
  2026-11-19: a `kill -9` of a running wrapper → sweep → `lost`; `tsp -K` with a running and a queued
  job → the queued one re-enqueued once, waiting until the P4 survivor ended;
  [remote-live-run-b4.md](../architecture/remote-live-run-b4.md)). A real reboot is simulation-only (a
  substituted `boot_id`), recorded at the first natural occasion.
- **Known limitation — the ended-first note** ("The job finished before the cancel reached the
  server.") is held in the poller's per-launch memory until the job's `finalise`, which may be several
  status periods later (failed fetches, a server that does not answer); a restart in between loses the
  note, never the result (ADR-024 o22.13.2, accepted residual).
