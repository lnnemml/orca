# University compute server (`uni`)

The first remote execution target for OrcaStudio (Phase 5). Everything below is **measured
2026-10-02** during provisioning; anything not measured is in **Open items** as `unknown` /
`pending`, never guessed (rule #10). This page describes the host; the *decision* about how
OrcaStudio runs jobs on it under an intermittent link is [ADR-024](../architecture/adr-024-remote-execution-intermittent-connectivity.md).
The per-profile measured specs that make it a usable run target live in the `server_profiles`
row once the connection-test passes ([ADR-023](../architecture/adr-023-server-agnostic-remote-execution.md),
[server-profiles.md](../modules/server-profiles.md)).

> No network identifiers (IP addresses, host keys, fingerprints) are recorded in the wiki by
> policy. Reachability is via Tailscale; auth is via the user's SSH setup (ADR-005).

## Purpose & ownership

- A dedicated compute node for the author's ORCA mechanism studies. A full study is 300–800
  jobs — the dev laptop is a development machine, not a compute node (`orca/performance.md`).
- Hardware is an HP ProLiant, hostname `yats-ProLiant`.
- **Accounts (since 2026-10-03, ADR-024 k):**
  - **`anton`** — the **dedicated OrcaStudio account**, no sudo. Laptop alias **`uni`**. Everything
    OrcaStudio does on the server (tsp queues, job dirs) runs as `anton` under
    `/home/anton/.orcastudio/`.
  - **`yats`** — the **shared lab account**, with sudo. Laptop alias **`uni-admin`**, used **only for
    administration** (packages, `/opt/orca`, system settings), never by the app or by agents. The
    server is dedicated to Anton in practice; others ask before using it. Do not disrupt other
    users' files or installs.

## Hardware

- **CPU** (measured 2026-10-02): **2× Intel Xeon E5-2673 v3** (Haswell, 12c/24t each → 24
  physical / 48 logical, hyper-threading on), 2.4 GHz base / 3.1 GHz turbo. **AVX2, no AVX-512.**
  **2 NUMA nodes** — node0: cores 0–11,24–35; node1: 12–23,36–47. The NUMA split is the natural
  candidate for a 2×12 slot layout (one slot per socket/node), but that is **unmeasured** — see
  Open items + rule #8 (taskset masks are measured, not assumed).
- **RAM:** 62 GiB. **Swap:** 119 GiB. Swapping during a calculation is a **failure mode**, so
  ORCA `%maxcore` must stay conservative (see Operational DO/DON'T).
- **Disk** (measured 2026-10-02): `sda` and `sdb` **both rotational HDD**, 465.8 G each, **no
  redundancy** (not RAID).
  - **`sdb`** — system + `/home` (`/home` on `/dev/sdb4`, ~79 G free at provisioning). SMART
    **healthy** (0 reallocated, 0 pending sectors), ~29k power-on hours. ORCA scratch I/O hits a
    spinning disk — one isolated job dir per calculation (rule #3) stays the discipline.
    **`sdb` holds the single copy of all server-side results** (see the backup DO below).
  - **`sda`** — label `Samsung_OLD`: a **previously-failed disk the owner reinstalled as a spare**
    (not a data archive). 1 pending sector; **not auto-mounted since 2026-08-27**. **Known
    unreliable — not used for anything (no scratch, no data).**

## Software (OS, MPI, ORCA)

- **OS:** Ubuntu 24.04.1 LTS, kernel 6.8.0.
- **MPI:** system OpenMPI **4.1.6** at `/usr/bin/mpirun`. A conda base auto-activates in the
  login shell but does **not** shadow `mpirun`. `ldd` of `orca_scfgrad_mpi` resolves
  `libmpi.so.40` to the system lib.
- **ORCA (ours):** **6.1.1** in **`/opt/orca-6.1.1`**, with **`/opt/orca` → `/opt/orca-6.1.1`** as a
  symlink — the same `/opt/orca-<version>` + symlink scheme as the laptop (since 2026-10-03; see the
  check below). Copied from the lab install.
  **Self-contained** (measured 2026-10-02): `ldd` shows `liborca_*` → `/opt/orca/lib` and
  `libmpi.so.40` → the **system** lib (so MPI uses the system OpenMPI 4.1.6 — rule #2).
  - **Version parity CONFIRMED** (2026-10-02): a water **r2SCAN-3c Opt+Freq** benchmark on 4
    procs gives **E = −76.418938720745 Ha**, freqs **1653.28 / 3813.59 / 3932.72 cm⁻¹**, *ORCA
    TERMINATED NORMALLY* — **matches the laptop (ORCA 6.1.0) to reported precision**, and
    `sha256(/opt/orca/orca)` is identical on laptop and server (`335aef8b…c4ea`). **The laptop
    now also defaults to 6.1.1** (via its own `/opt/orca`→`/opt/orca-6.1.1` symlink), and this
    water benchmark is **bit-identical laptop-6.1.1 ↔ server-6.1.1** (same binary hash → same
    numbers). Parity is
    further backed by the **6.1.0↔6.1.1 parser cross-version regression** (10 checks bit-identical;
    commits `626c8bf`, `c92c592`; `src-tauri/src/parse/cross_version_6_1_1.rs`). **NEB-TS
    cross-version parity is NOT checked** (laptop-non-reproducible — see
    [orca/gotchas.md](../orca/gotchas.md)); it is deferred to this server (ADR-023/024). 6.1.1 is
    now the current default on both hosts — see
    [orca/orca-basics.md](../orca/orca-basics.md#orca-611-install-scheme--laptopserver-parity-2026-10-02).
- **Legacy lab ORCA installs:** `/home/yats/calc/orca/{303,504,601,611}` — **leave untouched**.
- Packages we installed: `openssh-server`, `tmux`, `rsync`, `tailscale`, `task-spooler`
  (**1.0.1+dfsg1-1**, `/usr/bin/tsp`; behaviour measured 2026-10-03 in
  [task-spooler-uni-probe.md](../architecture/task-spooler-uni-probe.md)).
- **OrcaStudio's server root** is **`/home/anton/.orcastudio/`** (tsp sockets in `tsp/`, e.g.
  `tsp/slot0.sock`; job dirs; the account check in `verify/`). Its tsp queues use dedicated sockets
  there, never the default `/tmp/socket-ts.<uid>`. **`/home/yats/.orcastudio/probe/`** contains the
  2026-10-03 probe artefacts from the `yats` era. It is **historical and no longer used**.
- **`/opt/orca` layout & permissions** (measured as `anton`, 2026-10-03, after the admin moved the
  install to the versioned scheme): `/opt/orca -> /opt/orca-6.1.1` (`lrwxrwxrwx root`);
  `/opt/orca-6.1.1` is `drwxr-xr-x yats` (made `a+rX`); `readlink -f /opt/orca/orca` →
  `/opt/orca-6.1.1/orca`; **0 files unreadable** by `anton` (through the symlink), **0
  world-writable**, and `/opt/orca-6.1.1` is not writable by `anton`. `ldd orca_scfgrad_mpi` still
  resolves `liborca_tools_6_1_1_mpi.so.6 → /opt/orca/lib/…` and the system `libmpi.so.40`, with no
  `not found`. The water benchmark in `verify/water-symlink/`, run with `HWLOC_COMPONENTS=-gl`, gives
  **−76.418938720745 Eh**, `ORCA TERMINATED NORMALLY`, `Program Version 6.1.1`, and an **empty
  stderr (0 bytes)**. (Earlier the same day, `/opt/orca` was a plain directory. The account checks
  below were made on that layout.)
- **Job root filesystem:** `/home` (and so `/home/anton/.orcastudio/`) is local **ext4** on
  `/dev/sdb4` (`findmnt`, 2026-10-03) — the local-FS premise of ADR-024's cancel-race argument.
- **Clock:** the server runs `Etc/UTC` and is **not NTP-synchronised**. `timedatectl` (2026-10-03)
  reports `System clock synchronized: no` and `NTP service: active`; the RTC also differs from the
  system time. The clock was **~2 min 52 s ahead** of the laptop (same offset in the morning probe and
  in the `anton` check). ADR-024 j: the app never compares laptop and server times.

## Access

- **openssh-server**, **key-only**. Hardening via a drop-in
  `/etc/ssh/sshd_config.d/00-hardening.conf` (`PasswordAuthentication no`, `PermitRootLogin no`).
  A drop-in is used because sshd is first-match and `sshd_config.d` is read before the main file.
- **Reachability via Tailscale.** Laptop SSH alias `uni` configured with ControlMaster /
  ControlPersist 10m and ServerAliveInterval 30 (the persistent-connection substrate ADR-024
  relies on for detach/reconnect). `uni` logs in as `anton`. The separate `uni-admin` alias
  (`yats`, no ControlMaster) is for administration only.
- Auth stays entirely with the user's SSH config (ADR-005); the app stores no credentials
  (ADR-023).

## Connectivity constraints

- The university **cuts internet outside 08:00–22:00 Europe/Kyiv**: no remote access at night,
  but **computation continues** on the server. This is the core premise of ADR-024.
- The server is behind a lab router on a private /24 with DHCP — irrelevant for reach thanks to
  Tailscale.
- **Power:** on a UPS good for a few minutes. Whether the UPS signals the server (clean
  shutdown) is **unknown**.

- **Detachment works** (measured 2026-10-02): a detached `tmux` session **survives ssh logout** —
  the OS-level substrate ADR-024 (b) relies on for jobs that outlive the connection. (ADR-024's MVP
  queue is `task-spooler`, not tmux-per-job — this only confirms detached processes outlive the
  link, it does not reopen the rejected tmux-per-job alternative.)

- **task-spooler jobs survive logout** (measured 2026-10-03): a job enqueued over a one-shot ssh kept
  running after the ControlMaster was closed. This works because logind's `KillUserProcesses=false`
  leaves the ssh session scope `abandoned` instead of killing it (`Linger=no`). The `yats` desktop
  session `c1` is permanently logged in. Details, plus cancel and per-slot core masks, are in
  [task-spooler-uni-probe.md](../architecture/task-spooler-uni-probe.md).

These two facts — a nightly link cutoff and a short UPS window — are why the queue and the
source of truth live **on the server**, not the laptop (ADR-024).

## Dedicated account `anton` — measured 2026-10-03

Every check was run as `anton` over the `uni` alias (Part A of the account switch):

| Check | Result |
|---|---|
| `whoami; id; groups` | `anton`, `uid=1001 gid=1001 groups=anton,users` — **no `sudo`** |
| `touch /home/yats/.orcastudio_write_test` (negative control) | `Permission denied`, rc 1 — file not created |
| `rm /home/yats/.bashrc` (no `-f`, negative control) | `cannot remove … Permission denied`, rc 1 |
| `touch` inside `/home/yats/.orcastudio/probe/` | `Permission denied` |
| `ls /home/yats/calc` | `Permission denied` — `/home/yats` is `drwxr-x---`, so `anton` cannot even **read** it (stricter than required) |
| `ls -l /opt/orca/orca` | `-rwxrwxr-x yats yats … /opt/orca/orca` |
| `ldd /opt/orca/orca_scfgrad_mpi` | `liborca_tools_6_1_1_mpi.so.6 → /opt/orca/lib/…`, `libmpi.so.40 → /lib/x86_64-linux-gnu/libmpi.so.40`, no `not found` |
| water r2SCAN-3c Opt+Freq, 4 procs, `%maxcore 2000`, `/opt/orca/orca`, `taskset -c 0-3` | **E = −76.418938720745 Eh** (bit-identical to the reference), freqs 1653.28 / 3813.59 cm⁻¹, `ORCA TERMINATED NORMALLY`, 25.1 s |
| `which tsp`; one job on `TS_SOCKET=/home/anton/.orcastudio/tsp/slot0.sock`; `tsp -K` | `/usr/bin/tsp`; job ran as `anton` in `user-1001.slice/session-N.scope`; after `-K` the socket is gone, no tsp processes are left, and no default `/tmp/socket-ts.*` was created |
| `loginctl show-user anton -p Linger` | `Linger=no` |
| `KillUserProcesses` | `#KillUserProcesses=no` in `logind.conf`, no drop-ins; effective value (`busctl`) **`false`** |

- **hwloc X11 noise under `anton`.** Every ORCA run writes **310 lines** of
  `Authorization required, but no authorization protocol specified` to stderr, even though `DISPLAY`
  is unset. With **`HWLOC_COMPONENTS=-gl`**, stderr is **empty** and the energy is unchanged
  (−76.418938720745). Our reading: hwloc's GL plugin probes the X display `:0`, which belongs to
  `yats`'s desktop session; under `yats` the noise was absent. The results are unaffected; it only
  floods `stderr.log`. **The wrapper therefore exports `HWLOC_COMPONENTS=-gl`** (ADR-024 Decision b,
  amended 2026-10-03, d′ resolution).
- `anton`'s `systemd --user` instance starts `pipewire`/`wireplumber` on login (socket activation).
  These are not OrcaStudio processes.

## Operational DO / DON'T

**DO**
- Run everything OrcaStudio does as **`anton`** (alias `uni`); use **`uni-admin`** (`yats`, sudo) only
  for administration.
- Invoke ORCA by **absolute path** `/opt/orca/orca` (rule #1). `yats`'s `~/.bashrc` has stale
  ORCA `PATH` entries — never rely on `PATH`.
- Install packages with `apt-get install <pkg>` **directly**.
- Keep `%maxcore` conservative given the swap-is-failure rule; measure before trusting a value.
  A manual input with **no** `%maxcore` inherits ORCA's default **4000 MB/proc** (since 6.1.0),
  so 24 procs ≈ 96 GB > RAM — see [orca/gotchas.md](../orca/gotchas.md) (and ADR-024 Decision h's
  preflight).
- Locate ORCA installs via a real binary, e.g. `orca_scfgrad` (ORCA 6 has **no** `orca_scf`
  binary, and Ubuntu's `orca` screen-reader shares the binary name).
- **Pull results down to the laptop — the server is NOT a backup.** `sdb` is a single aging HDD
  with no redundancy and holds the only copy of server-side results; its sibling `sda` is a
  known-unreliable spare, not a fallback. So fetching results to the laptop (ADR-024
  `fetch_results` / rsync-down) is **backup by design**, not just convenience.

**DON'T**
- **Never** use or mount **`sda`** — a known-unreliable spare (1 pending sector, not auto-mounted;
  see Disk). No scratch, no data there.
- **Never** run `do-release-upgrade` — it would replace OpenMPI and break rule #2 (MPI must
  match the ORCA build).
- **Don't** `apt-get update`-gate installs: it exits non-zero because of third-party repos owned
  by others (winehq `NO_PUBKEY`, qtisas OBS expired key). Do **not** modify those repos.
- **Don't** touch the legacy `/home/yats/calc/orca/{303,504,601,611}` installs.
- **Don't** assume ORCA is parallelizing — confirm MPI resolves inside `/opt/orca` (rule #1/#2).

**Resolved 2026-10-02:** ORCA 6.1.1 install (copy done), self-contained `ldd` (libs inside
`/opt/orca`, MPI = system), the water r2SCAN-3c Opt+Freq benchmark, and 6.1.1↔6.1.0 parity — all
confirmed above. tmux detachment confirmed. CPU model + NUMA topology and disk type/layout measured.
`sda` identified (owner's reinstalled spare, known unreliable — do not use); `sdb` SMART healthy.

## Open items (pending / unknown)

- **UPS → clean shutdown:** does the UPS signal the server? (`lsusb` / `nut` pending.)
- **Exact nightly cutoff window in UTC** — to confirm (stated as 08:00–22:00 Europe/Kyiv).
- **Tailscale ACL** (laptop → server only) pending; confirm node-key expiry disabled.
- **Parallel-slot layout + OpenMPI binding for concurrent runs** — whether to run **1×24** or
  **2×12** (one slot per NUMA node) are **to measure** (rule #8/#10). The **binding** part is
  measured (2026-10-03): two tsp queues with `taskset` masks `0-11` / `12-23` and
  `binding_policy=none` keep every ORCA/MPI thread inside their own mask
  ([task-spooler-uni-probe.md](../architecture/task-spooler-uni-probe.md)). **Throughput** is
  still unmeasured. Until it is, the profile runs **1 slot** (ADR-024 Decision f).
- **tsp across a server restart** (ADR-024 Open question b) — **covered by simulation** in the
  implementation unit (`tsp -K` + a substituted `boot_id`). A real restart is recorded at the first
  natural occasion and is not triggered on purpose (an unattended ProLiant may stop at POST).
- **Clock not NTP-synchronised** (~2 min 52 s ahead of the laptop, 2026-10-03; `NTP service: active`
  but not synchronised) — fixing it needs `uni-admin`. The app never compares cross-host times
  (ADR-024 j), so this is hygiene, not a blocker.
- **`KillUserProcesses=false` is load-bearing** for detached jobs — if the host config ever changes
  it to `yes`, jobs die at logout. The ADR-023 connection test checks it as a mandatory precondition
  (ADR-024 Consequences).
- **`SshBackend` connection-test specs** (remote ORCA path resolution, OpenMPI version, `nproc`)
  are UNDETERMINED until the Phase 5 connection-test runs (ADR-023, `server-profiles.md`).
