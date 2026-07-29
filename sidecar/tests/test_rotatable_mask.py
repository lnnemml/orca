"""Tests for the bond-graph mask split (POST /geometry/rotatable-mask, 2.5.3a).

The load-bearing quality test is (a): bond PERCEPTION checked against known
valence for four molecules (including the charged BH₄⁻ trap). Perception is a
guess from geometry; if the multiplier is wrong these counts are wrong, and every
mask built on them is silently wrong.
"""

import itertools
import math

import numpy as np
from ase import Atoms
from ase.neighborlist import natural_cutoffs
from fastapi.testclient import TestClient

from app.geometry import _COVALENT_SCALE_DEFAULT, _bond_edges, _components
from app.main import app

client = TestClient(app)


# ── geometries ────────────────────────────────────────────────────────────────

# butane anti (the project's GOAT fixture) — carbons are atoms 0,1,2,3.
BUTANE_TXT = """C 1.93939185364579236293 0.07737406152119787051 0.24304182052609096809
C 0.53797179522872373703 0.18841655754956676549 -0.34457949257460168679
C -0.53797295432378267055 -0.18844136149474677300 0.67101001653575198524
C -1.93939200460205274368 -0.07735434549765553280 0.08339293823298575548
H 2.13568247859938598054 -0.94115392278442766560 0.57149043429382551107
H 2.68872000526646859342 0.34971942494684188363 -0.49673182931641063353
H 2.04710290775211722902 0.73876554547227901715 1.10022637882360330153
H 0.45628686535958468129 -0.46727555515909763306 -1.21471751981858355762
H 0.36748852742773696622 1.21234497266518936875 -0.68553458663930899597
H -0.45626639617516862035 0.46723783351233355576 1.54115443156927311996
H -0.36750752592934021745 -1.21238355959702492903 1.01192941848362494284
H -2.13578267466600157931 0.94125287881583941108 -0.24474770159437347905
H -2.04701298168341550010 -0.73849555042777181857 -0.77399646430233681738
H -2.68870889590005024417 -0.35000697952252329825 0.82306515578045891246"""


def _mk(txt: str) -> Atoms:
    """Parse RAW atom lines (no count/comment) — for the hardcoded fixtures."""
    syms, pos = [], []
    for ln in txt.strip().splitlines():
        q = ln.split()
        syms.append(q[0])
        pos.append([float(x) for x in q[1:4]])
    return Atoms(syms, positions=pos)


def _from_xyz(xyz: str) -> Atoms:
    """Parse a STANDARD xyz (count / comment / rows) — for endpoint responses."""
    lines = xyz.strip().splitlines()
    n = int(lines[0])
    return _mk("\n".join(lines[2 : 2 + n]))


def _to_xyz(atoms: Atoms) -> str:
    out = [str(len(atoms)), ""]
    for s, (x, y, z) in zip(atoms.get_chemical_symbols(), atoms.get_positions()):
        out.append(f"{s} {x:.10f} {y:.10f} {z:.10f}")
    return "\n".join(out) + "\n"


def _butane() -> Atoms:
    return _mk(BUTANE_TXT)


def _water() -> Atoms:
    return Atoms("OHH", positions=[[0, 0, 0.117], [0, 0.7554, -0.4712], [0, -0.7554, -0.4712]])


def _bh4() -> Atoms:
    d = 1.2368 / math.sqrt(3)
    return Atoms("BHHHH", positions=[[0, 0, 0], [d, d, d], [-d, -d, d], [-d, d, -d], [d, -d, -d]])


def _benzene() -> Atoms:
    cpos, hpos = [], []
    for k in range(6):
        a = math.radians(60 * k)
        cpos.append([1.39 * math.cos(a), 1.39 * math.sin(a), 0])
        hpos.append([2.48 * math.cos(a), 2.48 * math.sin(a), 0])
    return Atoms("C6H6", positions=cpos + hpos)


def _rotmask(atoms: Atoms, cut, moving, scale=None, within=None):
    body = {"xyz": _to_xyz(atoms), "cut": list(cut), "moving": moving}
    if scale is not None:
        body["scale"] = scale
    if within is not None:
        body["within"] = within
    return client.post("/geometry/rotatable-mask", json=body)


def _n_bonds(atoms: Atoms, scale: float) -> int:
    edges = _bond_edges(atoms, natural_cutoffs(atoms, mult=scale))
    return len(edges)


# ── (a) perception vs valence — the quality test ─────────────────────────────


def test_perception_matches_valence_at_default_scale():
    """At the shipped default multiplier, perceived bond counts equal the known
    valence: butane 13 (3 C–C + 10 C–H), benzene 12 (6 ring + 6 C–H), BH₄⁻ 4
    (the charged trap), water 2 — same multiplier for all, charged included."""
    scale = _COVALENT_SCALE_DEFAULT
    assert _n_bonds(_butane(), scale) == 13
    assert _n_bonds(_benzene(), scale) == 12
    assert _n_bonds(_bh4(), scale) == 4
    assert _n_bonds(_water(), scale) == 2


