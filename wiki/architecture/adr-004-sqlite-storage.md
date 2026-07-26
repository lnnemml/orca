# ADR-004: SQLite as the single store

**Status:** accepted · 2026-07-26

## Context
Need persistence for projects/molecules/jobs/results, plus full-text search for the manual.
Alternatives: JSON files, embedded key-value stores, Postgres.

## Decision
One SQLite database (`orcastudio.db`) owned by the Rust core (rusqlite), with FTS5 virtual
tables for manual search. Filesystem keeps large artifacts; DB keeps paths + parsed data.

## Rationale
- Zero-ops, single file, trivially backed up; FTS5 covers manual search with no extra infra.
- Author already runs this exact pattern elsewhere. The job catalog + notes turn the app
  into a lab journal, which is a mission-level feature (master's research record).

## Consequences
- Simple integer schema migrations, applied at startup.
- Never store cube/gbw blobs in the DB.
- If multi-device sync is ever wanted, revisit (out of scope now).
