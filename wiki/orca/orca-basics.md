# ORCA basics: installation, invocation, environment

**Status of this page:** VERIFIED on 2026-07-26 (Phase 0). Values below reflect the actual
working install on the author's laptop.

## Verified environment (2026-07-26)

| Item | Value |
|---|---|
| ORCA version | **6.1.0** |
| Install path | **`/opt/orca`** (binary: `/opt/orca/orca`) |
| OpenMPI version | **4.1.6** (system OpenMPI, compatible with this ORCA build) |
| Host | Laptop-main, Linux Mint |

Verification test (`water_optfreq`): r²SCAN-3c `Opt Freq TightSCF`, `%pal nprocs 4`,
`%maxcore 2000`. Geometry optimization converged in **4 cycles**; final energy
**−76.418938719971 Eh**; harmonic frequencies **1653.26 / 3813.32 / 3932.49 cm⁻¹**
(all positive → confirmed minimum); run ended with `ORCA TERMINATED NORMALLY`.
Full-path invocation with `%pal nprocs 4` parallelized correctly — the domain rule holds.

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
