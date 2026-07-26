# ADR-002: Python FastAPI sidecar for chemistry logic

**Status:** accepted · 2026-07-26

## Context
Parsing ORCA output, SMILES→3D, format conversion all have mature Python libraries
(cclib, RDKit, ASE, Open Babel) with no serious Rust/JS equivalents.

## Decision
A local FastAPI service (`localhost:8765`), spawned by the Rust core at app start and
terminated on exit. Frontend/Rust communicate with it over HTTP with Pydantic-validated
schemas.

## Rationale
- cclib alone removes ~80% of output-parsing pain and is battle-tested across ORCA versions.
- HTTP on localhost is a pattern the author already ships in production; trivially debuggable
  (curl, /docs swagger).
- Sidecar crash ≠ app crash; Rust supervises and restarts it.

## Consequences
- Distribution: bundle a venv or use PyInstaller/`uv` later; for personal use a documented
  `requirements.txt` + system Python is fine.
- Port collision handled by picking a free port at spawn and passing it to the frontend.
- Keep the sidecar stateless: all persistence lives in SQLite owned by Rust.
