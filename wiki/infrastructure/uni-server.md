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
- **Shared lab account** `yats` (with sudo). The server is dedicated to Anton in practice;
  others ask before using it. Treat it as shared: do not disrupt other users' files or installs.
- Laptop SSH alias: `uni` (see Access).

## Hardware

- **CPU** (measured 2026-10-02): **2× Intel Xeon E5-2673 v3** (Haswell, 12c/24t each → 24
  physical / 48 logical, hyper-threading on), 2.4 GHz base / 3.1 GHz turbo. **AVX2, no AVX-512.**
  **2 NUMA nodes** — node0: cores 0–11,24–35; node1: 12–23,36–47. The NUMA split is the natural
  candidate for a 2×12 slot layout (one slot per socket/node), but that is **unmeasured** — see
  Open items + rule #8 (taskset masks are measured, not assumed).
- **RAM:** 62 GiB. **Swap:** 119 GiB. Swapping during a calculation is a **failure mode**, so
  ORCA `%maxcore` must stay conservative (see Operational DO/DON'T).
- **Disk** (measured 2026-10-02): `sda` and `sdb` **both rotational HDD**, 465.8 G each. `/` and
  `/home` both live on **`sdb`** (`/home` on `/dev/sdb4`, ~79 G free at provisioning). ORCA
  scratch I/O therefore hits a spinning disk — one isolated job dir per calculation (rule #3)
  stays the discipline. `sda` contents **unknown**.

## Software (OS, MPI, ORCA)

- **OS:** Ubuntu 24.04.1 LTS, kernel 6.8.0.
- **MPI:** system OpenMPI **4.1.6** at `/usr/bin/mpirun`. A conda base auto-activates in the
  login shell but does **not** shadow `mpirun`. `ldd` of `orca_scfgrad_mpi` resolves
  `libmpi.so.40` to the system lib.
- **ORCA (ours):** **6.1.1** installed at `/opt/orca`, copied from the lab install.
  **Self-contained** (measured 2026-10-02): `ldd` shows `liborca_*` → `/opt/orca/lib` and
  `libmpi.so.40` → the **system** lib (so MPI uses the system OpenMPI 4.1.6 — rule #2).
  - **Version parity CONFIRMED** (2026-10-02): a water **r2SCAN-3c Opt+Freq** benchmark on 4
    procs gives **E = −76.418938720745 Ha**, freqs **1653.28 / 3813.59 / 3932.72 cm⁻¹**, *ORCA
    TERMINATED NORMALLY* — **matches the laptop (ORCA 6.1.0) to reported precision**. Parity no
    longer blocks anything. (The project's canonical ORCA reference stays 6.1.0 — this only
    records that the server's 6.1.1 install produces matching numbers; `CLAUDE.md` is unchanged.)
- **Legacy lab ORCA installs:** `/home/yats/calc/orca/{303,504,601,611}` — **leave untouched**.
- Packages we installed: `openssh-server`, `tmux`, `rsync`, `tailscale`.

## Access

- **openssh-server**, **key-only**. Hardening via a drop-in
  `/etc/ssh/sshd_config.d/00-hardening.conf` (`PasswordAuthentication no`, `PermitRootLogin no`).
  A drop-in is used because sshd is first-match and `sshd_config.d` is read before the main file.
- **Reachability via Tailscale.** Laptop SSH alias `uni` configured with ControlMaster /
  ControlPersist 10m and ServerAliveInterval 30 (the persistent-connection substrate ADR-024
  relies on for detach/reconnect).
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

These two facts — a nightly link cutoff and a short UPS window — are why the queue and the
source of truth live **on the server**, not the laptop (ADR-024).

## Operational DO / DON'T

**DO**
- Invoke ORCA by **absolute path** `/opt/orca/orca` (rule #1). `yats`'s `~/.bashrc` has stale
  ORCA `PATH` entries — never rely on `PATH`.
- Install packages with `apt-get install <pkg>` **directly**.
- Keep `%maxcore` conservative given the swap-is-failure rule; measure before trusting a value.
- Locate ORCA installs via a real binary, e.g. `orca_scfgrad` (ORCA 6 has **no** `orca_scf`
  binary, and Ubuntu's `orca` screen-reader shares the binary name).

**DON'T**
- **Never** run `do-release-upgrade` — it would replace OpenMPI and break rule #2 (MPI must
  match the ORCA build).
- **Don't** `apt-get update`-gate installs: it exits non-zero because of third-party repos owned
  by others (winehq `NO_PUBKEY`, qtisas OBS expired key). Do **not** modify those repos.
- **Don't** touch the legacy `/home/yats/calc/orca/{303,504,601,611}` installs.
- **Don't** assume ORCA is parallelizing — confirm MPI resolves inside `/opt/orca` (rule #1/#2).

**Resolved 2026-10-02:** ORCA 6.1.1 install (copy done), self-contained `ldd` (libs inside
`/opt/orca`, MPI = system), the water r2SCAN-3c Opt+Freq benchmark, and 6.1.1↔6.1.0 parity — all
confirmed above. tmux detachment confirmed. CPU model + NUMA topology and disk type/layout measured.

## Open items (pending / unknown)

- **`sda` contents** — unknown (both disks are HDD, 465.8 G; `/` and `/home` are on `sdb`).
- **UPS → clean shutdown:** does the UPS signal the server? (`lsusb` / `nut` pending.)
- **Exact nightly cutoff window in UTC** — to confirm (stated as 08:00–22:00 Europe/Kyiv).
- **Tailscale ACL** (laptop → server only) pending; confirm node-key expiry disabled.
- **Parallel-slot layout + OpenMPI binding for concurrent runs** — whether to run **1×24** or
  **2×12** (one slot per NUMA node) and how OpenMPI's default binding behaves for **concurrent
  independent** ORCA runs are **to measure** (rule #8/#10). Until measured the profile runs
  **1 slot** (ADR-024 Decision f).
- **`SshBackend` connection-test specs** (remote ORCA path resolution, OpenMPI version, `nproc`)
  are UNDETERMINED until the Phase 5 connection-test runs (ADR-023, `server-profiles.md`).
