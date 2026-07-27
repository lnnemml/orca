"""SMILES → 3D structure generation (RDKit).

Phase 2.2: the first chemistry endpoint. Given a SMILES string, generate a
reasonable 3D conformer (ETKDGv3 embedding + MMFF relaxation) and return it as
standard xyz plus a little metadata the input editor needs (formula, formal
charge). Stateless — no files touched.
"""

from fastapi import APIRouter, HTTPException
from pydantic import BaseModel
from rdkit import Chem
from rdkit.Chem import AllChem, rdMolDescriptors

router = APIRouter()


class SmilesToXyzRequest(BaseModel):
    smiles: str


class SmilesToXyzResponse(BaseModel):
    xyz: str  # standard xyz: count\ncomment\natom lines
    formula: str  # molecular formula, e.g. "C2H6O"
    charge: int  # formal charge derived from the SMILES
    multiplicity: int  # always 1 (singlet) for now
    num_atoms: int


@router.post("/smiles-to-3d", response_model=SmilesToXyzResponse)
def smiles_to_3d(req: SmilesToXyzRequest) -> SmilesToXyzResponse:
    mol = Chem.MolFromSmiles(req.smiles)
    if mol is None:
        raise HTTPException(status_code=400, detail="Invalid SMILES")

    mol = Chem.AddHs(mol)

    # ETKDGv3 embedding. On failure, retry with random starting coordinates
    # (helps for small/strained systems) before giving up. `useRandomCoords`
    # is a property of the params object, not a kwarg of EmbedMolecule when a
    # params object is passed.
    params = AllChem.ETKDGv3()
    if AllChem.EmbedMolecule(mol, params) != 0:
        params.useRandomCoords = True
        if AllChem.EmbedMolecule(mol, params) != 0:
            raise HTTPException(
                status_code=422, detail="Could not generate 3D coordinates"
            )

    # MMFF relaxation. Non-convergence (or missing MMFF params for exotic
    # species) is not fatal — the embedded geometry is still usable.
    try:
        AllChem.MMFFOptimizeMolecule(mol, maxIters=500)
    except (ValueError, RuntimeError):
        pass

    return SmilesToXyzResponse(
        xyz=Chem.MolToXYZBlock(mol),
        formula=rdMolDescriptors.CalcMolFormula(mol),
        charge=Chem.GetFormalCharge(mol),
        multiplicity=1,
        num_atoms=mol.GetNumAtoms(),
    )
