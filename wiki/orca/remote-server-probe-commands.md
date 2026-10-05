# Remote server probe commands — measured stdout formats

**Purpose:** Unit 5.1 connection-test runs shell commands on the remote server and parses their
stdout to establish the server's specs. This page records the **exact output shapes** measured on
the dev laptop (Linux Mint, same distro family as the target university server) so the Part A
parser targets the real format, not an assumed one. Domain rule #10 — every fact from a run.

**Measured:** 2026-08-27 on the dev laptop (Intel i5-12500H, 16 logical CPUs, Ubuntu/Mint,
ORCA 6.1.0 at `/opt/orca/orca`, OpenMPI 4.1.6).

---

## 1. ORCA path resolution

### Claim to settle
Can `which orca` or `command -v orca` locate ORCA, and what does the `--version` banner look like?

### Commands and verbatim output

```
$ which orca
/opt/orca/orca
which_exit=0

$ command -v orca
/opt/orca/orca
exit=0
```

Both `which` and `command -v` found ORCA on this machine because `/opt/orca` is on `$PATH`.
This is **not guaranteed on all servers** — domain rule #1 says ORCA is always invoked by its
absolute path. The correct resolution strategy for the connection-test is:

1. Accept the user-configured absolute path from the `ServerProfile`.
2. Check it directly: `test -x <path> && echo ok` (exit 0 = exists + executable).
3. Do NOT rely on `which`/`command -v`; they return empty stdout + exit 1 when ORCA is absent
   from `$PATH` (the common case on a university cluster where the module system or a private
   install puts ORCA in a non-`$PATH` location).

```
$ ls -la /opt/orca/orca
-rwxrwxr-x 1 root root 43453616 Jun 12  2025 /opt/orca/orca
ls_exit=0
```

```
$ /opt/orca/orca --version 2>&1 | grep 'Program Version'
                         Program Version 6.1.0  -  RELEASE   -
```

**The `--version` flag is NOT a proper flag** — ORCA tries to open a file named `--version` and
fails, but it still prints its banner to stdout before the error. The banner includes the version
line. ~~Exit code is 0.~~ **Corrected 2026-10-03:** ORCA itself exits **2** on `--version`, measured on
the laptop (6.1.1) and on uni (6.1.1). The "0" above was most likely the exit status of the `| grep`
pipeline, not of ORCA. Never use the rc as the success signal; parse the `Program Version` line. The
version line format is:

```
                         Program Version <MAJOR>.<MINOR>.<PATCH>  -  RELEASE   -
```

Leading whitespace is significant (heavy indent). The version token is at word position 3 (0-based)
after stripping whitespace: `["Program", "Version", "6.1.0", "-", "RELEASE", "-"]`.

### Parser must expect
- **Primary strategy:** `test -x <configured-path> && echo ok` — exit 0 means executable exists. *(superseded 2026-10-03: see "Connection-test formats on uni" and "Live connection test on uni" below — ORCA exits 2 on `--version`, so success is a `Program Version` line, never the rc. The connection test runs `test -x` first and does not run ORCA when it fails, so a missing or non-executable path is reported as a `test -x` failure; rc 127/126 classify ORCA only when it is actually executed)*
- **Version extraction:** run `<configured-path> --version 2>&1`, grep for the line containing
  `Program Version`, extract the third whitespace-delimited token (e.g. `6.1.0`).
- **Empty/not-found case:** if `test -x` exits non-zero, report "ORCA not found at `<path>`" and
  abort the connection-test. Do not fall back to `which`.
- Version regex: `Program Version\s+(\d+\.\d+\.\d+)` (after stripping leading whitespace).

---

## 2. OpenMPI version

### Claim to settle
Which command/line/token carries the OpenMPI version, for domain rule #2 (version must exactly
match the ORCA build)?

### Commands and verbatim output

```
$ ompi_info --version
Open MPI v4.1.6

http://www.open-mpi.org/community/help/
ompi_version_exit=0
```

Three lines total: the version string, a blank line, and a URL. The version is on **line 1**,
format `Open MPI v<MAJOR>.<MINOR>.<PATCH>`.

```
$ ompi_info | head -5
                 Package: Debian OpenMPI
                Open MPI: 4.1.6
  Open MPI repo revision: v4.1.6
   Open MPI release date: Sep 30, 2023
                Open RTE: 4.1.6
```