# ── (b) split ─────────────────────────────────────────────────────────────────


def test_split_butane_central_bond():
    """Cut the C1–C2 bond (indices 1,2), move the C2 side. The mask is the moving
    half by ELEMENT COMPOSITION + count, not a literal index list: one methyl
    carbon (C3) + the hydrogens that rotate (C2's two H + C3's three H) = 1 C +
    5 H. The axis carbon C2 itself is dropped (it sits on the rotation axis)."""
    at = _butane()
    r = _rotmask(at, cut=(1, 2), moving=3)
    assert r.status_code == 200, r.text
    mask = r.json()["mask"]
    els = sorted(at.get_chemical_symbols()[i] for i in mask)
    assert els == ["C", "H", "H", "H", "H", "H"]  # 1 C + 5 H
    assert 1 not in mask and 2 not in mask  # both axis atoms are static
    assert 3 in mask  # the moving methyl carbon
    assert r.json()["static_count"] == len(at) - len(mask)
    assert r.json()["cut_length"] > 0


# ── (c) ring ──────────────────────────────────────────────────────────────────


def test_ring_bond_is_refused_with_a_cycle_explanation():
    at = _benzene()  # ring carbons 0..5
    r = _rotmask(at, cut=(0, 1), moving=1)  # adjacent ring carbons
    assert r.status_code == 422
    detail = r.json()["detail"].lower()
    assert "ring" in detail
    assert "not part of a cycle" in detail  # tells the user what to do


# ── (d) not bonded ────────────────────────────────────────────────────────────


def test_unbonded_cut_is_refused_with_the_distance():
    at = Atoms("HH", positions=[[0, 0, 0], [5, 0, 0]])  # 5 Å apart
    r = _rotmask(at, cut=(0, 1), moving=1)
    assert r.status_code == 422
    detail = r.json()["detail"]
    assert "not bonded" in detail.lower()
    assert "5.000" in detail  # names the actual distance
    assert "Å" in detail


def test_moving_atom_off_the_cut_is_refused():
    # butane + a separate water FAR away (a different fragment). Cut a butane
    # bond, but ask to move a WATER atom → water touches neither cut atom.
    w = _water()
    w.translate([10, 0, 0])
    at = _butane() + w  # water O at index 14
    r = _rotmask(at, cut=(1, 2), moving=14)
    assert r.status_code == 422
    assert "not on either side" in r.json()["detail"].lower()


def test_more_than_two_components_is_refused():
    # butane + two stray far-apart atoms → perception yields 3 components.
    at = _butane()
    at += Atoms("He", positions=[[50, 0, 0]])
    at += Atoms("He", positions=[[-50, 0, 0]])
    r = _rotmask(at, cut=(1, 2), moving=3)
    assert r.status_code == 422
    assert "pieces" in r.json()["detail"].lower()
    assert "3" in r.json()["detail"]


# ── (e) acceptance: mask from here, applied via set-internal, rigid rotation ──


def _set_internal(xyz, op, indices, value, mask):
    return client.post(
        "/geometry/set-internal",
        json={"xyz": xyz, "op": op, "indices": indices, "value": value, "mask": mask},
    )


def test_acceptance_intra_dihedral_is_a_rigid_rotation():
    at = _butane()
    xyz = _to_xyz(at)
    # dihedral C0-C1-C2-C3 (anti ≈ 180) → 60°. cut = axis (C1,C2), moving = C3.
    r = _rotmask(at, cut=(1, 2), moving=3)
    assert r.status_code == 200, r.text
    mask = r.json()["mask"]

    # The two rigid sides of the cut (from the same graph), for the rigidity check.
    comps = _components(
        len(at), _bond_edges(at, natural_cutoffs(at, mult=_COVALENT_SCALE_DEFAULT)),
        exclude=frozenset((1, 2)),
    )
    moving_side = next(c for c in comps if 3 in c)
    static_side = set(range(len(at))) - moving_side

    before = at.get_positions().copy()
    resp = _set_internal(xyz, "dihedral", [0, 1, 2, 3], 60.0, mask)
    assert resp.status_code == 200, resp.text
    out = _from_xyz(resp.json()["xyz"])

    # target reached
    measured = out.get_dihedral(0, 1, 2, 3)
    print(f"\n  dihedral target 60.0  measured {measured:.6f}")
    assert abs((measured - 60.0 + 180) % 360 - 180) < 1e-4

    # static atoms didn't move
    after = out.get_positions()
    static_dev = max(np.linalg.norm(after[i] - before[i]) for i in static_side)
    assert static_dev < 1e-9
    assert resp.json()["max_static_displacement"] < 1e-9

    # RIGIDITY: every pairwise distance WITHIN each side is unchanged → the split
    # produced a rigid rotation, not a deformation.
    def max_pair_dev(side):
        return max(
            (
                abs(
                    np.linalg.norm(after[i] - after[j])
                    - np.linalg.norm(before[i] - before[j])
                )
                for i, j in itertools.combinations(sorted(side), 2)
            ),
            default=0.0,
        )

    mov_dev, stat_dev = max_pair_dev(moving_side), max_pair_dev(static_side)
    print(f"  rigidity: moving-side max pair dev {mov_dev:.2e}, static-side {stat_dev:.2e}")
    assert mov_dev < 1e-9
    assert stat_dev < 1e-9

    # count + order preserved
    assert out.get_chemical_symbols() == at.get_chemical_symbols()


