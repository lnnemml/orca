"""Geometry kernel (Phase 2.5.2c) — set a distance / angle / dihedral to a target
value by moving a masked subgroup, on ASE.

This is a **silent-error surface**: a wrong result here does not crash — it
returns coordinates from which ORCA happily computes *different* chemistry. So
the endpoint carries its own post-conditions (count / element order / measured
value), not only tests, and the reference-atom rule that makes sequential
placement safe is enforced as validation, not left to the caller.

The sidecar knows nothing about scenes or fragments: the caller (the frontend)
computes the mask — the set of atoms allowed to move — and sends it as an
explicit list of global indices (ADR-008 / the 2.5.0 decision;
`fragmentAtomIndices` *is* this mask). The atom index space is the same on both
sides of HTTP: index N in the request xyz is index N in the response xyz.

ASE mapping (verified against ase 3.29.0 — see wiki/modules/sidecar.md):
- `set_distance(a0, a1, d, fix=0, indices=mask)` — a0 is the fixed reference,
  a1 + the masked group translate along a0→a1. `fix=0` (fix the FIRST atom) is
  required because our mask excludes a0, so all the displacement must fall on the
  a1 side.
- `set_angle(a1, a2, a3, θ, indices=mask)` — rotates the masked group about the
  vertex a2; a1, a2 are the fixed reference, a3 (last) moves.
- `set_dihedral(a1, a2, a3, a4, φ, indices=mask)` — rotates the masked group
  about the a2–a3 axis; a4 (last) moves.
We pass **`indices=`** (a list of atom indices), NOT `mask=` (a boolean array) —
the request already carries a list, so `indices` is the exact fit and overrides
`mask` in all three methods.
"""

from typing import Literal

from ase import Atoms
from ase.neighborlist import natural_cutoffs, neighbor_list
from fastapi import APIRouter, HTTPException
from pydantic import BaseModel

router = APIRouter()

# Atoms defining each internal coordinate (the request's `indices`).
_OP_LEN = {"distance": 2, "angle": 3, "dihedral": 4}
# Tolerance for the measured-value post-condition, per op.
_OP_TOL = {"distance": 1e-6, "angle": 1e-4, "dihedral": 1e-4}

# Covalent-radius multiplier for bond perception (2.5.3a). ASE's own
# `natural_cutoffs` default is mult=1.0, which is TOO TIGHT for us — it misses
# C–H and even C–C in real geometries (butane → 0 bonds). Measured against known
# valence, mult in [1.1, 1.3] all give the right counts (butane 13, benzene 12,
# BH₄⁻ 4, water 2). We take 1.2: comfortably inside that plateau, a little
# tolerance for slightly-stretched real bonds, yet still below the threshold that
# would spuriously bond the ~2.2 Å reaction distances THIS editor creates (C···B
# at 1.2 → 1.92 Å < 2.2). See wiki/modules/sidecar.md.
_COVALENT_SCALE_DEFAULT = 1.2


class SetInternalRequest(BaseModel):
    xyz: str
    op: Literal["distance", "angle", "dihedral"]
    indices: list[int]  # 0-based GLOBAL indices defining the coordinate (2/3/4)
    value: float  # Å for distance, degrees for angle/dihedral
    mask: list[int]  # 0-based GLOBAL indices ALLOWED to move


class SetInternalResponse(BaseModel):
    xyz: str
    measured: float  # RE-DERIVED from the resulting coordinates
    max_static_displacement: float  # largest move of an atom OUTSIDE the mask


