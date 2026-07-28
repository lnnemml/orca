"""Molecular format conversion (ASE).

Phase 2.6 — the last Phase 2 endpoint. Convert a molecular structure between
common text formats (xyz, pdb, cif, mol/sdf, ...) using ASE's `io.read`/`write`.

Why ASE and not Open Babel (as the ROADMAP originally said): ASE is already a
sidecar dependency (ADR-007 — it's the geometry kernel for Phase 2.5's
set_distance/angle/dihedral), it installs as a pure-Python wheel (no system
binary, unlike Open Babel), and it covers every format we actually need. Open
Babel stays the fallback only if a format ASE lacks (e.g. mol2) is ever needed.

Security: we do **not** expose everything ASE can read. Its registry includes
calculation-package formats, some of which run code or read arbitrary files on
parse. Only the plain structure formats below are accepted (whitelist).
Stateless — the two tempfiles are always cleaned up.
"""

import os
import tempfile

from fastapi import APIRouter, HTTPException
from pydantic import BaseModel

from ase.io import read, write

router = APIRouter()

# Public format keys → human labels. Kept to plain structure formats; anything
# not in these maps is rejected before ASE ever sees it.
READ_FORMATS = {
    "xyz": "XYZ",
    "extxyz": "Extended XYZ",
    "pdb": "PDB",
    "cif": "CIF",
    "mol": "MDL Molfile",
    "sdf": "SDF",
    "gen": "DFTB+ gen",
    "turbomole": "Turbomole coord",
    "gaussian-in": "Gaussian input",
    "vasp": "VASP POSCAR",
    "traj": "ASE trajectory",
}
WRITE_FORMATS = {
    "xyz": "XYZ",
    "extxyz": "Extended XYZ",
    "pdb": "PDB",
    "cif": "CIF",
    "gen": "DFTB+ gen",
    "turbomole": "Turbomole coord",
    "vasp": "VASP POSCAR",
}

# Public key → ASE's internal format name where they differ. ASE calls PDB
# "proteindatabank"; the rest match their key.
_ASE_FORMAT = {"pdb": "proteindatabank"}


def _ase_format(fmt: str) -> str:
    return _ASE_FORMAT.get(fmt, fmt)


class ConvertRequest(BaseModel):
    content: str  # file content as text
    from_format: str  # e.g. "pdb", "xyz", "cif"
    to_format: str  # e.g. "xyz"


class ConvertResponse(BaseModel):
    content: str
    num_atoms: int
    formula: str


@router.post("/convert", response_model=ConvertResponse)
def convert(req: ConvertRequest) -> ConvertResponse:
    # Whitelist check first — never rely on ASE raising for an unknown format,
    # and never let a non-whitelisted (possibly code-executing) reader run.
    if req.from_format not in READ_FORMATS:
        raise HTTPException(
            status_code=400, detail=f"unsupported format: {req.from_format}"
        )
    if req.to_format not in WRITE_FORMATS:
        raise HTTPException(
            status_code=400, detail=f"unsupported format: {req.to_format}"
        )

    in_path: str | None = None
    out_path: str | None = None
    try:
        # ASE reads/writes by path; a suffix keeps the file tidy (format is
        # passed explicitly, so detection doesn't depend on it).
        with tempfile.NamedTemporaryFile(
            suffix=f".{req.from_format}", delete=False, mode="w"
        ) as f_in:
            f_in.write(req.content)
            in_path = f_in.name

        try:
            # index=-1: if the input holds several structures (a trajectory),
            # take the last one explicitly. Multi-frame handling is Phase 3.
            atoms = read(in_path, format=_ase_format(req.from_format), index=-1)
        except Exception as e:  # ASE raises many types on malformed input
            raise HTTPException(
                status_code=422,
                detail=f"could not parse as {req.from_format}: {e}",
            )

        if len(atoms) == 0:
            raise HTTPException(status_code=422, detail="no atoms found")

        out_path = in_path + f".out.{req.to_format}"
        try:
            write(out_path, atoms, format=_ase_format(req.to_format))
            content = open(out_path, encoding="utf-8").read()
        except Exception as e:
            raise HTTPException(
                status_code=422,
                detail=f"could not write as {req.to_format}: {e}",
            )

        return ConvertResponse(
            content=content,
            num_atoms=len(atoms),
            formula=atoms.get_chemical_formula(),
        )
    finally:
        for path in (in_path, out_path):
            if path and os.path.exists(path):
                os.unlink(path)


@router.get("/formats")
def formats() -> dict:
    """Supported input/output formats, for populating UI dropdowns."""
    return {"read": READ_FORMATS, "write": WRITE_FORMATS}
