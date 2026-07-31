# Module: Python sidecar (sidecar/)

**Status:** chemistry endpoints live — `/smiles-to-3d` (RDKit), `/convert` + `/formats` (ASE),
`/geometry/set-internal` and `/geometry/rotatable-mask` (the ASE geometry kernel). Sidecar
`__version__` `0.4.0`. **Result parsing is NOT a sidecar concern** — cclib was rejected and the
authoritative tier moved to Rust (ADR-012); see [artifact-readers.md](artifact-readers.md). **Manual
indexing is Rust too, not the sidecar** — [ADR-013](../architecture/adr-013-manual-indexing-ownership.md)
narrows ADR-006: the sidecar is not involved in Phase 4.

## Responsibilities & boundaries

Chemistry intelligence over file *content*: structure generation (RDKit), format conversion and
the geometry kernel (ASE). **Not** result parsing — that is Rust over structured artifacts
(ADR-012), never a sidecar/cclib endpoint. **Not** manual indexing either — that is Rust over the
raw Markdown docs (ADR-013), never a sidecar endpoint. Two hard boundaries:

- **Stateless.** All persistence lives in SQLite owned by Rust (ADR-002). Inputs are either file
  paths inside the app data dir (parsing endpoints) or small literals like a SMILES string — never
  large payloads.
- **Never spawns a process (ADR-009).** In-process library calls (`ase.io`, RDKit) are the
  sidecar's job; running an external binary belongs to Rust. It knows nothing about the jobs dir,
  settings, scenes, or fragments — the caller sends explicit index lists (see the geometry kernel).

## Scaffold (`app/main.py`, `app/__init__.py`)

- FastAPI app; `GET /health -> {"status":"ok","version": <__version__>}` (Pydantic
  `HealthResponse`). CORS restricted to localhost / `tauri://localhost` via `allow_origin_regex`.
  `__version__` lives in `app/__init__.py`.
- Launched by the Rust core as `python -m uvicorn app.main:app --host 127.0.0.1 --port <dynamic>`
  (cwd = `sidecar/`), stdout/stderr → `<data_dir>/sidecar.log`. The Rust core prefers
  `.venv/bin/python`, falling back to system `python3` with a warning. Debug builds add
  `--reload --reload-dir app` (see the handshake section).
- `requirements.txt`: fastapi / uvicorn[standard] / pydantic / rdkit / ase; `requirements-dev.txt`
  adds pytest + httpx. `tests/test_health.py` asserts the health body against `__version__`.

## `POST /smiles-to-3d` — SMILES → 3D (RDKit)

`app/smiles.py`, `SmilesToXyzRequest{smiles}` → `SmilesToXyzResponse{xyz, formula, charge,
multiplicity, num_atoms}`.

- **Pipeline:** `Chem.MolFromSmiles` (→ **400** `"Invalid SMILES"` on `None`) → `Chem.AddHs` →
  `AllChem.EmbedMolecule(mol, AllChem.ETKDGv3())`; on failure retry with
  `params.useRandomCoords = True`, then **422** `"Could not generate 3D coordinates"` →
  `AllChem.MMFFOptimizeMolecule(maxIters=500)` (non-convergence / missing MMFF params is **not**
  fatal — wrapped in try/except). Output via `Chem.MolToXYZBlock`,
  `rdMolDescriptors.CalcMolFormula`, `Chem.GetFormalCharge`; `multiplicity` hardcoded `1` for now.
- **API quirk:** `useRandomCoords` is a **property of the params object**, not a kwarg accepted
  alongside a params object — so the fallback sets `params.useRandomCoords = True` and re-embeds.
- **Install:** `pip install rdkit` pulls the modern wheel `rdkit==2026.3.4` directly (the deprecated
  `rdkit-pypi` fork is **not** needed); brings numpy + Pillow. `requirements.txt` pins
  `rdkit>=2024.3`.

## `POST /convert` + `GET /formats` — molecular format conversion (ASE)

`app/convert.py`, `ConvertRequest{content, from_format, to_format}` → `ConvertResponse{content,
num_atoms, formula}`; `GET /formats` → `{"read": {...}, "write": {...}}` for UI dropdowns.

- **Pipeline:** write `content` to a `NamedTemporaryFile(suffix=".{from_format}")` → `ase.io.read`
  (`format=` resolved, `index=-1` so a multi-frame input yields its last structure) → `ase.io.write`
  to a second tempfile → read text back. `num_atoms = len(atoms)`,
  `formula = atoms.get_chemical_formula()`. Both tempfiles removed in a `finally`.
