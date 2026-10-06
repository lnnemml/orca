# Remote jobs — scripts and classifier (`src-tauri/src/remote/`)

Phase 5 unit 5.2. Three static server-side scripts run and observe a remote ORCA job; the app
decides the job's state from a **snapshot of raw facts** they collect, per
[ADR-024](../architecture/adr-024-remote-execution-intermittent-connectivity.md) Decision l. The
server filesystem is the source of truth (ADR-024 c); the decision is made in Rust, never accepted
from the server (rule #9).

The module itself does no remote I/O: no ssh, no processes. The scripts are embedded bytes, and
the pure parts here build each call's values and parse its reply. The one exception is `sync.rs`,
which lists and hashes the **local** job dir for the transfer post-conditions. The calls are run by
the remote backend's core, `src-tauri/src/ssh_backend.rs` (submit, retry, label, withdraw over an
injected `CommandRunner`; [execution-backends.md](execution-backends.md#sshbackend-ssh_backendrs)),
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
| `ssh.rs` | `ssh_bash_argv` (`ssh -o BatchMode=yes -o ConnectTimeout=10 -- <host> bash -s`, host re-validated), `ssh_options` (the two `-o` options, shared with rsync's `-e`) and `CommandRunner`/`SystemRunner` (stdin and both streams on threads, 1 MiB cap, process group killed on timeout). Used by the 5.1 connection test (`modules/server-profiles.md`); meant for 5.3's submit too |
| `sync.rs` | rsync argv (`upload_argv`, `download_argv`), the download filter `download_filter_args`, its leaf patterns `download_patterns` and its Rust mirror `download_selects`, `list_dir`/`upload_expected`/`expected_values` (local file lists with sha256), `ListArgs` + `parse_list_reply` (the server's listing), `compare_download` (the download post-condition) |
| `poll.rs` | `PollLogArgs`, `parse_poll_reply` → `LogChunk` with the length post-condition; `check_echo` (the n-6d echo check every 5.3 reply shares) |
| `submit.rs` | `remote_job_dir`, `SubmitArgs`, `parse_submit_reply` → `SubmitReply`; `LabelArgs`, `parse_label_reply` → `LabelFacts`, `label` → `Label` (the label rules) |
| `prepare.rs` | the calls that ready the server before an upload or a withdraw: `UPLOADED` (wrapper, cancel, collect), `PrepareArgs`, `parse_prepare_reply` → `PrepareFacts`, `check_prepare` → `Prepared` or `ShapeRefusal`; `InstallArgs`, `parse_install_reply` → `[Installed; 3]` |
| `run.rs` | the trampoline's `RunArgs` (`JobScript::{Cancel, Collect}`) and `parse_run_reply` → `RunReply`; the withdraw's `MkjobArgs` and `parse_mkjob_reply` (o 14.1) |
| `race_model.rs` | test-only model of the d′ race (`.started`/`.cancelled`) |
| `script_tests.rs` | test-only: the real job scripts run on this machine; the shared `Lab` harness (see Tests) |
| `call_script_tests.rs` | test-only: the real 5.3 calls through `bash -s`, read by their Rust parsers (see Tests) |
| `backend_e2e_tests.rs` | test-only: `ssh_backend`'s submit, retry, label and withdraw end to end against the `Lab` (see Tests) |

Rule #6 is shared with the local backend: `local_backend::has_normal_termination` (the one
`ORCA TERMINATED NORMALLY` test, also used by `detect_completion`) over the last
`local_backend::TAIL_BYTES` (5 KiB) of the snapshot's output tail.

## A remote job in the database (schema v20)

`jobs` carries the job's **coordinates**: `remote_host`, `remote_job_dir` (absolute,
`<root>/jobs/<job_id>`) and `remote_socket`, nullable TEXT, written once at submit before any ssh and
never rewritten from the profile (ADR-024 o item 1, n 6b). A CHECK on the last column keeps the three
all-NULL or all-set. **A job is remote iff its coordinates are non-NULL**: `Job::is_remote()` is
`remote_host.is_some()`, the same column every local-only query filters on (`remote_host IS NULL`) —
today `local_backend::next_local_queued_job` (behind `try_start_next`) and `reconcile_on_startup`.
`backend_id` never decides it. A remote job keeps `queued`/`running` in `jobs.status`.
`submit::remote_job_dir(root, job_id)` builds the dir and checks it with `is_valid_path`; the job id
must be one component.

## Transfers (`sync.rs`, ADR-024 o items 3.2, 3.3.4, 6)

- **Upload:** `rsync -a --checksum --mkpath -e 'ssh -o BatchMode=yes -o ConnectTimeout=10' <local>/
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
refused <len>              exactly one outcome: reason bytes (nothing claimed)
refused-kup <len>          KillUserProcesses is not `b false` (nothing claimed): the evidence
enqueued <id>              decimal tsp id (.enqueued published)
failed-after-claim <len>   reason bytes (.submitting stays)
end
```

`parse_submit_reply` → `SubmitReply::{Refused, RefusedKup, Enqueued, FailedAfterClaim}`; a broken
reply or an `error` record is an error, never an outcome.

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
  refuses `lock busy: lock file open in: <pid> <cmdline>; …` — every process (other than itself)
  that has the lock file open, from one `find /proc/[0-9]*/fd` (only own processes' fds are
  readable): the holder, any other waiting submit, a daemon that leaked fd 9. It does not say which
  one holds the lock (o item 13.4). With none found: `lock busy: lock file open in: no process found
  (the holder may have exited)`;
- **2** `busctl … KillUserProcesses` exits 0 printing exactly the 8 bytes `b false\n` (content and
  size both checked: `read -d ''` alone stops at a NUL) — anything else (`b true`, other output, a
  NUL, rc ≠ 0, a `timeout` 124) is `refused-kup` with the evidence (n item 7, o item 13.3);
  `realpath -e <job> <job>/..` prints exactly `<job>` and `<root>/jobs` (a job dir reached through a
  symlink refuses); `<root>/bin/wrapper-<sha>.sh` is a regular file, not a symlink, whose `realpath`
  is itself (so `<root>/bin` is not a symlink either) and whose `sha256sum` is `<sha>` — an absent
  or mismatched wrapper is `refused`, never `failed-after-claim`;
- **3** no `.started`, `.enqueued`, `.exit_code`, `.cancelled`, `.submitting` in any form
  (`path_exists`, a dangling symlink counts), and — only if `/proc/net/unix` lists the slot socket —
  no `tsp -l` row of the slot holding the job dir as a whole token (a failed `tsp -l` refuses);
- **4** the upload post-condition: `find <job> -path <job>/.tsp-out -prune` lists every entry; the
  regular files must be exactly the expected names, and one `sha256sum` over them must give every
  expected hash. A refusal names `missing […]`, `extra […]` (an rsync temp, a non-file) and
  `differing […]` (a changed byte, or an expected name that is not a regular file);
- **5** the slot check (o item 9), below;
- **6** `mkdir -p <job>/.tsp-out`, which must then be a directory and not a symlink;
- **7** the claim `ln -sT x <job>/.submitting` — no-clobber; it fails if anything took the name
  since step 3;
- **8** `TMPDIR=<job>/.tsp-out TS_SOCKET=<sock> tsp bash <wrapper> <job> <mask> <orca>`; its stdout
  must be one decimal id (an optional final newline). Then `.enqueued` (`socket=…\nid=…\n`, temp
  file + `mv -fT`), then the post-condition: the socket is listed verbatim in `/proc/net/unix`. A
  failure from here on is `failed-after-claim` (the claim stays).

Every `refused` reason starts with its step (`values:`, `lock busy:`, `lock:`, `realpath:`,
`wrapper:`, `marker:`, `slot socket:`, `row:`, `upload:`, `slot check:`, `slot busy:`, `tsp-out:`,
`claim:`), for people reading it.

**The lock fd and the bounds.** Every `tsp` call (`-l` too), every child under `timeout` and the
claim/publish commands run with `</dev/null` and `9>&-`, so no daemon tsp starts can inherit the
account lock (probe 5.3c); the head's short readers (`stat`, `cat`) still inherit it and exit at once.
Every child that can block while the lock is held runs under `timeout -k 1 N` (TERM after N s, KILL
1 s later): busctl 2 s, realpath 2, the wrapper's realpath 1, the wrapper's sha256 2, the slot's
`tsp -l` 2, the upload `find` 2, the upload `sha256sum` 6, the slot scan 8 (its own `tsp -l` calls
each under `timeout -k 1 2` inside it), the enqueue 3 — 28 s, and 37 s worst case with the nine
kill-afters, under the 40 s of o item 3.3.1; a timeout is a refusal (`refused-kup` for busctl,
`failed-after-claim` after the claim). The short file operations — `stat`, `cat`, `mkdir`, `ln`,
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
  every mask: fail closed). The refusal is `slot busy: mask <m> is held by pid <n> (wrapper|pinned,
  cores <list>, job dir <dir>|cwd not readable); queued row <id> on <socket> (…)`.

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
(tsp runs it), **`cancel`** and **`collect`** (the trampoline runs them). Before an upload or a
withdraw, two calls make sure nothing is written through a symlink and that all three exist with the
right bytes. This section is the wire formats' spec of record (like the label call's, o item 13.5).

