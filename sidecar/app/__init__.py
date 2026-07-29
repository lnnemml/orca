"""OrcaStudio Python sidecar — chemistry logic service (FastAPI on localhost)."""

# Versioning rule: bump the MINOR every time an endpoint is added or its
# request/response shape changes. The Rust core compares this against its
# EXPECTED_MIN_SIDECAR_VERSION at /health time and flags a `stale` sidecar (a hot
# frontend reload picks up new routes; uvicorn without --reload does not). See
# wiki/modules/sidecar.md and wiki/debugging/005.
__version__ = "0.2.0"
