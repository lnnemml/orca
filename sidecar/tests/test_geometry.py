"""Tests for the ASE geometry kernel (POST /geometry/set-internal).

Two tripping guards, not one test set (this is a silent-error surface):
- (a) the DIHEDRAL CONVENTION tripwire — the same butane coordinates measure.ts
  pins to 67.523° / 179.998°, re-checked against ASE here;
- (c) the SEQUENTIAL d/θ/φ acceptance test from the 2026-07-28 decision —
  recompute all three from the final coordinates and match the TARGETS.
"""

import math

import numpy as np
from ase import Atoms
from fastapi.testclient import TestClient

from app.main import app

client = TestClient(app)


# ── Fixtures ──────────────────────────────────────────────────────────────────

# Butane conformers from src/scene/__fixtures__/butane.finalensemble.xyz — the
# SAME coordinates measure.test.ts uses (carbons are atoms 0,1,2,3).
BUTANE_ANTI = """14
anti
C 1.93939185364579236293 0.07737406152119787051 0.24304182052609096809
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
H -2.68870889590005024417 -0.35000697952252329825 0.82306515578045891246
"""

BUTANE_GAUCHE = """14
gauche
C 1.57493837534088720886 0.47560965087875917146 0.28746743038890165689
C 0.62028514539525791971 -0.37785166769397016129 -0.53930644713614017682
C -0.64413190616412951961 -0.77636815711506135251 0.22269040683532481673
C -1.55681068554076151855 0.40522811341468101265 0.52970409616902125460
H 1.84072813978161531345 -0.03642029037517269252 1.21005136198805751668
H 2.48773877337920579222 0.67014682530236679980 -0.27139603343838297755
H 1.12465750081146698136 1.43159957300775864120 0.54128168079159588721
H 1.14049403580416219839 -1.28714876858371973789 -0.85058159629168583749
H 0.33778352409513645593 0.16643320820288007300 -1.44353132318610333229
H -0.36075279667117676752 -1.26780892734441685832 1.15642404029273149213
H -1.19803127054222802172 -1.50266986554063564085 -0.37734113498986981972
H -2.47232234638838477281 0.06134923645843771978 1.00605499202453874119
H -1.07165667530126729190 1.10969862507090288872 1.20010746596129380670
H -1.82291881399980049139 0.92820244431718368361 -0.38662193940927253033
"""

# Substrate carbonyl H2C=O (0=C, 1=O, 2=Ha, 3=Hb) + reagent hydride (4=H⁻).
CARBONYL_HYDRIDE = (
    "5\ncarbonyl+hydride\n"
    "C 0.0 0.0 0.0\n"
    "O 0.0 0.0 1.21\n"
    "H 0.94 0.0 -0.54\n"
    "H -0.94 0.0 -0.54\n"
    "H 0.3 2.0 2.0\n"
)


def _atoms_from_xyz(xyz: str) -> Atoms:
    lines = xyz.strip().splitlines()
    n = int(lines[0])
    syms, pos = [], []
    for row in lines[2 : 2 + n]:
        p = row.split()
        syms.append(p[0])
        pos.append([float(p[1]), float(p[2]), float(p[3])])
    return Atoms(syms, positions=pos)


def _set(xyz, op, indices, value, mask):
    return client.post(
        "/geometry/set-internal",
        json={"xyz": xyz, "op": op, "indices": indices, "value": value, "mask": mask},
    )


# ── (a) The convention tripwire ──────────────────────────────────────────────


def test_dihedral_convention_tripwire():
    """ASE must reproduce the [0,360) numbers measure.ts pinned in 2.5.2b — anti
    179.998°, gauche 67.523° — to 3 dp. If this fails, a wrong assumption sits in
    ASE or in measure.ts and must be FOUND, not smoothed over."""
    anti = _atoms_from_xyz(BUTANE_ANTI)
    gauche = _atoms_from_xyz(BUTANE_GAUCHE)
    assert round(anti.get_dihedral(0, 1, 2, 3), 3) == 179.998
    assert round(gauche.get_dihedral(0, 1, 2, 3), 3) == 67.523


