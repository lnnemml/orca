# ORCA basics: installation, invocation, environment

**Status of this page:** pre-filled from planning; VERIFY every claim during Phase 0 and
update with actual paths/versions.

## Installation
- ORCA 6.x: free academic/personal license via FAccTs registration; download tarball,
  unpack to e.g. `/opt/orca`. Never bundle/redistribute.
- **OpenMPI version must match exactly** what the ORCA build was compiled against
  (stated on the download page). Mismatch = cryptic MPI startup failures.
- Record here after install: ORCA version, OpenMPI version, install path, `PATH`/
  `LD_LIBRARY_PATH` additions.

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
