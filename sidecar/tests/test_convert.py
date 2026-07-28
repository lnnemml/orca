"""Tests for the molecular format conversion endpoint (ASE)."""

from fastapi.testclient import TestClient

from app.main import app

client = TestClient(app)

WATER_XYZ = "3\nwater\nO 0.0 0.0 0.117\nH 0.0 0.755 -0.471\nH 0.0 -0.755 -0.471\n"


def _convert(content: str, from_format: str, to_format: str):
    return client.post(
        "/convert",
        json={
            "content": content,
            "from_format": from_format,
            "to_format": to_format,
        },
    )


def _atom_positions(xyz: str) -> list[tuple[float, float, float]]:
    """Pull (x, y, z) floats out of standard xyz text (skip count + comment)."""
    coords = []
    for line in xyz.splitlines()[2:]:
        parts = line.split()
        if len(parts) >= 4:
            coords.append((float(parts[1]), float(parts[2]), float(parts[3])))
    return coords


def test_xyz_to_pdb() -> None:
    resp = _convert(WATER_XYZ, "xyz", "pdb")
    assert resp.status_code == 200
    body = resp.json()
    assert body["num_atoms"] == 3
    assert body["formula"] == "H2O"
    # PDB records carry the atoms.
    assert "ATOM" in body["content"]


def test_pdb_to_xyz() -> None:
    # First produce a PDB, then convert it back and check coordinates survived.
    pdb = _convert(WATER_XYZ, "xyz", "pdb").json()["content"]
    resp = _convert(pdb, "pdb", "xyz")
    assert resp.status_code == 200
    body = resp.json()
    assert body["num_atoms"] == 3
    assert body["formula"] == "H2O"

    original = _atom_positions(WATER_XYZ)
    roundtrip = _atom_positions(body["content"])
    assert len(roundtrip) == len(original)
    for (x0, y0, z0), (x1, y1, z1) in zip(original, roundtrip):
        assert abs(x0 - x1) < 1e-3
        assert abs(y0 - y1) < 1e-3
        assert abs(z0 - z1) < 1e-3


def test_unsupported_format() -> None:
    # mol2 is not in the whitelist (ASE lacks it — Open Babel territory).
    resp = _convert(WATER_XYZ, "xyz", "mol2")
    assert resp.status_code == 400
    resp = _convert(WATER_XYZ, "mol2", "xyz")
    assert resp.status_code == 400


def test_garbage_content() -> None:
    resp = _convert("this is not a molecule at all\n@@@\n", "xyz", "pdb")
    assert resp.status_code == 422


def test_formats_endpoint() -> None:
    resp = client.get("/formats")
    assert resp.status_code == 200
    body = resp.json()
    assert "xyz" in body["read"]
    assert "xyz" in body["write"]
    # PDB is readable but the whitelist is intentionally narrower on write.
    assert "pdb" in body["read"]


def test_empty_structure() -> None:
    # A well-formed but atom-less xyz (count 0) → 422, not a 200 with 0 atoms.
    resp = _convert("0\nempty\n", "xyz", "xyz")
    assert resp.status_code == 422