**The `PREPARE` call** — read-only; values: the root, the job dir (`<root>/jobs/<id>` exactly), and
the sha256 of the wrapper, cancel and collect scripts (5 values). For `<root>`, `<root>/jobs`, the job
dir, `<root>/bin`, `<root>/tsp` and the three scripts, in that order, it reports whether the path
exists (only ENOENT is `absent`; any other `stat` failure is an `error` record and exit 3), its kind
(`stat -c %F`, which does not follow a symlink) and its `realpath -e` (`-` when it does not resolve).
For each script it adds the sha256, computed only for a regular file (a fifo would block
`sha256sum`), under `timeout -k 1 5`.

```text
orcastudio-prepare 1
argc 5, arg <len> × 5          the values, verbatim
<name> absent|present          root, jobs, job, bin, tsp, wrapper, cancel, collect — in this order
  kind <len>                   ┐ only for present
  realpath <len>|-             │
  sha256 <hex>|-               ┘ scripts only: a regular file's sha256, else -
end
```

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
the install's six `sha256sum`s at `timeout -k 1 5`, 36 s = 46 s); `RUN_TIMEOUT` 60 s for the
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

`parse_mkjob_reply` requires exactly `<job>` and `<root>/jobs` (o item 1), so a withdraw never
publishes `.cancelled` through a symlinked component.

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

