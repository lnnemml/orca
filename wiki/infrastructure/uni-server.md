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

- **CPU:** 48 logical / 24 physical cores (hyper-threading on). CPU model: **unknown** (`lscpu`
  pending).
- **RAM:** 62 GiB. **Swap:** 119 GiB. Swapping during a calculation is a **failure mode**, so
  ORCA `%maxcore` must stay conservative (see Operational DO/DON'T).
- **Disk:** `/home` on `/dev/sdb4` — 201 G total, ~79 G free at provisioning. Root filesystem
  size and disk type (HDD/SSD) **unknown**.

## Software (OS, MPI, ORCA)

- **OS:** Ubuntu 24.04.1 LTS, kernel 6.8.0.
- **MPI:** system OpenMPI **4.1.6** at `/usr/bin/mpirun`. A conda base auto-activates in the
  login shell but does **not** shadow `mpirun`. `ldd` of `orca_scfgrad_mpi` resolves
  `libmpi.so.40` to the system lib.
- **ORCA (ours):** **6.1.1** being installed to `/opt/orca`, copied from the lab install
  (copy in progress at provisioning). H2 HF/STO-3G smoke test: *Program Version 6.1.1* and
  *ORCA TERMINATED NORMALLY*.
  - Version parity is **pending**: the laptop runs ORCA **6.1.0**. Do **not** change the ORCA
    version in `CLAUDE.md` or the wiki until 6.1.1 parity is established.
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

## Open items (pending / unknown)

- **ORCA 6.1.1 install:** finish the `/opt/orca` copy; **rpath check** that `/opt/orca` libs
  resolve inside `/opt/orca`; water **r2SCAN-3c Opt+Freq benchmark** vs the laptop
  (E = −76.4189 Ha; 1653 / 3813 / 3932 cm⁻¹). Version parity (6.1.1 vs laptop 6.1.0) pending.
- **CPU model** — `lscpu` pending.
- **Root filesystem size** and **disk type** (HDD/SSD) — unknown.
- **UPS → clean shutdown:** does the UPS signal the server? (`lsusb` / `nut` pending.)
- **Exact nightly cutoff window in UTC** — to confirm (stated as 08:00–22:00 Europe/Kyiv).
- **Tailscale ACL** (laptop → server only) pending; confirm node-key expiry disabled.
- **Per-profile parallel slots:** whether the box can run parallel ORCA slots (e.g. 2×12 cores)
  is **unmeasured** — until measured the profile runs **1 slot** (rule #10, ADR-024 Decision f).
- **`SshBackend` connection-test specs** (remote ORCA path resolution, OpenMPI version, `nproc`)
  are UNDETERMINED until the Phase 5 connection-test runs (ADR-023, `server-profiles.md`).
