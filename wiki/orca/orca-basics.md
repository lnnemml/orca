# ORCA basics: installation, invocation, environment

**Status of this page:** VERIFIED on 2026-07-26 (Phase 0). Values below reflect the actual
working install on the author's laptop.

## Verified environment (2026-07-26)

| Item | Value |
|---|---|
| ORCA version | **6.1.1** — current default (see [install scheme & parity](#orca-611-install-scheme--laptopserver-parity-2026-10-02)); the 2026-07-26 numbers below were on **6.1.0**, retained for reproducibility |
| Install path | **`/opt/orca`** (binary: `/opt/orca/orca`) — a symlink to `/opt/orca-<version>` |
| OpenMPI version | **4.1.6** (system OpenMPI, compatible with this ORCA build) |
| Host | Laptop-main, Linux Mint |

Verification test (`water_optfreq`): r²SCAN-3c `Opt Freq TightSCF`, `%pal nprocs 4`,
`%maxcore 2000`. Geometry optimization converged in **4 cycles**; final energy
**−76.418938719971 Eh**; harmonic frequencies **1653.26 / 3813.32 / 3932.49 cm⁻¹**
(all positive → confirmed minimum); run ended with `ORCA TERMINATED NORMALLY`.
Full-path invocation with `%pal nprocs 4` parallelized correctly — the domain rule holds.

## ORCA 6.1.1 install scheme & laptop/server parity (2026-10-02)

**Install scheme.** Each version lives in its own dir `/opt/orca-<version>`
(`/opt/orca-6.1.0`, `/opt/orca-6.1.1`); **`/opt/orca` is a symlink** to the active one
(currently `/opt/orca-6.1.1`). So the domain-rule-#1 path `/opt/orca/orca` always resolves
to the current default, and an older version stays runnable by absolute path
(`/opt/orca-6.1.0/orca`) for reproducing old results. The app still invokes the
settings-configured path (rule #7) — the symlink is a host convenience, not an app
assumption. **Both hosts use this scheme** (the `uni` server since 2026-10-03:
`/opt/orca -> /opt/orca-6.1.1`, measured — [uni-server.md](../infrastructure/uni-server.md)).

**Laptop/server parity** — 6.1.1 is the default on both the laptop and the `uni` server
([infrastructure/uni-server.md](../infrastructure/uni-server.md)):
- **Binary identity:** `sha256(/opt/orca/orca)` is **identical** on laptop and server
  (`335aef8b…c4ea`).
- **Water r2SCAN-3c Opt+Freq** (4 procs) is **bit-identical** laptop↔server and matches the
  laptop's 6.1.0 to reported precision: E = −76.418938720745 Eh, 1653.28 / 3813.59 / 3932.72 cm⁻¹.
- **6.1.0 ↔ 6.1.1 parser cross-version regression** (commits `626c8bf`, `c92c592`;
  `src-tauri/src/parse/cross_version_6_1_1.rs`, fixtures `tests/fixtures/xver/`): **10 checks
  bit-identical** (Δ = 0, no tolerance consumed) — property SP energy, Opt+Freq
  (energy / final geometry / frequencies / IR intensities), `_trj.xyz` trajectory, Mayer bond
  orders, SMD SP energy, DLPNO-CCSD(T) SP energy, relaxed scan (`act`+`scf`), OptTS+Freq
  (energy / TS geometry / frequencies), and an open-shell CH3 doublet (energy + frequencies).
  `orca_plot` menu + cubes byte-identical (see [gotchas](gotchas.md)).
- **NEB-TS cross-version parity is NOT checked** — the HCN↔HNC r2SCAN-3c case is not
  completable within a laptop time guard (see [gotchas](gotchas.md)); it is **deferred to the
  server** (ADR-023/024), not asserted.

## Installation
- ORCA 6.x: free academic/personal license via FAccTs registration; download tarball,
  unpack to e.g. `/opt/orca`. Never bundle/redistribute.
- **OpenMPI version must match exactly** what the ORCA build was compiled against
  (stated on the download page). Here: ORCA 6.1.0 works with system **OpenMPI 4.1.6**.
  Mismatch = cryptic MPI startup failures.

## Invocation — the rule that breaks everyone
Parallel runs REQUIRE the full absolute path:

```bash
/opt/orca/orca input.inp > output.out 2>&1      # correct
orca input.inp                                   # WRONG: %pal will not work
```

Reason: ORCA re-invokes itself via MPI using the path it was called with.

## Runner script pattern (used by all backends)
```bash
#!/usr/bin/env bash
cd "$(dirname "$0")"
/opt/orca/orca input.inp > output.out 2>&1
echo $? > .exit_code
```

## Companion binaries we use
`orca_plot` (cube generation from .gbw), `orca_mapspc` (spectra processing, maybe later).
All live in the ORCA install dir; same full-path rule applies.

## Scratch behavior
ORCA writes many temp files next to the input (and honors scratch env vars). Our policy:
one isolated dir per job, cleanup of temp files (keep: inp, out, xyz, gbw, hess, cubes).

## Performance and parallelisation

Core-count and pinning decisions are measured, not assumed — see
[performance.md](performance.md) for the scaling benchmark on the dev machine
(i5-12500H), the recommended presets, and the memory ceiling on `nprocs`.
