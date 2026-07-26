# Module: Result parsing

**Status:** not started

## Two-tier design
1. **Streaming tier (Rust, during run):** lightweight regexes over incoming log lines for
   live UI only — SCF iteration energies, geometry cycle energy, gradient norms vs
   convergence criteria. Tolerant to partial lines; never authoritative.
2. **Authoritative tier (sidecar, after run):** cclib full parse of the final output →
   SQLite `results`. Everything the Results screen shows comes from here.

## Notes
- Keep a fixtures library of real outputs (per ORCA version, per job type) in
  `sidecar/tests/fixtures/` — regression safety when ORCA formatting shifts between versions.
- cclib gaps (if any ORCA 6 block is unsupported) get targeted supplemental parsers in
  Python, documented here as they appear.
