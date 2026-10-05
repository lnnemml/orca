# Remote sync probe (rsync argv + byte-offset reads over ssh), unit 5.3a

Measurement page (domain rule #10) for ADR-024 (l)(m)(n) / unit 5.3. Run 2026-10-05 from the laptop
to the uni server as `anton` via the ssh alias `uni` (no ORCA, synthetic files, no tsp). Remote scratch
`/home/anton/.orcastudio/probe-5.3a/` (removed afterwards). Sibling pages: `remote-deploy.md`,
`../architecture/task-spooler-uni-probe.md`, `../infrastructure/uni-server.md`.

## Setup facts
- rsync **3.2.7 (protocol 31)** on laptop and uni; `tail` GNU coreutils **9.4** (both); bash **5.2.21** (both);
  uni login shell `/bin/bash`.
- `~/.ssh/config` for `uni` **already has** `ControlMaster auto`, `ControlPath ~/.ssh/cm-%r@%h:%p`,
  `ControlPersist 10m`, `ServerAliveInterval 30`, `ServerAliveCountMax 4`. So every plain `ssh uni` / `rsync` uses the mux.

## 1-2. Upload
Commands (src = local dir with `input.inp`, `input.xyz`, `sub/f.txt`):
```
rsync -a src/jobA/ uni:$R/jobs/jobA/             # $R/jobs absent -> rc=11
  rsync: [Receiver] mkdir ".../jobs/jobA" failed: No such file or directory (2)
rsync -a --mkpath src/jobA/ uni:$R/jobs/jobA/    # rc=0, creates all parents (supported both ends, 3.2.7)
rsync -ai --mkpath src/jobA/ uni:$R/jobs/jobA/   # 2nd identical upload: rc=0, ZERO itemize lines (no-op)
rsync -ai --mkpath src/jobA/ uni:$R/jobs/jobB/   # new: "created 1 directory for .../jobB", .d..t.... ./, <f+++++++++ input.inp ..., cd+++++++++ sub/
rsync -a --mkpath src/jobA uni:$R/jobs/jobC/     # NO trailing slash -> jobC/jobA/{...} (nested)
ssh uni "mkdir -p $R/jobs2"; rsync -a src/jobA/ uni:$R/jobs2/jobD/   # rc=0 (only ONE missing level works without --mkpath)
rsync -a src/jobA/ uni:$R/jobs3/x/jobE/          # two missing levels, no --mkpath: rc=11
```
`-i` marks: `<f+++++++++` new file sent, `.d..t......` dir mtime fix, `>f.st......` updated (size+time).

## 3. Selective download (filter set)
Remote job dir had `output.out input.xyz input_trj.xyz input.hess input.gbw(3 MB) input.tmp input.densities
input.inp input.property.txt sub/deep.xyz .tmp/pmix.1/lit .tsp-out/ts-out.AbC .exit_code .started .enqueued .cancelled`.
```
FILT=(--include='output.out' --include='*.xyz' --include='*.hess' --include='.tsp-out/' --include='.tsp-out/**'
      --include='.exit_code' --include='.started' --include='.enqueued' --exclude='*')
rsync -ai "${FILT[@]}" uni:$R/jobs/jobS/ dlA/       # rc=0
```
Came down exactly: `.enqueued .exit_code .started input.hess input.xyz input_trj.xyz output.out .tsp-out/ts-out.AbC`.
Not down: gbw, tmp, densities, `.tmp/`, `sub/` (so `sub/deep.xyz` too), `.cancelled`, `input.inp`, `input.property.txt`.
- Variant with gbw: same list with `--include='*.gbw'` before the final `--exclude='*'` -> same set + `input.gbw`.
- `--prune-empty-dirs` changed nothing here (same output).
- **Dot-files are NOT special**: `*` and `*.xyz` match dot-names; each marker needs an explicit include
  (`.cancelled`, `input.property.txt`, `input.inp` are excluded by this set — add them if wanted).
- Order matters (first match wins): the final `--exclude='*'` must come last; `--include='.tsp-out/'` (dir) is
  required in addition to `.tsp-out/**`, else the dir is not entered. Excluding `*` also stops recursion into
  unlisted subdirs, so `*.xyz` in a subdir needs `--include='*/'` (not measured).

## 4. Download while the remote file grows (1 line / 0.1 s into output.out; 3 runs)
`rsync -ai --include=output.out --include='*.xyz' --exclude='*' uni:$R/jobs/jobG/ dgN/`
All 3 runs: **rc=0, no warning** (no 24, no "file has changed"). Delivered a prefix (650/800, 2400/2500, 4100/4250 B
vs remote size read after) that ended on a newline with sequential lines 1..N. A follow-up sync of the still-growing file
printed `>f.st...... output.out`, rc=0. **Caveat:** the writer used 50-byte single `printf`s, so each write was atomic;
a prefix ending mid-line is not excluded (rsync copies whatever bytes are on disk; the 5.x probe below shows mid-line
content is normal for a slow writer). The consumer must not assume the last line is complete.
rsync temp file: `.<name>.XXXXXX` (e.g. `.input.gbw.lMlWCT`, mode 0600) in the destination dir, renamed on success.

## 5. Byte-offset reads over ssh (`tail -c +K`)
- **1-based**: `tail -c +1 f` = whole file; to read from 0-based byte offset N use `tail -c +$((N+1))`.
  On `ABCDEFGHIJ` (10 B): `+0` -> whole (same as +1); `+2` -> `BCDEFGHIJ`; `+10` -> `J`; **`+11` (N==size) -> empty, rc 0**;
  **`+12`, `+15`, `+100`, `+1000000` (N>size) -> empty, rc 0**. So **truncation/replacement is NOT detectable from
  `tail`'s output or exit code**; a separate size (`stat -c %s`) is needed. `dd bs=1 skip=10` (N==size) empty rc 0;
  `dd bs=1 skip=15` (N>size) prints `dd: ... cannot skip to specified offset` on stderr, rc 0 (inspected via `echo rc=$?`
  after the dd; ssh-propagated rc not separately checked).
- **No gap / no dup** on a growing file: writer appended `L%04d Å ü → partial` + 0.05 s sleep + ` rest-of-line\n` x300
  (mid-line states guaranteed). Poll loop `ssh uni "tail -c +$((off+1)) f" > chunk; off+=size(chunk)` x66 polls ->
  11100 B, sha256 `bb88ecd5...cf398` equal to remote file's. Valid UTF-8 overall, 300 lines.
- **Cap**: `tail -c +K f | head -c 7` loop accumulated equals the corresponding prefix byte-for-byte (cmp). Chunks cut
  inside multibyte chars (Å, ü, →) are **not standalone-decodable** (e.g. chunk@84), so **offsets are bytes and must
  be accumulated as bytes; decode only complete lines/ the whole buffer**. 1 MiB cap (`head -c 1048576`) on a 3,000,000 B
  random file: 4 calls, sha256 identical to the remote.
- **Size-consistent single call**: `sz=$(stat -c %s f); echo $sz; head -c $sz f | tail -c +K | head -c CAP` gives a
  size header that matches the bytes that follow (output verified; avoids the stat/tail race on a growing file).
- **Latency of one call** (`ssh uni "tail -c +1 f"`, 10 runs, wall s, sorted):
  config mux (master up): 0.07-0.09 (median 0.08, one 0.13); **no mux** (`-o ControlMaster=no -o ControlPath=none`):
  0.66-0.79 (median 0.73); scratch mux via `-o ControlMaster=auto -o ControlPath=/tmp/cm53a/%C -o ControlPersist=60`:
  0.07-0.09 (median 0.08). `rsync` of one tiny file with the config mux: 0.33 s.
- **Gotcha**: `-o ControlPath` >= 108 bytes fails: `ControlPath too long (... >= 108 bytes)`, ssh rc 255 (a path under the
  scratchpad was 130 bytes). Use a short dir + `%C`.

## 6. `Cpus_allowed_list` on uni
`taskset -c 0-3 sleep 30 &` then `grep Cpus_allowed /proc/$P/status`:
```
Cpus_allowed:	0000,0000000f
Cpus_allowed_list:	0-3
```
(key, colon, **TAB**, value). `taskset -c 0,1,2,3,8` -> `Cpus_allowed_list:	0-3,8` (kernel-normalised ranges).
Without taskset: `Cpus_allowed:	ffff,ffffffff` / `Cpus_allowed_list:	0-47` (`nproc --all` = 48).
`/proc/<pid>/cmdline` of the taskset child: `sleep|30|` (taskset execs; no taskset in argv).

## 7. Connection drop (local ssh killed mid-transfer; 200 MB random file, `--bwlimit=20000`)
- **kill -9 of rsync's ssh child** (no `--partial`), config mux or `-e 'ssh -o ControlMaster=no -o ControlPath=none'`:
  rsync **rc=12** ("connection unexpectedly closed (N bytes received so far) [receiver]; error in rsync protocol data
  stream (code 12)"). The temp file was **deleted**: dest dir had no files at all. No remote leftovers.
- same with **`--partial`**: rc=12, and the half file remains **under its final name** `input.gbw` (2,719,744 B,
  **mtime 1970-01-01**) - looks complete to a name/size check. Re-running `rsync -ai --partial ...` resumed (`>f.st......`),
  rc=0, final sha256 `e28544ad...db55` == remote sha. mtime 1970 is how rsync forces a re-send.
- **kill -9 of rsync itself**: rc=137; temp file `.input.gbw.lMlWCT` (3.4 MB, mode 0600) **left behind** in the dest dir.
  Its **local `ssh` child survived as an orphan and the remote `rsync --server --sender` kept running** (>40 s, state
  `do_select`, only ended when I killed the remote pid by hand; with config mux). Clean up on the remote side is
  therefore not automatic when the *parent* is SIGKILLed.
- **`ssh uni 'tail -c +1 bigfile' > f`** SIGTERM'd -> shell rc 143; SIGKILL'd (no mux) -> 137; local file was 0 bytes in
  both (killed after 1 s). No `tail` left on the remote afterwards.

## Facts (summary)
0. **Filter globs beyond `*`** (`[0-9]` classes, `*.relaxscan*.dat`) were not run in this probe; they
   are pinned by the laptop gate `remote::sync::tests::real_rsync_downloads_exactly_the_artifact_set`
   (real rsync 3.2.7, protocol 31 — the same version as uni's sender, fact 1), which runs the derived
   filter over a fixture dir in both directions (unit 5.3 Part A1, 2026-10-05).
1. rsync 3.2.7 / tail 9.4 / bash 5.2.21 identical on laptop and uni; `--mkpath` works. Without it, >=1 missing
   parent level of the destination fails rc=11 (`mkdir ... failed`), except the single last level.
2. Trailing slash on SRC: `src/` copies contents; `src` nests under the dest. Second identical upload is a silent no-op (`-i` empty).
3. Filter above pulls exactly the intended set; `.gbw` is excluded unless `--include='*.gbw'`; dot-files obey the
   normal globs; `.tsp-out/` needs both the dir and `/**` includes.
4. Download of a growing file: rc=0, no warning, byte prefix; mid-line end is possible in principle.
5. `tail -c +K` is 1-based; offset == size and offset > size both give empty output rc 0 -> compare against `stat -c %s`
   to detect truncation.
6. Chunks accumulated by byte offset reproduce the remote file byte-for-byte (sha256), incl. with a cap and UTF-8 splits.
7. Per-call latency 0.08 s with a mux vs 0.73 s without; `uni` already has a mux configured.
8. `Cpus_allowed_list:<TAB>0-3` format as above.
9. Dropped connection -> rsync rc=12; without `--partial` no partial file survives (kill of the ssh); with `--partial` a
   half file has the final name; SIGKILL of rsync itself leaves `.name.XXXXXX` and an orphan remote sender.

## Not measured
- A real network drop (tailscale cut), rsync `--timeout`/`ServerAlive` behaviour on a silently dead link, exit 23/24/30.
- `--include='*/'` recursion into subdirs; filenames with spaces/globs in filters; `--append`/`--inplace` variants.
- Mid-line prefix delivery by rsync for a large non-atomic write (only reasoned).
- rsync of a file replaced (rename) during transfer; `tail` behaviour on a file being truncated mid-read.
- Whether the orphan remote sender eventually dies by itself (watched ~45 s only).
- Behaviour with the shared `yats` account or other servers; rsync without `ssh` mux but with `--rsh` options.

## Probe 5.3b — host-wide core scan (Part A)
Run 2026-10-05, uni as `anton` (uid 1001), kernel 6.8.0-138, `/proc` mounted `rw,nosuid,nodev,noexec,relatime` (**no `hidepid`**).
Remote scratch `/home/anton/.orcastudio/probe-5.3b/` (removed). Fixtures: the real wrapper (`head.sh`+`wrapper.sh`, sha prefix
`eb7105e8c9c8`) copied to `bin/wrapper-eb7105e8c9c8.sh`, run as `setsid bash <root>/bin/wrapper-<sha>.sh <root>/jobs/j1 0-3 <root>/bin/orca-stub`
(stub = `exec sleep 240`; no tsp); (b) `cd jobs/j2; taskset -c 4-5 sleep 241`; (c) `cd $HOME; sleep 242`; (d) `taskset -c 6-7 bash -c 'sleep 243 & wait'`.
Scan = per `/proc/[0-9]*`: `Uid:` + `Cpus_allowed_list:` from `status`, `readlink cwd`, `cmdline`.

### A1. What each fixture yields (`pid|Cpus_allowed_list|cwd|cmdline`)
```
414340|0-47|<root>/jobs/j1|bash <root>/bin/wrapper-eb7105e8c9c8.sh <root>/jobs/j1 0-3 <root>/bin/orca-stub   (the wrapper bash)
414359|0-3 |<root>/jobs/j1|sleep 240                                                                       (ORCA stand-in, under taskset)
414344|4-5 |<root>/jobs/j2|sleep 241                      (orphan taskset)
414347|0-47|/home/anton    |sleep 242                      (unrelated)
414348|6-7 |<root>/        |bash -c sleep 243 & wait       (taskset child)
414355|6-7 |<root>/        |sleep 243                      (grandchild)
```
- The **wrapper bash itself is NOT pinned** (`0-47`): only the `taskset`-launched ORCA is. A scan keyed on "Cpus_allowed_list != all cores"
  sees the ORCA process, not the wrapper; the wrapper's mask is only in its **argv[3]** (`0-3`). Both are therefore needed: pinned
  processes (reality) and wrapper argv[3] (a job between `.started` and ORCA's exec, or after ORCA exits, has no pinned child).
  The wrapper's `.started`/`.exit_code`/`output.out`/`.tmp` were written (job ran under the real wrapper; `.exit_code` = `0` after the stub ended).
- Own-uid entries whose cwd cannot be read: `systemd --user`, `(sd-pam)`, `sshd: anton@notty` — `readlink cwd` fails with **EACCES**
  (`ls -l`: "Permission denied") although same uid (they are not dumpable / privilege-separated). Bash `readlink` is silent: only rc=1, with no
  message and no ENOENT/EACCES distinction (use `ls -l` or a language with errno to tell them apart).
- A scan script also sees the **ssh remote shell's own `bash -c <whole script text>`** (and `bash -s` for stdin scripts shows just `bash -s`).
  A `bash -c` cmdline has argv[1]=`-c`, so it cannot satisfy `is_our_wrapper` (argv[0]=bash, argv[1]=`<root>/bin/wrapper-<hex>.sh`, argv[2]=job_dir).
  But it contains the wrapper text, so a `grep`-style match over a joined cmdline would false-positive; match on the argv elements.
- **Zombie** (own-uid, parent never reaps): `status` readable (`State: Z`, `Uid`, `Cpus_allowed_list: 0-47`), `readlink cwd` fails with
  ENOENT ("No such file or directory"), `cmdline` reads as 0 bytes. So zombies never count as a pinned process or a cwd match.

### A1b. Race — a process that exits mid-scan
Process gone before the read (pid 414326): `cat /proc/<pid>/status` -> `No such file or directory`; `readlink cwd` -> rc 1 silent (`ls -l`: ENOENT);
`cmdline` redirection -> `No such file or directory`. Churn run (400 short-lived `sleep 0.02` forked while a scan walked 581 dirs):
`dirs seen=581 status_err=2 cmdline_err=2` — the same pid (411019) vanished between the glob and the read. **So the race is real and
ENOENT-shaped; a scan must treat a failed read of a pid as "gone" and skip it, never as an error.** (A pid that vanishes between the
`status` read and the `cwd` read yields a cwd failure that looks like EACCES in bash; the uid test must already have passed.)

### A2. Other users' processes
`anton` can read other uids' `/proc/<pid>/status` (Uid, `Cpus_allowed_list`) and `cmdline` (e.g. yats' bash pid 297482: `Uid: 1000`,
`Cpus_allowed_list: 0-47`, cmdline `bash`), but **`readlink /proc/<pid>/cwd` -> EACCES** ("cannot read symbolic link ... Permission denied"; also
for pid 1, root). Confirmed. Consequence: the `Uid:` line of `status` is the filter; a scan restricted to uid == anton sees only its
own processes, and **processes of other uids (yats' jobs, if any) are invisible to a cwd-based scan but visible in `status`/`cmdline`**
(`Cpus_allowed_list` of another uid's taskset-pinned process is readable). Whether the check should also count other uids' pinned
cores is a design decision, not measured here (no other-uid pinned process existed on uni during the run).

### A3. Cost
Own-uid filter first, one bash script fed on stdin (`ssh uni 'bash -s' < scan.sh`), 579 -> 581 /proc dirs, ~12–37 own processes, 5 runs, whole ssh wall time:
- naive script (`$(<status)` + `$(readlink)` + `$(tr)` for every pid — forks per pid): 1.10 1.27 1.22 1.20 1.26 -> **median 1.22 s**.
- builtin-only filter (`while read` over `status`, `mapfile -d ''` for cmdline, fork `readlink` only for own-uid pids): 0.73 0.65 0.80 0.57 0.59 -> **median 0.65 s**.
- baseline `ssh uni true` through a live master: 0.08–0.09 s. `ps -u anton -o pid=,psr=,args=` alone on the server: 0.03 s (but `ps` has no `Cpus_allowed_list`; `ps -o psr` is the *current* cpu, not the allowed set).
The cost is dominated by per-pid forks on the server, not by ssh. `ps` is present (procps-ng 4.0.4, `/usr/bin/ps`).

### A4. Inheritance of the allowed list
`taskset -c 6-7 bash -c 'sleep 243 & wait'`: bash 414348 `6-7`, its child sleep 414355 `6-7` (ppid 414348). The wrapper case: `taskset -c 0-3 orca-stub` which
`exec`s sleep -> `0-3`. The unpinned fixtures stay `0-47`. **Inherited by children and grandchildren** (mpirun ranks included).

### Facts (Part A)
1. `/proc/<pid>/status` + `cmdline` are world-readable (no hidepid); `cwd` of another uid is EACCES. `Uid:` (2nd field) selects own-uid.
2. Wrapper bash is unpinned; the pinned process is the ORCA child (`taskset`); the mask also sits in wrapper argv[3]. Orphan `taskset -c 4-5 sleep` with cwd in a job dir is visible with both its list and cwd.
3. Own-uid unreadable-cwd processes exist (systemd --user, sd-pam, sshd): cwd EACCES ≠ "gone".
4. Zombies: status OK, cwd ENOENT, cmdline empty. Exited-mid-scan pids give ENOENT on every read (measured 2/581 under churn).
5. Cost: 0.65 s (builtin, fork only for own uid) vs 1.22 s (fork per pid), over a live ssh master.
6. `Cpus_allowed_list` is inherited by grandchildren.

### Not measured (Part A)
Real mpirun/ORCA rank list; a process pinned with a mask that is not a plain `a-b` list (e.g. `0,2,4-5` — the kernel format for a taskset `-c 0,2,4-5` is not measured here);
other-uid pinned processes; scan cost with hundreds of own-uid processes; `cpuset` cgroup effects on `Cpus_allowed_list` (uni shows `0-47` unpinned).

## Probe 5.3b — ssh runner vs ControlPersist (Part B)
Laptop OpenSSH client; `uni` has `ControlMaster auto`/`ControlPersist 10m` (above). Emulation of `SystemRunner`: Python
`subprocess.Popen(["ssh","-o","BatchMode=yes","-o","ConnectTimeout=10","--","uni","bash","-s"], stdin=PIPE, stdout=PIPE, stderr=PIPE, start_new_session=True)`
+ `communicate(input=b"echo hi; echo err >&2\n", timeout=15)` (argv copied from `ssh_bash_argv`, `ssh.rs:44-56`). Start state: `ssh -O check uni` ->
"Master running (pid=17087)", stopped with `ssh -O exit uni`; then "Control socket connect(...): No such file or directory".

### B1–B2. No master up: NO hang
```
no master : rc=0 out=hi err=err  secs=0.71   Popen pid/pgid/sid 18579/18579/18579
then      : ssh -O check uni -> Master running (pid=18582)
            ps: 18582 ppid=2103 pgid=18582 sid=18582  "ssh: /home/laptop/.ssh/cm-anton@100.126.48.99:22 [mux]"
master up : secs=0.13, 0.11
repeat no-master x4: 0.71 0.73 0.73 0.69  (never a timeout)
```
The persist daemon is a **different process** from the app's ssh (18582 vs 18579), in its **own session and process group**, reparented (ppid 2103,
the user's systemd), so it is outside the Popen group. Its fds 0/1/2 are `/dev/null` (`ls -l /proc/<master>/fd`), so it does **not** hold the caller's
stdout/stderr -> EOF arrives when the session ends. The master *does* hold the caller's pipes while a session runs: during a `sleep 5` session
the master's fd list had two extra `pipe:` fds (7, 8) in addition to `/dev/null`x3 and sockets; they are released when the session ends (mux client passes its stdio to the master).

### B3. Mitigations (not needed; measured for reference, no master up)
`-o ControlPersist=no`: 0.76 s, no master left. `-o ControlMaster=no`: 0.71 s, no master left (the same ~0.7 s as the default, i.e. the cost is the connection setup, not the mux).

### B4. Kill test (no master up, long session as would-be master)
Script `sleep 8; touch marker.$$; echo late`, `os.killpg` of the Popen group 2 s after start:
```
default config: communicate returned rc=-9, secs=8.12 (an earlier run with `sleep 30`: 30.77 s), out=b'' err=b''
                ssh -O check uni -> Master running (pid=18813)   (master SURVIVED; a later call took 0.11 s)
                marker.<pid> exists on uni  (remote command kept running after the kill)
-o ControlMaster=no -o ControlPath=none: rc=-9, secs=2.0; marker still created on uni afterwards (remote command survives the client kill here too)
```
- `killpg` kills only the **client** ssh. With mux, the master (own group) survives, keeps the session open, and **still holds the pipes -> `communicate()`
  returned only when the remote command finished (8.12 s / 30.77 s), not at the kill.** A runner that, after killpg, blocks on pipe EOF is
  therefore blocked for the remote command's remaining life; one that gives up on the pipes after kill returns at once. Without the mux the EOF is immediate (2.0 s).
- **In both cases the remote command is NOT killed** (marker written after the kill; bash -s had no tty). A timeout/kill on the laptop does not stop remote work.

### B5. rsync through the same runner (`rsync -a --mkpath -e 'ssh -o BatchMode=yes -o ConnectTimeout=10' <dir>/ uni:<root>/rs/`, stdin=b"")
No master: rc=0, 0.95 s, master left running (18961), file arrived (`hello`). Master up: rc=0, 0.28 s. No master with
`ControlPersist=no`: 0.89 s; `ControlMaster=no`: 0.93 s. No hang in any case.

### Facts (Part B)
1. With no master, the app's ssh forking the persist daemon does NOT hang the runner: EOF on both pipes, 0.7 s; the daemon has /dev/null stdio and its own session/group (reparented to systemd --user). (Closes verifier M7 for the happy path.)
2. `killpg` of the app's group does not take the master down; the next call works (0.11 s).
3. But after such a kill with the mux, the pipes stay open (held by the master for the session's duration) until the remote command ends: **never wait for EOF after a kill**.
4. The remote command keeps running after a client kill, with or without the mux (no tty, no SIGHUP effect on `sleep`/`touch` chain): cancellation of remote work must be done remotely.
5. rsync via `-e 'ssh ...'` behaves the same (0.95 s cold, 0.28 s warm).
6. `ControlPersist=no` / `ControlMaster=no` are unnecessary for correctness; they cost nothing in latency when no master is up but lose the 0.1 s warm path.

### Not measured (Part B)
A master left up by *another* app call dying mid-session; SIGTERM instead of SIGKILL to the group; `ServerAlive*` expiry on a dead link; ssh clients other than OpenSSH of this laptop; stderr noise ("Control socket connect") when a stale socket exists; a master launched with a tty on stdout.

## Probe 5.3c — single-call submit: lock, daemon fd inheritance, long rows, socket ownership (2026-10-05)

Host `uni` (account `anton`), util-linux `flock` 2.39.3, tsp 1.0.1, GNU coreutils. Everything under `/home/anton/.orcastudio/probe-5.3c/` (removed afterwards), own `TS_SOCKET`s, stub wrapper/ORCA (`sleep`). Scratch scripts: `/tmp/claude-1000/-home-laptop-Documents-orca-studio/1250eb99-e05f-4197-a7c1-970038b46592/scratchpad/probe-5.3c/`. Cleanup verified: no `tsp`/`sleep`/`flock` of `anton` left, `$HOME/.orcastudio-submit.lock` never created.

### C1. flock across ssh sessions
```
A: ssh uni 'flock -w 30 <lock> sleep 20'      (held)
B: flock -w 3 <lock> true   -> rc=1 after 3.005 s        (timeout = rc 1, and waits the full -w)
B: flock -n <lock> true     -> rc=1                      (immediate)
```
- **SIGKILL of session A's local ssh client at t0 does NOT release the lock**: the remote `flock ... sleep 20` (pid 416285 -> `sleep` 416286) kept running (seen with `pgrep` right after the kill), and B's polling `flock -n` first succeeded ~20 s after A started (17 s after the poll began, ~3 s after the kill), i.e. exactly when the remote `sleep` ended. Lock lifetime = lifetime of the remote process holding the fd, not of the ssh session (same as 5.3b fact 4).
- `bash -s` form: `exec 9><lock; flock -w 3 9; ...` in A (sleep 8): A got rc=0; B concurrently `exec 9>lock; flock -w 3 9` -> rc=1 after 3.0055 s; after A's script exited, a new session got rc=0. The fd-9 lock is released on script exit.

### C2. Lock fd inheritance by a fresh tsp daemon (no daemon on the socket before)
After the script exited (daemon forked inside it), `flock -n <lock> true` and `ls -l /proc/<pid>/fd` of the tsp processes and the running `sleep 60`:

| form | `flock -n` after script exit | lock fd in daemon / runner / task |
|---|---|---|
| (a) `flock <lock> -c 'tsp sleep 60'` (no `-o`) | **rc=1 (still locked)** | **fd 3 -> lock (read-only) in both tsp processes AND in `sleep 60`** |
| (b) `flock -o <lock> -c '...'` | rc=0 (released) | no lock fd anywhere |
| (c) `exec 9>lock; flock 9; tsp sleep 60 9>&-` | rc=0 (released) | no lock fd anywhere |
| (d) `exec 9>lock; flock 9; tsp sleep 60` (control, no `9>&-`) | **rc=1 (still locked)** | **fd 9 -> lock (write) in both tsp processes and `sleep 60`** |

Lock became free again only after `tsp -K` + `pkill sleep 60` (rc=0). **A fresh daemon inherits the lock fd and keeps the lock for the life of the daemon and of every task it runs; `flock -o` and `9>&-` (on the command that may start the daemon) both prevent it.** (The tsp daemon forks from the first client, and the runner/task inherit from it.)

### C3. Long tsp rows
Root = `/home/anton/.orcastudio/probe-5.3c/pad1234567890/pad1234567890/pad123456789` (75 bytes), socket `<root>/tsp/slot0.sock` = 90 bytes, id = 36-char UUID-shaped, argv `bash <root>/bin/wrapper-<64hex>.sh <root>/jobs/<id> 0-3 <root>/bin/stuborca`, `TMPDIR=<job>/.tsp-out` (dir made). 3 tasks, sleep 25 each, 1 slot:
```
header   len=78   ID   State      Output               E-Level  Times(r/u/s)   Command [run=1/1]
running  len=552  0 running <job>/.tsp-out/ts-out.21fCPs <25 spaces> bash <root>/bin/wrapper-<64hex>.sh <job> 0-3 <orca>
queued   len=432  1 queued  (file) <38 spaces>            bash ...same argv...
queued   len=432  2 queued  (file) ...
```
- **Nothing truncated at 552 chars** (running), 432 (queued): the full argv, with the mask and the orca path intact. Identical bytes (`cmp`) with and without `COLUMNS=60`, and under a real pty (`script -qec "stty cols 60; COLUMNS=60 tsp -l"`: same lengths 78/552/432/432). Extends P4 (256 chars) to ≥ 552.
- Output column: **queued** rows show the literal `(file)` followed by padding (the `TMPDIR` path is NOT shown yet); **running** and **finished** rows show `<job>/.tsp-out/ts-out.XXXXXX` (with `TMPDIR` as set at enqueue; the file is created when the task starts, 0 bytes, mode 0600). Output path is also the 3rd token only for running/finished.
- Finished row: `<id> finished <out> <E-level> <r/u/s> bash ...` (len 265 with the root elided to `<R>`); the extra Times column does not move the command, so counting from the command side works: token `wrapper-<64hex>.sh` (matched as `/wrapper-[0-9a-f]+\.sh$`), then **job dir, then mask `0-3`, then orca path — the token two after the wrapper path is the mask in running, queued and finished rows alike** (checked with awk on each state). Row order running -> queued -> finished as in P4; here after 25 s: `2 running`, `0 finished`, `1 finished` (running first, finished in id order).
- Parse hint: anchor on the command column from the right (`... wrapper-<hex>.sh <job> <mask> <orca>`), not on column index, since the Output column is empty-padded variably.

### C4. /proc/net/unix and ownership
```
0000000000000000: 00000002 00000000 00010000 0001 01 4887557 <root>/tsp/slot0.sock
header: Num       RefCount Protocol Flags    Type St Inode Path
```
- The 90-byte path appears **verbatim** (awk `$NF==path` -> 1 match); **no uid column** (columns: Num RefCount Protocol Flags Type St Inode Path). So ownership must come from `stat`/`-O`.
- `[[ -O <my socket> ]]` true; `stat -c '%U %F %a'` -> `anton socket 775`.
- Foreign sockets (not connected to): `/run/dbus/system_bus_socket` (root) and `/run/systemd/journal/socket` (root): `-O` false for both; `/run/user/1000/bus`: `stat` itself fails with Permission denied, `-O` false. After `tsp -K` the socket file is gone and its line gone from `/proc/net/unix` (count 0).

### C5. `ln -T` claim
```
ln -sT x .submitting  -> rc=0
ln -sT x .submitting  -> rc=1  "ln: failed to create symbolic link '.submitting': File exists"
ln -sT y .submitting  -> rc=1  same message
.submitting -> x  (dangling, [[ -e ]] false, [[ -L ]] true)
rm .submitting; ln -sT x .submitting -> rc=0 (re-claim works)
file form (the wrapper's .started shape): printf a >.t1; ln -T .t1 .started -> 0; ln -T .t2 .started -> 1 "failed to create hard link '.started': File exists"
```
The second claim fails even though the first link is dangling. After `rm` the claim can be re-taken. (Same atomic `linkat` behaviour as the wrapper's `publish_started`, coreutils on uni.)

### C6. stat / realpath shape
```
<root>/jobs/<id>      stat %s=4096  directory  realpath == path itself
<root>/jobs/<id>/..   stat %s=4096  directory  realpath == <root>/jobs   (the literal string is not equal; only realpath normalises it)
<root>/jobs, <root>   4096 directory, realpath == path
a symlink <p>/lnk -> <root>/jobs: realpath(<p>/lnk/<id>) = <root>/jobs/<id>  (symlinks are resolved)
```
`%s` of a directory is the ext4 block size (4096), not a content size, so it is not a usable fact beyond "is a directory". `[[ $(realpath <job>/..) == <root>/jobs ]]` holds; a job dir reached through a symlink resolves to a different path than the one given, so an equality assert on `realpath <job> == <job>` detects it.

### Facts (5.3c)
1. `flock -w N` times out with rc=1 after N s; `-n` rc=1 immediately. The lock lives as long as the remote holder process; killing the laptop ssh client does not release it (released when the remote command ends).
2. `exec 9>lock; flock -w 3 9` in `bash -s` works across sessions and releases at script exit.
3. **Without `flock -o` / `9>&-`, a fresh tsp daemon and every task inherit the lock fd and hold the lock for their lifetime (measured rc=1 after the script exit, fd listed in `/proc/<pid>/fd`). `flock -o` and `9>&-` each prevent it (rc=0, no fd).**
4. Long `tsp -l` rows (432-552 chars, root 75, id 36) are intact, tty or not, COLUMNS or not; the mask is always the token after the job dir; queued rows show `(file)` in the Output column.
5. The socket path is verbatim in `/proc/net/unix`; no uid column; ownership is checked with `[[ -O ]]` (true for ours, false for root-owned sockets).
6. `ln -T` second claim -> rc=1 "File exists", re-claim after `rm` -> rc=0.

### Not measured (5.3c)
A kill of the lock holder with SIGKILL on the server (as opposed to the client); `flock` on NFS/other filesystems (home is local ext4 here); a daemon forked by the 2nd call of a submit while a daemon already exists (only the fresh-daemon case was asked); `tsp -l` with a command containing spaces or quotes; lines beyond 552 chars.