- **ASE, not Open Babel** (ROADMAP originally said Open Babel): ASE is already a dependency (the
  geometry kernel, ADR-007), a pure-Python wheel (no system binary, no flaky `openbabel-wheel`
  build), and covers xyz / extxyz / pdb / cif / mol / sdf / gen / turbomole / gaussian-in / vasp /
  traj. Open Babel remains the fallback only for formats ASE lacks (e.g. `mol2`) — and **per
  ADR-009** that means the `pybel` *library*, not shelling out to the `obabel` binary.
- **Security — format whitelist.** ASE's registry includes calculation-package readers that can
  execute code / read arbitrary files on parse. Only the plain structure formats in `READ_FORMATS` /
  `WRITE_FORMATS` are accepted, and the check runs **before** ASE sees the input (never rely on ASE
  raising for an unknown format). `WRITE_FORMATS` is deliberately narrower than `READ_FORMATS` (no
  mol / sdf / gaussian-in / traj out).
- **Format-name gotcha:** ASE's internal name for PDB is **`proteindatabank`**, not `pdb` — the
  public API keeps the friendly key and maps it via `_ASE_FORMAT` (`{"pdb": "proteindatabank"}`).
  Passing `format="pdb"` directly raises `UnknownFileTypeError`.
- **Errors:** unknown format → **400** `unsupported format: <fmt>`; unparseable → **422**
  `could not parse as <from_format>: <e>`; 0 atoms (empty `0\n…` xyz, or garbage a lenient reader
  accepts as atom-less) → **422** `no atoms found`.

## `POST /geometry/set-internal` — set a distance/angle/dihedral (ASE)

`app/geometry.py`. Sets a distance / angle / dihedral to a target value by moving a masked subgroup.

- **Request:** `{xyz, op: "distance"|"angle"|"dihedral", indices: int[], value, mask: int[]}`.
  `indices` = the 0-based **global** atom indices defining the coordinate (2/3/4). `mask` = the
  0-based global indices **allowed to move**. `value` = Å (distance) or degrees (angle/dihedral).
- **Response:** `{xyz, measured, max_static_displacement}`. `xyz` is the same format and atom order;
  `measured` is **re-derived from the resulting coordinates**; `max_static_displacement` is the
  largest move of any atom OUTSIDE the mask.
- **The mask is the fragment, computed by the frontend.** The caller sends `fragmentAtomIndices` as
  the explicit `mask` list; the index space is identical on both sides of HTTP — index N in the
  request xyz is index N in the response (ADR-008). See `wiki/modules/scene.md`.

### ASE signatures (checked against the installed venv 3.29.0, not memory)

The three `ase.Atoms` methods (`atoms.py`) each take **both** `mask=` and `indices=`, and their
**precedence differs between methods** (the ASE docstrings and code are literally inconsistent):

- `mask=` is a **boolean array** of length N (`mask[i]` truthy → atom i moves); `indices=` is a
  **list of atom indices** to move.
- **`set_distance`**: `elif mask:` — a non-empty `mask` overwrites `indices`; an empty list is
  falsy, so `indices` then wins.
- **`set_angle` / `set_dihedral`**: `elif indices is not None:` — `indices` overwrites `mask`.

We pass **only `indices=`** (leaving `mask=None`) in all three, so with `mask=None` every method
uses `indices`, and the precedence tangle never bites. Signatures + mapping:

- `set_distance(a0, a1, distance, fix=0.5, mask=None, indices=None, ...)` — body:
  `for i in indices: R[i] -= x*(1-fix)*D` (a0 moves only `if i==a0`). With `indices` **excluding**
  a0, we must pass **`fix=0`** (fix the FIRST atom) so all displacement lands on the a1 side; else
  the a0-side term is silently dropped and the distance is wrong. Mapping:
  `set_distance(i, j, value, fix=0, indices=mask)` — i = reference (static), j = moving endpoint.
- `set_angle(a1, a2, a3, angle, mask=None, indices=None, add=False)` — rotates the masked group
  about the **vertex `a2`** (`axis = cross(a2→a1, a2→a3)`, `center = a2`). Mapping:
  `set_angle(i, vertex, j, value, indices=mask)`.
- `set_dihedral(a1, a2, a3, a4, angle, mask=None, indices=None)` — rotates about the **a2–a3 axis**
  (`axis = pos[a3]-pos[a2]`, `center = pos[a3]`); the docstring warns "if mask/indices does not
  contain a4, a4 will NOT be moved". Mapping: `set_dihedral(i, j, k, l, value, indices=mask)`.
  (`get_dihedral` returns **[0, 360)** — the convention `measure.ts` pins.)

### The reference-atom rule (what makes sequential placement safe)

