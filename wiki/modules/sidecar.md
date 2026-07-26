# Module: Python sidecar (sidecar/)

**Status:** not started

## Responsibilities
Chemistry intelligence: parsing, structure generation, conversions, manual indexing.

## Endpoints (planned)
- `GET  /health`
- `POST /parse` — path to output file → cclib-derived JSON (energies, orbitals, freqs,
  intensities, charges, dipole, TD-DFT states, geometry trajectory)
- `POST /smiles-to-3d` — SMILES → xyz (RDKit ETKDG + MMFF)
- `POST /convert` — format conversion (Open Babel)
- `POST /manual/build-index` — one-off docs indexing (Phase 4)
- `GET  /manual/search?q=` — FTS query proxy (or Rust queries SQLite directly — decide in Phase 4)

## Dependencies
fastapi, uvicorn, pydantic, cclib, rdkit, ase, (openbabel via system package).

## Conventions
Stateless; all inputs are file paths inside the app data dir; Pydantic models for every
request/response; pytest with recorded ORCA outputs as fixtures (`tests/fixtures/`).
