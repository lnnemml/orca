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
line. Exit code is 0. The version line format is:

```
                         Program Version <MAJOR>.<MINOR>.<PATCH>  -  RELEASE   -
```

Leading whitespace is significant (heavy indent). The version token is at word position 3 (0-based)
after stripping whitespace: `["Program", "Version", "6.1.0", "-", "RELEASE", "-"]`.

### Parser must expect
- **Primary strategy:** `test -x <configured-path> && echo ok` — exit 0 means executable exists.
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
- **Not-found case:** if both fail, report "OpenMPI not found" — the profile must not be used
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
| ORCA executable present + version | `test -x <path> && <path> --version 2>&1` | Line matching `Program Version`, token 3 | Exit non-zero from `test -x` → abort |
| OpenMPI version | `ompi_info --version` | Line 1: `Open MPI v(\d+\.\d+\.\d+)` | Command not found / non-zero → report missing |
| Logical CPU count | `nproc` | Entire stdout trimmed, parsed as `u32` | Non-zero exit → report missing |

All three commands exit 0 on success and produce compact, line-oriented output. The parser must
treat any non-zero exit or unexpected stdout shape as a hard error that blocks the profile from
being usable (rule #9 post-condition).

---

## Cross-references

- ADR-023: `wiki/architecture/adr-023-server-agnostic-remote-execution.md` — the `ServerProfile`
  design and the requirement for a connection-test.
- `wiki/orca/performance.md` — the taskset mask probe (rule #8), which uses the `nproc` ceiling
  established here.
- `wiki/orca/orca-basics.md` — rule #1 (always invoke ORCA by full absolute path).
