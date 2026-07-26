# resources/manual — RAW SOURCES (immutable)

This directory holds locally fetched ORCA documentation used to build the FTS index
(Phase 4, ADR-006). Rules:

- **Immutable**: the LLM and tooling read from here, never modify.
- **Never committed to a public repo / never redistributed** — ORCA docs are copyrighted;
  the index is built locally for personal use. This directory is in .gitignore except
  this README.
- Refreshed per ORCA version via `sidecar` indexing pipeline.
