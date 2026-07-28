"""OrcaStudio sidecar FastAPI application.

Phase 0: exposes only a health endpoint so the Tauri core can confirm the
service is alive. Chemistry endpoints (cclib parsing, RDKit, ASE) arrive in
later phases.
"""

from fastapi import FastAPI
from fastapi.middleware.cors import CORSMiddleware
from pydantic import BaseModel

from . import __version__
from .convert import router as convert_router
from .smiles import router as smiles_router

app = FastAPI(title="OrcaStudio sidecar", version=__version__)

# The sidecar is bound to localhost and only ever called by the Tauri shell.
# Allow the dev-server origin and the packaged Tauri webview origins.
app.add_middleware(
    CORSMiddleware,
    allow_origin_regex=r"^(https?://(localhost|127\.0\.0\.1)(:\d+)?|tauri://localhost)$",
    allow_credentials=True,
    allow_methods=["*"],
    allow_headers=["*"],
)


class HealthResponse(BaseModel):
    status: str
    version: str


@app.get("/health", response_model=HealthResponse)
def health() -> HealthResponse:
    """Liveness probe polled by the Tauri sidecar manager."""
    return HealthResponse(status="ok", version=__version__)


# Chemistry endpoints.
app.include_router(smiles_router)
app.include_router(convert_router)