def _parse_xyz(xyz: str) -> Atoms:
    """Parse standard xyz (count / comment / rows) into an Atoms object,
    preserving atom order. 422 on anything malformed (never a traceback)."""
    lines = xyz.strip().splitlines()
    if len(lines) < 1:
        raise HTTPException(422, "empty xyz")
    try:
        n = int(lines[0].strip())
    except ValueError:
        raise HTTPException(422, "xyz: first line must be the atom count")
    rows = lines[2 : 2 + n]
    if len(rows) != n:
        raise HTTPException(422, f"xyz: expected {n} atom rows, found {len(rows)}")
    symbols: list[str] = []
    positions: list[list[float]] = []
    for r, row in enumerate(rows):
        parts = row.split()
        if len(parts) < 4:
            raise HTTPException(422, f"xyz: row {r} is not 'element x y z'")
        symbols.append(parts[0])
        try:
            positions.append([float(parts[1]), float(parts[2]), float(parts[3])])
        except ValueError:
            raise HTTPException(422, f"xyz: row {r} has non-numeric coordinates")
    if n == 0:
        raise HTTPException(422, "xyz: no atoms")
    return Atoms(symbols, positions=positions)


def _to_xyz(atoms: Atoms) -> str:
    """Serialise back to standard xyz at high precision (12 dp) so the
    unbroken-round-trip post-conditions downstream hold."""
    symbols = atoms.get_chemical_symbols()
    out = [str(len(atoms)), ""]
    for s, (x, y, z) in zip(symbols, atoms.get_positions()):
        out.append(f"{s} {x:.12f} {y:.12f} {z:.12f}")
    return "\n".join(out) + "\n"


def _validate(req: SetInternalRequest, n: int) -> None:
    """Semantic validation → 422 with a message that names the cause. The
    reference-atom rule (last chain atom moves, the rest are static) is what makes
    distance→angle→dihedral safe to apply in one sequential pass."""
    op = req.op
    idx = req.indices
    mask = req.mask

    # ── the coordinate's atoms ────────────────────────────────────────────────
    if len(idx) != _OP_LEN[op]:
        raise HTTPException(
            422, f"{op} needs {_OP_LEN[op]} indices, got {len(idx)}"
        )
    if any(i < 0 or i >= n for i in idx):
        raise HTTPException(422, f"indices out of range [0, {n})")
    if len(set(idx)) != len(idx):
        raise HTTPException(422, "indices must be distinct")

    # ── the mask ──────────────────────────────────────────────────────────────
    if not mask:
        raise HTTPException(422, "mask must be non-empty")
    if any(i < 0 or i >= n for i in mask):
        raise HTTPException(422, f"mask index out of range [0, {n})")
    if len(set(mask)) != len(mask):
        raise HTTPException(422, "mask must not contain duplicates")
    if len(set(mask)) >= n:
        raise HTTPException(
            422, "mask must be a STRICT subset — at least one atom must be static"
        )

    # ── the reference-atom rule (the load-bearing one) ───────────────────────
    mask_set = set(mask)
    moving, refs = idx[-1], idx[:-1]
    if moving not in mask_set:
        raise HTTPException(
            422,
            f"the last atom of the chain (index {moving}) must be IN the mask — "
            "it is the atom the operation moves",
        )
    in_mask_refs = [r for r in refs if r in mask_set]
    if in_mask_refs:
        raise HTTPException(
            422,
            f"reference atoms {in_mask_refs} must NOT be in the mask: the "
            "reference atoms are taken from the static (substrate) side so that a "
            "later operation cannot undo an earlier one. Only the last atom of the "
            "chain moves.",
        )

    # ── the target value ──────────────────────────────────────────────────────
    if op == "distance" and req.value <= 0:
        raise HTTPException(422, "distance must be > 0")
    if op == "angle" and not (0 < req.value < 180):
        raise HTTPException(422, "angle must be in (0, 180) degrees")
    # dihedral: any real value is fine — it folds into [0, 360).


def _circular_error(measured: float, target: float) -> float:
    """Smallest angular distance between two degree values, handling the 359.99
    vs 0.01 wrap (both name ~the same dihedral)."""
    return abs((measured - target + 180.0) % 360.0 - 180.0)


