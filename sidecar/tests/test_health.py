"""Smoke test for the sidecar health endpoint."""

from fastapi.testclient import TestClient

from app import __version__
from app.main import app

client = TestClient(app)


def test_health_ok() -> None:
    resp = client.get("/health")
    assert resp.status_code == 200
    # Assert against __version__, not a literal, so a version bump (the handshake
    # rule — bump on every endpoint change) doesn't break this smoke test.
    assert resp.json() == {"status": "ok", "version": __version__}


def test_health_reports_a_dotted_numeric_version() -> None:
    # The Rust core parses this component-wise; keep it parseable.
    parts = client.get("/health").json()["version"].split(".")
    assert all(p.isdigit() for p in parts)