# ── (b) Each operation on its own ────────────────────────────────────────────


def test_distance_hits_target_static_atoms_frozen():
    # move the reagent hydride (4) relative to the fixed carbonyl C (0)
    r = _set(CARBONYL_HYDRIDE, "distance", [0, 4], 1.5, [4])
    assert r.status_code == 200, r.text
    body = r.json()
    assert abs(body["measured"] - 1.5) < 1e-6
    assert body["max_static_displacement"] < 1e-9
    out = _atoms_from_xyz(body["xyz"])
    assert len(out) == 5
    assert out.get_chemical_symbols() == ["C", "O", "H", "H", "H"]


def test_angle_hits_target():
    r = _set(CARBONYL_HYDRIDE, "angle", [1, 0, 4], 107.0, [4])  # O-C-H⁻, vertex C
    assert r.status_code == 200, r.text
    body = r.json()
    assert abs(body["measured"] - 107.0) < 1e-4
    assert body["max_static_displacement"] < 1e-9


def test_dihedral_hits_target_and_folds():
    # value −60 must land as 300 in [0,360)
    r = _set(CARBONYL_HYDRIDE, "dihedral", [2, 1, 0, 4], -60.0, [4])
    assert r.status_code == 200, r.text
    body = r.json()
    assert abs(body["measured"] - 300.0) < 1e-4
    assert body["max_static_displacement"] < 1e-9


# ── (c) The sequential d/θ/φ acceptance test (2026-07-28 decision) ───────────


def test_sequential_burgi_dunitz_acceptance():
    """substrate carbonyl + reagent hydride; apply distance→angle→dihedral, each a
    SEPARATE endpoint call whose input is the previous output; then recompute all
    three from the FINAL coordinates and match the TARGETS (not the intermediates).
    Also: the substrate's internal geometry is untouched."""
    D_TARGET, TH_TARGET, PHI_TARGET = 1.5, 107.0, 90.0
    mask = [4]  # the reagent hydride moves; substrate {0,1,2,3} is static

    sub_before = _atoms_from_xyz(CARBONYL_HYDRIDE)
    subs = [0, 1, 2, 3]
    pairs_before = {
        (i, j): sub_before.get_distance(i, j)
        for i in subs
        for j in subs
        if i < j
    }

    # 1) |C–H⁻|
    r1 = _set(CARBONYL_HYDRIDE, "distance", [0, 4], D_TARGET, mask)
    assert r1.status_code == 200, r1.text
    # 2) ∠O–C–H⁻ (vertex C)
    r2 = _set(r1.json()["xyz"], "angle", [1, 0, 4], TH_TARGET, mask)
    assert r2.status_code == 200, r2.text
    # 3) dihedral Ha–O–C–H⁻ (axis O–C)
    r3 = _set(r2.json()["xyz"], "dihedral", [2, 1, 0, 4], PHI_TARGET, mask)
    assert r3.status_code == 200, r3.text

    final = _atoms_from_xyz(r3.json()["xyz"])
    d = final.get_distance(0, 4)
    th = final.get_angle(1, 0, 4)
    phi = final.get_dihedral(2, 1, 0, 4)
    # The three targets and the three recomputed numbers (printed for the report).
    print(f"\n  targets   d={D_TARGET} θ={TH_TARGET} φ={PHI_TARGET}")
    print(f"  recomputed d={d:.8f} θ={th:.6f} φ={phi:.6f}")
    assert abs(d - D_TARGET) < 1e-6
    assert abs(th - TH_TARGET) < 1e-4
    assert abs(phi - PHI_TARGET) < 1e-4

    # substrate internal geometry preserved
    max_dev = max(
        abs(final.get_distance(i, j) - pairs_before[(i, j)])
        for i in subs
        for j in subs
        if i < j
    )
    assert max_dev < 1e-9


# ── (d) Idempotence ──────────────────────────────────────────────────────────