Validation enforces (**422**): the **last atom of the chain must be IN the mask, every preceding
atom must NOT be** — `distance(i,j)`: j∈mask, i∉; `angle(i,v,j)`: j∈mask, i,v∉;
`dihedral(i,j,k,l)`: l∈mask, i,j,k∉. This is the operational form of "reference atoms are taken from
the substrate side": each later op's rotation axis passes through the reference atoms of the earlier
constraint, so applying distance→angle→dihedral in **one sequential pass** cannot undo an earlier
value — without the rule the second op silently destroys the first (decision:
`[2026-07-28] Bürgi-Dunitz d/θ/φ apply sequentially`). Also validated: `indices` length matches
`op`, all in range, distinct; mask non-empty, in range, a **strict** subset; `distance > 0`;
`angle ∈ (0, 180)`; dihedral any real (folds to [0, 360)).

### Post-conditions INSIDE the endpoint (not only in tests)

Before returning, the endpoint raises **500 with a diagnostic** (never silently returns wrong
coordinates) if: the atom count changed; the element sequence changed positionally; or `measured` is
outside tolerance of the target (`1e-6` Å distance, `1e-4°` angle/dihedral, the dihedral compared
**circularly** so 359.99 vs 0.01 doesn't false-fail). This is the crux of the unit — an error here
doesn't crash, it returns coordinates ORCA computes *other* chemistry from — so it is a running
guard, not a test-only assertion.

The convention tripwire is that the SAME butane coordinates `measure.ts` pins give ASE
`get_dihedral(0,1,2,3)` = **179.998** (anti) / **67.523** (gauche) — so the ASE [0,360) fold and
`measure.ts` agree. A sequential acceptance test (carbonyl + hydride, three separate calls) targets
`d=1.5 / θ=107.0 / φ=90.0` → recomputes `1.50000000 / 107.000000/ 90.000000`, substrate internal
geometry unchanged to 1e-9.

## `POST /geometry/rotatable-mask` — the rotatable side of a bond (bond-graph split)

For an **intra-fragment** edit (rotating a molecule's own torsion — a side-chain conformation, an OH
orientation, an aryl-ring flip) the mask is not a whole fragment but the **connected side of a
broken bond**. Request `{ xyz, cut: [i, j], moving, scale, within? }` → response `{ mask,
static_count, cut_length }`. Algorithm: perceive bonds → build the graph → remove the `cut` edge →
the connected component containing `moving` is the mask.

### Bond perception is a GUESS — so it is checked, and it can refuse

Bonds are perceived from geometry via **`ase.neighborlist`** (ASE 3.29.0): `natural_cutoffs(atoms,
mult=scale)` gives a per-atom cutoff = covalent radius × `scale`, and `neighbor_list("ij", atoms,
cutoffs)` returns pairs with `d_ij < cutoffs[i] + cutoffs[j]`.

- **The multiplier is explicit** (`scale`, request param), not a hidden constant, because this
  editor can create geometries where the guess is wrong (a stretched bond vanishing; two fragments
  placed at ~2.2 Å spuriously bonding).
- **ASE's own default `mult=1.0` is TOO TIGHT** — it misses C–H and even C–C (butane → 0 bonds). Our
  default is **`_COVALENT_SCALE_DEFAULT = 1.2`**. Measured against known valence, `mult` in [1.1,
  1.3] all give the correct counts (butane **13**, benzene **12**, BH₄⁻ **4** — the charged trap,
  same multiplier as neutrals — water **2**); 1.2 sits mid-plateau (margin for slightly-stretched
  bonds) yet still below the threshold that would bond the ~2.2 Å reaction distances (C···B at 1.2 →
  1.92 Å < 2.2). `test_perception_matches_valence_at_default_scale` is the quality gate.
- **The endpoint refuses (422) rather than guess wrong**, each with what to do: cut atoms **not
  bonded** → names the actual distance and threshold, suggests raising `scale`; the bond is in a
  **RING** (removing it doesn't split the graph — `moving`'s side still reaches both cut atoms) →
  says it's a cycle, pick a non-cyclic bond (*this IS the cycle detection — operational, no SSSR*);
  `moving` on **neither side** → refuse; **> 2 components** before any cut → names the count.

### Which bond to cut, and the axis atom

- `distance(i, j)` → `cut = (i, j)`, `moving = j`; `angle(i, v, j)` → `cut = (v, j)`, `moving = j`;
  `dihedral(i, j, k, l)` → `cut = (j, k)`, `moving = l`.
- **Axis atom → automatically outside the mask.** For a dihedral `cut = (j, k)` is the rotation
  axis: `k` lands in `moving`'s component but sits ON the axis (it doesn't move). The endpoint
  **drops any cut atom that isn't `moving` itself** from the mask, so the reference atoms fall
  outside — exactly what `set-internal`'s reference-atom rule needs. For a distance/angle the mover
  IS a cut atom, so nothing is dropped and the mask is the full moving side.