In the full `ompi_info` banner (no flags), the version appears on the second line as a key-value
pair: `                Open MPI: 4.1.6` — a label padded to column 22, then `: `, then the bare
version (no `v` prefix here).

```
$ mpirun --version
mpirun (Open MPI) 4.1.6

Report bugs to http://www.open-mpi.org/community/help/
mpirun_exit=0
```

`mpirun --version` emits: `mpirun (Open MPI) <version>` on line 1. Same three-line structure as
`ompi_info --version`.

### Parser must expect

**Preferred command:** `ompi_info --version` (shortest, most structured output, exit 0 when
OpenMPI is installed).

- **stdout line 1:** `Open MPI v<version>` — extract with regex `Open MPI v(\d+\.\d+\.\d+)`.
- **Exit 0** on success; **non-zero or command-not-found** means OpenMPI is absent.
- **Fallback:** `mpirun --version` — line 1: `mpirun (Open MPI) <version>`, regex
  `Open MPI\) (\d+\.\d+\.\d+)`.
- **Not-found case** *(superseded 2026-10-03 by ADR-024 n item 9: OpenMPI is recorded, not gating; an absent version is stored as NULL)*: if both fail, report "OpenMPI not found" — the profile must not be used
  until this is resolved (domain rule #2).
- Both commands return only 3 lines; no need to `head`-limit, but doing so is harmless.

---

## 3. CPU / core count

### Claim to settle
What does `nproc` return, and how does it relate to physical cores vs logical threads?

### Commands and verbatim output

```
$ nproc
16
nproc_exit=0

$ nproc --all
16
nproc_exit=0
```

`nproc` returns a **single integer on a single line**, no trailing whitespace, no label, exit 0.
On this machine `nproc` and `nproc --all` are identical (16) because no cores are offline.

```
$ lscpu | grep -iE '^CPU\(s\)|Thread|Core|Socket|Model name'
CPU(s):                                  16
Model name:                              12th Gen Intel(R) Core(TM) i5-12500H
Thread(s) per core:                      2
Core(s) per socket:                      12
Socket(s):                               1
```

Derived topology: 1 socket × 12 physical cores × 2 threads = 24 logical CPUs? No — `nproc`
returns 16. The i5-12500H has a **hybrid architecture** (P-cores with HT + E-cores without HT):
4 P-cores × 2 threads = 8 + 8 E-cores × 1 thread = 8 → 16 logical total. `lscpu`'s
"Core(s) per socket: 12" counts P+E; "Thread(s) per core: 2" is only accurate for P-cores.
The **`nproc` value (16) is authoritative** for the scheduler-visible logical CPU count.

For the taskset mask stored in `ServerProfile` (domain rule #8), the connection-test should
record `nproc` as the available logical core count. The actual mask is then derived from the
performance probe (see `wiki/orca/performance.md`); the connection-test only establishes the
**ceiling**.

### Parser must expect

- **Command:** `nproc`
- **stdout:** a single line containing exactly one non-negative integer, e.g. `16`, followed by
  `\n`. No label, no units, no extra whitespace.
- **Exit 0** on success; non-zero means `nproc` is absent (extremely unlikely on any Linux).
- Parse with: trim whitespace, parse as `u32`. Refuse and report if not a valid positive integer.
- **`nproc --all`** vs plain `nproc`: on a server with offline CPUs these may differ. Use plain
  `nproc` (scheduler-visible count = what ORCA will actually see).

---

## Summary: three commands for the connection-test

| Fact | Command | Parse target | Not-found behaviour |
|---|---|---|---|
| ORCA executable present + version | `test -x <path> && <path> --version 2>&1` | Line matching `Program Version`, token 3 | Exit non-zero from `test -x` → abort *(superseded 2026-10-03: see "Connection-test formats on uni" and "Live connection test on uni" below — ORCA exits 2 on `--version`, so success is a `Program Version` line, never the rc. The connection test runs `test -x` first and does not run ORCA when it fails, so a missing or non-executable path is reported as a `test -x` failure; rc 127/126 classify ORCA only when it is actually executed)* |
| OpenMPI version | `ompi_info --version` | Line 1: `Open MPI v(\d+\.\d+\.\d+)` | Command not found / non-zero → report missing |
| Logical CPU count | `nproc` | Entire stdout trimmed, parsed as `u32` | Non-zero exit → report missing |

~~All three commands exit 0 on success~~ (ORCA `--version` exits 2; see the uni section below) and produce compact, line-oriented output. The parser must
*(superseded 2026-10-03 by ADR-024 n items 8–9: only the item-8 checks gate; OpenMPI does not)* treat any non-zero exit or unexpected stdout shape as a hard error that blocks the profile from
being usable (rule #9 post-condition).

---

## Cross-references

- ADR-023: `wiki/architecture/adr-023-server-agnostic-remote-execution.md` — the `ServerProfile`
  design and the requirement for a connection-test.
- `wiki/orca/performance.md` — the taskset mask probe (rule #8), which uses the `nproc` ceiling
  established here.
- `wiki/orca/orca-basics.md` — rule #1 (always invoke ORCA by full absolute path).


## Connection-test formats on uni (`anton`, 2026-10-03, for unit 5.1 Part B)

Measured over `ssh uni 'bash -s' < script` with `LC_ALL=C`, in a non-tty session, exactly as the app
will run the connection test. Host: Ubuntu, systemd 255.4, ORCA 6.1.1.

| Check | Command | Output, rc | Rule for the test |
|---|---|---|---|
| KillUserProcesses | `busctl get-property org.freedesktop.login1 /org/freedesktop/login1 org.freedesktop.login1.Manager KillUserProcesses` | `b false`, rc 0. A bad property gives `Failed to get property … Unknown interface … or property …` on stderr, rc 1. | Accept only exactly `b false` / `b true` with rc 0. Anything else is **undetermined**, which blocks the profile. `loginctl show -p …` does **not** exist on systemd 255 (`Unknown command verb 'show'`). |
| sudo group | `id -nG` | `anton users`, rc 0 (`id -Gn` and `groups` give the same) | Split on whitespace and look for the exact token `sudo` (warning only). `admin`/`wheel` were not measured. |
| local FS of the job root | `findmnt -no FSTYPE,SOURCE,TARGET --target <root>` | `ext4   /dev/sdb4 /home`, rc 0; a missing path → empty stdout, rc 1 | The first token is the fs type; allow-list local types (`ext4` measured). `stat -f -c %T` names ext4 **`ext2/ext3`** (and `tmpfs` for `/dev/shm`), so prefer `findmnt`. How nfs/cifs are named was not measured. |
| ORCA version | `<path> --version </dev/null 2>&1` (capture it whole, do not pipe into `head`, which gave rc 141 from SIGPIPE) | 183 lines; line 54 is `                         Program Version 6.1.1  -  RELEASE   -`; the stderr tail is `Cannot open input file: --version`; **rc 2** | Success = a `Program Version x.y.z` line, whatever the rc. `</dev/null` keeps ORCA off the script's stdin. |
| ORCA not runnable | missing path / mode-644 file / directory | rc **127** `…: No such file or directory`; rc **126** `…: Permission denied`; rc 126 `…: Is a directory`. The message carries a `line N:` prefix. | Classify by rc (127 = missing, 126 = not executable), never by the whole message. `test -x` cannot tell missing from non-executable (rc 1 for both). |
| OpenMPI | `ompi_info --version` | `Open MPI v4.1.6` / blank line / URL, rc 0 | As already recorded on this page. |
| cores | `nproc` | `48`, rc 0 | As already recorded on this page. |

**Timing.** The whole set takes ≈0.17 s on the server, and ≈1.4 s wall time from the laptop over
an existing ControlMaster. The cold-connect cost was not measured, so "a 10 s timeout is enough" is
**inference**. *(Measured 2026-10-03 in the live connection test below: 1.1 s cold, ≈0.3 s warm.)*


## 5.1 Part B probes — script plus data on one stdin; mpirun (2026-10-03, probe 5.1c)

On both the laptop and uni (bash 5.2.21), in a non-tty `bash -s` run.

**One stdin for the script and its values.** `ssh uni 'bash -s' < stream`, where the stream is the
script text followed by `printf '%s\0'` of the values.
- bash does **not** read ahead into the trailing data. But it reads the script one command at a time,
  so a `read` loop in the script consumes any **later script lines** as data. Attempt 1 (with
  `printf` lines after the loop) printed nothing, rc 0, both locally and on uni.
- **Working shape**, verified by local file redirect, local pipe and over ssh: the last line is
  `args=(); while IFS= read -r -d '' a; do args+=("$a"); done; show; exit`. It produced
  `5 / /plain/path / with\ space / '' / $'line1\nline2' / it\'s\ \$HOME\;\ rm\ -x`: the empty
  value and the embedded newline survived, and nothing was expanded.
- **Child commands swallow stdin.** After `cat >/dev/null` the later script line never ran, and a
  following NUL list was read as `0` values. With `cat </dev/null`, both worked. So every child
  command needs `</dev/null`.
- A large (MB) NUL list through sshd was not tested.

**OpenMPI in the same context.**

| | uni | laptop |
|---|---|---|
| `command -v mpirun` → `readlink -f` | `/usr/bin/mpirun` → `/usr/bin/orterun` | same |
| `mpirun --version` | `mpirun (Open MPI) 4.1.6`, rc 0 | same |
| `ompi_info --version` | `Open MPI v4.1.6`, rc 0 | same |
| package (`dpkg -S`) | `openmpi-bin` for both | same |

- The non-tty ssh PATH on uni is the stock system PATH, without `/opt/orca`, so the ORCA path must be
  absolute (rule #1).
- `strings /opt/orca/orca` contains a bare `mpirun` and no absolute path. So ORCA most likely execs
  `mpirun` from PATH — **inference from strings, not confirmed by a run**.
- No other Open MPI was found (`/opt/openmpi*`, `/usr/local/bin/mpirun*` are absent).


## Live connection test on uni (2026-10-03, unit 5.1 Part B)

The app's own command path (`live_uni_connection_test`, a throwaway database), run from the laptop
as `anton` over the `uni` alias with values `/opt/orca/orca`, `/home/anton/.orcastudio`, `0-3`.
OpenSSH 9.6p1 on the laptop. The uni ssh config has `ControlMaster auto`, `ControlPersist 10m`; no
master was running before the cold run.

**argv**, exactly: `ssh -o BatchMode=yes -o ConnectTimeout=10 -- uni bash -s`. ssh's stderr was
empty.

**Raw record stream** (verbatim; the 12 742-byte ORCA banner is elided to its version line and its
tail):

```text
orcastudio-conntest 1
argc 3
arg 14
/opt/orca/orca
arg 23
/home/anton/.orcastudio
arg 3
0-3
mkdir 0
out 0

err 0

realpath 0
out 24
/home/anton/.orcastudio

err 0

findmnt 0
out 5
ext4

err 0

busctl 0
out 8
b false

err 0

id 0
out 12
anton users

err 0

nproc 0
out 3
48

err 0

orca_x 0
out 0

err 0

orca 2
out 12742
[…]
                         Program Version 6.1.1  -  RELEASE   -
[…]
[file orca_main/run.cpp, line 393]: Cannot open input file: --version


err 0

ompi 0
out 57
Open MPI v4.1.6

http://www.open-mpi.org/community/help/

err 0

end
```

Verdict: `FullPass { orca_version: "6.1.1", openmpi_version: Some("4.1.6"), core_count: 48 }`, no
warnings.

What this settles (verifier F1, previously seen only on the laptop):
- **`findmnt -no FSTYPE --target <root>`** prints **one column**, `ext4\n`, rc 0, on uni too.
- **`realpath -e -- <root>`** prints the root itself, `/home/anton/.orcastudio\n`, rc 0 (the root is
  not behind a symlink).
- `mkdir -p` of the existing root: rc 0, no output.

**A bogus ORCA path** (`/opt/orca/no-such-orca`): `orca_x` rc 1, `orca` skipped, so the verdict is
"ORCA is missing or not executable (test -x failed)". The rc 127 shape above is not reached for a
missing path, because `test -x` runs first.

**Wall time** (laptop clock; the raw run times ssh alone, the command runs time the whole
command including its database reads and writes):

| Run | ControlMaster | Time |
|---|---|---|
| raw run, cold | none (this run started one) | 1114 ms |
| command path, warm | reused | 353 ms |
| bogus ORCA, warm | reused | 285 ms |

ssh's `ControlPersist` master did **not** keep the runner's pipes open: the cold run returned as soon
as the client exited. The test closed the master it had started (`ssh -O exit uni`) afterwards.

**`--` before the host** (OpenSSH 9.6p1, `ssh -G`, no connection made):
`ssh -G '-oProxyCommand=echo PWNED' somehost` resolves `proxycommand echo PWNED`, so a host value
that starts with `-` is an option. `ssh -G -- '-oProxyCommand=echo PWNED' somehost` refuses it:
`hostname contains invalid characters`.