# ── (f) ibuprofen: cut Cα–COOH, mask = the carboxyl group ────────────────────


def test_ibuprofen_carboxyl_split_has_the_expected_size():
    # generate the real molecule via the existing SMILES endpoint.
    gen = client.post("/smiles-to-3d", json={"smiles": "CC(C)Cc1ccc(cc1)C(C)C(=O)O"})
    assert gen.status_code == 200, gen.text
    at = _from_xyz(gen.json()["xyz"])
    n = len(at)
    assert n == 33  # C13H18O2

    edges = _bond_edges(at, natural_cutoffs(at, mult=_COVALENT_SCALE_DEFAULT))
    adj: dict[int, set[int]] = {i: set() for i in range(n)}
    for e in edges:
        a, b = tuple(e)
        adj[a].add(b)
        adj[b].add(a)
    syms = at.get_chemical_symbols()

    # carboxyl carbon = a C bonded to exactly two O; Cα = its carbon neighbour.
    carboxyl_c = next(
        i for i in range(n)
        if syms[i] == "C" and sum(syms[k] == "O" for k in adj[i]) == 2
    )
    c_alpha = next(k for k in adj[carboxyl_c] if syms[k] == "C")

    r = _rotmask(at, cut=(c_alpha, carboxyl_c), moving=carboxyl_c)
    assert r.status_code == 200, r.text
    mask = r.json()["mask"]
    # The moving side is the –COOH group: the carboxyl C, its two O, and the
    # hydroxyl H = 4 atoms. (Cα and the rest of ibuprofen stay.)
    mask_els = sorted(syms[i] for i in mask)
    print(f"\n  ibuprofen COOH mask size {len(mask)} elements {mask_els}")
    assert len(mask) == 4
    assert mask_els == ["C", "H", "O", "O"]
    assert r.json()["static_count"] == n - 4


# ── within: perception restricted to one fragment (2.5.3b) ───────────────────


def _metal_ligand_scene():
    """A C-chain substrate (torsion C1–C2) + a Pd reagent at 2.1 Å from the
    terminal C3. The Pd–C contact (2.1 Å) is BELOW the mult=1.2 threshold
    (C–Pd = 2.580 Å), so whole-scene perception fuses Pd into the substrate —
    exactly the ADR-007 metal+ligand trap (real Pd–N is 2.05–2.15 Å, threshold
    2.520 Å). Substrate = indices 0..3, reagent Pd = index 4."""
    at = Atoms("CCCC", positions=[[0, 0, 0], [1.54, 0, 0], [3.08, 0, 0], [4.62, 0, 0]])
    at += Atoms("Pd", positions=[[4.62, 2.1, 0]])  # 2.1 Å from C3, off-axis
    return at


def test_within_threshold_is_actually_crossed():
    # Prove the contact crosses the perception threshold (computed, not guessed).
    at = _metal_ligand_scene()
    cut = natural_cutoffs(at, mult=_COVALENT_SCALE_DEFAULT)
    threshold = cut[3] + cut[4]  # C3 + Pd
    contact = at.get_distance(3, 4)
    assert contact < threshold  # 2.100 < 2.580 → perceived as bonded
    assert frozenset((3, 4)) in _bond_edges(at, cut)  # Pd fused into the substrate


def test_without_within_the_mask_swallows_the_reagent_documented_trap():
    # Cutting the substrate torsion C1–C2, moving C3 — WITHOUT `within`, the
    # moving side reaches the Pd through the spurious contact, so the reagent
    # (index 4) ends up in the mask. This is the trap `within` exists to fix; it
    # is pinned here, not merely described.
    at = _metal_ligand_scene()
    r = _rotmask(at, cut=(1, 2), moving=3)  # no `within`
    assert r.status_code == 200, r.text
    assert 4 in r.json()["mask"]  # the Pd reagent is wrongly captured


def test_within_keeps_the_mask_inside_the_substrate():
    at = _metal_ligand_scene()
    r = _rotmask(at, cut=(1, 2), moving=3, within=[0, 1, 2, 3])
    assert r.status_code == 200, r.text
    mask = r.json()["mask"]
    assert 4 not in mask  # the Pd reagent is NOT moved
    assert set(mask) <= {0, 1, 2, 3}  # mask is a subset of `within`
    assert mask == [3]  # only the moving terminal carbon (axis carbon 2 dropped)


def test_within_must_contain_cut_and_moving():
    at = _metal_ligand_scene()
    # cut atom 2 is not in `within`
    r = _rotmask(at, cut=(1, 2), moving=1, within=[0, 1])
    assert r.status_code == 422
    assert "within" in r.json()["detail"].lower()