### `within` restricts perception to one fragment (metal–ligand trap)

The endpoint gets the xyz of the **entire scene**, not one fragment. That is a trap for a
**metal–ligand** scene: at `mult=1.2` the Pd–N threshold is **2.520 Å** (`covalent_radii`: Pd 1.390,
N 0.710) and Pd–C is **2.580 Å** (C 0.760), but a real dative Pd–N bond is **2.05–2.15 Å** — under
the threshold. So perception **fuses the metal centre and its ligands into one molecule**, and a
torsion split inside the substrate would swallow the coordinated reagent.

`within: list[int] | None` fixes this: when given, perception considers **only bonds with BOTH ends
in `within`**, the component universe is restricted to `within`, and the returned mask ⊆ `within`.
**Indices stay GLOBAL** — `within` is a filter, not a re-basing (the 2.5.0 one-index-space rule).
`cut` and `moving` must be inside `within` (else **422** naming `within`). The frontend passes the
editing fragment's atoms, so a split can never reach across into another fragment.
`test_within_threshold_is_actually_crossed` pins the trap with a **computed** contact (Pd at 2.10 Å
from a substrate C, below the 2.580 Å threshold — from covalent radii); the paired test shows the
mask swallows Pd **without** `within` and is clean **with** it.

Acceptance (the intra analogue of set-internal): butane dihedral anti → 60° using this endpoint's
mask through `set-internal` — target `60.000000`; static side unmoved; **rigidity**: every pairwise
distance within each side unchanged (moving-side max dev **4.7e-11**, static-side **8.5e-11**) → a
rigid rotation, not a deformation. Ibuprofen (via `/smiles-to-3d`): cut Cα–COOH → mask = the carboxyl
group **{C, O, O, H} = 4 atoms**, `static_count` 29.

## Versioning + the stale-sidecar handshake

**Versioning rule:** bump `app/__init__.py` `__version__` **minor** every time an endpoint is added
or its request/response shape changes. This is not cosmetic — the Rust core reads it to detect a
sidecar running behind the app.

**Why it exists:** `npm run tauri dev` hot-reloads only the frontend; `SidecarManager::start` runs
once at Rust startup and (in release) launches uvicorn without `--reload`. So after adding an
endpoint, the reloaded frontend calls the new route while Python still runs old code → **404
`{"detail":"Not Found"}`** mid-scenario, with no hint. Full write-up: `wiki/debugging/005`.

**Handshake:** after `/health` answers, the Rust core parses the reported `version` and compares it
against `EXPECTED_MIN_SIDECAR_VERSION` **component-wise as numbers** (string compare is wrong:
`"0.10.0" < "0.9.0"` lexically). Older, or unparseable → `SidecarStatus.status = "stale"` (distinct
from `down`), and the status bar shows it prominently; `SidecarStatus` also carries `version` and
`expected_version`. Mechanics in `wiki/modules/tauri-core.md`.

**Human errors, one wrapper:** `src/sidecar-client.ts` (`postSidecar` + the pure, tested
`describeSidecarError`) is the single path for EditPanel / import-file / smiles: 404 → "older build,
restart" (naming the route); 422 → the `detail` verbatim; 5xx → the `detail` prominently; network →
"isn't running". No caller sees a bare `Not Found`.

**`--reload` in dev:** debug builds (`cfg!(debug_assertions)`) launch uvicorn with
`--reload --reload-dir app`; `start` puts the sidecar in its **own process group** and `stop`/`Drop`
`killpg` the whole tree (the `debugging/004` pattern), so the `--reload` worker child isn't
orphaned. Release builds keep the single non-reload process; the handshake is the guard there.

## Dependencies

fastapi, uvicorn, pydantic, rdkit, ase (pulls numpy/scipy/matplotlib). **cclib is NOT a dependency**
(rejected — ADR-012). Open Babel is NOT a dependency — ASE covers conversions.

## Endpoints (planned, beyond those above)

- ~~`POST /parse` — output → cclib JSON~~ **REJECTED (ADR-012):** result parsing is Rust over
  structured artifacts, not a sidecar/cclib endpoint. Not built, and will not be.
- ~~`POST /manual/build-index` — one-off docs indexing~~ **REJECTED (ADR-013):** manual indexing runs
  in Rust over the raw Markdown docs and writes the Rust-owned SQLite (the sidecar is stateless, and
  only Rust's bundled SQLite carries the FTS5 we tested). Not built, and will not be.
- ~~`GET /manual/search?q=` — FTS query proxy~~ **REJECTED (ADR-013):** Rust queries SQLite directly;
  the sidecar has no read path to the manual index either.

## Conventions

Stateless; Pydantic models for every request/response; pytest with recorded ORCA outputs as
fixtures (`tests/fixtures/`).
