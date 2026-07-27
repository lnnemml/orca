# Module: Python sidecar (sidecar/)

**Status:** First chemistry endpoint live (Phase 2.2) — `/smiles-to-3d` (RDKit). Builds on the
Phase 0 scaffold (`/health`, venv, pytest).

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

## As built (Phase 2.2) — SMILES → 3D
- **`app/smiles.py`** — `APIRouter` with `POST /smiles-to-3d`
  (`SmilesToXyzRequest{smiles}` → `SmilesToXyzResponse{xyz, formula, charge, multiplicity,
  num_atoms}`), registered in `main.py` via `app.include_router(smiles_router)`.
- **Pipeline:** `Chem.MolFromSmiles` (→ 400 `"Invalid SMILES"` on `None`) → `Chem.AddHs` →
  `AllChem.EmbedMolecule(mol, AllChem.ETKDGv3())`; on failure retry with
  `params.useRandomCoords = True`, then 422 `"Could not generate 3D coordinates"` →
  `AllChem.MMFFOptimizeMolecule(maxIters=500)` (non-convergence / missing MMFF params is **not**
  fatal — wrapped in try/except). Output built with `Chem.MolToXYZBlock` (standard xyz),
  `rdMolDescriptors.CalcMolFormula`, `Chem.GetFormalCharge`; `multiplicity` hardcoded `1`
  (singlet) for now.
- **API-correctness note:** the task spec suggested
  `AllChem.EmbedMolecule(mol, AllChem.ETKDGv3(), useRandomCoords=True)`, but `useRandomCoords`
  is a **property of the params object**, not a kwarg accepted alongside a params object — so the
  fallback sets `params.useRandomCoords = True` and re-embeds.
- **RDKit install:** `pip install rdkit` worked directly — modern PyPI wheel
  `rdkit==2026.3.4` (the deprecated `rdkit-pypi` fork was **not** needed). Pulls numpy + Pillow.
  `requirements.txt` pins `rdkit>=2024.3`. Venv recreated cleanly.
- **Tests (`tests/test_smiles.py`):** water `O` → 3 atoms / `H2O` / charge 0 / xyz starts `"3\n"`;
  ethanol `CCO` → 9 atoms / `C2H6O`; `[NH4+]` → charge 1; `not_a_smiles` → 400. `pytest` green
  (5 total incl. health). Live-verified with `curl` (`O`, `[O-]`→charge −1, invalid→400).

## Responsibilities
Chemistry intelligence: parsing, structure generation, conversions, manual indexing.

## Endpoints (planned)
- `GET  /health`
- `POST /parse` — path to output file → cclib-derived JSON (energies, orbitals, freqs,
  intensities, charges, dipole, TD-DFT states, geometry trajectory)
- `POST /smiles-to-3d` — SMILES → xyz (RDKit ETKDG + MMFF) — **done (Phase 2.2)**
- `POST /convert` — format conversion (Open Babel)
- `POST /manual/build-index` — one-off docs indexing (Phase 4)
- `GET  /manual/search?q=` — FTS query proxy (or Rust queries SQLite directly — decide in Phase 4)

## Dependencies
fastapi, uvicorn, pydantic, cclib, rdkit, ase, (openbabel via system package).

## Conventions
Stateless; Pydantic models for every request/response; pytest with recorded ORCA outputs as
fixtures (`tests/fixtures/`). Inputs are either file paths inside the app data dir (parsing
endpoints) or small literals like a SMILES string (`/smiles-to-3d`) — never large payloads.