@router.post("/geometry/set-internal", response_model=SetInternalResponse)
def set_internal(req: SetInternalRequest) -> SetInternalResponse:
    atoms = _parse_xyz(req.xyz)
    n = len(atoms)
    _validate(req, n)

    before = atoms.get_positions().copy()
    before_symbols = atoms.get_chemical_symbols()
    idx = req.indices
    mask = req.mask

    if req.op == "distance":
        i, j = idx
        atoms.set_distance(i, j, req.value, fix=0, indices=mask)
        measured = atoms.get_distance(i, j)
        target, err = req.value, abs(measured - req.value)
    elif req.op == "angle":
        i, v, j = idx
        atoms.set_angle(i, v, j, req.value, indices=mask)
        measured = atoms.get_angle(i, v, j)
        target, err = req.value, abs(measured - req.value)
    else:  # dihedral
        i, j, k, ll = idx
        atoms.set_dihedral(i, j, k, ll, req.value, indices=mask)
        measured = atoms.get_dihedral(i, j, k, ll)
        target, err = req.value % 360.0, _circular_error(measured, req.value)

    # ── Post-conditions: the endpoint refuses to return a wrong result ────────
    after = atoms.get_positions()
    if len(atoms) != n:
        raise HTTPException(500, f"atom count changed: {n} -> {len(atoms)}")
    if atoms.get_chemical_symbols() != before_symbols:
        raise HTTPException(500, "element sequence changed during the operation")
    if err > _OP_TOL[req.op]:
        raise HTTPException(
            500,
            f"{req.op} not reached: target {target}, measured {measured} "
            f"(error {err:.3e} > tol {_OP_TOL[req.op]:.0e})",
        )

    mask_set = set(mask)
    max_static = max(
        (
            float(((after[a] - before[a]) ** 2).sum() ** 0.5)
            for a in range(n)
            if a not in mask_set
        ),
        default=0.0,
    )

    return SetInternalResponse(
        xyz=_to_xyz(atoms),
        measured=float(measured),
        max_static_displacement=max_static,
    )


# ── Bond-graph mask split (2.5.3a) ────────────────────────────────────────────
# For an INTRA-fragment edit — rotating a molecule's own torsion — the mask is
# not a whole fragment but the connected side of a broken bond. Bond perception
# is a GUESS from geometry (covalent radii × a multiplier), and this editor can
# create geometries where the guess is wrong (a stretched bond vanishing, close
# fragments spuriously bonding). So the multiplier is an explicit parameter, and
# the endpoint REFUSES with an explanation when the guess produces something odd
# rather than returning a silently-wrong mask.


class RotatableMaskRequest(BaseModel):
    xyz: str
    cut: list[int]  # the bond to break: [i, j], 0-based GLOBAL indices
    moving: int  # an atom on the side that should move
    scale: float = _COVALENT_SCALE_DEFAULT  # covalent-radius multiplier


class RotatableMaskResponse(BaseModel):
    mask: list[int]  # sorted global indices of the moving side
    static_count: int  # atoms left fixed
    cut_length: float  # length of the broken bond, Å (for the UI)


def _bond_edges(atoms: Atoms, cutoffs: list[float]) -> set[frozenset[int]]:
    """Perceived covalent bonds as `{frozenset({i, j})}`: a bond exists iff
    `d_ij < cutoffs[i] + cutoffs[j]` (`cutoffs` from `natural_cutoffs(atoms,
    mult=scale)`). Non-periodic input — no cell needed."""
    i_arr, j_arr = neighbor_list("ij", atoms, cutoffs)
    return {frozenset((int(a), int(b))) for a, b in zip(i_arr, j_arr)}