def test_idempotent_on_current_value():
    # feed the CURRENT measured value → coordinates must not move
    a = _atoms_from_xyz(CARBONYL_HYDRIDE)
    cur = a.get_angle(1, 0, 4)
    r = _set(CARBONYL_HYDRIDE, "angle", [1, 0, 4], cur, [4])
    assert r.status_code == 200, r.text
    out = _atoms_from_xyz(r.json()["xyz"])
    assert np.max(np.abs(out.get_positions() - a.get_positions())) < 1e-9


# ── (e) Rigid-motion invariance ──────────────────────────────────────────────


def test_rigid_motion_invariance():
    """Rotate + translate the whole input, apply the same op → measured is the
    same to 1e-9 (the value is intrinsic, not frame-dependent)."""
    plain = _set(CARBONYL_HYDRIDE, "distance", [0, 4], 1.7, [4]).json()["measured"]

    a = _atoms_from_xyz(CARBONYL_HYDRIDE)
    # explicit proper rotation Rz·Ry·Rx (fixed angles) + translation
    ax, ay, az = 0.3, -0.7, 1.1
    cx, sx = math.cos(ax), math.sin(ax)
    cy, sy = math.cos(ay), math.sin(ay)
    cz, sz = math.cos(az), math.sin(az)
    R = np.array(
        [
            [cz * cy, cz * sy * sx - sz * cx, cz * sy * cx + sz * sx],
            [sz * cy, sz * sy * sx + cz * cx, sz * sy * cx - cz * sx],
            [-sy, cy * sx, cy * cx],
        ]
    )
    moved = (a.get_positions() @ R.T) + np.array([1.234, -5.6, 7.89])
    moved_xyz = "5\nmoved\n" + "\n".join(
        f"{s} {x:.12f} {y:.12f} {z:.12f}"
        for s, (x, y, z) in zip(a.get_chemical_symbols(), moved)
    )
    rigid = _set(moved_xyz, "distance", [0, 4], 1.7, [4]).json()["measured"]
    assert abs(plain - rigid) < 1e-9


# ── (f) Validation ───────────────────────────────────────────────────────────


def test_reference_atom_in_mask_is_rejected():
    """The load-bearing rule: a mask containing a REFERENCE atom is refused, and
    the message explains why (reference atoms are the static substrate side)."""
    # angle O(1)-C(0)-H⁻(4): put the reference C (0) in the mask alongside 4
    r = _set(CARBONYL_HYDRIDE, "angle", [1, 0, 4], 107.0, [0, 4])
    assert r.status_code == 422
    detail = r.json()["detail"].lower()
    assert "reference" in detail and "mask" in detail


def test_moving_atom_not_in_mask_is_rejected():
    # distance i-j needs j in the mask; here the mask has only the reference i
    r = _set(CARBONYL_HYDRIDE, "distance", [0, 4], 1.5, [0])
    assert r.status_code == 422
    assert "last atom" in r.json()["detail"].lower()


def test_empty_mask_is_rejected():
    r = _set(CARBONYL_HYDRIDE, "distance", [0, 4], 1.5, [])
    assert r.status_code == 422
    assert "non-empty" in r.json()["detail"].lower()


def test_mask_all_atoms_is_rejected():
    r = _set(CARBONYL_HYDRIDE, "distance", [0, 4], 1.5, [0, 1, 2, 3, 4])
    assert r.status_code == 422
    assert "strict subset" in r.json()["detail"].lower()


def test_index_out_of_range_is_rejected():
    r = _set(CARBONYL_HYDRIDE, "distance", [0, 99], 1.5, [4])
    assert r.status_code == 422
    assert "out of range" in r.json()["detail"].lower()


def test_wrong_indices_length_is_rejected():
    r = _set(CARBONYL_HYDRIDE, "angle", [0, 4], 107.0, [4])  # angle needs 3
    assert r.status_code == 422
    assert "needs 3" in r.json()["detail"].lower()


def test_bad_value_is_rejected():
    assert _set(CARBONYL_HYDRIDE, "distance", [0, 4], -1.0, [4]).status_code == 422
    assert _set(CARBONYL_HYDRIDE, "angle", [1, 0, 4], 200.0, [4]).status_code == 422
