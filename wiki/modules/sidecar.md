# Module: Python sidecar (sidecar/)

**Status:** Two chemistry endpoints live — `/smiles-to-3d` (RDKit, Phase 2.2) and
`/convert` + `/formats` (ASE, Phase 2.6, closes Phase 2). Builds on the Phase 0 scaffold
(`/health`, venv, pytest).

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

## As built (Phase 2.6) — molecular format conversion
- **`app/convert.py`** — `APIRouter` with `POST /convert`
  (`ConvertRequest{content, from_format, to_format}` → `ConvertResponse{content, num_atoms,
  formula}`) and `GET /formats` (`{"read": {...}, "write": {...}}` for UI dropdowns). Registered
  in `main.py` via `app.include_router(convert_router)`.
- **Pipeline:** write `content` to a `NamedTemporaryFile(suffix=".{from_format}")` → `ase.io.read`
  (`format=` resolved, `index=-1` so a multi-frame input yields its last structure) → `ase.io.write`
  to a second tempfile → read text back. `num_atoms = len(atoms)`,
  `formula = atoms.get_chemical_formula()`. Both tempfiles removed in a `finally`.
- **Decision — ASE, not Open Babel** (ROADMAP originally said Open Babel):
  1. ASE is **already** a sidecar dependency (ADR-007 — the Phase 2.5 geometry kernel:
     `atoms.set_distance/angle/dihedral`), so no second library.
  2. Pure-Python wheel (`pip install ase`) — **no system binary** and no `openbabel-wheel`, which
     frequently fails to build.
  3. Covers every format we actually need: xyz, extxyz, pdb, cif, mol, sdf, gen, turbomole,
     gaussian-in, vasp, traj.
  Open Babel stays the **fallback** only if a format ASE lacks is ever needed (e.g. `mol2`).
- **Security — format whitelist.** We do NOT expose everything ASE can read: its registry includes
  calculation-package readers, some of which execute code or read arbitrary files on parse. Only
  the plain structure formats in `READ_FORMATS` / `WRITE_FORMATS` are accepted, and the check runs
  **before** ASE sees the input (never rely on ASE raising for an unknown format). `WRITE_FORMATS`
  is deliberately narrower than `READ_FORMATS` (no mol/sdf/gaussian-in/traj out).
- **Format-name gotcha:** ASE's internal name for PDB is **`proteindatabank`**, not `pdb` — the
  public API keeps the friendly key and maps it via `_ASE_FORMAT` (`{"pdb": "proteindatabank"}`).
  Passing `format="pdb"` directly to ASE raises `UnknownFileTypeError`.
- **Errors:** unknown format → **400** `unsupported format: <fmt>`; unparseable content → **422**
  `could not parse as <from_format>: <e>`; 0 atoms (e.g. an empty `0\n...` xyz, or garbage a
  lenient reader accepts as atom-less) → **422** `no atoms found`.
- **ASE install:** `ase>=3.23` added to `requirements.txt`; `pip install -r requirements.txt`
  pulled `ase==3.29` + numpy/scipy/matplotlib (no venv recreate needed).
- **Tests (`tests/test_convert.py`, 6):** xyz→pdb (3 atoms, `H2O`, `ATOM` records); pdb→xyz
  round-trip (coords within 1e-3); `mol2` → 400; garbage → 422; `/formats` → 200 with `xyz` in
  read+write; empty `0`-count xyz → 422. `pytest` 11 total green. Live-verified with `curl`
  (xyz→pdb text, bad format → 400, garbage → 422, `/formats`).

## Responsibilities
Chemistry intelligence: parsing, structure generation, conversions, manual indexing.

## Endpoints (planned)
- `GET  /health`
- `POST /parse` — path to output file → cclib-derived JSON (energies, orbitals, freqs,
  intensities, charges, dipole, TD-DFT states, geometry trajectory)
- `POST /smiles-to-3d` — SMILES → xyz (RDKit ETKDG + MMFF) — **done (Phase 2.2)**
- `POST /convert` — format conversion (**ASE**, not Open Babel) — **done (Phase 2.6)**
- `GET  /formats` — supported read/write formats for UI dropdowns — **done (Phase 2.6)**
- `POST /parse` — path to output file → cclib-derived JSON (Phase 3)
- `POST /manual/build-index` — one-off docs indexing (Phase 4)
- `GET  /manual/search?q=` — FTS query proxy (or Rust queries SQLite directly — decide in Phase 4)

## Dependencies
fastapi, uvicorn, pydantic, rdkit, ase (pulls numpy/scipy/matplotlib); cclib later (Phase 3).
Open Babel is NOT a dependency — ASE covers conversions (see Phase 2.6 above).

## Conventions
Stateless; Pydantic models for every request/response; pytest with recorded ORCA outputs as
fixtures (`tests/fixtures/`). Inputs are either file paths inside the app data dir (parsing
endpoints) or small literals like a SMILES string (`/smiles-to-3d`) — never large payloads.
