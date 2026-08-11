"""Tests for POST /geometry/connected-component (Stage 3.x — a drag moves the
dragged atom's PERCEIVED connected component, not the whole fragment).

The load-bearing (negative) test is the broken-bond HCN case that surfaced this:
break H–C geometrically (move H far away) and the component of H must be the lone
`[H]`, while C still travels with N as `[C, N]`. A fully-bonded HCN is the
backward-compat control: every atom's component is all three (== the whole
fragment → the drag is unchanged).
"""

from fastapi.testclient import TestClient

from app.main import app

client = TestClient(app)


def _xyz(rows: list[str]) -> str:
    return f"{len(rows)}\n\n" + "\n".join(rows) + "\n"


# Linear HCN along x: H(0) at 0, C(1) at 1.07, N(2) at 2.22 Å.
# H–C ≈ 1.07, C–N ≈ 1.15, H···N ≈ 2.22 (unbonded at scale 1.2).
HCN_BONDED = _xyz(
    [
        "H 0.000 0.0 0.0",
        "C 1.070 0.0 0.0",
        "N 2.220 0.0 0.0",
    ]
)

# H pulled far from C → the H–C bond vanishes under perception (4.07 Å apart);
# C–N is untouched. This is the "broken H–C" HCN→HNC geometry.
HCN_BROKEN_HC = _xyz(
    [
        "H -3.000 0.0 0.0",
        "C 1.070 0.0 0.0",
        "N 2.220 0.0 0.0",
    ]
)


def _component(xyz: str, atom: int) -> list[int]:
    r = client.post("/geometry/connected-component", json={"xyz": xyz, "atom": atom})
    assert r.status_code == 200, r.text
    return r.json()["component"]


# ── backward-compat: a fully-bonded fragment is ONE component ──────────────────
def test_fully_bonded_hcn_is_one_component_for_every_atom():
    for atom in (0, 1, 2):
        assert _component(HCN_BONDED, atom) == [0, 1, 2]


# ── (negative) a broken bond splits the fragment into independent pieces ───────
def test_broken_hc_splits_into_lone_h_and_cn():
    # The atom whose all bonds are broken comes back as a singleton...
    assert _component(HCN_BROKEN_HC, 0) == [0]
    # ...while C still travels with N.
    assert _component(HCN_BROKEN_HC, 1) == [1, 2]
    assert _component(HCN_BROKEN_HC, 2) == [1, 2]


# ── an out-of-range atom is a 4xx, never a guessed component ───────────────────
def test_out_of_range_atom_is_422():
    r = client.post(
        "/geometry/connected-component", json={"xyz": HCN_BONDED, "atom": 3}
    )
    assert r.status_code == 422
    assert "out of range" in r.text


def test_negative_atom_is_422():
    r = client.post(
        "/geometry/connected-component", json={"xyz": HCN_BONDED, "atom": -1}
    )
    assert r.status_code == 422