def _components(
    n: int, edges: set[frozenset[int]], exclude: frozenset[int] | None = None
) -> list[set[int]]:
    """Connected components of an `n`-node graph, optionally without one edge."""
    adj: dict[int, set[int]] = {k: set() for k in range(n)}
    for e in edges:
        if e == exclude:
            continue
        a, b = tuple(e)
        adj[a].add(b)
        adj[b].add(a)
    seen: set[int] = set()
    comps: list[set[int]] = []
    for start in range(n):
        if start in seen:
            continue
        stack = [start]
        comp: set[int] = set()
        while stack:
            x = stack.pop()
            if x in seen:
                continue
            seen.add(x)
            comp.add(x)
            stack.extend(adj[x] - seen)
        comps.append(comp)
    return comps


@router.post("/geometry/rotatable-mask", response_model=RotatableMaskResponse)
def rotatable_mask(req: RotatableMaskRequest) -> RotatableMaskResponse:
    atoms = _parse_xyz(req.xyz)
    n = len(atoms)

    # ── request shape ─────────────────────────────────────────────────────────
    if len(req.cut) != 2:
        raise HTTPException(422, f"cut must be a bond [i, j], got {len(req.cut)} atoms")
    i, j = req.cut
    if i == j:
        raise HTTPException(422, "cut atoms must be distinct")
    for label, a in (("cut", i), ("cut", j), ("moving", req.moving)):
        if a < 0 or a >= n:
            raise HTTPException(422, f"{label} index {a} out of range [0, {n})")
    if req.scale <= 0:
        raise HTTPException(422, "scale must be > 0")

    cutoffs = natural_cutoffs(atoms, mult=req.scale)
    edges = _bond_edges(atoms, cutoffs)
    cut_length = float(atoms.get_distance(i, j))

    # ── the cut atoms must actually be perceived as bonded ────────────────────
    if frozenset((i, j)) not in edges:
        threshold = float(cutoffs[i] + cutoffs[j])
        raise HTTPException(
            422,
            f"atoms {i} and {j} are not bonded: they are {cut_length:.3f} Å apart, "
            f"but a bond needs < {threshold:.3f} Å (covalent radii × scale {req.scale}). "
            "Pick a real bond, or raise the scale if this bond is genuinely stretched.",
        )

    # ── perception sanity: an isolated molecule is one component; substrate +
    # reagent is two. More than two means perception produced something odd
    # (a lone atom, or a fragment split by a stretched bond). ───────────────────
    comps_before = _components(n, edges)
    if len(comps_before) > 2:
        raise HTTPException(
            422,
            f"bond perception split the structure into {len(comps_before)} pieces "
            "before any cut — that usually means a stretched bond or a stray atom. "
            "Check the geometry, or adjust the scale.",
        )

    # ── remove the bond, find the moving side ─────────────────────────────────
    comps_after = _components(n, edges, exclude=frozenset((i, j)))
    moving_comp = next(c for c in comps_after if req.moving in c)

    if i in moving_comp and j in moving_comp:
        raise HTTPException(
            422,
            f"bond {i}–{j} is in a RING: removing it does not split the molecule "
            f"(atom {req.moving}'s side still reaches both ends), so there is no "
            "rotatable subgroup. Rotating a ring bond would deform the ring — pick "
            "a bond that is not part of a cycle.",
        )
    if i not in moving_comp and j not in moving_comp:
        raise HTTPException(
            422,
            f"the moving atom {req.moving} is not on either side of bond {i}–{j} "
            "(it sits in a disconnected part of the structure). Pick a moving atom "
            "adjacent to the bond being rotated.",
        )

    # Drop any cut atom that is NOT the mover itself. For a dihedral the cut is
    # the rotation AXIS (j, k): k lands in the mover's component but sits ON the
    # axis — it's a reference atom that doesn't move, so dropping it makes this a
    # valid set-internal mask (the reference atoms fall outside automatically —
    # task 3). For a distance/angle the mover IS a cut atom, so nothing is
    # dropped and the mask is the full moving side.
    drop = {c for c in req.cut if c != req.moving}
    mask = sorted(moving_comp - drop)
    return RotatableMaskResponse(
        mask=mask,
        static_count=n - len(mask),
        cut_length=cut_length,
    )
