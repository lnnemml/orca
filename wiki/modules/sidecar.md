# Module: Python sidecar (sidecar/)

**Status:** Chemistry endpoints live — `/smiles-to-3d` (RDKit, Phase 2.2), `/convert` + `/formats`
(ASE, Phase 2.6), and `/geometry/set-internal` (ASE geometry kernel, Phase 2.5.2c). Builds on the
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

## As built (Phase 2.5.2c) — geometry kernel (ASE)
- **`app/geometry.py`** — `APIRouter` with `POST /geometry/set-internal`, registered in `main.py`.
  Sets a distance / angle / dihedral to a target value by moving a masked subgroup.
  - **Request:** `{xyz, op: "distance"|"angle"|"dihedral", indices: int[], value, mask: int[]}`.
    `indices` = the 0-based **global** atom indices defining the coordinate (2/3/4). `mask` = the
    0-based global indices **allowed to move**. `value` = Å (distance) or degrees (angle/dihedral).
  - **Response:** `{xyz, measured, max_static_displacement}`. `xyz` is the same format and the same
    atom order; `measured` is **re-derived from the resulting coordinates**;
    `max_static_displacement` is the largest move of any atom OUTSIDE the mask.
- **The mask is the fragment, computed by the frontend.** The sidecar knows nothing about scenes or
  fragments (the 2.5.0 decision): the caller sends `fragmentAtomIndices` as the explicit `mask`
  list. The index space is identical on both sides of HTTP — index N in the request xyz is index N
  in the response (ADR-008). See `wiki/modules/scene.md`.

### ASE version + signatures (checked against the installed version, not memory)
**ASE 3.29.0** (`sidecar/.venv`). The three `ase.Atoms` methods (`atoms.py`) each take **both**
`mask=` and `indices=` — they are NOT interchangeable:
- `mask=` is a **boolean array** of length N (`mask[i]` truthy → atom i moves);
- `indices=` is a **list of atom indices** to move, and **overrides `mask`** in all three.

We pass **`indices=`** because the request already carries a list of global indices — an exact fit,
no boolean conversion. Signatures and the mapping:
- `set_distance(a0, a1, distance, fix=0.5, mask=None, indices=None, ...)` — body:
  `for i in indices: R[i] -= x*(1-fix)*D` (a0 moves only `if i==a0`). With `indices` **excluding**
  a0, we must pass **`fix=0`** (fix the FIRST atom) so all displacement lands on the a1 side; else
  the a0-side term is silently dropped and the distance is wrong. Mapping: `set_distance(i, j,
  value, fix=0, indices=mask)` — i = reference (static), j = moving endpoint.
- `set_angle(a1, a2, a3, angle, mask=None, indices=None, add=False)` — rotates the masked group
  about the **vertex `a2`** (`axis = cross(a2→a1, a2→a3)`, `center = a2`). Mapping:
  `set_angle(i, vertex, j, value, indices=mask)`.
- `set_dihedral(a1, a2, a3, a4, angle, mask=None, indices=None)` — rotates the masked group about
  the **a2–a3 axis** (`axis = pos[a3]-pos[a2]`, `center = pos[a3]`); the docstring warns "if
  mask/indices does not contain a4, a4 will NOT be moved". Mapping:
  `set_dihedral(i, j, k, l, value, indices=mask)`.
  (`get_dihedral` returns **[0, 360)** — the convention `measure.ts` pins; re-verified below.)

### The reference-atom rule (what makes sequential placement safe)
Validation enforces (422): the **last atom of the chain must be IN the mask, every preceding atom
must NOT be** — `distance(i,j)`: j∈mask, i∉; `angle(i,v,j)`: j∈mask, i,v∉;
`dihedral(i,j,k,l)`: l∈mask, i,j,k∉. This is the operational form of "reference atoms are taken
from the substrate side" (2026-07-28 decision): each later op's rotation axis passes through the
reference atoms of the earlier constraint, so applying distance→angle→dihedral in **one sequential
pass** cannot undo an earlier value. Without the rule the second op silently destroys the first.
Also validated: `indices` length matches `op`, all in range, distinct; mask non-empty, in range, a
**strict** subset; `distance > 0`; `angle ∈ (0, 180)`; dihedral any real (folds to [0, 360)).

### Post-conditions INSIDE the endpoint (not only in tests)
Before returning, the endpoint checks and raises **500 with a diagnostic** (never silently returns
wrong coordinates) if: the atom count changed; the element sequence changed positionally; or
`measured` is outside tolerance of the target (`1e-6` Å for distance, `1e-4°` for angle/dihedral,
the dihedral compared circularly so 359.99 vs 0.01 doesn't false-fail). This is the crux of the
unit: an error here doesn't crash — it returns coordinates ORCA computes *other* chemistry from —
so the check is a running guard, not a test-only assertion. Cost is negligible; the price of a
missed error is weeks of the wrong calculation.

### Verification (the convention tripwire FIRED and PASSED)
The 2.5.2b tripwire was carried into pytest: the SAME butane coordinates `measure.test.ts` pins →
ASE `get_dihedral(0,1,2,3)` = **179.998** (anti) / **67.523** (gauche) to 3 dp — exact. So the ASE
[0,360) convention and our `measure.ts` agree; the tripwire caught no divergence. The sequential
acceptance test (carbonyl + hydride, three separate endpoint calls) recomputes all three from the
final coordinates: targets `d=1.5 / θ=107.0 / φ=90.0` → recomputed `1.50000000 / 107.000000 /
90.000000`, substrate internal geometry unchanged to 1e-9. `pytest` 25 (was 11 → +14). Live-verified
with `uvicorn` + `curl` (a real set-distance call, and a reference-atom-in-mask → 422).

## Responsibilities
Chemistry intelligence: parsing, structure generation, conversions, manual indexing.

## Endpoints (planned)
- `GET  /health`
- `POST /parse` — path to output file → cclib-derived JSON (energies, orbitals, freqs,
  intensities, charges, dipole, TD-DFT states, geometry trajectory)
- `POST /smiles-to-3d` — SMILES → xyz (RDKit ETKDG + MMFF) — **done (Phase 2.2)**
- `POST /convert` — format conversion (**ASE**, not Open Babel) — **done (Phase 2.6)**
- `GET  /formats` — supported read/write formats for UI dropdowns — **done (Phase 2.6)**
- `POST /geometry/set-internal` — set a distance/angle/dihedral with a mask (ASE) — **done (2.5.2c)**
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
