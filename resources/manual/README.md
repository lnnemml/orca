# resources/manual — RAW SOURCES (immutable)

This directory holds locally fetched ORCA documentation used to build the FTS index
(Phase 4, ADR-006; indexing ownership narrowed by ADR-013). Rules:

- **Immutable**: the LLM and tooling read from here, never modify.
- **Never committed to a public repo / never redistributed** — ORCA docs are copyrighted;
  the index is built locally for personal use. This directory is in .gitignore except
  this README.
- Refreshed per ORCA version by an out-of-band author-run **fetch script** (the app never fetches
  the manual over the network — ADR-013). Indexing of these files into SQLite FTS5 is done in
  **Rust**, not the sidecar (ADR-013 narrows ADR-006).