214 tests in the module.
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
  `.started` from another boot with `proc skipped`) and its read errors. Each test signals only processes it started, by recorded PID and start
  time (a wrapper that is not the test's own child is tracked from its `.started`), and cleans up in
  `Drop`: SIGSTOP to every live process it knows of, then SIGKILL, then a bounded retry of the dir
  removal, so a tracked wrapper is stopped before its ORCA is killed. Two cancel tests also
  assert, after the drop, that no process has a cwd in or an argument under the lab root.

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
  byte for byte, no install temp left, label `Classifier`, the 5.2 collector `Queued`, withdraw
  refused); a failed enqueue → `FailedAfterClaim` → label `SubmitInterrupted` → retry refused →
  withdraw (label, prepare, mkjob, run cancel, run collect) → classifier `Cancelled` → row
  `cancelled`; `b true` from busctl → `KillUserProcesses`, stamp cleared, label `NotOnServer`, no ssh
  for an unverified profile, retry after re-stamping → `Enqueued`, a second such job withdrawn with the
  profile unverified → `cancelled`; `%pal nprocs 48` uploaded as the 4-CPU mask's 4 (the server's hash
  check accepts it) and a retry after the mask narrows to 1 CPU uploading `nprocs 1`, the database
  keeping the original; a symlinked `<root>/bin` → refused at the prepare step, nothing uploaded. The
  scripts on their own: install readies a fresh root (all three), `kept` on a second install, wrong
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
2026-10-05). The five of 5.3 A2 are
also permanent tests: each runs its guard's check on a mutated copy of the script (exactly one
occurrence replaced) and requires it to fail.

## Not built yet

- The UI over the remote job commands (B3), the poller (B2).
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
- Reconcile wiring, the DB re-enqueue counter (schema v21; v19 is the 5.1 Part B profile columns, v20 the 5.3 job coordinates), `Lost`/`Cancelling` in `JobStatus`,
  sweep counting — unit 5.4.
