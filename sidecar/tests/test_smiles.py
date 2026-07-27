"""Tests for the SMILES → 3D endpoint (RDKit)."""

from fastapi.testclient import TestClient

from app.main import app

client = TestClient(app)


def _post(smiles: str):
    return client.post("/smiles-to-3d", json={"smiles": smiles})


def test_water_smiles() -> None:
    resp = _post("O")
    assert resp.status_code == 200
    body = resp.json()
    assert body["num_atoms"] == 3
    assert body["formula"] == "H2O"
    assert body["charge"] == 0
    assert body["multiplicity"] == 1
    assert body["xyz"].startswith("3\n")


def test_ethanol_smiles() -> None:
    resp = _post("CCO")
    assert resp.status_code == 200
    body = resp.json()
    assert body["num_atoms"] == 9
    assert body["formula"] == "C2H6O"


def test_charged_smiles() -> None:
    resp = _post("[NH4+]")
    assert resp.status_code == 200
    assert resp.json()["charge"] == 1


def test_invalid_smiles() -> None:
    resp = _post("not_a_smiles")
    assert resp.status_code == 400
