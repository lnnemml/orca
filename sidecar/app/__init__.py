"""OrcaStudio Python sidecar — chemistry logic service (FastAPI on localhost)."""

# Versioning rule: bump the MINOR every time an endpoint is added or its
# request/response shape changes. The Rust core compares this against its
# EXPECTED_MIN_SIDECAR_VERSION at /health time and flags a `stale` sidecar (a hot
# frontend reload picks up new routes; uvicorn without --reload does not). See
# wiki/modules/sidecar.md and wiki/debugging/005.
# 0.3.0: +POST /geometry/rotatable-mask (bond-graph split for intra-fragment edits).
# 0.4.0: rotatable-mask gained the `within` param (perception restricted to one fragment).
__version__ = "0.4.0"
