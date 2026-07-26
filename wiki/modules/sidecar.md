# Module: Python sidecar (sidecar/)

**Status:** Phase 0 scaffold done — FastAPI app with `/health`, venv, pytest.

## As built (Phase 0)
- `app/main.py`: FastAPI app, `GET /health -> {"status":"ok","version":"0.1.0"}`
  (Pydantic `HealthResponse`), CORS middleware restricted to localhost / `tauri://localhost`
  via `allow_origin_regex`. `__version__` lives in `app/__init__.py`.
- `requirements.txt`: fastapi / uvicorn[standard] / pydantic only (cclib/rdkit/ase later).
  `requirements-dev.txt` adds pytest + httpx for tests.
- Venv at `sidecar/.venv`; the Rust core prefers `.venv/bin/python`, falling back to system
  `python3` with a warning.
- Launched by the Rust core as `python -m uvicorn app.main:app --host 127.0.0.1 --port <dynamic>`
  (cwd = `sidecar/`), stdout/stderr → `<data_dir>/sidecar.log`.
- `tests/test_health.py`: FastAPI `TestClient` smoke test (`pytest` green).

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
